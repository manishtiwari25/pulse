package board

import (
	"database/sql"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// RemoveItem is one line of a remove preview section.
type RemoveItem struct {
	Label  string `json:"label"`
	Detail string `json:"detail"`
}

// RemovePreview is what `pulse remove` shows before (and records after) it
// acts: what changes in the store, what stays, and the commands that are the
// human's to run. Nothing is ever deleted.
type RemovePreview struct {
	Kind    string `json:"kind"` // project or executor
	Name    string `json:"name"`
	DryRun  bool   `json:"dry_run"`
	Applied bool   `json:"applied"`

	Changes []RemoveItem `json:"changes"`
	Stays   []RemoveItem `json:"stays"`
	Yours   []string     `json:"yours"`

	CancelledJobs []string `json:"cancelled_jobs"`
	LeasedJobs    []string `json:"leased_jobs"`
	Executors     []string `json:"executors"`
}

func (p *RemovePreview) change(label, detail string) {
	p.Changes = append(p.Changes, RemoveItem{label, detail})
}
func (p *RemovePreview) stay(label, detail string) {
	p.Stays = append(p.Stays, RemoveItem{label, detail})
}

func count(s *store.Store, query string, args ...any) (int, error) {
	var n int
	err := s.DB.QueryRow(query, args...).Scan(&n)
	return n, err
}

// RemoveProject deactivates a project: status inactive, its queued jobs
// cancelled, executors that named it updated; posts, runs, events and the
// checkout stay. With dryRun the preview is computed and nothing is written.
func RemoveProject(s *store.Store, slug, actor string, dryRun bool) (*RemovePreview, error) {
	p, err := s.Project(slug)
	if err != nil {
		return nil, err
	}
	if p == nil {
		return nil, refuseFix(2, "No project "+pyRepr(slug), "only a registered project can be removed", "pulse projects")
	}
	if p.Brain {
		return nil, refuseFix(2, "Refusing to remove "+slug+": the brain is the framework checkout; deactivate with pulse settings, not remove",
			"the brain holds the constitution, the guide and the framework's own board", "pulse settings set stop 1  (pauses every runner)")
	}
	if p.Status != "active" {
		return nil, refuseFix(2, "Project "+slug+" is already "+p.Status, "remove deactivates an active project", "pulse init "+p.Path+"  (registers it again)")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	now := s.Now()
	pre := &RemovePreview{Kind: "project", Name: slug, DryRun: dryRun, CancelledJobs: []string{}, LeasedJobs: []string{}, Executors: []string{}, Yours: []string{}}
	pre.change("project "+slug, "status active -> inactive")

	rows, err := s.DB.Query("SELECT id, status, lease_agent, title FROM jobs WHERE project = ? AND status IN ('queued', 'leased') ORDER BY status, priority DESC, created_at", slug)
	if err != nil {
		return nil, err
	}
	type jobLine struct{ id, status, agent, title string }
	var jobs []jobLine
	for rows.Next() {
		var j jobLine
		if err := rows.Scan(&j.id, &j.status, &j.agent, &j.title); err != nil {
			rows.Close()
			return nil, err
		}
		jobs = append(jobs, j)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}
	for _, j := range jobs {
		if j.status == "queued" {
			pre.CancelledJobs = append(pre.CancelledJobs, j.id)
		} else {
			pre.LeasedJobs = append(pre.LeasedJobs, j.id)
		}
	}
	if len(pre.CancelledJobs) > 0 {
		pre.change(fmt.Sprintf("%d queued %s", len(pre.CancelledJobs), plural(len(pre.CancelledJobs), "job")), "-> cancelled")
		for _, j := range jobs {
			if j.status == "queued" {
				pre.change("  "+j.id, trunc(j.title, 60))
			}
		}
	} else {
		pre.stay("no queued jobs", "nothing to cancel")
	}
	if len(pre.LeasedJobs) > 0 {
		pre.change(fmt.Sprintf("%d leased %s", len(pre.LeasedJobs), plural(len(pre.LeasedJobs), "job")), "left to finish (the worker reports; nothing new is leased)")
		for _, j := range jobs {
			if j.status == "leased" {
				pre.change("  "+j.id, "leased by "+j.agent)
			}
		}
	}

	type exLine struct{ name, projects, after string }
	var exs []exLine
	erows, err := s.DB.Query("SELECT name, projects FROM executors WHERE projects != '*' ORDER BY name")
	if err != nil {
		return nil, err
	}
	for erows.Next() {
		var e exLine
		if err := erows.Scan(&e.name, &e.projects); err != nil {
			erows.Close()
			return nil, err
		}
		var keep []string
		hit := false
		for _, x := range strings.Split(e.projects, ",") {
			if x == slug {
				hit = true
			} else if x != "" {
				keep = append(keep, x)
			}
		}
		if hit {
			e.after = strings.Join(keep, ",")
			exs = append(exs, e)
		}
	}
	erows.Close()
	if err := erows.Err(); err != nil {
		return nil, err
	}
	for _, e := range exs {
		pre.Executors = append(pre.Executors, e.name)
		if e.after == "" {
			pre.change("executor "+e.name, fmt.Sprintf("served only %s: projects list left as is; drain it: pulse executor drain %s", slug, e.name))
		} else {
			pre.change("executor "+e.name, fmt.Sprintf("projects %s -> %s", e.projects, e.after))
		}
	}

	posts, err := count(s, "SELECT COUNT(*) FROM posts WHERE project = ?", slug)
	if err != nil {
		return nil, err
	}
	runs, err := count(s, "SELECT COUNT(*) FROM runs WHERE project = ?", slug)
	if err != nil {
		return nil, err
	}
	done, err := count(s, "SELECT COUNT(*) FROM jobs WHERE project = ? AND status NOT IN ('queued', 'leased')", slug)
	if err != nil {
		return nil, err
	}
	decisions, err := count(s, "SELECT COUNT(*) FROM decisions WHERE project = ?", slug)
	if err != nil {
		return nil, err
	}
	events, err := count(s, "SELECT COUNT(*) FROM events WHERE subject = ? OR subject IN (SELECT id FROM posts WHERE project = ?) "+
		"OR subject IN (SELECT id FROM jobs WHERE project = ?) OR subject IN (SELECT id FROM runs WHERE project = ?)", slug, slug, slug, slug)
	if err != nil {
		return nil, err
	}
	records, err := count(s, "SELECT COUNT(*) FROM records WHERE project = ?", slug)
	if err != nil {
		records = 0
	}
	pre.stay(fmt.Sprintf("%d %s", posts, plural(posts, "post")), "immutable; the board keeps its history")
	pre.stay(fmt.Sprintf("%d %s, %d finished %s", runs, plural(runs, "run"), done, plural(done, "job")), "the runner's history")
	pre.stay(fmt.Sprintf("%d %s", decisions, plural(decisions, "decision")), "the human's answers")
	pre.stay(fmt.Sprintf("%d %s", events, plural(events, "event")), "append-only; remove adds project.removed")
	if records > 0 {
		pre.stay(fmt.Sprintf("%d %s", records, plural(records, "record")), "decisions, plans and guides in the store")
	}
	pre.stay("the checkout "+p.Path, "untouched on disk")

	pre.Yours = yoursForProject(s, p)

	if dryRun {
		return pre, nil
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive', last_seen = ? WHERE slug = ?", now, slug); err != nil {
		return nil, err
	}
	for _, id := range pre.CancelledJobs {
		if _, err := s.DB.Exec("UPDATE jobs SET status = 'cancelled', result = ?, finished_at = ?, lease_until = '' WHERE id = ? AND status = 'queued'",
			"project "+slug+" removed by "+actor, now, id); err != nil {
			return nil, err
		}
		if err := event(s, actor, "job.cancelled", id, "project "+slug+" removed"); err != nil {
			return nil, err
		}
	}
	for _, e := range exs {
		if e.after == "" {
			continue
		}
		if _, err := s.DB.Exec("UPDATE executors SET projects = ? WHERE name = ?", e.after, e.name); err != nil {
			return nil, err
		}
		if err := event(s, actor, "executor.update", e.name, map[string]any{"projects": e.after, "removed_project": slug}); err != nil {
			return nil, err
		}
	}
	if err := event(s, actor, "project.removed", slug, map[string]any{"path": p.Path, "cancelled": pre.CancelledJobs,
		"leased": pre.LeasedJobs, "executors": pre.Executors}); err != nil {
		return nil, err
	}
	pre.Applied = true
	return pre, nil
}

// yoursForProject lists the git and shell commands that clean what the store
// never touches: pulse/* and evolve/* branches, land worktrees, job clones
// and patches under PULSE_HOME. Printed, never run.
func yoursForProject(s *store.Store, p *store.Project) []string {
	var out []string
	if refs, ok := git(p.Path, "for-each-ref", "--format=%(refname:short)", "refs/heads/pulse/", "refs/heads/evolve/"); ok && refs != "" {
		for _, b := range strings.Split(refs, "\n") {
			if b = strings.TrimSpace(b); b != "" {
				out = append(out, fmt.Sprintf("git -C %s branch -D %s", shellQuote(p.Path), shellQuote(b)))
			}
		}
	} else {
		out = append(out, "# no pulse/* or evolve/* branches in "+p.Path)
	}
	var worktrees []string
	worktrees = append(worktrees, globDirs(filepath.Join(s.Home, "work", "land-"+p.Slug+"-*"))...)
	worktrees = append(worktrees, globDirs(filepath.Join(s.Home, "worktrees", p.Slug+"*"))...)
	for _, wt := range worktrees {
		out = append(out, fmt.Sprintf("git -C %s worktree remove --force %s", shellQuote(p.Path), shellQuote(wt)))
	}
	rows, err := s.DB.Query("SELECT id FROM jobs WHERE project = ? ORDER BY created_at", p.Slug)
	if err == nil {
		for rows.Next() {
			var id string
			if rows.Scan(&id) == nil {
				dir := filepath.Join(s.Home, "work", id)
				if st, err := os.Stat(dir); err == nil && st.IsDir() {
					out = append(out, "rm -rf "+shellQuote(dir))
				}
			}
		}
		rows.Close()
	}
	if patches := filepath.Join(s.Home, "patches", p.Slug); dirExists(patches) {
		out = append(out, "rm -rf "+shellQuote(patches))
	}
	return out
}

// RemoveExecutor retires a pair of hands: the row stays with status removed
// (never deleted), its runs and events stay; the machine, its key and its
// login are the human's to retire. Refused while a run is in progress.
func RemoveExecutor(s *store.Store, name, actor string, dryRun bool) (*RemovePreview, error) {
	if name == "" || name == LocalExecutor {
		return nil, refuseFix(2, "The local executor is this machine; use `pulse settings set workers N` to size it",
			"local is not a row in the store", "pulse settings set workers 0")
	}
	var status, address, workdir, projects string
	var workers int
	err := s.DB.QueryRow("SELECT status, address, workdir, projects, workers FROM executors WHERE name = ?", name).Scan(&status, &address, &workdir, &projects, &workers)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, refuseFix(2, "No executor "+name, "only a registered executor can be removed", "pulse executor list")
	}
	if err != nil {
		return nil, err
	}
	if status == "removed" {
		return nil, refuseFix(2, "Executor "+name+" was already removed", "its row stays for the runs it made", "pulse executor list")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	now := s.Now()
	// a run older than twice the job budget plus slack is a crashed runner's leftover, not a busy slot
	horizon, err := PlusHours(now, -float64(2*s.SettingInt("minutes_per_job")+15)/60.0)
	if err != nil {
		return nil, err
	}
	running, err := count(s, "SELECT COUNT(*) FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?", name, horizon)
	if err != nil {
		return nil, err
	}
	if running > 0 {
		return nil, refuseFix(2, fmt.Sprintf("Executor %s has a run in progress; drain it and wait for the run to finish", name),
			fmt.Sprintf("%d %s started after %s", running, plural(running, "run"), horizon), "pulse executor drain "+name)
	}
	pre := &RemovePreview{Kind: "executor", Name: name, DryRun: dryRun, CancelledJobs: []string{}, LeasedJobs: []string{}, Executors: []string{name}, Yours: []string{}}
	pre.change("executor "+name, fmt.Sprintf("status %s -> removed (no job is sent there again)", status))
	if projects != "*" {
		pre.change("  projects "+projects, "no longer served by these hands")
	}
	runs, err := count(s, "SELECT COUNT(*) FROM runs WHERE executor = ?", name)
	if err != nil {
		return nil, err
	}
	events, err := count(s, "SELECT COUNT(*) FROM events WHERE subject = ?", name)
	if err != nil {
		return nil, err
	}
	pre.stay("the executor row", fmt.Sprintf("%s, %s, %d %s; kept for its history", address, workdir, workers, plural(workers, "worker")))
	pre.stay(fmt.Sprintf("%d %s", runs, plural(runs, "run")), "the runner's history on these hands")
	pre.stay(fmt.Sprintf("%d %s", events, plural(events, "event")), "append-only; remove adds executor.removed")
	pre.Yours = []string{
		fmt.Sprintf("ssh %s 'sudo systemctl disable --now pulse-hands 2>/dev/null; sudo userdel -r pulse; sudo rm -rf %s'", name, shellQuote(workdir)),
		fmt.Sprintf("ssh-keygen -R %s", hostOf(address)),
		fmt.Sprintf("# drop the `Host %s` block from ~/.ssh/config and the key ~/.ssh/pulse_hands_ed25519 when no other hands use it", name),
	}
	if dryRun {
		return pre, nil
	}
	if _, err := s.DB.Exec("UPDATE executors SET status = 'removed', probe_note = ?, next_probe_at = '' WHERE name = ?",
		trunc("removed by "+actor+" on "+now, 300), name); err != nil {
		return nil, err
	}
	if err := event(s, actor, "executor.removed", name, map[string]any{"address": address}); err != nil {
		return nil, err
	}
	pre.Applied = true
	return pre, nil
}

func hostOf(address string) string {
	if i := strings.LastIndex(address, "@"); i >= 0 {
		address = address[i+1:]
	}
	if i := strings.LastIndex(address, ":"); i >= 0 {
		address = address[:i]
	}
	if address == "" {
		return "<host>"
	}
	return address
}

func shellQuote(s string) string {
	if s == "" {
		return "''"
	}
	if strings.IndexFunc(s, func(r rune) bool {
		return !(r == '/' || r == '.' || r == '-' || r == '_' || r == '@' || r == ':' || r == '+' || r == '=' ||
			(r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9'))
	}) < 0 {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

func globDirs(pattern string) []string {
	hits, _ := filepath.Glob(pattern)
	var out []string
	for _, h := range hits {
		if dirExists(h) {
			out = append(out, h)
		}
	}
	sort.Strings(out)
	return out
}

func dirExists(p string) bool {
	st, err := os.Stat(p)
	return err == nil && st.IsDir()
}

// Render prints the preview as three aligned sections: Changes (yellow),
// Stays (green), Yours (cyan) when colour is on.
func (p *RemovePreview) Render(w io.Writer, st cli.Style) {
	title := "pulse remove " + p.Kind + " " + p.Name
	switch {
	case p.Applied:
		title += "  " + st.Dim("(applied)")
	case p.DryRun:
		title += "  " + st.Dim("(dry run - nothing changed)")
	default:
		title += "  " + st.Dim("(preview)")
	}
	fmt.Fprintln(w, st.Bold(title))
	fmt.Fprintln(w)
	section := func(name string, colour func(string) string, items []RemoveItem) {
		fmt.Fprintln(w, colour(st.Bold(name)))
		width := 0
		for _, it := range items {
			if n := len([]rune(it.Label)); n > width {
				width = n
			}
		}
		for _, it := range items {
			pad := strings.Repeat(" ", width-len([]rune(it.Label)))
			if it.Detail == "" {
				fmt.Fprintf(w, "  %s\n", colour(it.Label))
			} else {
				fmt.Fprintf(w, "  %s%s  %s\n", colour(it.Label), pad, it.Detail)
			}
		}
		fmt.Fprintln(w)
	}
	section("Changes", st.Yellow, p.Changes)
	section("Stays", st.Green, p.Stays)
	fmt.Fprintln(w, st.Cyan(st.Bold("Yours")), st.Dim("(printed, never run by pulse)"))
	if len(p.Yours) == 0 {
		fmt.Fprintln(w, "  nothing")
	}
	for _, line := range p.Yours {
		if strings.HasPrefix(line, "#") {
			fmt.Fprintf(w, "  %s\n", st.Dim(line))
		} else {
			fmt.Fprintf(w, "  %s\n", st.Cyan(line))
		}
	}
}
