package runner

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Hygiene: the daemon cleans this machine after itself. Every run and landing leaves things that only
// matter while it is in flight - a clone, a signing key and a bundle under PULSE_HOME/work, a throwaway
// landing worktree, a refs/pulse/landing/<job> holding ref, a run log, a pulse/<agent>/<job> branch that
// waits to land. A run that ends removes its own; a runner that died mid-run, a landing killed half-way or
// an older hub does not, and what stays is not small (a clone with its dependencies is gigabytes). So the
// daemon sweeps once at start and then every sweepEvery: it removes what the store says nothing holds any
// more and keeps what a run in flight, a pending landing or the owner still owns. Each pass logs what went,
// writes one daemon.sweep event and meta.hygiene (pulse status, the hub); an orphan branch is saved as a
// patch before it goes (land.swept). `pulse sweep` is the same pass by hand; daemon.sweep=off leaves it there.

const sweepEvery = 10 * time.Minute

var (
	sweepJobEntryRe = regexp.MustCompile(`^(job-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})(\.[A-Za-z0-9_.-]+)?$`)
	sweepLandRe     = regexp.MustCompile(`^land-(.+)-([0-9]+)$`)
	sweepRunLogRe   = regexp.MustCompile(`^(run-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})\.log$`)
	sweepBranchRe   = regexp.MustCompile(`^pulse/([^/]+)/(job-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})$`)
)

// sweepKinds in the order the report lists them; the bulk kinds are said as one line per pass.
var (
	sweepKinds     = []string{"clone", "worktree", "stray-worktree", "branch", "ref", "scratch", "log", "patch"}
	sweepBulkKinds = map[string]bool{"scratch": true, "log": true, "patch": true}
)

type sweepOpts struct {
	DryRun   bool
	Actor    string
	Inflight map[string]bool // job ids this daemon has in flight, beside the store's running runs
}

// swept is one item the pass removed (Removed) or left (Held) and why.
type swept struct {
	Kind    string `json:"kind"`
	Path    string `json:"path"`
	Project string `json:"project,omitempty"`
	Why     string `json:"why"`
	Bytes   int64  `json:"bytes,omitempty"`
	Patch   string `json:"patch,omitempty"`
}

type sweepReport struct {
	At      string   `json:"at"`
	DryRun  bool     `json:"dry_run"`
	Removed []swept  `json:"removed"`
	Held    []swept  `json:"held"`
	Errors  []string `json:"errors"`
}

func (r *sweepReport) bytes() int64 {
	var n int64
	for _, x := range r.Removed {
		n += x.Bytes
	}
	return n
}

func (r *sweepReport) counts(items []swept) map[string]int {
	out := map[string]int{}
	for _, x := range items {
		out[x.Kind]++
	}
	return out
}

// sweeper carries one pass: the store, the clock, the settings read once, and the projects whose checkout
// answered this tick (a held checkout is not touched, as nothing about it is known).
type sweeper struct {
	s        *store.Store
	now      string
	clock    time.Time
	home     string
	o        sweepOpts
	grace    time.Duration
	branches time.Duration
	keep     time.Duration
	rep      sweepReport
	projects []row
	prune    map[string]bool // project roots whose worktree list needs a prune after this pass
	running  map[string]bool // ids of runs in progress, read once for the log files
}

