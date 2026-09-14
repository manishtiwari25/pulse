package runner

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Landing (ADR-024): verified work reaches the default branch by itself. The merge is tried in a throwaway
// worktree, the project's own check runs on the merged result, and only then does the checkout fast-forward.
const (
	landLockMinutes  = 30
	landRequeueLimit = 2
	resultFile       = "PULSE_RESULT.md" // older workers committed one; landing still drops it
)

var (
	landedBranchPrefixes = []string{"pulse/", "evolve/"}
	landRefusedPaths     = []string{"docs/evolution/constitution.md", "docs/evolution/evaluators.md", "docs/evolution/STOP", ".github/workflows/"}
	humanLandedPaths     = []string{recipePath}
	checkFiles           = append([]string{"Makefile", "GNUmakefile", "makefile", "package.json", "pyproject.toml", "setup.cfg", "setup.py", "tox.ini",
		"pytest.ini", "noxfile.py", "conftest.py", "Justfile", "justfile", "Taskfile.yml", "Taskfile.yaml", "Rakefile"}, setupLockfiles()...)
	checkConfigRe = regexp.MustCompile(`^[^/]+\.config\.(js|cjs|mjs|ts|json)$`)
)

func setupLockfiles() []string {
	out := make([]string, 0, len(setupRules)+1)
	for _, r := range setupRules {
		out = append(out, r.lockfile)
	}
	return append(out, recipePath)
}

func branchFiles(root, base, branch string) []string {
	out := gitTimeout(root, 60*time.Second, "diff", "--name-only", base+".."+branch)
	if !out.ok() {
		return nil
	}
	var files []string
	for _, f := range strings.Split(out.Stdout, "\n") {
		if strings.TrimSpace(f) != "" {
			files = append(files, f)
		}
	}
	sort.Strings(files)
	return files
}

func refusedPaths(files []string, branch string) []string {
	guarded := append([]string{}, landRefusedPaths...)
	if strings.HasPrefix(branch, "pulse/") {
		guarded = append(guarded, ownerPaths...)
	}
	var out []string
	for _, f := range files {
		for _, p := range guarded {
			if f == p || strings.HasPrefix(f, p) {
				out = append(out, f)
				break
			}
		}
	}
	sort.Strings(out)
	return out
}

// checkDefinitionFiles: the changed paths that define the check itself.
func checkDefinitionFiles(verifyCmd string, files []string, root string) []string {
	names := map[string]bool{}
	for _, f := range checkFiles {
		names[f] = true
	}
	for _, tok := range splitShell(verifyCmd) {
		clean := strings.TrimRight(strings.TrimLeft(strings.Trim(tok, `'"`), "./"), "/")
		if clean != "" && !strings.HasPrefix(clean, "-") && (strings.Contains(clean, "/") || fileExists(filepath.Join(root, clean))) {
			names[clean] = true
		}
	}
	var out []string
	for _, f := range files {
		if names[f] || checkConfigRe.MatchString(f) {
			out = append(out, f)
			continue
		}
		for n := range names {
			if strings.HasPrefix(f, n+"/") {
				out = append(out, f)
				break
			}
		}
	}
	return out
}

func runBaseCheckFiles(worktree, before string, files []string) {
	for _, f := range files {
		if gitTimeout(worktree, 10*time.Second, "cat-file", "-e", before+":"+f).ok() {
			gitTimeout(worktree, 60*time.Second, "checkout", before, "--", f)
		} else {
			_ = os.Remove(filepath.Join(worktree, f))
		}
	}
}

func checkoutState(root, defaultBranch string) (onDefault, clean bool) {
	head := gitTimeout(root, 10*time.Second, "rev-parse", "--abbrev-ref", "HEAD")
	status := gitTimeout(root, 60*time.Second, "status", "--porcelain", "--untracked-files=no")
	return head.ok() && strings.TrimSpace(head.Stdout) == defaultBranch, status.ok() && strings.TrimSpace(status.Stdout) == ""
}

// landingLock: one landing at a time per checkout; "" when another landing holds it.
func landingLock(root, now string) string {
	path := filepath.Join(root, ".git", "pulse-land.lock")
	for i := 0; i < 2; i++ {
		f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err != nil {
			if !os.IsExist(err) {
				return ""
			}
			st, serr := os.Stat(path)
			if serr != nil {
				continue
			}
			if time.Since(st.ModTime()) < landLockMinutes*time.Minute {
				return ""
			}
			_ = os.Remove(path)
			continue
		}
		fmt.Fprintf(f, "%d %s\n", os.Getpid(), now)
		f.Close()
		return path
	}
	return ""
}

