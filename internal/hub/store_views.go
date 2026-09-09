package hub

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net/http"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/inspect"
	"github.com/manishtiwari25/pulse/internal/store"
)

func (h *Handler) metadata() (map[string]any, error) {
	schema, _, err := h.Store.Meta("schema_version")
	if err != nil {
		return nil, err
	}
	records, _, err := h.Store.Meta("records_version")
	if err != nil {
		return nil, err
	}
	info, err := os.Stat(h.Store.Path)
	if err != nil {
		return nil, err
	}
	var sequence int64
	if err := h.Store.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&sequence); err != nil {
		return nil, err
	}
	fingerprint := fmt.Sprintf("store:%d:%d:%d", info.ModTime().UnixNano(), info.Size(), sequence)
	return object("store schema_version records_version binary_version generated_at fingerprint tool note",
		h.Store.Path, schema, records, cli.Version(), h.Store.Now(), fingerprint, "pulse "+cli.Version(), "computed from the store; no projection database"), nil
}

func (h *Handler) settings() (map[string]any, error) {
	if h.Store == nil {
		return object("available can_edit settings", false, false, []any{}), nil
	}
	reports, err := inspect.SettingsReport(h.Store)
	if err != nil {
		return nil, err
	}
	rows := make([]map[string]any, 0, len(reports))
	for _, report := range reports {
		row, err := jsonMap(report)
		if err != nil {
			return nil, err
		}
		row["secret"] = false
		if bus.PrivateSettings[report.Name] {
			row["value"], row["default"], row["secret"] = hiddenSettingValue(report.Value), "", true
		} else if store.SecretShaped(report.Value) {
			// Credentials already present in an old store must not be disclosed.
			row["value"], row["default"], row["secret"] = "", "", true
		}
		rows = append(rows, row)
	}
	return object("available can_edit settings", true, true, rows), nil
}