// sweep is one hygiene pass over this machine. projects are the active projects whose checkout is readable.
func sweep(s *store.Store, projects []row, now string, o sweepOpts) *sweepReport {
	w := &sweeper{s: s, now: now, clock: wallClock(), home: s.Home, o: o, projects: projects, prune: map[string]bool{},
		grace:    time.Duration(settingInt(s, "daemon.sweep_grace_minutes")) * time.Minute,
		branches: time.Duration(settingInt(s, "daemon.sweep_branch_days")) * 24 * time.Hour,
		keep:     time.Duration(settingInt(s, "daemon.sweep_keep_days")) * 24 * time.Hour}
	w.rep = sweepReport{At: now, DryRun: o.DryRun, Removed: []swept{}, Held: []swept{}, Errors: []string{}}
	if w.o.Actor == "" {
		w.o.Actor = "daemon"
	}
	w.sweepWork()
	w.sweepStrayWorktrees()
	for _, proj := range projects {
		w.sweepCheckout(proj)
	}
	w.sweepFiles(filepath.Join(w.home, "logs"), "log", func(name string) bool { return sweepRunLogRe.MatchString(name) })
	if folders, err := os.ReadDir(filepath.Join(w.home, "patches")); err == nil {
		for _, f := range folders {
			if f.IsDir() {
				w.sweepFiles(filepath.Join(w.home, "patches", f.Name()), "patch", func(name string) bool { return strings.HasSuffix(name, ".patch") })
			}
		}
	}
	sort.SliceStable(w.rep.Removed, func(a, b int) bool { return w.kindRank(w.rep.Removed[a].Kind) < w.kindRank(w.rep.Removed[b].Kind) })
	sort.SliceStable(w.rep.Held, func(a, b int) bool { return w.kindRank(w.rep.Held[a].Kind) < w.kindRank(w.rep.Held[b].Kind) })
	return &w.rep
}

func (w *sweeper) kindRank(kind string) int {
	for i, k := range sweepKinds {
		if k == kind {
			return i
		}
	}
	return len(sweepKinds)
}

func (w *sweeper) held(kind, path, project, why string) {
	w.rep.Held = append(w.rep.Held, swept{Kind: kind, Path: path, Project: project, Why: why})
}

func (w *sweeper) failed(format string, args ...any) {
	w.rep.Errors = append(w.rep.Errors, fmt.Sprintf(format, args...))
}

// remove takes one path off the disk (a directory whole) and records it; in a dry run it records only.
func (w *sweeper) remove(kind, path, project, why string) bool {
	item := swept{Kind: kind, Path: path, Project: project, Why: why, Bytes: diskUsage(path)}
	if !w.o.DryRun {
		if err := os.RemoveAll(path); err != nil {
			w.failed("cannot remove %s: %s", path, err)
			return false
		}
	}
	w.rep.Removed = append(w.rep.Removed, item)
	return true
}

// jobHold says what still owns a job's leftovers: a run in progress, a lease, this daemon's own dispatch.
func (w *sweeper) jobHold(jobID string) string {
	if w.o.Inflight[jobID] {
		return "run in flight"
	}
	if run := mustOne(w.s.DB, "SELECT id FROM runs WHERE job_id = ? AND status = 'running' LIMIT 1", jobID); run != nil {
		return "run " + run.str("id") + " in progress"
	}
	if job := mustOne(w.s.DB, "SELECT status FROM jobs WHERE id = ?", jobID); job != nil && job.str("status") == "leased" {
		return "job leased"
	}
	return ""
}

// jobKnown reports whether the store has the job and whether its last run was lost (a dead runner's leftover
// needs no grace: the runner that made it is gone).
func (w *sweeper) jobKnown(jobID string) (known, lost bool) {
	if mustOne(w.s.DB, "SELECT 1 AS x FROM jobs WHERE id = ?", jobID) == nil {
		return false, false
	}
	last := mustOne(w.s.DB, "SELECT status FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1", jobID)
	return true, last != nil && last.str("status") == "lost"
}

func (w *sweeper) age(path string) (time.Duration, bool) {
	st, err := os.Lstat(path)
	if err != nil {
		return 0, false
	}
	return w.clock.Sub(st.ModTime()), true
}