func saveBranchPatch(root, base, branch, home, slug string) string {
	folder := filepath.Join(home, "patches", slug)
	_ = os.MkdirAll(folder, 0o755)
	out := gitTimeout(root, 120*time.Second, "format-patch", "--stdout", base+".."+branch)
	path := filepath.Join(folder, strings.ReplaceAll(branch, "/", "__")+".patch")
	text := ""
	if out.ok() {
		text = out.Stdout
	}
	_ = os.WriteFile(path, []byte(text), 0o644)
	return path
}

func requeueJob(s *store.Store, now string, job row, actor, why, patch string) string {
	earlier := mustOne(s.DB, "SELECT count(*) AS n FROM events WHERE action = 'job.requeue' AND subject = ?", job.str("id")).int("n")
	kept := ""
	if patch != "" {
		kept = "; earlier attempt kept at " + patch
	}
	if earlier >= landRequeueLimit {
		_, _ = finishJob(s, job.str("id"), actor, "failed", fmt.Sprintf("not landed after %d requeues: %s%s", earlier, why, kept), "", -1)
		exec1(s.DB, "UPDATE jobs SET branch = '' WHERE id = ?", job.str("id"))
		eventAt(s.DB, now, actor, "job.abandoned", job.str("id"), map[string]any{"why": why, "patch": patch, "requeues": earlier})
		return "failed"
	}
	_, _ = finishJob(s, job.str("id"), actor, "queued", fmt.Sprintf("requeued: %s%s", why, kept), "", -1)
	exec1(s.DB, "UPDATE jobs SET attempts = 0, branch = '' WHERE id = ?", job.str("id"))
	eventAt(s.DB, now, actor, "job.requeue", job.str("id"), map[string]any{"why": why, "patch": patch})
	return "queued"
}

func jobIDOf(job row) string {
	if job == nil {
		return ""
	}
	return job.str("id")
}

// holdRecipeBranch: a branch that changes the recipe file alone waits for a human landing (ADR-026).
func holdRecipeBranch(s *store.Store, project row, root, base, branch, actor string, job row, now string) (string, string) {
	slug := project.str("slug")
	recipe, problems, _ := readRecipe(s, slug, root, branch)
	diff := gitTimeout(root, 60*time.Second, "diff", base+".."+branch, "--", recipePath).Stdout
	command := fmt.Sprintf("pulse land --project %s --branch %s", slug, branch)
	existing := mustOne(s.DB, "SELECT id FROM posts WHERE project = ? AND kind = 'question' AND area = 'runner' AND thread = 'environment' AND summary LIKE ?",
		slug, "%"+branch+"%")
	if existing == nil {
		state := recipeSummary(recipe)
		if len(problems) > 0 {
			shown := problems
			if len(shown) > 3 {
				shown = shown[:3]
			}
			state = "invalid: " + strings.Join(shown, "; ")
		}
		summary := trunc(fmt.Sprintf("Recipe branch %s waits for a human landing (%s)", branch, state), 230)
		forJob := ""
		if job != nil {
			forJob = " for job " + job.str("id")
		}
		invalid := ""
		if len(problems) > 0 {
			var items []string
			for _, p := range problems {
				items = append(items, "- "+p)
			}
			invalid = "\n\nThe branch's recipe does not parse:\n" + strings.Join(items, "\n")
		}
		diffText := trunc(strings.TrimSpace(diff), 3000)
		if diffText == "" {
			diffText = "(no diff text)"
		}
		body := fmt.Sprintf("`%s` changed alone on `%s`%s (ADR-026). The runner lands no recipe change by itself in this slice: "+
			"read the diff, then land it with\n\n    %s\n\nor delete the branch (`git branch -D %s`) to drop it.%s\n\n```diff\n%s\n```\n",
			recipePath, branch, forJob, command, branch, invalid, diffText)
		postID := ""
		if p, err := createPost(s, now, postOpts{Kind: "question", Project: slug, Area: "runner", Thread: "environment", Summary: summary,
			Body: body, Agent: actorAgent(s, actor), Date: now, ForOwner: true}); err == nil {
			postID = p.str("id")
		}
		eventAt(s.DB, now, actor, "env.held", branch, map[string]any{"project": slug, "job": jobIDOf(job), "post": postID, "valid": len(problems) == 0})
	}
	return "held", fmt.Sprintf("%s changes %s alone; it waits for a human: %s", branch, recipePath, command)
}

