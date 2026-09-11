package inspect

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/runner"
	"github.com/manishtiwari25/pulse/internal/store"

	_ "github.com/manishtiwari25/pulse/internal/provider/copilot"
)

type StatusCount struct {
	Status string `json:"status"`
	N      int    `json:"n"`
}

type StatusReport struct {
	Store         string             `json:"store"`
	SchemaVersion string             `json:"schema_version"`
	Projects      []store.Project    `json:"projects"`
	Agents        []StatusCount      `json:"agents"`
	Posts         int                `json:"posts"`
	Jobs          []StatusCount      `json:"jobs"`
	RunsToday     int                `json:"runs_today"`
	DailyCap      int                `json:"daily_cap"`
	Runner        string             `json:"runner"`
	Harness       string             `json:"harness"`
	Daemon        string             `json:"daemon"`
	Bus           BusStatus          `json:"bus"`
	Executors     string             `json:"executors"`
	Capacity      string             `json:"capacity"`
	Hygiene       string             `json:"hygiene"`
	Planner       []PlannerNote      `json:"planner"`
	Recovery      []RecoveryNote     `json:"recovery"`
	Landing       []LandingNote      `json:"landing"`
	Settings      []store.SettingRow `json:"settings"`
}

type BusStatus struct {
	Subscribers int   `json:"subscribers"`
	Dropped     int64 `json:"dropped"`
	Deduped     int64 `json:"deduped"`
}

type PlannerNote struct {
	Project string `json:"project"`
	At      string `json:"at"`
	Note    string `json:"note"`
	Stale   bool   `json:"stale"`
}

type RecoveryNote = PlannerNote

type LandingNote = PlannerNote

func statusCounts(s *store.Store, query string) ([]StatusCount, error) {
	rows, err := s.DB.Query(query)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []StatusCount{}
	for rows.Next() {
		var count StatusCount
		if err := rows.Scan(&count.Status, &count.N); err != nil {
			return nil, err
		}
		out = append(out, count)
	}
	return out, rows.Err()
}

func parseUTC(text string) (time.Time, error) {
	at, err := time.Parse("2006-01-02T15:04:05Z", text)
	if err != nil {
		return time.Time{}, cli.Errorf(2, "pulse: Not a UTC time like 2026-09-05T08:00:00Z: "+pyRepr(text), "", "")
	}
	return at, nil
}

func settingInt(values map[string]string, name string) int {
	raw, ok := values[name]
	if !ok {
		raw = store.DefaultSettings[name]
	}
	n, err := strconv.Atoi(strings.TrimSpace(raw))
	if err != nil {
		n, _ = strconv.Atoi(store.DefaultSettings[name])
	}
	return n
}

func dailyCap(values map[string]string) int {
	raw, ok := values["jobs_per_day"]
	if !ok {
		raw = store.DefaultSettings["jobs_per_day"]
	}
	switch strings.ToLower(strings.TrimSpace(raw)) {
	case "", "0", "none", "off", "unlimited":
		return 0
	}
	return max(0, settingInt(values, "jobs_per_day"))
}

// RunnerStopped reports the store kill switch and the old brain STOP file.
// It is read-only; in particular, it never modifies the running daemon.
func RunnerStopped(s *store.Store) (string, error) {
	values, err := s.Settings()
	if err != nil {
		return "", err
	}
	if values["stop"] == "1" {
		return "the runner is stopped in the store (pulse start to resume)", nil
	}
	brain, err := s.Brain()
	if err != nil {
		return "", err
	}
	if brain != nil {
		_, err := os.Stat(filepath.Join(brain.Path, "docs", "evolution", "STOP"))
		if err == nil {
			return fmt.Sprintf("docs/evolution/STOP is present in the brain (%s); a human removes it", brain.Path), nil
		}
		if !os.IsNotExist(err) {
			return "", err
		}
	}
	return "", nil
}