// sweepWork walks PULSE_HOME/work: a job's clone (a directory named by the job id), its key, bundle and
// signers files and an older hub's brief (job id plus a suffix), and land-<slug>-<pid> landing worktrees.
// Anything else there is not the runner's and is left alone.
func (w *sweeper) sweepWork() {
	work := filepath.Join(w.home, "work")
	entries, err := os.ReadDir(work)
	if err != nil {
		return
	}
	for _, e := range entries {
		name := e.Name()
		path := filepath.Join(work, name)
		if m := sweepLandRe.FindStringSubmatch(name); m != nil && e.IsDir() {
			w.sweepLandWorktree(path, m[1], m[2])
			continue
		}
		m := sweepJobEntryRe.FindStringSubmatch(name)
		if m == nil {
			continue
		}
		jobID := m[1]
		kind := "scratch"
		if e.IsDir() && m[2] == "" {
			kind = "clone"
		}
		if why := w.jobHold(jobID); why != "" {
			w.held(kind, path, "", why)
			continue
		}
		known, lost := w.jobKnown(jobID)
		age, ok := w.age(path)
		if !ok {
			continue
		}
		switch {
		case lost:
			w.remove(kind, path, "", "left by a runner that stopped mid-run ("+jobID+")")
		case known && age >= w.grace:
			w.remove(kind, path, "", fmt.Sprintf("%s finished, untouched for %s", jobID, roundDuration(age)))
		case !known && age >= w.keep:
			w.remove(kind, path, "", fmt.Sprintf("no job %s in this store, untouched for %s", jobID, roundDuration(age)))
		case known:
			w.held(kind, path, "", fmt.Sprintf("%s finished %s ago; removed after %s", jobID, roundDuration(age), roundDuration(w.grace)))
		default:
			w.held(kind, path, "", fmt.Sprintf("no job %s in this store; removed after %s untouched", jobID, roundDuration(w.keep)))
		}
	}
}

// sweepLandWorktree removes a landing worktree whose landing process is gone: `git worktree remove` in the
// project it belongs to when that is known and readable, else the directory, and a prune of the project's
// worktree list either way.
func (w *sweeper) sweepLandWorktree(path, slug, pidText string) {
	pid, _ := strconv.Atoi(pidText)
	if pid > 0 && processAlive(pid) {
		w.held("worktree", path, slug, fmt.Sprintf("landing by pid %d still running", pid))
		return
	}
	age, ok := w.age(path)
	if !ok {
		return
	}
	if age < w.grace {
		w.held("worktree", path, slug, fmt.Sprintf("pid %d gone; removed after %s untouched", pid, roundDuration(w.grace)))
		return
	}
	root := ""
	for _, proj := range w.projects {
		if proj.str("slug") == slug {
			root = proj.str("path")
		}
	}
	why := fmt.Sprintf("landing by pid %d is gone", pid)
	if w.o.DryRun {
		w.rep.Removed = append(w.rep.Removed, swept{Kind: "worktree", Path: path, Project: slug, Why: why, Bytes: diskUsage(path)})
		return
	}
	if root != "" {
		gitTimeout(root, 60*time.Second, "worktree", "remove", "--force", path)
		w.prune[root] = true
	}
	if _, err := os.Lstat(path); err == nil || !os.IsNotExist(err) {
		w.remove("worktree", path, slug, why)
	} else {
		w.rep.Removed = append(w.rep.Removed, swept{Kind: "worktree", Path: path, Project: slug, Why: why})
	}
}

// sweepStrayWorktrees looks after PULSE_HOME/worktrees, where an older hub kept per-project worktrees. An
// entry that is (or holds) a worktree whose repository still lists it, or a repository of its own, is
// somebody's and is only reported; a dead one - its repository forgot it, or it is a plain folder - goes
// after the grace. The check reads the entries' own .git pointers, so a project whose checkout did not
// answer this tick is still safe.
func (w *sweeper) sweepStrayWorktrees() {
	dir := filepath.Join(w.home, "worktrees")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	for _, e := range entries {
		path := filepath.Join(dir, e.Name())
		if live, isRepo := liveWorktree(path); live != "" {
			if isRepo {
				w.held("stray-worktree", path, "", "a repository of its own ("+live+"); remove it by hand")
			} else {
				w.held("stray-worktree", path, "", "a worktree git still lists ("+live+"); `git worktree remove` it by hand")
			}
			continue
		}
		age, ok := w.age(path)
		if !ok {
			continue
		}
		if age < w.grace {
			w.held("stray-worktree", path, "", fmt.Sprintf("no repository lists it; removed after %s untouched", roundDuration(w.grace)))
			continue
		}
		w.remove("stray-worktree", path, "", fmt.Sprintf("no repository lists it, untouched for %s", roundDuration(age)))
	}
}