func (h *Handler) writeAPI(w http.ResponseWriter, r *http.Request, route string) {
	if h.Store == nil || h.Store.DB == nil || h.Store.DB.PingContext(r.Context()) != nil {
		writeError(w, 409, "store is not available; POST /api/v1/"+route+" requires the pulse store", "No writable PULSE store is open.", "Run pulse init, then restart the hub.")
		return
	}
	header := "X-Pulse-Decide"
	if route == "settings" {
		header = "X-Pulse-Settings"
	}
	if r.Header.Get(header) != "yes" {
		writeError(w, 403, "missing "+header+": yes", "An explicit confirmation header is required.", "Confirm the change in the hub.")
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	if route == "settings" {
		name := strings.TrimSpace(str(payload["name"]))
		var value string
		switch raw := payload["value"].(type) {
		case string:
			value = raw
		case json.Number:
			value = string(raw)
		default:
			writeError(w, 400, "value must be a string or a number", "This setting value has an unsupported JSON type.", "Send a string or number.")
			return
		}
		if err := h.Store.SetSetting("human:dashboard", name, value); err != nil {
			var input *store.InputError
			if errors.As(err, &input) {
				writeError(w, 400, input.Error(), "The value does not satisfy this setting's rule.", "Use a value shown in GET /api/v1/settings.")
			} else {
				writeError(w, 500, err.Error(), "The store could not save the setting.", "Check pulse status and the store's write permissions.")
			}
			return
		}
		settings, err := h.settings()
		if err != nil {
			h.respond(w, nil, err)
			return
		}
		for _, row := range settings["settings"].([]map[string]any) {
			if row["name"] == name {
				writeJSON(w, 200, object("ok setting", true, row))
				return
			}
		}
		writeError(w, 503, "The saved setting is unavailable", "The settings report did not contain the updated key.", "Refresh the settings page.")
		return
	}
	id := strings.TrimSpace(str(payload["id"]))
	if id == "" {
		id = strings.TrimSpace(str(payload["post_id"]))
	}
	answer, words := strings.ToLower(strings.TrimSpace(str(payload["answer"]))), strings.TrimSpace(str(payload["words"]))
	decision, err := board.Decide(h.Store, id, answer, words, store.HumanAgent)
	if err != nil {
		var input *cli.Error
		if errors.As(err, &input) && input.Code == 2 {
			writeError(w, 400, err.Error(), "The proposal could not be decided.", "Refresh the proposal and choose yes or no.")
		} else {
			writeError(w, 500, err.Error(), "The store could not save the decision.", "Check pulse status and the store's write permissions.")
		}
		return
	}
	writeJSON(w, 200, object("ok result_post post_id answer", true, decision.Result.ID, id, answer))
}

func (h *Handler) executors(r *http.Request) ([]map[string]any, error) {
	if h.Store == nil {
		return []map[string]any{}, nil
	}
	rows, err := h.additionRows(r, "SELECT * FROM executors ORDER BY name")
	if err != nil {
		return nil, err
	}
	local := object("name kind address workdir workers projects status last_probe_at probe_note errors backoff_minutes next_probe_at runs_done runs_failed created_at created_by",
		"local", "local", "", "", max(0, h.Store.SettingInt("workers")), "*", "active", "", "this machine", 0, 0, "", 0, 0, "", "")
	rows = append([]map[string]any{local}, rows...)
	horizon := parseTime(h.Store.Now()).Add(-time.Duration(2*h.Store.SettingInt("minutes_per_job")+15) * time.Minute).Format(time.RFC3339)
	for _, row := range rows {
		var running int
		if err := h.Store.DB.QueryRowContext(r.Context(), "SELECT count(*) FROM runs WHERE status='running' AND executor=? AND started_at>?", row["name"], horizon).Scan(&running); err != nil {
			return nil, err
		}
		row["running"], row["free"] = running, 0
		if row["status"] == "active" {
			row["free"] = max(0, num(row["workers"])-running)
		}
	}
	return rows, nil
}

func (h *Handler) runner(r *http.Request) (map[string]any, error) {
	if h.Store == nil {
		return object("available", false), nil
	}
	rows, err := h.executors(r)
	if err != nil {
		return nil, err
	}
	executors := []map[string]any{}
	free, slots, active, running := 0, 0, 0, 0
	anyProject := false
	for _, row := range rows {
		executors = append(executors, pick(row, "name kind status address workers projects last_probe_at probe_note running free"))
		running += num(row["running"])
		if row["status"] == "active" {
			active++
			free += num(row["free"])
			slots += num(row["workers"])
			anyProject = anyProject || (num(row["free"]) > 0 && row["projects"] == "*")
		}
	}
	workers, err := h.additionRows(r, "SELECT name FROM agents WHERE status='active' AND kind='worker' ORDER BY name")
	if err != nil {
		return nil, err
	}
	leases, err := h.additionRows(r, "SELECT DISTINCT lease_agent FROM jobs WHERE status='leased'")
	if err != nil {
		return nil, err
	}
	leased := map[string]bool{}
	for _, row := range leases {
		leased[str(row["lease_agent"])] = true
	}
	idle := []string{}
	for _, row := range workers {
		if !leased[str(row["name"])] {
			idle = append(idle, str(row["name"]))
		}
	}
	counts, err := h.additionRows(r, "SELECT status,count(*) AS n FROM jobs WHERE status IN ('queued','leased','blocked') GROUP BY status")
	if err != nil {
		return nil, err
	}
	jobs := object("queued leased blocked running runs_today cap", 0, 0, 0, running, 0, 0)
	for _, count := range counts {
		jobs[str(count["status"])] = count["n"]
	}
	var runsToday int
	if err := h.Store.DB.QueryRowContext(r.Context(), "SELECT count(*) FROM runs WHERE mode!='dry-run' AND status!='error' AND substr(started_at,1,10)=?", h.Store.Now()[:10]).Scan(&runsToday); err != nil {
		return nil, err
	}
	jobs["runs_today"], jobs["cap"] = runsToday, max(0, num(h.Store.Setting("jobs_per_day", "")))
	text, err := inspect.DaemonStatus(h.Store)
	if err != nil {
		return nil, err
	}
	state := strings.Split(text, " ")[0]
	if strings.HasPrefix(text, "never") {
		state = "never started"
	}
	daemon := object("state text", state, text)
	if raw, ok, err := h.Store.Meta("daemon"); err != nil {
		return nil, err
	} else if ok {
		var beat map[string]any
		if err := json.Unmarshal([]byte(raw), &beat); err != nil {
			// The CLI describes malformed metadata as "unreadable heartbeat".
			daemon["state"], daemon["text"] = "unreadable", "unreadable heartbeat"
		} else {
			daemon["pid"], daemon["at"], daemon["loops"], daemon["interval"], daemon["note"] = beat["pid"], beat["at"], num(beat["loops"]), num(beat["interval"]), str(beat["note"])
		}
	}
	stopped, err := inspect.RunnerStopped(h.Store)
	if err != nil {
		return nil, err
	}
	capacity, err := inspect.CapacityStatus(h.Store)
	if err != nil {
		return nil, err
	}
	// ADR-024: projects whose verified branches wait on the owner (a dirty or switched checkout, a lock, an
	// unreadable folder), as the daemon last wrote them; empty when everything lands.
	landing, err := inspect.LandingNotes(h.Store)
	if err != nil {
		return nil, err
	}
	return object("available now daemon runner hands executors workers jobs capacity landing", true, h.Store.Now(), daemon,
		object("allowed reason", stopped == "", stopped),
		object("free slots executors_active executors any_project", free, slots, active, len(executors), anyProject), executors,
		object("pool idle busy target idle_names prune_after_days", len(workers), len(idle), len(workers)-len(idle), max(1, h.Store.SettingInt("quorum")+1), idle, h.Store.SettingInt("prune_after_days")),
		jobs, capacity, landing), nil
}

func newPerson(agent store.Agent) map[string]any {
	name := agent.Display
	if name == "" {
		name = agent.Name
	}
	sessions := agent.Sessions
	if sessions == nil {
		sessions = []string{}
	}
	names := []string{}
	if name != "" {
		names = append(names, name)
	}
	return object("agent name names kind status agent_status runner model session sessions session_count jobs_done jobs_failed posts store_posts pruned_at pruned_reason last_useful_at last_active_at created_at legacy_id notes votes claims questions answers suggestions findings first_seen last_seen active_claims projects current_job current_job_kind current_job_title current_thread executor focus_since current_task current_task_title current_project current_source days_since approximate name_shared shared_with",
		agent.Name, name, names, agent.Kind, agent.Status, agent.Status, agent.Runner, agent.Model, "", sessions, len(sessions),
		agent.JobsDone, agent.JobsFailed, agent.Posts, agent.Posts, agent.PrunedAt, agent.PrunedReason, agent.LastUsefulAt, agent.LastActiveAt,
		agent.CreatedAt, agent.LegacyID, agent.Notes, 0, 0, 0, 0, 0, 0, "", "", []map[string]any{}, []string{},
		"", "", "", "", "", "", "", "", "", "", 0.0, false, false, []string{})
}

func (h *Handler) roster(views []projectView) ([]map[string]any, error) {
	stored, err := h.Store.Agents("")
	if err != nil {
		return nil, err
	}
	people := map[string]map[string]any{}
	for _, agent := range stored {
		people[agent.Name] = newPerson(agent)
	}
	now := h.Store.Now()
	presence, err := board.AgentPresences(h.Store, now)
	if err != nil {
		return nil, err
	}
	focuses, err := board.AgentFocuses(h.Store, now)
	if err != nil {
		return nil, err
	}
	reference := parseTime(now)
	for _, view := range views {
		d := view.Data
		if d == nil {
			continue
		}
		byAgent := map[string][]map[string]any{}
		for _, post := range d.Posts {
			byAgent[str(post["agent"])] = append(byAgent[str(post["agent"])], post)
		}
		for name, posts := range byAgent {
			row, exists := people[name]
			if !exists {
				runner, model := "", ""
				for _, p := range posts {
					runner, model = max(runner, str(p["runner"])), max(model, str(p["model"]))
				}
				row = newPerson(store.Agent{Name: name, Kind: "legacy", Status: "active", Runner: runner, Model: model})
				row["names"] = []string{}
				people[name] = row
			}
			order(posts, "-date")
			label, home := "", ""
			for _, p := range posts {
				if label == "" {
					label = str(p["name"])
				}
				if home == "" {
					home = str(p["project"])
				}
				date := str(p["date"])
				if date > str(row["last_seen"]) {
					row["last_seen"] = date
				}
				if str(row["first_seen"]) == "" || date < str(row["first_seen"]) {
					row["first_seen"] = date
				}
				switch p["kind"] {
				case "vote", "claim", "question", "answer", "suggestion", "finding":
					key := str(p["kind"]) + "s"
					row[key] = num(row[key]) + 1
				}
			}
			if label != "" {
				row["names"] = uniqueAppend(row["names"].([]string), label)
			}
			if num(row["store_posts"]) == 0 {
				row["posts"] = len(posts)
			}
			project := view.Project.Slug
			if project == store.CommonsSlug && home != "" {
				project += " (from " + home + ")"
			}
			row["projects"] = uniqueAppend(row["projects"].([]string), project)
		}
		for _, claim := range d.Claims {
			if claim["status"] != "active" {
				continue
			}
			person := people[str(claim["agent"])]
			if person != nil {
				row := pick(claim, "thread_key summary expires id")
				row["slug"], row["base"] = view.Project.Slug, baseFor(view.Project.Slug)
				person["active_claims"] = append(person["active_claims"].([]map[string]any), row)
			}
		}
	}
	out := make([]map[string]any, 0, len(people))
	for _, row := range people {
		p, ok := presence[str(row["agent"])]
		if !ok {
			p = board.AgentPresence{Presence: "legacy", PresenceReason: "historical identity; no supervised execution", PresenceCheckedAt: now}
		}
		row["presence"], row["presence_reason"], row["last_heartbeat_at"] = p.Presence, p.PresenceReason, p.LastHeartbeatAt
		row["presence_session"], row["presence_checked_at"] = p.PresenceSession, p.PresenceCheckedAt
		row["presence_expires_at"] = p.PresenceExpiresAt
		if f, ok := focuses[str(row["agent"])]; ok {
			row["current_job"], row["current_job_kind"], row["current_job_title"] = f.JobID, f.JobKind, f.JobTitle
			row["current_thread"], row["executor"], row["focus_since"] = f.Thread, f.Executor, f.Since
			row["current_task"], row["current_task_title"] = f.Task, f.TaskTitle
			row["current_project"], row["current_source"] = f.Project, f.Source
		}
		if row["first_seen"] == "" {
			row["first_seen"] = row["created_at"]
		}
		if row["last_seen"] == "" {
			row["last_seen"] = row["last_active_at"]
			if row["last_seen"] == "" {
				row["last_seen"] = row["created_at"]
			}
		}
		recent := str(row["last_useful_at"])
		if recent == "" {
			recent = str(row["last_seen"])
		}
		last := parseTime(recent)
		if last.IsZero() {
			last = reference
		}
		row["days_since"] = math.RoundToEven(max(0, reference.Sub(last).Hours()/24)*1000) / 1000
		if p.Presence == "online" && len(row["active_claims"].([]map[string]any)) > 0 {
			row["status"] = "working"
		}
		out = append(out, row)
	}
	rank := map[string]int{"working": 0, "active": 1, "idle": 2, "pruned": 3}
	statusOrder := func(v any) int {
		if n, ok := rank[str(v)]; ok {
			return n
		}
		return 9
	}
	sort.Slice(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if statusOrder(a["status"]) != statusOrder(b["status"]) {
			return statusOrder(a["status"]) < statusOrder(b["status"])
		}
		if a["days_since"].(float64) != b["days_since"].(float64) {
			return a["days_since"].(float64) < b["days_since"].(float64)
		}
		return str(a["agent"]) < str(b["agent"])
	})
	return out, nil
}

func uniqueAppend(values []string, value string) []string {
	for _, old := range values {
		if old == value {
			return values
		}
	}
	return append(values, value)
}
