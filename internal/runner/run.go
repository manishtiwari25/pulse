package runner

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	launchSeconds = 30
	maxAttempts   = 3
)

// runOpts is the keyword surface of run_job() / work_once().
type runOpts struct {
	DryRun    bool
	WorkerCmd string
	Executor  string
	Ctx       context.Context
}

// wallClock is the run's stopwatch; tests may shorten launchSeconds through it.
var wallClock = func() time.Time { return time.Now() }

func runnerStopped(s *store.Store) string {
	if s.Setting("stop", "0") == "1" {
		return "the runner is stopped in the store (pulse start to resume)"
	}
	if brain := brainProject(s.DB); brain != nil && fileExists(filepath.Join(brain.str("path"), "docs", "evolution", "STOP")) {
		return fmt.Sprintf("docs/evolution/STOP is present in the brain (%s); a human removes it", brain.str("path"))
	}
	return ""
}

// runsToday counts worker runs started today; dry runs and runner errors do not count.
func runsToday(s *store.Store, now string) int {
	r := mustOne(s.DB, "SELECT COUNT(*) AS n FROM runs WHERE mode != 'dry-run' AND status != 'error' AND substr(started_at, 1, 10) = ?", now[:10])
	return r.int("n")
}

func dailyCap(s *store.Store) int {
	raw := strings.ToLower(strings.TrimSpace(s.Setting("jobs_per_day", "")))
	switch raw {
	case "", "0", "none", "off", "unlimited":
		return 0
	}
	n, err := strconv.Atoi(raw)
	if err != nil {
		return 2
	}
	if n < 0 {
		return 0
	}
	return n
}

// takeJob is take_job(): kill switch, daily cap, the executor's state and limits, then the lease.
func takeJob(s *store.Store, agent, project, jobID string, dryRun bool, now, executor string, taken int) (row, row, string, error) {
	if why := runnerStopped(s); why != "" {
		return nil, nil, why, nil
	}
	if !dryRun {
		cap := dailyCap(s)
		done := runsToday(s, now) + taken
		if cap > 0 && done >= cap {
			return nil, nil, fmt.Sprintf("daily cap reached: %d of %d jobs today (setting jobs_per_day)", done, cap), nil
		}
	}
	if executor == "" {
		executor = localExecutor
	}
	exrow := executorRow(s, executor)
	if exrow == nil {
		return nil, nil, "", fail(2, "unknown executor %s (pulse executor list)", executor)
	}
	if exrow.str("status") != "active" {
		return nil, exrow, fmt.Sprintf("executor %s is %s", exrow.str("name"), exrow.str("status")), nil
	}
	var projects []string
	anyProj := exrow.str("projects") == "*"
	if !anyProj {
		projects = strings.Split(exrow.str("projects"), ",")
	}
	if project != "" && !anyProj && !contains(projects, project) {
		return nil, exrow, fmt.Sprintf("executor %s does not serve %s (projects %s)", exrow.str("name"), project, exrow.str("projects")), nil
	}
	unfit := unfitProjects(s.DB, exrow.str("name"), now)
	var only []string
	if project != "" {
		only = []string{project}
	}
	for slug, why := range recipeBlocks(s, exrow.str("name"), only) {
		if _, ok := unfit[slug]; !ok {
			unfit[slug] = why
		}
	}
	// A checkout the daemon could not read this loop is not cloned from, bundled or landed into (any executor).
	for slug := range heldCheckouts() {
		unfit[slug] = recipeUnfitCheckout
	}
	if len(unfit) > 0 {
		if why, ok := unfit[project]; project != "" && ok {
			if why == recipeUnfitServices || why == recipeUnfitHands || why == recipeUnfitCheckout {
				return nil, exrow, fmt.Sprintf("executor %s %s for %s (%s)", exrow.str("name"), unfitPhrase(why), project, toolHint(why)), nil
			}
			return nil, exrow, fmt.Sprintf("executor %s lacks %s for %s (install it, then `pulse executor probe %s`)",
				exrow.str("name"), why, project, exrow.str("name")), nil
		}
		if anyProj {
			projects = nil
			for _, r := range mustRows(s.DB, "SELECT slug FROM projects WHERE status = 'active'") {
				projects = append(projects, r.str("slug"))
			}
			anyProj = false
		}
		var kept []string
		for _, p := range projects {
			if _, bad := unfit[p]; !bad {
				kept = append(kept, p)
			}
		}
		projects = kept
		if len(projects) == 0 {
			slugs := sortedKeys(unfit)
			var reasons []string
			for _, slug := range slugs {
				reasons = append(reasons, fmt.Sprintf("%s for %s", unfitPhrase(unfit[slug]), slug))
			}
			first := unfit[slugs[0]]
			hint := fmt.Sprintf("install it, then `pulse executor probe %s`", exrow.str("name"))
			if first == recipeUnfitServices || first == recipeUnfitHands || first == recipeUnfitCheckout {
				hint = toolHint(first)
			}
			return nil, exrow, fmt.Sprintf("executor %s %s (%s)", exrow.str("name"), strings.Join(reasons, "; "), hint), nil
		}
	}
	job, err := leaseJob(s, agent, now, leaseOpts{Project: project, JobID: jobID, Projects: projects, AnyProj: anyProj})
	if err != nil {
		return nil, exrow, "", err
	}
	if job == nil {
		why := "no queued job"
		if project != "" {
			why += " for " + project
		}
		if exrow.str("name") != localExecutor {
			why += " on " + exrow.str("name")
		}
		return nil, exrow, why, nil
	}
	return job, exrow, "", nil
}

// workOnce leases one job and runs it; a reason string when nothing ran.
func workOnce(s *store.Store, agent, project, jobID, now string, o runOpts) (row, string, error) {
	job, exrow, reason, err := takeJob(s, agent, project, jobID, o.DryRun, now, o.Executor, 0)
	if err != nil {
		return nil, "", err
	}
	if job == nil {
		return nil, reason, nil
	}
	o.Executor = exrow.str("name")
	run, err := runJob(s, job, agent, now, o)
	return run, "", err
}

