package tasks

import (
	"database/sql"
	"errors"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

// ComputeStatuses is the CHECK list of compute_requests.status.
var ComputeStatuses = []string{"pending", "approved", "provisioning", "ready", "denied", "released", "failed"}

// ComputeRequest is one row of compute_requests (D-D).
type ComputeRequest struct {
	ID           string `json:"id"`
	Project      string `json:"project"`
	RequestedBy  string `json:"requested_by"`
	Size         string `json:"size"`
	Hours        int    `json:"hours"`
	Reason       string `json:"reason"`
	Status       string `json:"status"`
	DecidedBy    string `json:"decided_by"`
	Executor     string `json:"executor"`
	Words        string `json:"words"`
	ProposalPost string `json:"proposal_post"`
	At           string `json:"at"`
	UpdatedAt    string `json:"updated_at"`
}

// Caps is the compute configuration and its current use, as the UI shows it.
type Caps struct {
	Provider       string `json:"provider"`
	MaxVMs         int    `json:"max_vms"`
	MaxHoursPerDay int    `json:"max_hours_per_day"`
	AutoApprove    bool   `json:"auto_approve"`
	BudgetCredits  int    `json:"budget_credits"`
	VMsNow         int    `json:"vms_now"`
	HoursToday     int    `json:"hours_today"`
	LoginSource    bool   `json:"login_source"`
}

const computeCols = "id, project, requested_by, size, hours, reason, status, decided_by, executor, words, proposal_post, at, updated_at"

func scanCompute(row interface{ Scan(...any) error }) (*ComputeRequest, error) {
	var r ComputeRequest
	if err := row.Scan(&r.ID, &r.Project, &r.RequestedBy, &r.Size, &r.Hours, &r.Reason, &r.Status, &r.DecidedBy, &r.Executor, &r.Words, &r.ProposalPost, &r.At, &r.UpdatedAt); err != nil {
		return nil, err
	}
	return &r, nil
}

// GetCompute returns one request.
func GetCompute(s *store.Store, id string) (*ComputeRequest, error) {
	r, err := scanCompute(s.DB.QueryRow("SELECT "+computeCols+" FROM compute_requests WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, input(2, fmt.Sprintf("No compute request %q", id), "the id is not in this store", "pulse compute list")
	}
	return r, err
}

// ListCompute returns requests newest first; project/status "" match everything.
func ListCompute(s *store.Store, project, status string) ([]ComputeRequest, error) {
	q := "SELECT " + computeCols + " FROM compute_requests WHERE 1=1"
	var args []any
	if project != "" {
		q += " AND project = ?"
		args = append(args, project)
	}
	if status != "" {
		q += " AND status = ?"
		args = append(args, status)
	}
	q += " ORDER BY id DESC"
	rows, err := s.DB.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []ComputeRequest
	for rows.Next() {
		r, err := scanCompute(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *r)
	}
	return out, rows.Err()
}

// LoginSourceCheck says whether a login source exists for the machines
// compute provisions (ADR-034 rule 6: the executor token, or a hub login that
// reaches no repository). The runner installs it; nil means no check, so a
// store without the runner behaves as before.
var LoginSourceCheck func(s *store.Store) bool

// NoLoginSource is the reason a request is held while no login source exists.
const NoLoginSource = "no executor login source"

// GetCaps reads the compute.* settings and the current use: vms_now counts
// requests approved, provisioning or ready; hours_today sums the hours of
// requests made today (UTC) that were not denied. LoginSource is false when
// no machine could be logged in (the request is held, not denied).
func GetCaps(s *store.Store) (Caps, error) {
	c := Caps{
		Provider:       s.Setting("compute.provider", ""),
		MaxVMs:         s.SettingInt("compute.max_vms"),
		MaxHoursPerDay: s.SettingInt("compute.max_hours_per_day"),
		AutoApprove:    s.Setting("compute.auto_approve", "") == "1",
		BudgetCredits:  s.SettingInt("compute.budget_credits"),
		LoginSource:    LoginSourceCheck == nil || LoginSourceCheck(s),
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM compute_requests WHERE status IN ('approved','provisioning','ready')").Scan(&c.VMsNow); err != nil {
		return c, err
	}
	day := s.Now()[:10]
	if err := s.DB.QueryRow("SELECT COALESCE(SUM(hours), 0) FROM compute_requests WHERE substr(at, 1, 10) = ? AND status NOT IN ('denied')", day).Scan(&c.HoursToday); err != nil {
		return c, err
	}
	return c, nil
}

// RequestCompute records a request. It is approved at once (decided_by
// "pulse") when compute.auto_approve is 1 and both caps hold - fewer than
// compute.max_vms machines approved/provisioning/ready, and hours plus the
// hours already requested today within compute.max_hours_per_day; otherwise it
// stays pending and a proposal post on runner/compute asks the owner.
func RequestCompute(s *store.Store, project, agent, size string, hours int, reason string) (*ComputeRequest, error) {
	if agent == "" {
		return nil, input(2, "An agent is needed", "compute is requested by an agent", "pass --agent NAME or set PULSE_AGENT")
	}
	size = strings.TrimSpace(size)
	if size == "" {
		size = s.Setting("compute.azure.vm_size", "")
	}
	if size == "" {
		return nil, input(2, "A machine size is needed", "compute.azure.vm_size is empty and --size was not given", "pulse compute request --size Standard_B2ms ...")
	}
	if hours <= 0 {
		return nil, input(2, "Hours must be at least 1", "a request names how long the machine is needed", "pulse compute request --hours 4 ...")
	}
	reason = clean(reason)
	if reason == "" {
		return nil, input(2, "A reason is needed", "the owner reads it before approving", "pulse compute request --reason 'why' ...")
	}
	if err := checkProject(s, project); err != nil {
		return nil, err
	}
	caps, err := GetCaps(s)
	if err != nil {
		return nil, err
	}
	auto := caps.AutoApprove && caps.VMsNow < caps.MaxVMs && (caps.MaxHoursPerDay == 0 || caps.HoursToday+hours <= caps.MaxHoursPerDay) && caps.LoginSource
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	id, err := nextID(tx, "compute_requests", "CR-")
	if err != nil {
		return nil, err
	}
	status, decidedBy := "pending", ""
	if auto {
		status, decidedBy = "approved", "pulse"
	}
	if _, err := tx.Exec("INSERT INTO compute_requests (id, project, requested_by, size, hours, reason, status, decided_by, at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		id, project, agent, size, hours, reason, status, decidedBy, now, now); err != nil {
		return nil, err
	}
	var events []bus.Event
	ev, err := bus.EmitTx(tx, now, agent, "compute.requested", id, map[string]any{"project": project, "size": size, "hours": hours, "status": status})
	if err != nil {
		return nil, err
	}
	events = append(events, ev)
	if auto {
		ev, err := bus.EmitTx(tx, now, "pulse", "compute.approved", id, map[string]any{"project": project, "auto": true, "vms_now": caps.VMsNow + 1, "max_vms": caps.MaxVMs})
		if err != nil {
			return nil, err
		}
		events = append(events, ev)
	} else {
		summary := fmt.Sprintf("compute request %s: %s for %dh - %s", id, size, hours, reason)
		body := fmt.Sprintf("%s asks for one %s machine for %d hours.\n\nReason: %s\n\nCaps: %d of %d machines in use, auto_approve=%v.\n\nApprove with `pulse compute approve %s --words '...'` or deny with `pulse compute deny %s --words '...'` (or from the hub's Compute page).\n",
			agent, size, hours, reason, caps.VMsNow, caps.MaxVMs, caps.AutoApprove, id, id)
		if !caps.LoginSource {
			body += "\nHeld: " + NoLoginSource + " - a provisioned machine could not log in. Place a Copilot-only fine-grained token with `pulse executor token --with-token`; an approval waits for one (ADR-034).\n"
		}
		postID, ev, err := insertPost(tx, now, project, "runner", "compute", "proposal", agent, summary, body, "", "")
		if err != nil {
			return nil, err
		}
		events = append(events, ev)
		if _, err := tx.Exec("UPDATE compute_requests SET proposal_post = ? WHERE id = ?", postID, id); err != nil {
			return nil, err
		}
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(events...)
	return GetCompute(s, id)
}

// Approve answers a pending request yes; Deny answers no. Both record the
// owner's words, decide the proposal post when there is one (a result post
// and a decisions row, the way `pulse decide` does) and log
// compute.approved / compute.denied.
func Approve(s *store.Store, id, words, actor string) (*ComputeRequest, error) {
	return decideCompute(s, id, words, actor, true)
}

// Deny answers a pending request no (see Approve).
func Deny(s *store.Store, id, words, actor string) (*ComputeRequest, error) {
	return decideCompute(s, id, words, actor, false)
}

func decideCompute(s *store.Store, id, words, actor string, yes bool) (*ComputeRequest, error) {
	if actor == "" {
		actor = store.HumanAgent
	}
	cur, err := GetCompute(s, id)
	if err != nil {
		return nil, err
	}
	if cur.Status != "pending" {
		return nil, input(2, fmt.Sprintf("%s is %s, not pending", id, cur.Status), "only a pending request takes an answer", "pulse compute list")
	}
	status, action, answer := "denied", "compute.denied", "no"
	if yes {
		status, action, answer = "approved", "compute.approved", "yes"
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE compute_requests SET status = ?, decided_by = ?, words = ?, updated_at = ? WHERE id = ?", status, actor, words, now, id); err != nil {
		return nil, err
	}
	var events []bus.Event
	if cur.ProposalPost != "" {
		var decided int
		if err := tx.QueryRow("SELECT COUNT(*) FROM decisions WHERE post_id = ?", cur.ProposalPost).Scan(&decided); err != nil {
			return nil, err
		}
		if decided == 0 {
			resolution := "declined"
			if yes {
				resolution = "accepted"
			}
			var summary string
			if err := tx.QueryRow("SELECT summary FROM posts WHERE id = ?", cur.ProposalPost).Scan(&summary); err != nil {
				return nil, err
			}
			if len(summary) > 200 {
				summary = summary[:197] + "..."
			}
			text := words
			if text == "" {
				text = "(no words recorded)"
			}
			body := fmt.Sprintf("The human answered **%s**.\n\n%s\n\nProposal: %s\n", answer, text, cur.ProposalPost)
			resultID, ev, err := insertPost(tx, now, cur.Project, "runner", "compute", "result", actor, resolution+": "+summary, body, cur.ProposalPost, resolution)
			if err != nil {
				return nil, err
			}
			events = append(events, ev)
			if _, err := tx.Exec("INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
				cur.ProposalPost, cur.Project, answer, words, now, actor, resultID); err != nil {
				return nil, err
			}
			ev, err = bus.EmitTx(tx, now, actor, "decide", cur.ProposalPost, map[string]any{"answer": answer, "result": resultID})
			if err != nil {
				return nil, err
			}
			events = append(events, ev)
		}
	}
	ev, err := bus.EmitTx(tx, now, actor, action, id, map[string]any{"project": cur.Project, "words": words, "requested_by": cur.RequestedBy})
	if err != nil {
		return nil, err
	}
	events = append(events, ev)
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(events...)
	return GetCompute(s, id)
}

// SetComputeStatus is the runner's hook: approved -> provisioning -> ready ->
// released, or failed, recording the executor it registered. It logs
// compute.<status>.
func SetComputeStatus(s *store.Store, id, status, executor, actor string) (*ComputeRequest, error) {
	ok := false
	for _, st := range ComputeStatuses {
		if st == status {
			ok = true
		}
	}
	if !ok {
		return nil, input(2, fmt.Sprintf("Unknown compute status %q", status), "statuses are "+strings.Join(ComputeStatuses, ", "), "")
	}
	cur, err := GetCompute(s, id)
	if err != nil {
		return nil, err
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE compute_requests SET status = ?, executor = CASE WHEN ? = '' THEN executor ELSE ? END, updated_at = ? WHERE id = ?", status, executor, executor, now, id); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, actor, "compute."+status, id, map[string]any{"project": cur.Project, "executor": executor, "from": cur.Status})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	return GetCompute(s, id)
}

// insertPost writes one board post the way legacy/pulse.py create_post does
// for the two kinds this package needs (proposal, result): id
// <YYYYMMDDTHHMMSSZ>-<agent> with -2, -3 on a same-second collision, the
// agent's post counter bumped when the agent row exists, and a `post` event.
// It stays in-transaction on purpose: the request row and its proposal post
// commit together, which board.CreatePost (its own transaction) cannot offer.
func insertPost(tx *sql.Tx, now, project, area, thread, kind, agent, summary, body, on, resolution string) (string, bus.Event, error) {
	summary = clean(summary)
	if len(summary) > 240 {
		summary = summary[:237] + "..."
	}
	base := stamp(now) + "-" + agent
	id := base
	for n := 2; ; n++ {
		var one int
		err := tx.QueryRow("SELECT 1 FROM posts WHERE id = ?", id).Scan(&one)
		if errors.Is(err, sql.ErrNoRows) {
			break
		}
		if err != nil {
			return "", bus.Event{}, err
		}
		id = fmt.Sprintf("%s-%d", base, n)
	}
	var runner, model string
	_ = tx.QueryRow("SELECT runner, model FROM agents WHERE name = ?", agent).Scan(&runner, &model)
	if _, err := tx.Exec("INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at) "+
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, '', ?, '', '', ?, '', '', 0, ?, ?, ?, '', '', '', '', '{}', 'store', ?)",
		id, project, area, thread, kind, agent, runner, model, now, on, resolution, summary, body, now); err != nil {
		return "", bus.Event{}, fmt.Errorf("post: %w", err)
	}
	if _, err := tx.Exec("UPDATE agents SET posts = posts + 1, last_active_at = ? WHERE name = ?", now, agent); err != nil {
		return "", bus.Event{}, err
	}
	ev, err := bus.EmitTx(tx, now, agent, "post", id, map[string]any{"kind": kind, "project": project, "thread": area + "/" + thread})
	return id, ev, err
}