func HarnessLine(s *store.Store) string {
	mode := s.Setting("harness", "native")
	if mode == "copilot-cli" {
		return "copilot-cli (legacy: runner_cmd=" + s.Setting("runner_cmd", "copilot") + ")"
	}
	name := s.Setting("provider", provider.Default)
	label := provider.LabelOf(name)
	if label == "" {
		label = name
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	p, err := provider.Open(name, provider.Options{Home: s.Home})
	if err != nil {
		return "native \u00b7 " + label + " \u00b7 status unknown (" + oneStatusLine(err.Error(), 200) + ")"
	}
	login, err := p.Status(ctx)
	if err != nil {
		return "native \u00b7 " + p.Label() + " \u00b7 status unknown (" + oneStatusLine(err.Error(), 200) + ")"
	}
	if login.LoggedIn {
		return "native \u00b7 " + p.Label() + " \u00b7 signed in as " + login.User
	}
	if login.Detail != "" {
		return "native \u00b7 " + p.Label() + " \u00b7 not signed in - " + oneStatusLine(login.Detail, 240)
	}
	return "native \u00b7 " + p.Label() + " \u00b7 not signed in - open Settings \u2192 Harness or run pulse harness login"
}

func oneStatusLine(s string, limit int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > limit {
		return s[:limit]
	}
	return s
}

type daemonBeat struct {
	PID      *int      `json:"pid"`
	At       string    `json:"at"`
	Loops    int       `json:"loops"`
	Interval *int      `json:"interval"`
	Note     string    `json:"note"`
	Bus      BusStatus `json:"bus"`
}

func readDaemonBeat(s *store.Store) (daemonBeat, bool, error) {
	raw, exists, err := s.Meta("daemon")
	if err != nil {
		return daemonBeat{}, false, err
	}
	if !exists {
		return daemonBeat{}, false, nil
	}
	var beat daemonBeat
	if err := json.Unmarshal([]byte(raw), &beat); err != nil {
		return daemonBeat{}, true, err
	}
	return beat, true, nil
}

func daemonIntervalSeconds(s *store.Store) (int, error) {
	beat, _, err := readDaemonBeat(s)
	if err != nil {
		return runner.DefaultDaemonIntervalSeconds, nil
	}
	if beat.Interval != nil && *beat.Interval != 0 {
		return *beat.Interval, nil
	}
	return runner.DefaultDaemonIntervalSeconds, nil
}

// DaemonStatus describes the heartbeat without probing or signaling its PID.
func DaemonStatus(s *store.Store) (string, error) {
	beat, exists, err := readDaemonBeat(s)
	if err != nil {
		return "unreadable heartbeat", nil
	}
	if !exists {
		return "never started", nil
	}
	pid, interval := "None", "None"
	if beat.PID != nil {
		pid = strconv.Itoa(*beat.PID)
	}
	if beat.Interval != nil {
		interval = strconv.Itoa(*beat.Interval)
	}
	if beat.Note == "exited" {
		return fmt.Sprintf("exited %s (pid %s)", beat.At, pid), nil
	}
	at, err := parseUTC(beat.At)
	if err != nil {
		return "", err
	}
	now, err := parseUTC(s.Now())
	if err != nil {
		return "", err
	}
	age := int(now.Sub(at).Seconds())
	seconds := runner.DefaultDaemonIntervalSeconds
	if beat.Interval != nil && *beat.Interval != 0 {
		seconds = *beat.Interval
	}
	state := "alive"
	if age > 2*seconds+60 {
		state = "silent"
	}
	return fmt.Sprintf("%s pid %s, last loop %ds ago, %d loop(s), interval %ss", state, pid, age, beat.Loops, interval), nil
}

// metaNotes reads one per-project note set the daemon keeps in meta as {slug: {at, note}} - the planner's
// idle notes, the recovery holds, the landing holds - sorted by project, each marked stale when the daemon
// has not refreshed it for two intervals.
func metaNotes(s *store.Store, key string) ([]PlannerNote, error) {
	raw, exists, err := s.Meta(key)
	if err != nil {
		return nil, err
	}
	if !exists {
		return []PlannerNote{}, nil
	}
	var stored map[string]struct {
		At   string `json:"at"`
		Note string `json:"note"`
	}
	if err := json.Unmarshal([]byte(raw), &stored); err != nil {
		return nil, fmt.Errorf("read %s notes: %w", key, err)
	}
	interval, err := daemonIntervalSeconds(s)
	if err != nil {
		return nil, err
	}
	now, err := parseUTC(s.Now())
	if err != nil {
		return nil, err
	}
	projects := make([]string, 0, len(stored))
	for project := range stored {
		projects = append(projects, project)
	}
	sort.Strings(projects)
	out := make([]PlannerNote, 0, len(projects))
	for _, project := range projects {
		item := stored[project]
		at, err := parseUTC(item.At)
		if err != nil {
			return nil, err
		}
		out = append(out, PlannerNote{Project: project, At: item.At, Note: item.Note, Stale: now.Sub(at) > time.Duration(2*interval)*time.Second})
	}
	return out, nil
}

func PlannerNotes(s *store.Store) ([]PlannerNote, error) { return metaNotes(s, "planner") }

func RecoveryNotes(s *store.Store) ([]RecoveryNote, error) { return metaNotes(s, "recovery") }

// LandingNotes are the projects whose verified branches cannot land right now and why (ADR-024): the
// checkout is off its default branch or has uncommitted changes, a lock, an unreadable folder, a stop. The
// daemon writes one per held project and clears it when the branches land again.
func LandingNotes(s *store.Store) ([]LandingNote, error) { return metaNotes(s, "landing") }

func CapacityStatus(s *store.Store) (string, error) {
	raw, exists, err := s.Meta("capacity")
	if err != nil || !exists {
		return "", err
	}
	var note struct {
		Queued       int    `json:"queued"`
		Slots        int    `json:"slots"`
		Inflight     int    `json:"inflight"`
		StarvedSince string `json:"starved_since"`
		Held         string `json:"held"`
	}
	if err := json.Unmarshal([]byte(raw), &note); err != nil {
		return "", fmt.Errorf("read capacity heartbeat: %w", err)
	}
	if note.Queued == 0 {
		return "", nil
	}
	line := fmt.Sprintf("%d job(s) queued, %d slot(s), %d in flight", note.Queued, note.Slots, note.Inflight)
	if note.Held != "" {
		// The daemon's login gate: this machine starts no worker until someone signs in.
		line += "; " + note.Held
	}
	if note.StarvedSince != "" {
		start, err := parseUTC(note.StarvedSince)
		if err != nil {
			return "", err
		}
		now, err := parseUTC(s.Now())
		if err != nil {
			return "", err
		}
		minutes := int(math.Floor(now.Sub(start).Minutes()))
		line += fmt.Sprintf("; every slot busy for %d min - another machine would help (pulse pair; pulse record show WF-executors)", minutes)
	}
	return line, nil
}

type unfitMark struct {
	Tool string `json:"tool"`
	At   string `json:"at"`
}

// HygieneStatus is one line from the last sweep the daemon (or pulse sweep) ran: when, what it removed and
// how much that freed, what it held, what it could not remove. Empty until a sweep has run.
func HygieneStatus(s *store.Store) (string, error) {
	raw, exists, err := s.Meta("hygiene")
	if err != nil || !exists {
		return "", err
	}
	var note struct {
		At      string         `json:"at"`
		Removed map[string]int `json:"removed"`
		Held    map[string]int `json:"held"`
		Bytes   int64          `json:"bytes"`
		Errors  int            `json:"errors"`
	}
	if err := json.Unmarshal([]byte(raw), &note); err != nil {
		return "", fmt.Errorf("read hygiene note: %w", err)
	}
	counts := func(m map[string]int) string {
		keys := make([]string, 0, len(m))
		for k := range m {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		parts := make([]string, 0, len(keys))
		for _, k := range keys {
			parts = append(parts, fmt.Sprintf("%s %d", k, m[k]))
		}
		return strings.Join(parts, ", ")
	}
	line := "last sweep " + note.At + ": "
	if len(note.Removed) == 0 {
		line += "nothing to remove"
	} else {
		line += "removed " + counts(note.Removed)
		if note.Bytes > 0 {
			line += fmt.Sprintf(" (%s freed)", sizeText(note.Bytes))
		}
	}
	if len(note.Held) > 0 {
		line += "; held " + counts(note.Held)
	}
	if note.Errors > 0 {
		line += fmt.Sprintf("; %d error(s) - pulse sweep --dry-run shows them", note.Errors)
	}
	return line, nil
}

func sizeText(n int64) string {
	switch {
	case n >= 1<<30:
		return fmt.Sprintf("%.1f GB", float64(n)/float64(1<<30))
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MB", float64(n)/float64(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%.0f KB", float64(n)/float64(1<<10))
	}
	return fmt.Sprintf("%d B", n)
}

func unfitMarks(s *store.Store) (map[string]map[string]unfitMark, error) {
	raw, exists, err := s.Meta("unfit")
	if err != nil {
		return nil, err
	}
	marks := map[string]map[string]unfitMark{}
	if exists {
		if err := json.Unmarshal([]byte(raw), &marks); err != nil {
			return nil, fmt.Errorf("read executor unfit marks: %w", err)
		}
	}
	return marks, nil
}

func unfitPhrase(tool string) string {
	switch tool {
	case "services":
		return "needs services"
	case "hands outdated":
		return tool
	default:
		return "lacks " + strings.TrimPrefix(tool, "tool:")
	}
}

func ExecutorsLine(s *store.Store) (string, error) {
	values, err := s.Settings()
	if err != nil {
		return "", err
	}
	now, err := parseUTC(s.Now())
	if err != nil {
		return "", err
	}
	horizon := now.Add(-time.Duration(2*settingInt(values, "minutes_per_job")+15) * time.Minute).Format("2006-01-02T15:04:05Z")
	type executor struct {
		name, status string
		workers      int
	}
	executors := []executor{{"local", "active", max(0, settingInt(values, "workers"))}}
	rows, err := s.DB.Query("SELECT name, status, workers FROM executors ORDER BY name")
	if err != nil {
		return "", err
	}
	for rows.Next() {
		var e executor
		if err := rows.Scan(&e.name, &e.status, &e.workers); err != nil {
			rows.Close()
			return "", err
		}
		executors = append(executors, e)
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return "", err
	}
	marks, err := unfitMarks(s)
	if err != nil {
		return "", err
	}
	parts := make([]string, 0, len(executors))
	for _, e := range executors {
		var busy int
		if err := s.DB.QueryRow("SELECT COUNT(*) FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?", e.name, horizon).Scan(&busy); err != nil {
			return "", err
		}
		part := fmt.Sprintf("%s %s (%d/%d busy)", e.name, e.status, busy, e.workers)
		unfit := map[string]string{}
		for project, mark := range marks[e.name] {
			at, err := parseUTC(mark.At)
			if err != nil {
				return "", err
			}
			if now.Sub(at) < 24*time.Hour {
				unfit[project] = mark.Tool
			}
		}
		blocks, err := RecipeBlocks(s, e.name)
		if err != nil {
			return "", err
		}
		for project, reason := range blocks {
			if _, marked := unfit[project]; !marked {
				unfit[project] = reason
			}
		}
		projects := make([]string, 0, len(unfit))
		for project := range unfit {
			projects = append(projects, project)
		}
		sort.Strings(projects)
		if len(projects) > 0 {
			notes := make([]string, 0, len(projects))
			for _, project := range projects {
				notes = append(notes, unfitPhrase(unfit[project])+" for "+project)
			}
			part += "; " + strings.Join(notes, ", ")
		}
		parts = append(parts, part)
	}
	return strings.Join(parts, ", "), nil
}

func Status(s *store.Store) (*StatusReport, error) {
	r := &StatusReport{Store: s.Path}
	var err error
	var exists bool
	r.SchemaVersion, exists, err = s.Meta("schema_version")
	if err != nil {
		return nil, err
	}
	if !exists {
		return nil, fmt.Errorf("store has no schema_version")
	}
	r.Projects, err = s.Projects(true)
	if err != nil {
		return nil, err
	}
	if r.Projects == nil {
		r.Projects = []store.Project{}
	}
	r.Agents, err = statusCounts(s, "SELECT status, COUNT(*) FROM agents GROUP BY status")
	if err != nil {
		return nil, err
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM posts").Scan(&r.Posts); err != nil {
		return nil, err
	}
	r.Jobs, err = statusCounts(s, "SELECT status, COUNT(*) FROM jobs GROUP BY status")
	if err != nil {
		return nil, err
	}
	now, err := parseUTC(s.Now())
	if err != nil {
		return nil, err
	}
	if err := s.DB.QueryRow(`SELECT COUNT(*) FROM runs WHERE mode != 'dry-run' AND status != 'error'
		AND substr(started_at, 1, 10) = ?`, now.Format("2006-01-02")).Scan(&r.RunsToday); err != nil {
		return nil, err
	}
	r.Settings, err = s.SettingRows()
	if err != nil {
		return nil, err
	}
	if r.Settings == nil {
		r.Settings = []store.SettingRow{}
	}
	values := make(map[string]string, len(r.Settings))
	for i, setting := range r.Settings {
		values[setting.Name] = setting.Value
		r.Settings[i].Value = agentSettingValue(setting.Name, setting.Value)
	}
	r.DailyCap = dailyCap(values)
	r.Runner, err = RunnerStopped(s)
	if err != nil {
		return nil, err
	}
	if r.Runner == "" {
		r.Runner = "running allowed"
	}
	r.Harness = HarnessLine(s)
	r.Daemon, err = DaemonStatus(s)
	if err != nil {
		return nil, err
	}
	if beat, exists, err := readDaemonBeat(s); err == nil && exists {
		r.Bus = beat.Bus
	}
	r.Executors, err = ExecutorsLine(s)
	if err != nil {
		return nil, err
	}
	r.Capacity, err = CapacityStatus(s)
	if err != nil {
		return nil, err
	}
	r.Hygiene, err = HygieneStatus(s)
	if err != nil {
		return nil, err
	}
	r.Planner, err = PlannerNotes(s)
	if err != nil {
		return nil, err
	}
	r.Recovery, err = RecoveryNotes(s)
	if err != nil {
		return nil, err
	}
	r.Landing, err = LandingNotes(s)
	if err != nil {
		return nil, err
	}
	return r, nil
}

func (r *StatusReport) Text() string {
	var out strings.Builder
	fmt.Fprintf(&out, "store      %s (schema %s)\nprojects   %d active\n", r.Store, r.SchemaVersion, len(r.Projects))
	for _, p := range r.Projects {
		brain := ""
		if p.Brain {
			brain = "  (brain)"
		}
		fmt.Fprintf(&out, "  %-18s %s%s\n", p.Slug, p.Path, brain)
	}
	fmt.Fprintf(&out, "agents     %s\nposts      %d\n", joinCounts(r.Agents), r.Posts)
	jobs := joinCounts(r.Jobs)
	if jobs == "" {
		jobs = "none"
	}
	fmt.Fprintf(&out, "jobs       %s\nruns today %d", jobs, r.RunsToday)
	if r.DailyCap > 0 {
		fmt.Fprintf(&out, " of %d (jobs_per_day)\n", r.DailyCap)
	} else {
		out.WriteString(" (no daily cap; jobs_per_day=0)\n")
	}
	fmt.Fprintf(&out, "runner     %s\nharness    %s\ndaemon     %s\n", r.Runner, r.Harness, r.Daemon)
	if r.Bus.Dropped > 0 {
		fmt.Fprintf(&out, "events     %d event(s) dropped for slow subscribers since the daemon started\n", r.Bus.Dropped)
	}
	fmt.Fprintf(&out, "executors  %s\n", r.Executors)
	if r.Capacity != "" {
		fmt.Fprintf(&out, "capacity   %s\n", r.Capacity)
	}
	if r.Hygiene != "" {
		fmt.Fprintf(&out, "hygiene    %s\n", r.Hygiene)
	}
	for _, note := range r.Planner {
		stale := ""
		if note.Stale {
			stale = " (stale)"
		}
		fmt.Fprintf(&out, "planner    %s: %s%s\n", note.Project, note.Note, stale)
	}
	for _, note := range r.Recovery {
		stale := ""
		if note.Stale {
			stale = " (stale)"
		}
		fmt.Fprintf(&out, "recovery   %s: %s%s\n", note.Project, note.Note, stale)
	}
	for _, note := range r.Landing {
		stale := ""
		if note.Stale {
			stale = " (stale)"
		}
		fmt.Fprintf(&out, "landing    %s: %s%s\n", note.Project, note.Note, stale)
	}
	settings := make([]string, len(r.Settings))
	for i, setting := range r.Settings {
		settings[i] = setting.Name + "=" + setting.Value
	}
	fmt.Fprintf(&out, "settings   %s\n", strings.Join(settings, ", "))
	return out.String()
}

func StatusText(s *store.Store) (string, error) {
	report, err := Status(s)
	if err != nil {
		return "", err
	}
	return report.Text(), nil
}

func runStatus(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if err := noArgs(args); err != nil {
		return err
	}
	report, err := Status(c.Store)
	if err != nil {
		return err
	}
	if c.JSON {
		return jsonOutput(c, report)
	}
	c.Printf("%s", report.Text())
	return nil
}