var miseNotFoundRe = regexp.MustCompile(`mise: (command )?not found|mise: No such file or directory`)

func squeeze(s string, n int) string {
	t := tail(strings.Join(strings.Fields(s), " "), n)
	if t == "" {
		return "no output"
	}
	return t
}

// runJob is run_job(): one leased job in a clone on an executor, commits landed on a local branch, everything
// recorded. The brief is the harness prompt and a `briefs` row (D-L); the worker reports with
// `pulse job finish`, the harness output is parsed as the fallback.
func runJob(s *store.Store, job row, agent, now string, o runOpts) (row, error) {
	ctx := o.Ctx
	if ctx == nil {
		ctx = context.Background()
	}
	project := projectBySlug(s.DB, job.str("project"))
	if project == nil {
		return nil, fail(2, "Job %s names an unknown project %s", job.str("id"), job.str("project"))
	}
	root := project.str("path")
	if !fileExists(filepath.Join(root, ".git")) {
		return nil, fail(2, "Project %s at %s is not a git checkout", project.str("slug"), root)
	}
	minutes := settingInt(s, "minutes_per_job")
	home := s.Home
	workDir := filepath.Join(home, "work")
	logsDir := filepath.Join(home, "logs")
	_ = os.MkdirAll(workDir, 0o755)
	_ = os.MkdirAll(logsDir, 0o755)
	exName := o.Executor
	if exName == "" {
		exName = localExecutor
	}
	exrow := executorRow(s, exName)
	if exrow == nil {
		return nil, fail(2, "unknown executor %s (pulse executor list)", o.Executor)
	}
	runID := newID("run", now)
	mode := "real"
	if o.DryRun {
		mode = "dry-run"
	} else if o.WorkerCmd != "" {
		mode = "custom"
	}
	logPath := filepath.Join(logsDir, runID+".log")
	log := &runLog{}
	ex, err := makeExecutor(exrow, s, home, job, project, agent, log)
	if err != nil {
		return nil, err
	}
	clone := ex.clone()
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) "+
		"VALUES (?, ?, ?, ?, ?, 'running', ?, ?, ?, ?)", runID, job.str("id"), agent, project.str("slug"), now, clone, logPath, mode, ex.name())
	eventAt(s.DB, now, agent, "run.start", runID, map[string]any{"job": job.str("id"), "mode": mode, "executor": ex.name()})
	ctx, cancelRun := context.WithCancel(ctx)
	var ownerCancelled atomic.Bool
	stopCancelWatch := watchRunHarnessCancel(ctx, s, runID, cancelRun, &ownerCancelled)
	defer stopCancelWatch()
	if job.str("kind") == "baseline" {
		// ADR-031: the runner measures the default branch itself; no clone, brief, worker or landing.
		return runBaseline(s, job, project, agent, now, runID, logPath, ex, o)
	}
	if agent != humanAgent {
		// The model follows the job kind (store.RunModel), so a worker or lead runs under
		// whichever model this job gets; the agent is joined without one - joinAgent refuses a
		// name whose recorded model differs - and its row then records the model of this run,
		// which the posts of the run report.
		model := s.RunModel(job.str("kind"))
		if o.WorkerCmd != "" {
			model = ""
		}
		kind := "worker"
		if a := agentRow(s.DB, agent); a != nil {
			kind = a.str("kind")
		}
		if _, _, err := joinAgent(s, agent, "", "", runID, "", kind, ""); err != nil {
			return nil, err
		}
		if model != "" {
			exec1(s.DB, "UPDATE agents SET model = ? WHERE name = ?", model, agent)
		}
	}
	started := wallClock()
	status, commits, branch, verifyNote, summary := "failed", 0, "", "", ""
	var exitCode *int
	parsed := parseResultText(nil)
	var ballot []ballotItem
	transportError, fault := false, false
	var unfit *ExecutorUnfit
	pubkey := ""
	workerReported := "" // the status the worker gave through `pulse job finish`, when it did
	setInt := func(n int) { exitCode = &n }

	body := func() error {
		var err error
		pubkey, err = makeRunKey(ex.keyPath(), agent, runID)
		if err != nil {
			return err
		}
		exec1(s.DB, "UPDATE runs SET signing_pubkey = ? WHERE id = ?", pubkey, runID)
		// A missing or empty key is a setup-error before any clone exists, on
		// both the local and the ssh branch; the later checks before runWorker
		// catch a key that vanishes in between.
		if err := preflightRunKey(ex.keyPath()); err != nil {
			return err
		}
		base, err := ex.prepare(ctx)
		if err != nil {
			return err
		}
		recipe, problems, found := readRecipe(s, project.str("slug"), root, base)
		if len(problems) > 0 {
			recipeFinding(s, project, problems, base, now, agent)
			shown := problems
			if len(shown) > 3 {
				shown = shown[:3]
			}
			return fail(1, "%s at %s is not a valid recipe (%s); the job runs again once it is fixed", recipePath, short(base, 12), strings.Join(shown, "; "))
		}
		switch recipeBlock(recipe, ex.remote()) {
		case recipeUnfitServices:
			svc := recipe.Services
			if len(svc) > 4 {
				svc = svc[:4]
			}
			return &ExecutorUnfit{fmt.Sprintf("%s lists services (%s) that no machine starts; %s waits until an owner clears the list",
				recipePath, strings.Join(svc, ", "), project.str("slug")), recipeUnfitServices, project.str("slug")}
		case recipeUnfitHands:
			return &ExecutorUnfit{fmt.Sprintf("%s pins tools or env that the hands on %s cannot take yet; %s jobs run on the hub until the hands are refreshed",
				recipePath, ex.name(), project.str("slug")), recipeUnfitHands, project.str("slug")}
		}
		var pins []string
		if recipe != nil {
			pins = recipe.Tools
		}
		setupBudget := minutes
		if setupBudget < setupMinutes {
			setupBudget = setupMinutes
		}
		if len(pins) > 0 && !o.DryRun {
			rc, mout := ex.verify(ctx, miseInstallCommand(pins, clone), setupBudget)
			log.add("$ mise install %s  # tools from %s\n%s", strings.Join(pins, " "), recipePath, tail(mout, 20000))
			if rc == nil {
				return fail(1, "mise install on %s timed out after %d minutes (%s); the job goes back to the queue", ex.name(), setupBudget, strings.Join(pins, ", "))
			}
			if *rc != 0 {
				t := squeeze(mout, 300)
				if *rc == 127 || miseNotFoundRe.MatchString(mout) {
					return &ExecutorUnfit{fmt.Sprintf("%s pins tools but `mise` is not installed on %s (%s); %s jobs run elsewhere until it is and `pulse executor probe %s` passes",
						recipePath, ex.name(), t, project.str("slug"), ex.name()), recipeUnfitMise, project.str("slug")}
				}
				failed := miseFailedPin(pins, mout)
				return &ExecutorUnfit{fmt.Sprintf("mise could not install %s on %s (%s); %s jobs run elsewhere until it can and `pulse executor probe %s` passes",
					failed, ex.name(), t, project.str("slug"), ex.name()), "tool:" + failed, project.str("slug")}
			}
		}
		setupCmd, setupFiles, setupShown := projectSetup(root, base, recipe, clone)
		if setupCmd != "" && !o.DryRun {
			rc, sout := ex.verify(ctx, setupCmd, setupBudget)
			log.add("$ %s  # setup from %s\n%s", setupCmd, strings.Join(setupFiles, ", "), tail(sout, 20000))
			if rc == nil {
				return fail(1, "setup on %s timed out after %d minutes (%s); the job goes back to the queue", ex.name(), setupBudget, strings.Join(setupFiles, ", "))
			}
			if *rc != 0 {
				tool := missingTool(setupCmd, *rc, sout)
				t := squeeze(sout, 300)
				if tool != "" {
					return &ExecutorUnfit{fmt.Sprintf("setup on %s needs `%s`, which is not installed there (%s); %s jobs run elsewhere until it is and `pulse executor probe %s` passes",
						ex.name(), tool, t, project.str("slug"), ex.name()), tool, project.str("slug")}
				}
				return fail(1, "setup failed on %s (exit %d): %s", ex.name(), *rc, t)
			}
		}
		func() {
			defer func() {
				if r := recover(); r != nil {
					log.add("ballot unavailable: %v", r)
				}
			}()
			ballot = ballotItemsFor(s, project.str("slug"), agent, now, runID)
			if job.str("kind") == "ballot" {
				for i := range ballot {
					if p := postRow(s.DB, ballot[i].ID); p != nil {
						ballot[i].Body = p.str("body")
					}
				}
			}
		}()
		briefText := writeBrief(briefOpts{Job: job, Project: project, Agent: agent, Clone: clone, Minutes: minutes,
			Ballot: ballot, HasBall: len(ballot) > 0, Remote: ex.remote(), Setup: setupShown, Recipe: recipe, Found: found,
			Lessons: recentFailureLessons(s, project.str("slug"), now)})
		exec1(s.DB, "INSERT OR REPLACE INTO briefs (run_id, job_id, agent, text, at) VALUES (?, ?, ?, ?, ?)", runID, job.str("id"), agent, briefText, now)
		if o.DryRun {
			status = "dry-run"
			setInt(0)
			summary = fmt.Sprintf("dry run: clone prepared at %s on %s, brief written, no worker started", clone, ex.name())
			log.put(summary)
			return nil
		}
		cmd, customWorker := workerCommand(s, job.str("kind"), briefText, clone, o.WorkerCmd, ex.workerDirs(), ex.remote())
		env := map[string]string{"PULSE_JOB": job.str("id"), "PULSE_BRIEF": runID, "PULSE_CLONE": clone,
			"PULSE_AGENT": agent, "PULSE_SESSION": runID, "PULSE_PROJECT": project.str("slug"), "GIT_TERMINAL_PROMPT": "0",
			"PULSE_GENERATION": strconv.Itoa(job.int("generation")), "PULSE_EXECUTOR": ex.name()}
		if ex.remote() && s.HarnessNative() && !customWorker {
			env["PULSE_HARNESS_STEPS"] = filepath.Join(ex.workerDirs()[0], "harness-steps-"+runID+".jsonl")
		}
		if !ex.remote() {
			if exe := localCLI(home); exe != "" {
				env["PULSE_CLI"] = exe
				// The brief tells the worker to run `pulse claim`, `pulse vote` and `pulse job finish`;
				// make sure the binary that launched this run is actually on the worker's PATH.
				if p := cliOnPath(os.Getenv("PATH"), exe); p != "" {
					env["PATH"] = p
				}
			}
			env["PULSE_HOME"] = home
			for _, kv := range workerEnv {
				env[kv[0]] = kv[1]
			}
			if recipe != nil {
				for k, v := range recipe.Env {
					env[k] = v
				}
			}
			if len(pins) > 0 {
				for _, kv := range miseEnv(clone) {
					env[kv[0]] = kv[1]
				}
				cmd = append(miseExecArgv(pins), cmd...)
			}
		} else {
			// A recipe-only PULSE_CLI names the pulse binary on the host; the hands allow PULSE_* through (WU-021).
			// Without one, point at the binary `pulse pair` stages so the brief's `$PULSE_CLI` fallback holds remotely.
			env["PULSE_CLI"] = remotePulseCLI(recipe)
		}
		var shown []string
		for _, c := range cmd {
			if c == briefText {
				shown = append(shown, "<brief>")
			} else {
				shown = append(shown, shellQuote(c))
			}
		}
		if !ex.remote() {
			log.add("%s", preflightLine(pulseOnPath(envWith(environ(), env)), env["PULSE_CLI"]))
		} else if r, ok := ex.(interface{ preflightRemoteRunKey(string) error }); ok {
			// Same preflightRunKey check as localExec.runWorker, run on the host:
			// the key uploaded in prepare must still be there right before the
			// worker starts and signs with it (ADR-022).
			if err := r.preflightRemoteRunKey("before the worker starts"); err != nil {
				return err
			}
		}
		log.add("$ %s", strings.Join(shown, " "))
		launched := wallClock()
		out, err := ex.runWorker(ctx, cmd, env, minutes)
		if err != nil {
			return err
		}
		if !out.Timeout {
			setInt(out.ExitCode)
		}
		log.add("%s", tail(out.Stdout, 20000))
		log.add("%s", tail(out.Stderr, 20000))
		parsed = parseResultText(ex.resultText())
		// D-L: a worker that already ran `pulse job finish` reported through the store, not a file.
		if j := jobRow(s.DB, job.str("id")); j != nil && j.int("generation") == job.int("generation") &&
			j.str("lease_agent") == agent && (j.str("status") == "done" || j.str("status") == "failed" || j.str("status") == "cancelled") {
			workerReported = j.str("status")
			result := j.str("result")
			stored := parseResultText(&result)
			if parsed.Summary == "" {
				parsed.Summary = stored.Summary
				if !stored.reported() {
					parsed.Summary = strings.TrimSpace(result)
				}
			}
			parsed.Found = true
			for _, question := range stored.Questions {
				if !contains(parsed.Questions, question) {
					parsed.Questions = append(parsed.Questions, question)
				}
			}
			log.add("the worker finished job %s itself (%s): %s", job.str("id"), j.str("status"), j.str("result"))
		}
		leaseOK := func(mins int) bool {
			if workerReported != "" {
				j := jobRow(s.DB, job.str("id"))
				return j != nil && j.int("generation") == job.int("generation") && j.str("lease_agent") == agent
			}
			return renewLease(s.DB, now, job.str("id"), agent, job.int("generation"), mins)
		}
		rescued := false
		if out.Timeout {
			if ownerCancelled.Load() {
				status, summary = "failed", "cancelled by the owner"
				setInt(5)
			} else {
				status, exitCode = "timeout", nil
			}
			log.add("timeout after %d minutes", minutes)
			if status == "timeout" && ex.commitCount(base) > 0 {
				rescued = true
				setInt(0)
				if !parsed.Found {
					parsed.Found = true
					parsed.Summary = fmt.Sprintf("hit the %d-minute cap before reporting; the commits it made are kept only because the check passed - the work may be partial", minutes)
				} else {
					sm := parsed.Summary
					if sm == "" {
						sm = "no summary"
					}
					parsed.Summary = fmt.Sprintf("hit the %d-minute cap after committing: %s", minutes, sm)
				}
				log.add("commits found after the timeout: judged like a finished run")
			}
		}
		if status != "timeout" || rescued {
			commits = ex.commitCount(base)
			if dirty := ex.dirty(); dirty != "" {
				log.add("uncommitted changes left in the clone were discarded:\n%s", dirty)
			}
			// Measured from the harness launch, not the clone: a long setup must not
			// hide a harness that died on its first breath.
			elapsed := wallClock().Sub(launched)
			code := -1
			if exitCode != nil {
				code = *exitCode
			}
			switch {
			case code == 5 || ownerCancelled.Load() || workerReported == "cancelled":
				status = "failed"
				summary = "cancelled by the owner"
				if parsed.Summary != "" {
					summary = "cancelled by the owner: " + parsed.Summary
				}
			case code != 0 && !parsed.reported() && workerReported == "" && commits == 0 && (elapsed < launchSeconds*time.Second || (code == 4 && loginFault(out.Stderr+"\n"+out.Stdout))):
				// The harness died before any work: no report line, no commit, seconds in.
				// (parsed.Found is true whenever the harness produced output, D-L.)
				status = "error"
				src := out.Stderr
				if src == "" {
					src = out.Stdout
				}
				summary = fmt.Sprintf("worker rejected at launch (exit %d after %ds): %s", code, int(elapsed.Seconds()), squeeze(src, 300))
				if loginFault(squeeze(src, 300)) {
					if ex.remote() {
						fault = true
						summary += fmt.Sprintf(" - the login on %s, not the job; run `pulse executor login %s --admin <admin@host> --with-token`", ex.name(), ex.name())
					} else if s.HarnessNative() {
						fault = true
						summary += " - the provider login on this machine, not the job; sign in with GitHub in the console (Settings -> Harness) or run `pulse harness login`"
					} else {
						summary += " - the Copilot login on this machine, not the job; log in again as the daemon's user (`copilot` then `/login`, or `gh auth login`)"
					}
				}
			case code != 0:
				status = "failed"
				summary = fmt.Sprintf("worker exited %d", code)
				if parsed.Summary != "" {
					summary = fmt.Sprintf("worker exited %d: %s", code, parsed.Summary)
				}
			case workerReported == "failed":
				status = "failed"
				summary = parsed.Summary
				if summary == "" {
					summary = "the worker reported failure"
				}
			case parsed.incomplete():
				status, summary = "failed", parsed.Summary
			case !parsed.reported() && workerReported == "":
				status = "failed"
				summary = "worker wrote no report (no `pulse job finish`, no SUMMARY line); nothing to land"
			case commits == 0:
				status = "done"
				summary = parsed.Summary
				if summary == "" {
					summary = "no commits; nothing to land"
				}
			case !leaseOK(minutes + 5):
				status = "stale"
				summary = fmt.Sprintf("lease generation %d is no longer current; %d commit(s) discarded", job.int("generation"), commits)
			case len(ex.ownerPathsTouched(base)) > 0:
				status = "failed"
				summary = fmt.Sprintf("%d commit(s) touch owner-owned paths (%s); nothing landed - propose the change instead", commits, strings.Join(ex.ownerPathsTouched(base), ", "))
			default:
				ok := true
				checkTool := ""
				if vc := project.str("verify_cmd"); vc != "" {
					checkCmd := recipeEnvExports(recipe) + vc
					if len(pins) > 0 {
						checkCmd = miseWrapShell(pins, checkCmd, clone)
					}
					rc, vout := ex.verify(ctx, checkCmd, minutes)
					if rc == nil {
						verifyNote, ok = "verify timed out", false
					} else {
						verifyNote = fmt.Sprintf("verify exit %d", *rc)
						ok = *rc == 0
						if !ok {
							checkTool = checkToolMissing(vc, *rc, vout)
						}
					}
					log.add("$ %s\n%s", vc, tail(vout, 40000))
				}
				switch {
				case checkTool != "":
					status = "failed"
					summary = fmt.Sprintf("%d commit(s) but the check could not start on %s: `%s` is not installed there (%s); nothing landed", commits, ex.name(), checkTool, verifyNote)
					unfit = &ExecutorUnfit{summary, checkTool, project.str("slug")}
				case !ok:
					status = "failed"
					summary = fmt.Sprintf("%d commit(s) but %s; nothing landed", commits, verifyNote)
				case !leaseOK(5):
					status = "stale"
					summary = fmt.Sprintf("lease generation %d is no longer current after verify; %d commit(s) discarded", job.int("generation"), commits)
				default:
					// A job created with a branch (an experiment's evolve/<suggestion-id>, D-M) keeps it; others are named here.
					branch = job.str("branch")
					if branch == "" {
						branch = fmt.Sprintf("pulse/%s/%s", agent, job.str("id"))
					}
					if git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch).ok() {
						status = "failed"
						summary = fmt.Sprintf("branch %s already exists in %s; nothing landed", branch, root)
						branch = ""
						break
					}
					holding := "refs/pulse/landing/" + job.str("id")
					fetched, ferr := runGroup(ctx, []string{"git", "-C", root, "fetch", "-q", "--no-tags", ex.fetchSource(), "+HEAD:" + holding}, "", ex.fetchEnv(), nil, 300*time.Second)
					log.add("$ git fetch <%s clone> HEAD:%s\n%s", ex.name(), holding, fetched.Stderr)
					if ferr != nil || fetched.ExitCode != 0 {
						status, branch = "failed", ""
						summary = "could not land the branch: " + strings.TrimSpace(fetched.Stderr)
						break
					}
					problems := landingProblems(root, base, holding, agent, job.str("id"), pubkey, filepath.Join(workDir, job.str("id")+".signers"))
					if len(problems) > 0 {
						status, branch = "failed", ""
						shownP := problems
						if len(shownP) > 3 {
							shownP = shownP[:3]
						}
						summary = fmt.Sprintf("%d commit(s) refused: %s; nothing landed", commits, strings.Join(shownP, "; "))
						log.add("identity check (ADR-022):\n%s", strings.Join(problems, "\n"))
					} else {
						if _, err := gitCheck(root, "update-ref", "refs/heads/"+branch, holding); err != nil {
							return err
						}
						status = "done"
						summary = parsed.Summary
						if summary == "" {
							summary = fmt.Sprintf("%d commit(s) landed on %s", commits, branch)
						}
						if verifyNote != "" {
							summary += fmt.Sprintf(" (%s)", verifyNote)
						}
					}
					git(root, "update-ref", "-d", holding)
				}
			}
		}
		return nil
	}

	func() {
		defer func() {
			if r := recover(); r != nil {
				status, summary = "error", fmt.Sprintf("runner error: %v", r)
				log.put(summary)
			}
		}()
		err := body()
		switch e := err.(type) {
		case nil:
		case *ExecutorUnreachable:
			transportError = true
			status = "error"
			when := "before"
			if ex.started() {
				status, when = "failed", "after"
			}
			summary = fmt.Sprintf("executor %s unreachable %s worker start: %s", ex.name(), when, e.Error())
			log.put(summary)
		case *ExecutorFault:
			fault = true
			status, summary = "error", e.Error()
			log.put(summary)
		case *ExecutorUnfit:
			unfit = e
			status, summary = "error", e.Error()
			log.put(summary)
		default:
			status, summary = "error", errMessage(err)
			log.put(summary)
		}
	}()
	if status != "done" {
		branch = ""
	}
	seconds := float64(int(wallClock().Sub(started).Seconds()*10+0.5)) / 10
	func() {
		defer func() {
			if r := recover(); r != nil {
				log.add("cleanup failed: %v", r)
			}
		}()
		ex.cleanup()
	}()
	_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
	var exitVal any
	if exitCode != nil {
		exitVal = *exitCode
	}
	recordRun := func() {
		exec1(s.DB, "UPDATE runs SET finished_at = ?, status = ?, exit_code = ?, seconds = ?, branch = ?, commits = ?, summary = ?, verify = ? WHERE id = ?",
			s.Now(), status, exitVal, seconds, branch, commits, summary, verifyNote, runID)
	}
	noteExecutorRun(s, now, ex.name(), status, summary, transportError, fault)
	if unfit != nil {
		markUnfit(s.DB, now, ex.name(), unfit.Project, unfit.Tool)
		toolsWanted(s, brainProject(s.DB), ex.name(), unfit.Tool, unfit.Project, now, agent)
	}
	runRow := func() row { return mustOne(s.DB, "SELECT * FROM runs WHERE id = ?", runID) }
	finishEvent := func(st, br string) {
		detail := map[string]any{"status": st, "commits": commits, "branch": br, "executor": ex.name()}
		if fault {
			detail["fault"] = true
		}
		eventAt(s.DB, now, agent, "run.finish", runID, detail)
	}
	if status == "stale" {
		recordRun()
		finishEvent(status, "")
		return runRow(), nil
	}
	jobStatus := "failed"
	if status == "done" || status == "dry-run" {
		jobStatus = "done"
	}
	switch {
	case status == "dry-run":
		jobStatus = "queued"
	case status == "error" && (fault || unfit != nil):
		jobStatus = "queued"
		exec1(s.DB, "UPDATE jobs SET attempts = MAX(attempts - 1, 0) WHERE id = ?", job.str("id"))
	case status == "failed" && unfit != nil && commits > 0 && job.int("attempts") < maxAttempts:
		jobStatus = "queued"
	case status == "error":
		if job.int("attempts") < maxAttempts {
			jobStatus = "queued"
		} else {
			jobStatus = "failed"
			summary = fmt.Sprintf("failed after %d runner errors; last: %s", job.int("attempts"), summary)
		}
	}
	finalBranch := branch
	if jobStatus == "queued" {
		finalBranch = job.str("branch")
	}
	if _, err := board.FinishRunJob(s, board.RunJobResult{RunID: runID, JobID: job.str("id"), Agent: agent,
		Status: jobStatus, Result: summary, Branch: finalBranch, Generation: job.int("generation")}); err != nil {
		log.put(errMessage(err))
		_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
		status, summary = "stale", fmt.Sprintf("%s (%s)", summary, errMessage(err))
		recordRun()
		finishEvent("stale", branch)
		return runRow(), nil
	}
	recordRun()
	if jobStatus == "done" {
		exec1(s.DB, "UPDATE agents SET jobs_done = jobs_done + 1 WHERE name = ?", agent)
		touchAgent(s, agent, commits > 0, now)
	} else if jobStatus == "failed" && status != "error" {
		exec1(s.DB, "UPDATE agents SET jobs_failed = jobs_failed + 1 WHERE name = ?", agent)
	}
	if status != "dry-run" && !(status == "error" && jobStatus == "queued") {
		lines := []string{fmt.Sprintf("Run `%s` of job `%s` (%s, Tier %d) by %s on executor `%s`: **%s** in %ss.",
			runID, job.str("id"), job.str("kind"), job.int("tier"), agent, ex.name(), status, strconv.FormatFloat(seconds, 'f', -1, 64)), ""}
		br := ""
		if branch != "" {
			br = fmt.Sprintf("; branch `%s` (local; lands on the default branch at the next landing pass, ADR-024)", branch)
		}
		lines = append(lines, fmt.Sprintf("- Commits: %d%s", commits, br))
		if verifyNote != "" {
			lines = append(lines, "- Verify: "+verifyNote)
		}
		lines = append(lines, "- Log: "+logPath)
		for _, f := range parsed.Findings {
			lines = append(lines, "- Finding: "+f)
		}
		arow := actorAgent(s, agent)
		cast := castResultVotes(s, project.str("slug"), arow, agent, now, runID, parsed.Votes, ballot, log)
		if len(parsed.Votes) > 0 {
			lines = append(lines, fmt.Sprintf("- Votes: %d cast of %d `VOTE:` line(s)", cast, len(parsed.Votes)))
		}
		decided := applyResultDecisions(s, job, parsed, log)
		if len(parsed.Decisions) > 0 && job.str("kind") == leadKind && s.OwnerAway() {
			lines = append(lines, fmt.Sprintf("- Decisions: %d recorded of %d DECIDE: line(s)", decided, len(parsed.Decisions)))
		}
		post := func(kind, area, thread, sum, text, resolution string) {
			if _, err := createPost(s, now, postOpts{Kind: kind, Project: project.str("slug"), Area: area, Thread: thread,
				Summary: trunc(sum, 230), Body: text, Agent: arow, Session: runID, Branch: &branch, Resolution: resolution,
				ForOwner: kind == "question"}); err != nil {
				log.add("post skipped: %s", errMessage(err))
			}
		}
		res := ""
		if jobStatus == "done" {
			res = "tested"
		}
		post("result", "jobs", job.str("id"), fmt.Sprintf("%s: %s", status, summary), strings.Join(lines, "\n")+"\n", res)
		if jobStatus == "done" {
			if err := postResultAnswer(s, job, agent, now, runID, branch, parsed); err != nil {
				log.add("answer skipped: %s", errMessage(err))
			}
		}
		for _, q := range parsed.Questions {
			post("question", "jobs", job.str("id"), q, "The worker stopped because this needs a real decision or crosses a hard stop.\n\n"+q+"\n", "")
		}
		for _, sg := range parsed.Suggestions {
			area, thread := sg.Area, sg.Thread
			if area == "" {
				area = "evolution"
			}
			if thread == "" {
				thread = job.str("id")
			}
			post("suggestion", area, thread, sg.Card, fmt.Sprintf("One-variable improvement seen by the worker in run `%s` of job `%s` (%s), not made there.\n\n%s\n",
				runID, job.str("id"), job.str("title"), sg.Card), "")
		}
		_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
	}
	finishEvent(status, branch)
	return runRow(), nil
}