func refuseBranch(s *store.Store, now, root, base, branch, actor string, job row, home, slug, why string) (string, string) {
	patch := saveBranchPatch(root, base, branch, home, slug)
	if job != nil {
		_, _ = finishJob(s, job.str("id"), actor, "failed", fmt.Sprintf("not landed: %s; patch at %s", why, patch), "", -1)
		exec1(s.DB, "UPDATE jobs SET branch = '' WHERE id = ?", job.str("id"))
	}
	gitTimeout(root, 30*time.Second, "branch", "-D", branch)
	eventAt(s.DB, now, actor, "land.refused", branch, map[string]any{"project": slug, "patch": patch, "why": why, "job": jobIDOf(job)})
	what := "branch removed"
	if job != nil {
		what = fmt.Sprintf("job %s failed", job.str("id"))
	}
	return "refused", fmt.Sprintf("%s not landed: %s; %s; patch at %s", branch, why, what, patch)
}

type landOpts struct {
	Job              row
	Verify           bool
	ByHand           bool
	Ctx              context.Context
	ArchiveID, RunID string
}

func shWithTimeout(dir, cmd string, budget time.Duration) (rc *int, stdout, stderr string) {
	return shWithContext(nil, dir, cmd, budget)
}

// landBranch is land_branch(): land one branch on the project's default branch. Returns (state, note).
func landBranch(s *store.Store, project row, branch, actor, now string, o landOpts) (state, note string, err error) {
	root := project.str("path")
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	home := s.Home
	job := o.Job
	if why := landingStop(o.Ctx, s, root); why != "" {
		return "stopped", why, nil
	}
	okPrefix := false
	for _, p := range landedBranchPrefixes {
		if strings.HasPrefix(branch, p) {
			okPrefix = true
		}
	}
	if !okPrefix {
		return "", "", fail(2, "pulse lands only %s branches, not %s", "pulse/* and evolve/*", branch)
	}
	exists, unreadable := branchExists(root, branch)
	if unreadable != "" {
		return "unreadable", fmt.Sprintf("%s kept: %s", branch, unreadable), nil
	}
	if !exists {
		return "missing", fmt.Sprintf("branch %s does not exist in %s", branch, root), nil
	}
	if job != nil && job.str("kind") == "experiment" && o.ArchiveID == "" {
		o.RunID = lastRunID(s, job.str("id"))
		archived, err := experimentArchive(s, project.str("slug"), branch, o.RunID)
		if err != nil {
			return "", "", err
		}
		if archived != nil && archived.int("kept") == 1 {
			o.ArchiveID = archived.str("id")
		}
	}
	if gitTimeout(root, 30*time.Second, "merge-base", "--is-ancestor", branch, def).ok() {
		if err := recordLanding(s, project, branch, actor, "land.merged", "", o, map[string]any{}); err != nil {
			return "", "", err
		}
		gitTimeout(root, 30*time.Second, "branch", "-D", branch)
		return "merged", fmt.Sprintf("%s was already contained in %s; branch removed", branch, def), nil
	}
	base := strings.TrimSpace(gitTimeout(root, 30*time.Second, "merge-base", def, branch).Stdout)
	files := branchFiles(root, base, branch)
	slug := project.str("slug")
	if refused := refusedPaths(files, branch); len(refused) > 0 {
		if len(refused) > 4 {
			refused = refused[:4]
		}
		st, n := refuseBranch(s, now, root, base, branch, actor, job, home, slug, fmt.Sprintf("it changes %s, which no landing accepts", strings.Join(refused, ", ")))
		return st, n, nil
	}
	if len(files) == 0 || (len(files) == 1 && files[0] == resultFile) {
		st, n := refuseBranch(s, now, root, base, branch, actor, job, home, slug, "it changes nothing but "+resultFile)
		return st, n, nil
	}
	var humanLanded, others []string
	for _, f := range files {
		if contains(humanLandedPaths, f) {
			humanLanded = append(humanLanded, f)
		} else if f != resultFile {
			others = append(others, f)
		}
	}
	if len(humanLanded) > 0 {
		if len(others) > 0 {
			if len(others) > 3 {
				others = others[:3]
			}
			st, n := refuseBranch(s, now, root, base, branch, actor, job, home, slug,
				fmt.Sprintf("recipe changes travel alone: %s changed with %s", strings.Join(humanLanded, ", "), strings.Join(others, ", ")))
			return st, n, nil
		}
		if !o.ByHand {
			st, n := holdRecipeBranch(s, project, root, base, branch, actor, job, now)
			return st, n, nil
		}
	}
	onDefault, clean := checkoutState(root, def)
	if !onDefault {
		return "deferred", fmt.Sprintf("the checkout of %s is not on %s", root, def), nil
	}
	if !clean {
		return "deferred", fmt.Sprintf("the checkout of %s has uncommitted changes", root), nil
	}
	lock := landingLock(root, now)
	if lock == "" {
		return "locked", fmt.Sprintf("another landing holds %s", filepath.Join(root, ".git", "pulse-land.lock")), nil
	}
	worktree := filepath.Join(home, "work", fmt.Sprintf("land-%s-%d", slug, os.Getpid()))
	title := branch
	if job != nil && job.str("title") != "" {
		title = job.str("title")
	}
	defer func() {
		gitTimeout(root, 60*time.Second, "worktree", "remove", "--force", worktree)
		gitTimeout(root, 30*time.Second, "worktree", "prune")
		_ = os.Remove(lock)
	}()
	before := strings.TrimSpace(gitTimeout(root, 10*time.Second, "rev-parse", "--verify", "refs/heads/"+def).Stdout)
	gitTimeout(root, 60*time.Second, "worktree", "remove", "--force", worktree)
	if added := gitTimeout(root, 120*time.Second, "worktree", "add", "--detach", worktree, def); !added.ok() {
		return "", "", fail(1, "git worktree add failed: %s", strings.TrimSpace(added.Stderr))
	}
	merged := gitTimeout(worktree, 120*time.Second, "merge", "--ff-only", branch)
	if !merged.ok() {
		jobLine := ""
		if job != nil {
			jobLine = "Pulse-Job: " + job.str("id") + "\n"
		}
		msg := fmt.Sprintf("Land %s: %s\n\nPulse-Landed-By: %s\n%s", branch, trunc(title, 200), actor, jobLine)
		merged = gitTimeout(worktree, 300*time.Second, "-c", "user.name=pulse", "-c", "user.email=pulse@pulse.local", "-c", "commit.gpgsign=false",
			"merge", "--no-ff", "--no-edit", "-m", msg, branch)
	}
	if !merged.ok() {
		gitTimeout(worktree, 60*time.Second, "merge", "--abort")
		patch := saveBranchPatch(root, base, branch, home, slug)
		if job != nil {
			requeueJob(s, now, job, actor, fmt.Sprintf("%s conflicts with %s", branch, def), patch)
		}
		gitTimeout(root, 30*time.Second, "branch", "-D", branch)
		eventAt(s.DB, now, actor, "land.conflict", branch, map[string]any{"project": slug, "patch": patch, "job": jobIDOf(job)})
		what := "branch removed"
		if job != nil {
			what = fmt.Sprintf("job %s requeued on the current %s", job.str("id"), def)
		}
		return "conflict", fmt.Sprintf("%s conflicts with %s; %s; patch at %s", branch, def, what, patch), nil
	}
	if gitTimeout(worktree, 10*time.Second, "ls-files", "--error-unmatch", resultFile).ok() {
		gitTimeout(worktree, 30*time.Second, "rm", "-q", resultFile)
		gitTimeout(worktree, 60*time.Second, "-c", "user.name=pulse", "-c", "user.email=pulse@pulse.local", "-c", "commit.gpgsign=false",
			"commit", "-q", "-m", fmt.Sprintf("Land %s: drop %s", branch, resultFile))
	}
	sha := strings.TrimSpace(gitTimeout(worktree, 10*time.Second, "rev-parse", "HEAD").Stdout)
	verifyNote := "no verify command"
	if vc := project.str("verify_cmd"); o.Verify && vc != "" {
		minutes := settingInt(s, "minutes_per_job")
		setupBudget := minutes
		if setupBudget < setupMinutes {
			setupBudget = setupMinutes
		}
		guarded := checkDefinitionFiles(vc, files, root)
		if len(guarded) > 0 {
			runBaseCheckFiles(worktree, before, guarded)
		}
		baseRecipe, baseProblems, _ := readRecipe(s, slug, root, before)
		if len(baseProblems) > 0 {
			baseRecipe = nil
		}
		setupCmd, setupFiles, _ := projectSetup(root, before, baseRecipe, worktree)
		if setupCmd != "" {
			budget := time.Duration(setupBudget) * time.Minute
			if budget < 60*time.Second {
				budget = 60 * time.Second
			}
			rc, so, se := shWithContext(o.Ctx, worktree, setupCmd, budget)
			if why := landingStop(o.Ctx, s, root); why != "" {
				return "stopped", why, nil
			}
			prepTail := squeeze(so+se, 300)
			if rc == nil {
				prepTail = fmt.Sprintf("timed out after %d min", setupBudget)
			}
			if rc == nil || *rc != 0 {
				var rcVal any
				if rc != nil {
					rcVal = *rc
				}
				eventAt(s.DB, now, actor, "land.deferred", branch, map[string]any{"project": slug, "setup": strings.Join(setupFiles, ", "), "rc": rcVal, "tail": prepTail})
				return "deferred", fmt.Sprintf("setup from %s failed on this machine (%s); nothing landed, try again once it is fixed", strings.Join(setupFiles, ", "), prepTail), nil
			}
		}
		budget := time.Duration(minutes) * time.Minute
		if budget < 60*time.Second {
			budget = 60 * time.Second
		}
		rc, so, se := shWithContext(o.Ctx, worktree, vc, budget)
		if why := landingStop(o.Ctx, s, root); why != "" {
			return "stopped", why, nil
		}
		ok, tailText := false, ""
		if rc == nil {
			verifyNote = fmt.Sprintf("verify timed out after %d min", minutes)
		} else {
			ok, verifyNote = *rc == 0, fmt.Sprintf("verify exit %d", *rc)
			tailText = tail(so, 4000) + tail(se, 4000)
		}
		if len(guarded) > 0 {
			shown := guarded
			extra := ""
			if len(shown) > 3 {
				shown, extra = shown[:3], fmt.Sprintf(" +%d", len(guarded)-3)
			}
			verifyNote += fmt.Sprintf(", check run with %s's %s", def, strings.Join(shown, ", ")+extra)
		}
		if !ok {
			patch := saveBranchPatch(root, base, branch, home, slug)
			if job != nil {
				requeueJob(s, now, job, actor, fmt.Sprintf("%s on the merged %s: %s", verifyNote, def, branch), patch)
			}
			gitTimeout(root, 30*time.Second, "branch", "-D", branch)
			eventAt(s.DB, now, actor, "land.failed-verify", branch, map[string]any{"project": slug, "patch": patch, "note": verifyNote, "tail": tail(tailText, 1500)})
			what := "branch removed"
			if job != nil {
				what = fmt.Sprintf("job %s requeued", job.str("id"))
			}
			return "failed-verify", fmt.Sprintf("%s on the merged %s; %s; patch at %s", verifyNote, def, what, patch), nil
		}
	}
	if why := landingStop(o.Ctx, s, root); why != "" {
		return "stopped", why, nil
	}
	onDefault, clean = checkoutState(root, def)
	if !(onDefault && clean) {
		return "deferred", "the checkout changed while the merge was verified; nothing landed, try again", nil
	}
	if ff := gitTimeout(root, 120*time.Second, "merge", "--ff-only", sha); !ff.ok() {
		return "deferred", fmt.Sprintf("%s moved while the merge was verified; nothing landed, try again (%s)", def, trunc(strings.TrimSpace(ff.Stderr), 200)), nil
	}
	commits := strings.TrimSpace(gitTimeout(root, 30*time.Second, "rev-list", "--count", before+".."+sha).Stdout)
	if commits == "" {
		commits = "?"
	}
	if err := recordLanding(s, project, branch, actor, "land.landed", fmt.Sprintf(" | landed %s from %s", short(sha, 12), branch), o,
		map[string]any{"sha": sha, "commits": commits, "verify": verifyNote}); err != nil {
		return "", "", err
	}
	gitTimeout(root, 30*time.Second, "branch", "-D", branch)
	area, thread := "evolution", slugify(branch[strings.LastIndex(branch, "/")+1:], 40)
	if job != nil {
		area, thread = "jobs", job.str("id")
	}
	vcShown := project.str("verify_cmd")
	if vcShown == "" {
		vcShown = "no verify command"
	}
	defCopy := def
	_, _ = createPost(s, now, postOpts{Kind: "result", Project: slug, Area: area, Thread: thread,
		Summary: trunc(fmt.Sprintf("landed on %s as %s (%s commit(s), %s): %s", def, short(sha, 12), commits, verifyNote, title), 230),
		Body: fmt.Sprintf("Branch `%s` landed on `%s` by %s under ADR-024 (agent-driven landing): merged in a throwaway worktree, "+
			"`%s` on the merged result (%s), then a fast-forward of the checkout. The branch is removed; the commits are on %s.\n",
			branch, def, actor, vcShown, verifyNote, def),
		Agent: actorAgent(s, actor), Branch: &defCopy, Resolution: "proceeded"})
	return "landed", fmt.Sprintf("%s landed on %s as %s (%s, %s commit(s))", branch, def, short(sha, 12), verifyNote, commits), nil
}