// liveWorktree looks at path and one level under it for a git worktree whose repository still has it (the
// .git file's gitdir exists) or a repository of its own (a .git directory). It returns the first such
// path and whether that is a repository; "" when there is none.
func liveWorktree(path string) (live string, isRepo bool) {
	check := func(p string) bool {
		st, err := os.Stat(filepath.Join(p, ".git"))
		if err != nil {
			return false
		}
		if st.IsDir() {
			live, isRepo = p, true
			return true
		}
		b, err := os.ReadFile(filepath.Join(p, ".git"))
		if err != nil {
			return false
		}
		gitdir, ok := strings.CutPrefix(strings.TrimSpace(string(b)), "gitdir: ")
		if !ok {
			return false
		}
		if !filepath.IsAbs(gitdir) {
			gitdir = filepath.Join(p, gitdir)
		}
		if fileExists(gitdir) {
			live, isRepo = p, false
			return true
		}
		return false
	}
	if check(path) {
		return live, isRepo
	}
	if kids, err := os.ReadDir(path); err == nil {
		for _, k := range kids {
			if k.IsDir() && check(filepath.Join(path, k.Name())) {
				return live, isRepo
			}
		}
	}
	return "", false
}

// sweepCheckout tends one project's checkout: holding refs a dead landing step left, branches no job will
// land, and the worktree list when a registered worktree is gone.
func (w *sweeper) sweepCheckout(proj row) {
	root, slug := proj.str("path"), proj.str("slug")
	if !fileExists(filepath.Join(root, ".git")) {
		return
	}
	refs := gitTimeout(root, 30*time.Second, "for-each-ref", "--format=%(refname)", "refs/pulse/landing/")
	if refs.ok() {
		for _, ref := range strings.Fields(refs.Stdout) {
			jobID := strings.TrimPrefix(ref, "refs/pulse/landing/")
			if why := w.jobHold(jobID); why != "" {
				w.held("ref", ref, slug, why)
				continue
			}
			if !w.o.DryRun && !gitTimeout(root, 30*time.Second, "update-ref", "-d", ref).ok() {
				w.failed("cannot delete %s in %s", ref, root)
				continue
			}
			w.rep.Removed = append(w.rep.Removed, swept{Kind: "ref", Path: ref, Project: slug, Why: "holding ref of a landing step that is over (" + jobID + ")"})
		}
	}
	def := proj.str("default_branch")
	if def == "" || def == "HEAD" {
		def = "main"
	}
	head := gitBranch(root)
	tips := gitTimeout(root, 30*time.Second, "for-each-ref", "--format=%(refname:short)%09%(committerdate:unix)%09%(objectname)",
		"refs/heads/pulse/", "refs/heads/evolve/")
	if tips.ok() {
		for _, line := range strings.Split(tips.Stdout, "\n") {
			f := strings.Split(strings.TrimSpace(line), "\t")
			if len(f) < 3 || f[0] == "" {
				continue
			}
			w.sweepBranch(root, slug, def, head, f[0], f[1], f[2])
		}
	}
	list := gitTimeout(root, 30*time.Second, "worktree", "list", "--porcelain")
	if w.prune[root] || (list.ok() && strings.Contains(list.Stdout, "\nprunable")) {
		if !w.o.DryRun {
			gitTimeout(root, 30*time.Second, "worktree", "prune")
		}
	}
}