func postResultAnswer(s *store.Store, job row, agent, now, runID, branch string, rep report) error {
	if job.str("kind") != "answer" || len(rep.Questions) != 0 || strings.TrimSpace(rep.Summary) == "" {
		return nil
	}
	if hardStopText(rep.Summary) {
		return fail(2, "job %s reported a hard stop", job.str("id"))
	}
	id, ok := strings.CutPrefix(job.str("source"), "question:")
	if !ok || id == "" {
		return fail(2, "job %s has no question source", job.str("id"))
	}
	q := postRow(s.DB, id)
	if q == nil || q.str("kind") != "question" || q.str("project") != job.str("project") {
		return fail(2, "job %s does not name a question in its project", job.str("id"))
	}
	if !answerable(q, job.str("created_by")) || board.Tier2Proposal(q.str("summary"), q.str("body")) != "" ||
		hardStopText(q.str("summary")+"\n"+q.str("body")) {
		return fail(2, "question %s requires the owner", id)
	}
	open := false
	for _, candidate := range openQuestions(s, job.str("project"), now) {
		if candidate.str("id") == id {
			open = true
			break
		}
	}
	if !open {
		return nil
	}
	// Unlinked replies can share the question's timestamp. Only a reply
	// inserted after this question can stand in for its answer.
	if mustOne(s.DB, `SELECT 1 FROM posts a JOIN posts q ON q.id = ?
		WHERE a.project = q.project AND a.kind = 'answer' AND a.area = q.area
		AND a.thread = q.thread AND a.date = q.date AND a.rowid > q.rowid LIMIT 1`, id) != nil {
		return nil
	}
	_, err := createPost(s, now, postOpts{Kind: "answer", Project: q.str("project"), Area: q.str("area"), Thread: q.str("thread"),
		Summary: trunc(rep.Summary, 230), Body: strings.TrimSpace(rep.Summary) + "\n", On: id,
		Agent: actorAgent(s, agent), Session: runID, Branch: &branch})
	return err
}