func pendingLandings(q querier, slug string) []row {
	return mustRows(q, `SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'done' AND j.branch != ''
		AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
		ORDER BY j.finished_at, j.created_at`, slug)
}

type landResult struct{ State, Note string }

// landPending lands every waiting branch of one project, oldest first; stops at the first deferral.
func landPending(s *store.Store, project row, actor, now string, verify bool, limit int, force bool) []landResult {
	return landPendingContext(context.Background(), s, project, actor, now, verify, limit, force)
}

func landPendingContext(ctx context.Context, s *store.Store, project row, actor, now string, verify bool, limit int, force bool) []landResult {
	if !force && s.Setting("landing", "") != "agents" {
		return nil
	}
	var out []landResult
	for _, job := range pendingLandings(s.DB, project.str("slug")) {
		if limit > 0 && len(out) >= limit {
			break
		}
		if job.str("kind") == "experiment" {
			// D-M: an experiment is measured before it may land; a dropped or unmeasured one never lands.
			state, note := landExperimentContext(ctx, s, project, job, actor, now, verify)
			out = append(out, landResult{state, note})
			if state == "deferred" || state == "locked" || state == "stopped" || state == "unreadable" {
				break
			}
			continue
		}
		state, note, err := landBranch(s, project, job.str("branch"), actor, now, landOpts{Job: job, Verify: verify, Ctx: ctx})
		if err != nil {
			state, note = "error", errMessage(err)
		}
		if state == "missing" {
			exec1(s.DB, "UPDATE jobs SET branch = '' WHERE id = ?", job.str("id"))
			eventAt(s.DB, now, actor, "land.missing", job.str("branch"), map[string]any{"job": job.str("id")})
		}
		out = append(out, landResult{state, note})
		if state == "deferred" || state == "locked" || state == "stopped" || state == "unreadable" {
			break
		}
	}
	return out
}