// branchHold says who still needs a pulse/* or evolve/* branch: the checkout (it is HEAD), a job that waits
// to land it or has it in flight, or a run in progress on its job.
func (w *sweeper) branchHold(branch, head string) string {
	if branch == head {
		return "checked out"
	}
	if job := mustOne(w.s.DB, "SELECT id, status FROM jobs WHERE branch = ? AND status IN ('done', 'queued', 'leased') ORDER BY created_at DESC LIMIT 1", branch); job != nil {
		if job.str("status") == "done" {
			return "waiting to land (job " + job.str("id") + ")"
		}
		return "job " + job.str("id") + " " + job.str("status")
	}
	if m := sweepBranchRe.FindStringSubmatch(branch); m != nil {
		if why := w.jobHold(m[2]); why != "" {
			return why
		}
	}
	return ""
}

func (w *sweeper) sweepBranch(root, slug, def, head, branch, tipUnix, tip string) {
	if why := w.branchHold(branch, head); why != "" {
		w.held("branch", branch, slug, why)
		return
	}
	secs, _ := strconv.ParseInt(tipUnix, 10, 64)
	age := w.clock.Sub(time.Unix(secs, 0))
	if age < w.branches {
		w.held("branch", branch, slug, fmt.Sprintf("no job will land it; last commit %s ago, removed after %s", roundDuration(age), roundDuration(w.branches)))
		return
	}
	why := fmt.Sprintf("no job will land it; last commit %s ago", roundDuration(age))
	item := swept{Kind: "branch", Path: branch, Project: slug, Why: why}
	merged := gitTimeout(root, 30*time.Second, "merge-base", "--is-ancestor", branch, def).ok()
	if merged {
		item.Why += "; every commit is on " + def
	} else if !w.o.DryRun {
		base := strings.TrimSpace(gitTimeout(root, 30*time.Second, "merge-base", def, branch).Stdout)
		if base == "" {
			w.failed("cannot find where %s left %s in %s; branch kept", branch, def, root)
			return
		}
		item.Patch = saveBranchPatch(root, base, branch, w.home, slug)
		if st, err := os.Stat(item.Patch); err != nil || st.Size() == 0 {
			_ = os.Remove(item.Patch)
			w.held("branch", branch, slug, "no job will land it, but its commits do not fit a patch; `git branch -D` it by hand once they are safe")
			return
		}
		item.Why += "; commits saved at " + item.Patch
	} else {
		item.Why += "; its commits would be saved as a patch first"
	}
	if !w.o.DryRun {
		if del := gitTimeout(root, 30*time.Second, "branch", "-D", branch); !del.ok() {
			w.failed("cannot delete %s in %s: %s", branch, root, strings.TrimSpace(del.Stderr))
			return
		}
		eventAt(w.s.DB, w.now, w.o.Actor, "land.swept", branch, map[string]any{"project": slug, "tip": tip, "patch": item.Patch, "merged": merged, "why": why})
	}
	w.rep.Removed = append(w.rep.Removed, item)
}

// sweepFiles removes the files in dir that match and have not been touched for the keep period; a running
// run's log is held whatever its age.
func (w *sweeper) sweepFiles(dir, kind string, match func(name string) bool) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	if w.running == nil {
		w.running = map[string]bool{}
		for _, r := range mustRows(w.s.DB, "SELECT id FROM runs WHERE status = 'running'") {
			w.running[r.str("id")] = true
		}
	}
	for _, e := range entries {
		if e.IsDir() || !match(e.Name()) {
			continue
		}
		path := filepath.Join(dir, e.Name())
		if m := sweepRunLogRe.FindStringSubmatch(e.Name()); m != nil && w.running[m[1]] {
			w.held(kind, path, "", "run in progress")
			continue
		}
		age, ok := w.age(path)
		if !ok || age < w.keep {
			continue
		}
		w.remove(kind, path, "", fmt.Sprintf("older than %s", roundDuration(w.keep)))
	}
}

// processAlive: kill -0; EPERM means the process exists but is not ours.
func processAlive(pid int) bool {
	err := syscall.Kill(pid, 0)
	return err == nil || err == syscall.EPERM
}

// diskUsage is the size of a file or of everything under a directory; 0 when unreadable.
func diskUsage(path string) int64 {
	var n int64
	_ = filepath.WalkDir(path, func(_ string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		if info, err := d.Info(); err == nil {
			n += info.Size()
		}
		return nil
	})
	return n
}