func watchRunHarnessCancel(ctx context.Context, s *store.Store, runID string, cancel context.CancelFunc, ownerCancelled *atomic.Bool) func() {
	return watchRunHarnessCancelEvery(ctx, s, runID, cancel, ownerCancelled, 3*time.Second)
}

func watchRunHarnessCancelEvery(ctx context.Context, s *store.Store, runID string, cancel context.CancelFunc, ownerCancelled *atomic.Bool, interval time.Duration) func() {
	watchCtx, stop := context.WithCancel(ctx)
	done := make(chan struct{})
	check := func() bool {
		session, err := s.HarnessSessionHead(runID)
		if err == nil && session != nil && session.CancelRequestedAt != "" {
			if ownerCancelled != nil {
				ownerCancelled.Store(true)
			}
			cancel()
			return true
		}
		return false
	}
	go func() {
		defer close(done)
		if check() {
			return
		}
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
				if check() {
					return
				}
			case <-watchCtx.Done():
				return
			}
		}
	}()
	return func() {
		stop()
		<-done
	}
}

func errMessage(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// applyResultDecisions records `DECIDE:` lines from a project's lead while the owner is away.
func applyResultDecisions(s *store.Store, job row, rep report, log *runLog) int {
	if len(rep.Decisions) == 0 || job == nil || job.str("kind") != leadKind || !s.OwnerAway() {
		return 0
	}
	slug := job.str("project")
	lead := leadName(slug)
	recorded := 0
	seen := map[string]bool{}
	for _, d := range rep.Decisions {
		if seen[d.PostID] {
			log.add("decision skipped: %s was already named by this report", d.PostID)
			continue
		}
		seen[d.PostID] = true
		if strings.TrimSpace(d.Reason) == "" {
			log.add("decision skipped: %s has no reason", d.PostID)
			continue
		}
		post := mustOne(s.DB, "SELECT id, project, resolution, summary, body FROM posts WHERE id = ?", d.PostID)
		if post == nil {
			log.add("decision skipped: %s does not exist", d.PostID)
			continue
		}
		if post.str("project") != slug {
			log.add("decision skipped: %s belongs to %s, not %s", d.PostID, post.str("project"), slug)
			continue
		}
		if post.str("resolution") != "agreed" {
			log.add("decision skipped: %s is not an agreed proposal", d.PostID)
			continue
		}
		if mustOne(s.DB, "SELECT 1 AS x FROM decisions WHERE post_id = ?", d.PostID) != nil {
			log.add("decision skipped: %s was already decided", d.PostID)
			continue
		}
		if mustOne(s.DB, "SELECT 1 AS x FROM compute_requests WHERE proposal_post = ?", d.PostID) != nil {
			log.add("decision skipped: %s is a compute proposal", d.PostID)
			continue
		}
		if leadHumanOnlyMarkerRe.MatchString(post.str("body")) {
			log.add("decision skipped: %s has a pair/onboard marker", d.PostID)
			continue
		}
		if what := board.Tier2Proposal(post.str("summary"), post.str("body")); what != "" {
			log.add("decision skipped: %s %s: Tier 2 waits for the owner", d.PostID, what)
			continue
		}
		if latest := mustOne(s.DB, "SELECT resolution FROM posts WHERE project = ? AND area = (SELECT area FROM posts WHERE id = ?) AND thread = (SELECT thread FROM posts WHERE id = ?) AND resolution <> '' ORDER BY id DESC LIMIT 1", slug, d.PostID, d.PostID); latest == nil || latest.str("resolution") != "agreed" {
			log.add("decision skipped: %s is no longer awaiting", d.PostID)
			continue
		}
		if _, err := board.Decide(s, d.PostID, d.Answer, d.Reason, lead); err != nil {
			var refusal *cli.Error
			if errors.As(err, &refusal) {
				log.add("decision refused: %s: %s", d.PostID, errMessage(err))
				continue
			}
			log.add("decision skipped: %s: %s", d.PostID, errMessage(err))
			continue
		}
		recorded++
	}
	log.add("- Decisions: %d recorded of %d DECIDE: line(s)", recorded, len(rep.Decisions))
	return recorded
}

// castResultVotes casts the `VOTE:` lines that were on this run's ballot, one per target, with a reason.
func castResultVotes(s *store.Store, slug string, arow *store.Agent, agent, now, runID string, votes []vote, ballot []ballotItem, log *runLog) int {
	allowed := map[string]ballotItem{}
	for _, it := range ballot {
		allowed[it.ID] = it
	}
	cast := 0
	seen := map[string]bool{}
	for _, v := range votes {
		if seen[v.On] {
			continue
		}
		seen[v.On] = true
		item, ok := allowed[v.On]
		if !ok {
			log.add("vote skipped: %s was not on this run's ballot", v.On)
			continue
		}
		if strings.TrimSpace(v.Reason) == "" {
			log.add("vote skipped: %s has no reason", v.On)
			continue
		}
		key := item.Key
		if key == "" {
			key = "evolution/"
		}
		area, thread, _ := strings.Cut(key, "/")
		if area == "" {
			area = "evolution"
		}
		if thread == "" {
			thread = v.On
		}
		_, err := createPost(s, now, postOpts{Kind: "vote", Project: slug, Area: area, Thread: thread, Summary: trunc(v.Reason, 230),
			Body: fmt.Sprintf("Cast by the hub from a `VOTE:` line in run `%s`.\n", runID), Agent: arow, Session: runID, On: v.On, Vote: v.Vote,
			Extra: map[string]any{"cast_by": "hub-from-result", "run": runID}})
		if err != nil {
			log.add("vote skipped: %s: %s", v.On, errMessage(err))
			continue
		}
		cast++
	}
	return cast
}

// pickWorker is pick_worker(): the next free worker, rotated; names are never reused.
// Every active worker is in the pool whatever model it last ran with: runJob records the model
// of each run on the worker's row (worker_model or light_model by the job kind), so a worker
// is never tied to one model and the pool stays at quorum+1.
func pickWorker(s *store.Store, now, agent string, exclude []string) (string, error) {
	if agent != "" {
		r, err := resolveAgent(s, agent, true)
		if err != nil {
			return "", err
		}
		return r.Name, nil
	}
	pool := settingInt(s, "quorum") + 1
	if pool < 1 {
		pool = 1
	}
	runner, model := s.Setting("runner_cmd", ""), s.Setting("worker_model", "")
	active := mustOne(s.DB, "SELECT COUNT(*) AS n FROM agents a WHERE a.status = 'active' AND a.kind = 'worker'").int("n")
	for active < pool {
		if _, err := spawnAgent(s, runner, model, "worker"); err != nil {
			return "", err
		}
		active++
	}
	q := "SELECT a.name FROM agents a WHERE a.status = 'active' AND a.kind = 'worker' AND NOT EXISTS " +
		"(SELECT 1 FROM jobs k WHERE k.lease_agent = a.name AND " + occupiedJobSQL + ") "
	var args []any
	if len(exclude) > 0 {
		marks := make([]string, len(exclude))
		for i, x := range exclude {
			marks[i] = "?"
			args = append(args, x)
		}
		q += "AND a.name NOT IN (" + strings.Join(marks, ",") + ") "
	}
	q += "ORDER BY COALESCE((SELECT MAX(e.seq) FROM events e WHERE e.actor = a.name AND e.action = 'job.lease'), 0), a.name LIMIT 1"
	if r := mustOne(s.DB, q, args...); r != nil {
		return r.str("name"), nil
	}
	r, err := spawnAgent(s, runner, model, "worker")
	if err != nil {
		return "", err
	}
	return r.str("name"), nil
}

// executorSlots is executor_slots(): one entry per free slot, remotes first, this machine last.
// localClosed holds this machine's slots back (the login gate, login_gate.go).
func executorSlots(s *store.Store, now string, busy map[string]int, localClosed bool) []string {
	var slots []string
	remotes := executorRows(s, false)
	for _, r := range remotes {
		for i := 0; i < executorFreeSlots(s, r, now, busy[r.str("name")]); i++ {
			slots = append(slots, r.str("name"))
		}
	}
	if localClosed {
		return slots
	}
	local := localExecutorRow(s)
	for i := 0; i < executorFreeSlots(s, local, now, busy[localExecutor]); i++ {
		slots = append(slots, localExecutor)
	}
	if len(slots) == 0 && len(remotes) == 0 && executorRunning(s, localExecutor, now) == 0 && busy[localExecutor] == 0 {
		slots = []string{localExecutor}
	}
	return slots
}

// sweepLostRuns closes runs a dead runner left `running` and releases their
// jobs: a lost run's lease would otherwise hold the job (and a worker slot in
// every view) until minutes_per_job passes. The job is queued again while it
// has attempts left, and failed once it has been lost maxAttempts times.
func sweepLostRuns(s *store.Store, now string) int {
	rows := mustRows(s.DB, "SELECT id, job_id, agent FROM runs WHERE status = 'running'")
	for _, r := range rows {
		exec1(s.DB, "UPDATE runs SET status = 'lost', finished_at = ?, summary = 'the runner stopped while this run was in progress' WHERE id = ?", now, r.str("id"))
		eventAt(s.DB, now, "runner", "run.lost", r.str("id"), "")
		releaseLostJob(s, now, r.str("job_id"), r.str("agent"))
	}
	return len(rows)
}

// releaseLostJob frees the lease a lost run still holds on its job.
func releaseLostJob(s *store.Store, now, jobID, agent string) {
	job := mustOne(s.DB, "SELECT * FROM jobs WHERE id = ? AND status = 'leased' AND lease_agent = ?", jobID, agent)
	if job == nil {
		return
	}
	if job.int("attempts") >= maxAttempts {
		result := fmt.Sprintf("failed: the runner stopped while this job was in progress %d times", job.int("attempts"))
		exec1(s.DB, "UPDATE jobs SET status = 'failed', lease_agent = '', lease_until = '', finished_at = ?, result = ? WHERE id = ?", now, result, jobID)
		eventAt(s.DB, now, "runner", "job.failed", jobID, result)
		return
	}
	exec1(s.DB, "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?", jobID)
	eventAt(s.DB, now, "runner", "job.requeue", jobID, "run lost: the runner stopped while it was in progress")
}

// cliOnPath returns pathEnv with the directory of exe prepended when no entry of pathEnv
// already resolves a `pulse` binary, and "" when pathEnv already exposes one (nothing to change).
// Workers read the brief's `pulse ...` instructions literally, so the runner's own binary must be
// reachable by name in every local clone, whatever the login shell's PATH looks like.
func cliOnPath(pathEnv, exe string) string {
	if exe == "" {
		return ""
	}
	name := filepath.Base(exe)
	for _, dir := range filepath.SplitList(pathEnv) {
		if dir == "" {
			continue
		}
		if st, err := os.Stat(filepath.Join(dir, name)); err == nil && !st.IsDir() && st.Mode()&0o111 != 0 {
			return ""
		}
	}
	dir := filepath.Dir(exe)
	if pathEnv == "" {
		return dir
	}
	return dir + string(os.PathListSeparator) + pathEnv
}

// remotePulseCLI is the PULSE_CLI a remote worker gets: the recipe's value when set, else the
// pulse binary `pulse pair` stages on every executor (pairPulsePath), so the brief's fallback holds there.
func remotePulseCLI(recipe *Recipe) string {
	if cli := recipePulseCLI(recipe); cli != "" {
		return cli
	}
	return pairPulsePath
}