// branchExists answers whether refs/heads/<branch> exists in root, or why the checkout could not be read at
// all (not a repository, a folder macOS has not let the daemon open, git missing or hung). A branch is only
// "missing" when the repository itself answered; an unreadable checkout keeps the job's branch so verified
// work is not forgotten over a permission prompt.
func branchExists(root, branch string) (exists bool, unreadable string) {
	if fi, err := os.Stat(root); err != nil || !fi.IsDir() {
		return false, fmt.Sprintf("cannot read %s: %s", root, errText(err, "not a directory"))
	}
	probe := gitTimeout(root, 10*time.Second, "rev-parse", "--git-dir")
	if !probe.ok() {
		return false, fmt.Sprintf("cannot read the repository at %s: %s", root, gitFailure(probe))
	}
	res := gitTimeout(root, 10*time.Second, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch)
	if res.ok() {
		return true, ""
	}
	if res.Err != nil || res.Code != 1 || strings.TrimSpace(res.Stderr) != "" {
		return false, fmt.Sprintf("git could not answer for %s in %s: %s", branch, root, gitFailure(res))
	}
	return false, ""
}

func gitFailure(g gitResult) string {
	if g.Err != nil {
		return g.Err.Error()
	}
	if msg := strings.TrimSpace(g.Stderr); msg != "" {
		return msg
	}
	if g.Code == -1 {
		return "git did not answer in time and was stopped"
	}
	return fmt.Sprintf("exit %d", g.Code)
}

func errText(err error, fallback string) string {
	if err != nil {
		return err.Error()
	}
	return fallback
}

// heldLandings: waiting branches that change the recipe alone, newest first.
func heldLandings(q querier, slug, root string) []string {
	rows := mustRows(q, "SELECT subject FROM events WHERE action = 'env.held' AND json_extract(detail, '$.project') = ? ORDER BY seq DESC", slug)
	seen := map[string]bool{}
	var out []string
	for _, r := range rows {
		b := r.str("subject")
		if seen[b] {
			continue
		}
		seen[b] = true
		if root != "" && !gitTimeout(root, 10*time.Second, "rev-parse", "--verify", "--quiet", "refs/heads/"+b).ok() {
			continue
		}
		out = append(out, b)
	}
	return out
}