func roundDuration(d time.Duration) string {
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	default:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	}
}

func sizeText(n int64) string {
	switch {
	case n >= 1<<30:
		return fmt.Sprintf("%.1f GB", float64(n)/(1<<30))
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%.0f KB", float64(n)/(1<<10))
	}
	return fmt.Sprintf("%d B", n)
}

// sweepLines are the daemon's log lines for one pass: one per clone, worktree, branch or ref, one per bulk
// kind (scratch files, logs, patches), one per error; nothing when nothing was removed.
func sweepLines(rep *sweepReport) []string {
	verb := "swept"
	if rep.DryRun {
		verb = "would sweep"
	}
	var lines []string
	bulk := map[string]struct {
		n     int
		bytes int64
	}{}
	for _, x := range rep.Removed {
		if sweepBulkKinds[x.Kind] {
			b := bulk[x.Kind]
			b.n++
			b.bytes += x.Bytes
			bulk[x.Kind] = b
			continue
		}
		size := ""
		if x.Bytes > 0 {
			size = ", " + sizeText(x.Bytes)
		}
		where := x.Path
		if x.Project != "" && (x.Kind == "branch" || x.Kind == "ref") {
			where = x.Project + " " + x.Path
		}
		lines = append(lines, fmt.Sprintf("%s %s %s (%s%s)", verb, x.Kind, where, x.Why, size))
	}
	for _, kind := range sweepKinds {
		b, ok := bulk[kind]
		if !ok {
			continue
		}
		noun := map[string]string{"scratch": "scratch file", "log": "run log", "patch": "patch"}[kind]
		lines = append(lines, fmt.Sprintf("%s %d %s (%s)", verb, b.n, plural(b.n, noun), sizeText(b.bytes)))
	}
	for _, e := range rep.Errors {
		lines = append(lines, "sweep: "+e)
	}
	return lines
}

// recordSweep writes the pass where the owner looks: meta.hygiene for pulse status and the hub, and one
// daemon.sweep event when something went or failed.
func recordSweep(s *store.Store, rep *sweepReport, now, actor string) {
	note := map[string]any{"at": now, "removed": rep.counts(rep.Removed), "held": rep.counts(rep.Held),
		"bytes": rep.bytes(), "errors": len(rep.Errors)}
	text, _ := json.Marshal(note)
	_, _ = s.DB.Exec("INSERT INTO meta (key, value) VALUES ('hygiene', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(text))
	if len(rep.Removed) == 0 && len(rep.Errors) == 0 {
		return
	}
	examples := []string{}
	for _, x := range rep.Removed {
		if len(examples) == 5 {
			break
		}
		examples = append(examples, x.Kind+" "+x.Path)
	}
	eventAt(s.DB, now, actor, "daemon.sweep", localExecutor, map[string]any{"removed": rep.counts(rep.Removed), "bytes": rep.bytes(),
		"held": len(rep.Held), "errors": rep.Errors, "examples": examples})
}

// tendSweep is the daemon phase: one pass when due, its log lines. inflight are the job ids of this daemon's
// dispatches; a run's clone is never swept from under it.
func tendSweep(s *store.Store, projects []row, now, actor string, inflight map[string]bool, last *time.Time) []string {
	if s.Setting("daemon.sweep", "on") == "off" {
		return nil
	}
	if !last.IsZero() && wallClock().Sub(*last) < sweepEvery {
		return nil
	}
	*last = wallClock()
	rep := sweep(s, projects, now, sweepOpts{Actor: actor, Inflight: inflight})
	recordSweep(s, rep, now, actor)
	lines := sweepLines(rep)
	olderThan := plusMinutes(now, -float64(settingInt(s, "minutes_per_job")*2+10))
	if n, err := s.MarkLostHarnessSessions(olderThan); err == nil && n > 0 {
		lines = append(lines, fmt.Sprintf("closed %d harness session(s) left running before %s", n, olderThan))
	}
	return lines
}
