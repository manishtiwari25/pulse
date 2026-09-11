package runner

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// The runner's commands: plan, work, land, daemon, service, executor, env, hands, pair, onboard.

func init() {
	// `pulse remove service` (internal/board) uninstalls the daemon's user
	// service through this hook; board cannot import runner.
	board.UninstallService = UninstallService
	// `pulse compute providers` (internal/tasks) lists the providers registered here; tasks cannot import runner.
	tasks.ComputeProviders = cmdComputeProviders

	cli.Register(cli.Command{Name: "plan", Group: "runner", Summary: "the job planner: turn the tally into queued jobs",
		Usage: "pulse plan [--project SLUG] [--now T]", Run: cmdPlan})
	cli.Register(cli.Command{Name: "work", Group: "runner", Summary: "lease the next job and run one worker here",
		Usage: "pulse work [--project SLUG] [--job ID] [--dry-run] [--worker-cmd CMD] [--executor NAME] [--agent A] [--now T]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("job", "", "one job id")
			fs.Bool("dry-run", false, "lease nothing; show what would run")
			fs.String("worker-cmd", "", "harness command (the brief text goes in for {brief}; default: the runner_cmd setting)")
			fs.String("executor", "", "run on this executor")
		}, Run: cmdWork})
	cli.Register(cli.Command{Name: "land", Group: "runner", Summary: "merge verified branches into the default branch",
		Usage: "pulse land [--project SLUG] [--branch B] [--all] [--limit N] [--no-verify] [--dry-run]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("branch", "", "land this branch")
			fs.Bool("all", false, "every active project")
			fs.Int("limit", 5, "at most N branches per project")
			fs.Bool("no-verify", false, "skip the verify command")
			fs.Bool("dry-run", false, "list what waits, land nothing")
		}, Run: cmdLand})
	cli.Register(cli.Command{Name: "daemon", Group: "runner", Summary: "the runner loop: land, plan, dispatch workers, sweep",
		Usage: "pulse daemon [--interval S] [--once] [--max-loops N] [--dry-run] [--worker-cmd CMD] [--no-restart] [--agent A]",
		Flags: func(fs *flag.FlagSet) {
			fs.Int("interval", DefaultDaemonIntervalSeconds, "idle wait and runner-error retry delay in seconds")
			fs.Bool("once", false, "one loop, then exit")
			fs.Int("max-loops", 0, "stop after N loops (0 = forever)")
			fs.Bool("dry-run", false, "lease and plan nothing")
			fs.String("worker-cmd", "", "harness command for every run")
			fs.Bool("no-restart", false, "do not exit for a restart when the pulse binary changes")
		}, Run: cmdDaemon})
	cli.Register(cli.Command{Name: "sweep", Group: "runner", Summary: "remove what runs and landings left on this machine and nothing holds any more (the daemon does this every ten minutes)",
		Usage: "pulse sweep [--dry-run] [--held] [--json]",
		Flags: func(fs *flag.FlagSet) {
			fs.Bool("dry-run", false, "list what would go and what is held; remove nothing")
			fs.Bool("held", false, "also list what the sweep keeps and why")
		}, Run: cmdSweep})
	cli.Register(cli.Command{Name: "service", Group: "runner", Summary: "install, uninstall or show the user services that keep the daemon and the hub alive (launchd / systemd --user)",
		Usage: "pulse service install|uninstall|status|show [--interval S] [--hub-port N] [--no-hub] [--dry-run]",
		Flags: func(fs *flag.FlagSet) {
			fs.Int("interval", DefaultDaemonIntervalSeconds, "daemon idle wait and runner-error retry delay in seconds")
			fs.Int("hub-port", hubPort, "port the hub service listens on (127.0.0.1)")
			fs.Bool("no-hub", false, "install the daemon only; run the console by hand with `pulse hub`")
			fs.Bool("dry-run", false, "print the units or plists instead of installing")
		}, Run: cmdService})
	cli.Register(cli.Command{Name: "executor", Group: "runner", Summary: "list, add, probe, drain, activate, remove or log in the executors (remote hands)",
		Usage: "pulse executor list | add NAME --ssh user@host[:port] --workdir /abs [--workers N] [--projects a,b|*] | probe NAME [--login] | drain NAME | activate NAME | remove NAME | login NAME --admin admin@host (--with-token | --remove) | token [--with-token | --remove]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("ssh", "", "user@host[:port] of the executor")
			fs.String("workdir", "", "absolute work directory on the executor")
			fs.Int("workers", 1, "parallel workers there")
			fs.String("projects", "*", "comma-separated project slugs or *")
			fs.String("admin", "", "admin ssh address for login")
			fs.Bool("with-token", false, "read a GitHub token from stdin and place the worker login")
			fs.Bool("remove", false, "remove the worker login")
			fs.Bool("login", false, "probe: also run the login check")
		}, Run: cmdExecutor})
	cli.Register(cli.Command{Name: "env", Group: "runner", Summary: "the environment recipe per project (a CTX-environment record): show, set, import, check",
		Usage: "pulse env show|set|import|check [--project SLUG] [--executor NAME] [--file F] [--json]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("executor", "", "one executor; default: every executor")
			fs.String("file", "", "recipe text for `env set` (default: stdin)")
		}, Run: cmdEnv})
	cli.Register(cli.Command{Name: "hands", Group: "runner", Summary: "the forced command on a paired machine (probe, prepare, key, run, ...)",
		Usage: "pulse hands VERB [ARGS]  (or via SSH_ORIGINAL_COMMAND)", NoStore: true, Raw: true, Run: cmdHands})
	cli.Register(cli.Command{Name: "pair", Group: "runner", Summary: "turn a Linux machine into an executor over its admin ssh session",
		Usage: "pulse pair ADMIN@HOST[:PORT] --name N [--workdir D] [--workers N] [--projects P] [--install-copilot] [--dry-run | --inspect]",
		Flags: func(fs *flag.FlagSet) {
			fs.String("name", "", "executor name (lowercase words with dashes)")
			fs.String("workdir", "/srv/pulse", "work directory on the machine")
			fs.Int("workers", 1, "parallel workers there")
			fs.String("projects", "*", "comma-separated project slugs or *")
			fs.Bool("install-copilot", false, "legacy copilot-cli harness: install node and copilot through apt")
			fs.Bool("dry-run", false, "inspect and print the scripts; change nothing")
			fs.Bool("inspect", false, "alias for --dry-run")
		}, Run: cmdPair})
	cli.Register(cli.Command{Name: "onboard", Group: "runner", Summary: "register a project (init) and queue its onboarding job; prints what stays human",
		Usage: "pulse onboard [PATH] [--name N] [--remote URL] [--verify CMD] [--brain] [--now T]", NoStore: true,
		Flags: func(fs *flag.FlagSet) {
			fs.String("name", "", "project name")
			fs.String("remote", "", "git remote URL")
			fs.String("verify", "", "the project's verify command")
			fs.Bool("brain", false, "this checkout is the brain")
		}, Run: cmdOnboard})
}

func nowOf(c *cli.Context) string {
	if c.Now != "" {
		return c.Now
	}
	if c.Store != nil {
		return c.Store.Now()
	}
	return time.Now().UTC().Format(isoLayout)
}

func actorOf(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	return humanAgent
}

func strFlag(fs *flag.FlagSet, name string) string {
	if f := fs.Lookup(name); f != nil {
		return f.Value.String()
	}
	return ""
}

func boolFlag(fs *flag.FlagSet, name string) bool { return strFlag(fs, name) == "true" }

func intFlag(fs *flag.FlagSet, name string) int {
	n := 0
	fmt.Sscanf(strFlag(fs, name), "%d", &n)
	return n
}

func cmdPlan(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	var slugs []string
	if c.Project != "" {
		p, err := currentProject(s, c.Project)
		if err != nil {
			return err
		}
		slugs = []string{p.str("slug")}
	} else {
		for _, p := range projectRows(s.DB, false) {
			slugs = append(slugs, p.str("slug"))
		}
	}
	for _, slug := range slugs {
		jobs, err := planJobs(s, slug, now, "plan")
		if err != nil {
			return err
		}
		for _, j := range jobs {
			c.Printf("planned %s %s: %s\n", j.str("kind"), j.str("id"), j.str("title"))
		}
	}
	return nil
}

func cmdWork(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	worker, err := pickWorker(s, now, c.Agent, nil)
	if err != nil {
		return err
	}
	run, reason, err := workOnce(s, worker, c.Project, strFlag(fs, "job"), now,
		runOpts{DryRun: boolFlag(fs, "dry-run"), WorkerCmd: strFlag(fs, "worker-cmd"), Executor: strFlag(fs, "executor")})
	if err != nil {
		return err
	}
	if run == nil {
		c.Printf("idle: %s\n", reason)
		return cli.Errorf(3, "", "", "")
	}
	c.Printf("%s %s by %s on %s: %s\n", run.str("status"), run.str("job_id"), run.str("agent"), run.str("executor"), run.str("summary"))
	if run.str("branch") != "" {
		c.Printf("branch %s (local; `pulse land` or the daemon merges it once the checkout is clean)\n", run.str("branch"))
	}
	if st := run.str("status"); st != "done" && st != "dry-run" {
		return cli.Errorf(1, "", "", "")
	}
	return nil
}

func cmdLand(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	actor := actorOf(c)
	var targets []row
	if boolFlag(fs, "all") {
		targets = projectRows(s.DB, false)
	} else {
		p, err := currentProject(s, c.Project)
		if err != nil {
			return err
		}
		targets = []row{p}
	}
	worst := 0
	rank := map[string]int{"landed": 0, "merged": 0, "conflict": 1, "failed-verify": 1, "missing": 1}
	branch, verify := strFlag(fs, "branch"), !boolFlag(fs, "no-verify")
	for _, proj := range targets {
		slug := proj.str("slug")
		if boolFlag(fs, "dry-run") {
			pending := pendingLandings(s.DB, slug)
			if len(pending) == 0 {
				c.Printf("%s: nothing waiting\n", slug)
			}
			for _, j := range pending {
				c.Printf("%s would land: %s (%s %s)\n", slug, j.str("branch"), j.str("kind"), j.str("id"))
			}
			continue
		}
		var results []landResult
		if branch != "" {
			job := queryOneRow(s.DB, "SELECT * FROM jobs WHERE project = ? AND branch = ? ORDER BY finished_at DESC", slug, branch)
			state, note, err := landBranch(s, proj, branch, actor, now, landOpts{Job: job, Verify: verify, ByHand: actor == humanAgent})
			if err != nil {
				return err
			}
			results = []landResult{{State: state, Note: note}}
		} else {
			results = landPending(s, proj, actor, now, verify, intFlag(fs, "limit"), true)
		}
		if len(results) == 0 {
			c.Printf("%s: nothing waiting\n", slug)
		}
		landed := false
		for _, r := range results {
			c.Printf("%s %s: %s\n", slug, r.State, r.Note)
			if r.State == "landed" {
				landed = true
			}
			w, ok := rank[r.State]
			if !ok {
				w = 3
			}
			if w > worst {
				worst = w
			}
		}
		// ADR-037: by hand as by the daemon - with landing.push=main, what landed (now or earlier) is pushed.
		if line, note := pushLanded(s, proj, actor, now, landed); line != "" {
			c.Printf("%s\n", line)
		} else if note != "" {
			c.Printf("%s %s\n", slug, note)
			if worst < 1 {
				worst = 1
			}
		}
	}
	if worst != 0 {
		return cli.Errorf(worst, "", "", "")
	}
	return nil
}

func queryOneRow(q querier, sql string, args ...any) row {
	r, _ := queryOne(q, sql, args...)
	return r
}

func cmdDaemon(c *cli.Context, fs *flag.FlagSet, args []string) error {
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	loops := intFlag(fs, "max-loops")
	if boolFlag(fs, "once") {
		loops = 1
	}
	result := daemon(ctx, c.Store, daemonOpts{Agent: c.Agent, Interval: intFlag(fs, "interval"), DryRun: boolFlag(fs, "dry-run"),
		WorkerCmd: strFlag(fs, "worker-cmd"), MaxLoops: loops, RestartOnChange: !boolFlag(fs, "no-restart"), Out: c.Out, HubHome: homeDir()})
	switch result {
	case "done", "interrupted":
		return nil
	case "restart":
		fmt.Fprintln(c.Err, "pulse binary changed on disk; exiting 3 so the service restarts the daemon")
	}
	return cli.Errorf(3, "", "", "")
}

// cmdSweep is the daemon's hygiene pass by hand: over every active project whose checkout answers, with the
// same holds. Without --dry-run it removes; the report goes to meta.hygiene and the event log as the
// daemon's would.
func cmdSweep(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	if len(args) > 0 {
		return fail(2, "pulse sweep takes no arguments (pulse sweep [--dry-run] [--held] [--json])")
	}
	dryRun := boolFlag(fs, "dry-run")
	probes := &checkoutProbes{}
	projects, held, _ := probes.ready(projectRows(s.DB, false), checkoutProbeTimeout)
	rep := sweep(s, projects, now, sweepOpts{DryRun: dryRun, Actor: actorOf(c)})
	if !dryRun {
		recordSweep(s, rep, now, actorOf(c))
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, map[string]any{"at": rep.At, "dry_run": rep.DryRun, "removed": rep.Removed, "held": rep.Held,
			"errors": rep.Errors, "bytes": rep.bytes(), "unreadable": held})
	}
	for slug, why := range held {
		c.Printf("skipped %s: %s\n", slug, why)
	}
	for _, l := range sweepLines(rep) {
		c.Printf("%s\n", l)
	}
	if boolFlag(fs, "held") {
		for _, x := range rep.Held {
			where := x.Path
			if x.Project != "" && (x.Kind == "branch" || x.Kind == "ref") {
				where = x.Project + " " + x.Path
			}
			c.Printf("held %s %s (%s)\n", x.Kind, where, x.Why)
		}
	}
	verb := "removed"
	if dryRun {
		verb = "would remove"
	}
	c.Printf("%s %d item(s), %s; %d held\n", verb, len(rep.Removed), sizeText(rep.bytes()), len(rep.Held))
	if len(rep.Errors) > 0 {
		return cli.Errorf(1, fmt.Sprintf("%s could not be swept", plural(len(rep.Errors), "item")), rep.Errors[0], "fix the cause above and run pulse sweep again; the daemon tries again every ten minutes")
	}
	return nil
}

func cmdService(c *cli.Context, fs *flag.FlagSet, args []string) error {
	action := "status"
	if len(args) > 0 {
		action = args[0]
	}
	kind, path := servicePaths("", "")
	units := serviceUnits(intFlag(fs, "interval"), intFlag(fs, "hub-port"), boolFlag(fs, "no-hub"))
	switch action {
	case "install":
		dryRun := boolFlag(fs, "dry-run")
		if _, err := InstallService(c.Out, c.Home, ServiceInstallOptions{Interval: intFlag(fs, "interval"), HubPort: intFlag(fs, "hub-port"), NoHub: boolFlag(fs, "no-hub"), DryRun: dryRun}); err != nil {
			return err
		}
		if dryRun {
			return nil
		}
		c.Printf("the daemon is kept alive by %s; watch: tail -f %s\n", kind, c.Home+"/logs/daemon.log")
		if !boolFlag(fs, "no-hub") {
			c.Printf("the hub is kept alive by %s at http://127.0.0.1:%d and restarts itself on a new build; watch: tail -f %s\n",
				kind, intFlag(fs, "hub-port"), c.Home+"/logs/hub.log")
		}
		if note := protectedFoldersNote(c.Store, "", ""); note != "" {
			c.Printf("%s\n", note)
		}
	case "uninstall":
		uninstallService(c.Out, "", "")
	case "show":
		for _, u := range units {
			if _, err := installUnit(c.Out, u, c.Home, "", "", true); err != nil {
				return err
			}
		}
	case "status":
		serviceStatus(c.Out, c.Store, "", "")
		if note := protectedFoldersNote(c.Store, "", ""); note != "" {
			c.Printf("%s\n", note)
		}
	default:
		return fail(2, "pulse service: unknown action %q (install|uninstall|status|show); file would be %s", action, path)
	}
	return nil
}

// executorsText is executors_text(): one line per pair of hands, the local machine first.
func executorsText(s *store.Store, now string) string {
	var out []string
	for _, r := range executorRows(s, true) {
		name := r.str("name")
		busy := executorRunning(s, name, now)
		addr := r.str("address")
		if addr == "" {
			addr = "this machine"
		}
		out = append(out, fmt.Sprintf("%-14s %-11s %-30s workers %d busy %d projects %s", name, r.str("status"), addr, r.int("workers"), busy, r.str("projects")))
		if name == localExecutor {
			continue
		}
		next := ""
		if r.str("next_probe_at") != "" {
			next = "; next probe " + r.str("next_probe_at")
		}
		out = append(out, fmt.Sprintf("%14s workdir %s; runs done %d failed %d; errors %d%s", "", r.str("workdir"), r.int("runs_done"), r.int("runs_failed"), r.int("errors"), next))
		last, note := r.str("last_probe_at"), r.str("probe_note")
		if last == "" {
			last = "never"
		}
		if note == "" {
			note = "-"
		}
		out = append(out, fmt.Sprintf("%14s probe %s: %s", "", last, note))
		check := lastLoginCheck(s, name)
		expires := loginExpiry(s, name)
		warn := expiryWarning(expires, now, 7)
		if check != nil || expires != "" {
			text := "never checked"
			if check != nil {
				state := "FAILED"
				if ok, _ := check["ok"].(bool); ok {
					state = "ok"
				}
				text = fmt.Sprintf("%s at %v: %v", state, check["at"], check["note"])
			}
			exp, w := "", ""
			if expires != "" {
				exp = "; token expires " + expires
			}
			if warn != "" {
				w = "; WARNING " + warn
			}
			out = append(out, fmt.Sprintf("%14s login %s%s%s", "", text, exp, w))
		}
	}
	return strings.Join(out, "\n") + "\n"
}

func cmdExecutor(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	if len(args) == 0 {
		return fail(2, "pulse executor needs an action: list|add|probe|drain|activate|remove|login|token")
	}
	action := args[0]
	if action == "list" {
		fmt.Fprint(c.Out, executorsText(s, now))
		return nil
	}
	if action == "token" {
		// ADR-034: the executor token the daemon places wherever an executor needs a login.
		withToken, remove := boolFlag(fs, "with-token"), boolFlag(fs, "remove")
		var lines []string
		var err error
		switch {
		case withToken && remove:
			return fail(2, "executor token takes one of --with-token or --remove")
		case withToken:
			lines, err = executorTokenSet(s, now, os.Stdin, cli.IsTTY(os.Stdin), humanAgent)
		case remove:
			lines, err = executorTokenRemove(s, now, humanAgent)
		default:
			lines = executorTokenReport(s, now)
		}
		if err != nil {
			return err
		}
		c.Println(strings.Join(lines, "\n"))
		return nil
	}
	if len(args) < 2 {
		return fail(2, "executor %s needs a NAME", action)
	}
	name := args[1]
	actor := actorOf(c)
	switch action {
	case "add":
		if strFlag(fs, "ssh") == "" || strFlag(fs, "workdir") == "" {
			return fail(2, "executor add needs --ssh user@host[:port] and --workdir /abs/path")
		}
		r, err := addExecutor(s, now, name, strFlag(fs, "ssh"), strFlag(fs, "workdir"), intFlag(fs, "workers"), strFlag(fs, "projects"), actor)
		if err != nil {
			return err
		}
		c.Printf("added %s (%s, %s); it is drained until `pulse executor probe %s` passes\n", r.str("name"), r.str("address"), r.str("workdir"), r.str("name"))
	case "probe":
		r := executorRow(s, name)
		deep := boolFlag(fs, "login") || (r != nil && loginDrained(r))
		ok, note, err := probeExecutor(s, name, actor, now)
		if err != nil {
			return err
		}
		r = executorRow(s, name)
		status := ""
		if r != nil {
			status = r.str("status")
		}
		if ok {
			c.Printf("%s ok, %s: %s\n", name, status, note)
		} else {
			c.Printf("%s FAILED (%s): %s\n", name, status, note)
		}
		if ok && deep {
			ok, note, err = loginCheck(s, name, actor, now, true)
			if err != nil {
				return err
			}
			r = executorRow(s, name)
			state := "FAILED"
			if ok {
				state = "ok"
			}
			c.Printf("%s login check %s (%s): %s\n", name, state, r.str("status"), note)
		}
		if !ok {
			return cli.Errorf(1, "", "", "")
		}
	case "drain":
		note := "drained by the human"
		if _, err := setExecutorStatus(s, now, name, "drained", actor, &note); err != nil {
			return err
		}
		c.Printf("%s drained: running jobs finish, no new job starts there\n", name)
	case "activate":
		r := executorRow(s, name)
		if r == nil && mustOne(s.DB, "SELECT 1 FROM executors WHERE name = ? AND status = 'removed'", name) != nil {
			// ADR-034: the undo of a retirement - the removed row comes back drained and is probed as you.
			exec1(s.DB, "UPDATE executors SET status = 'drained', probe_note = '', errors = 0, backoff_minutes = 0, next_probe_at = '', healed_at = '' WHERE name = ?", name)
			eventAt(s.DB, now, actor, "executor.revived", name, nil)
			ok, note, err := probeExecutor(s, name, actor, now)
			if err != nil {
				return err
			}
			if !ok {
				return fail(1, "%s is back but its probe failed (%s); fix that, then `pulse executor probe %s` and activate again", name, note, name)
			}
			r = executorRow(s, name)
		}
		if r == nil {
			return fail(2, "No executor %s", name)
		}
		if r.str("last_probe_at") == "" || !strings.HasPrefix(r.str("probe_note"), "ok") {
			note := r.str("probe_note")
			if note == "" {
				note = "never probed"
			}
			return fail(2, "Probe %s first; its last probe did not pass: %s", name, note)
		}
		if _, err := setExecutorStatus(s, now, name, "active", actor, nil); err != nil {
			return err
		}
		c.Printf("%s active\n", name)
	case "remove":
		// kept for compatibility; `pulse remove executor NAME` shows the full preview
		if _, err := board.RemoveExecutor(s, name, actor, false); err != nil {
			return err
		}
		c.Printf("%s removed; the machine, its key, and its login are yours to retire\n", name)
	case "login":
		withToken, remove := boolFlag(fs, "with-token"), boolFlag(fs, "remove")
		if strFlag(fs, "admin") == "" || withToken == remove {
			return fail(2, "executor login needs --admin admin@host[:port] and exactly one of --with-token or --remove")
		}
		var token io.Reader
		if withToken {
			token = os.Stdin
		}
		rep, err := executorLogin(s, now, name, strFlag(fs, "admin"), token, cli.IsTTY(os.Stdin), remove, humanAgent)
		if err != nil {
			return err
		}
		c.Println(strings.Join(rep.Lines, "\n"))
		if !rep.OK {
			return cli.Errorf(1, "", "", "")
		}
	default:
		return fail(2, "pulse executor: unknown action %q (list|add|probe|drain|activate|remove|login|token)", action)
	}
	return nil
}

func cmdEnv(c *cli.Context, fs *flag.FlagSet, args []string) error {
	s, now := c.Store, nowOf(c)
	action := "check"
	if len(args) > 0 {
		action = args[0]
	}
	switch action {
	case "check":
		rep, err := envCheck(s, c.Project, strFlag(fs, "executor"), now)
		if err != nil {
			return err
		}
		if c.JSON {
			data, _ := json.MarshalIndent(rep, "", "  ")
			c.Println(string(data))
		} else {
			fmt.Fprint(c.Out, envCheckText(rep))
		}
		if len(rep.Unfit) > 0 {
			return cli.Errorf(1, "", "", "")
		}
		return nil
	case "show":
		p, err := currentProject(s, c.Project)
		if err != nil {
			return err
		}
		text, from, ok := envShow(s, p)
		if !ok {
			return fail(3, "%s has no environment recipe (`pulse env set --file F` or `pulse env import` stores one)", p.str("slug"))
		}
		if c.JSON {
			r, problems := parseRecipe(text)
			data, _ := json.MarshalIndent(map[string]any{"project": p.str("slug"), "source": from, "text": text, "recipe": r, "problems": problems}, "", "  ")
			c.Println(string(data))
			return nil
		}
		fmt.Fprintf(c.Err, "# %s: %s\n", p.str("slug"), from)
		fmt.Fprint(c.Out, text)
		if !strings.HasSuffix(text, "\n") {
			fmt.Fprintln(c.Out)
		}
		return nil
	case "set":
		p, err := currentProject(s, c.Project)
		if err != nil {
			return err
		}
		var data []byte
		if f := strFlag(fs, "file"); f != "" && f != "-" {
			data, err = os.ReadFile(f)
		} else {
			data, err = io.ReadAll(os.Stdin)
		}
		if err != nil {
			return fail(2, "Cannot read the recipe: %v", err)
		}
		if strings.TrimSpace(string(data)) == "" {
			return fail(2, "The recipe is empty; pass --file F or the text on stdin")
		}
		id, rev, r, err := envSet(s, p, string(data), actorOf(c), "store", "pulse env set")
		if err != nil {
			return err
		}
		c.Printf("%s rev %d for %s: %s\n", id, rev, p.str("slug"), recipeSummary(r))
		return nil
	case "import":
		p, err := currentProject(s, c.Project)
		if err != nil {
			return err
		}
		id, rev, r, err := envImport(s, p, actorOf(c))
		if err != nil {
			return err
		}
		c.Printf("%s rev %d for %s imported from %s: %s\n", id, rev, p.str("slug"), recipePath, recipeSummary(r))
		return nil
	}
	return fail(2, "pulse env: unknown action %q (show|set|import|check)", action)
}

func cmdHands(c *cli.Context, _ *flag.FlagSet, args []string) error {
	h := newHands(os.Stdin, c.Out, c.Err)
	err := h.handsMain(args)
	var he *handsError
	var hx *handsExit
	switch {
	case err == nil:
		return nil
	case asHandsError(err, &he):
		fmt.Fprintln(c.Err, he.Error())
		return cli.Errorf(2, "", "", "")
	case asHandsExit(err, &hx):
		return cli.Errorf(hx.code, "", "", "")
	}
	return err
}

func cmdPair(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) != 1 {
		return fail(2, "pulse pair needs the admin ssh address (admin@host[:port]) and --name")
	}
	if strFlag(fs, "name") == "" {
		return fail(2, "pulse pair needs --name (the executor's name)")
	}
	rep, err := pairExecutor(c.Store, args[0], strFlag(fs, "name"), nowOf(c), pairOpts{Workdir: strFlag(fs, "workdir"), Workers: intFlag(fs, "workers"),
		Projects: strFlag(fs, "projects"), InstallCopilot: boolFlag(fs, "install-copilot"),
		DryRun: boolFlag(fs, "dry-run") || boolFlag(fs, "inspect"), Actor: actorOf(c)})
	if err != nil {
		return err
	}
	c.Println(strings.Join(rep.Lines, "\n"))
	if !rep.OK {
		return cli.Errorf(1, "", "", "")
	}
	return nil
}

func cmdOnboard(c *cli.Context, fs *flag.FlagSet, args []string) error {
	path := "."
	if len(args) > 0 {
		path = args[0]
	}
	s, err := store.Open(store.Options{Home: c.Home, Create: true, Now: c.Now})
	if err != nil {
		return err
	}
	defer s.Close()
	lines, err := onboardProject(s, path, nowOf(&cli.Context{Now: c.Now, Store: s}), onboardOpts{Name: strFlag(fs, "name"), Remote: strFlag(fs, "remote"),
		VerifyCmd: strFlag(fs, "verify"), Brain: boolFlag(fs, "brain"), Actor: actorOf(c)})
	if err != nil {
		return err
	}
	c.Println(strings.Join(lines, "\n"))
	return nil
}

func asHandsError(err error, target **handsError) bool {
	e, ok := err.(*handsError)
	if ok {
		*target = e
	}
	return ok
}

func asHandsExit(err error, target **handsExit) bool {
	e, ok := err.(*handsExit)
	if ok {
		*target = e
	}
	return ok
}

// cmdComputeProviders is `pulse compute providers`: the compute providers this binary can provision with,
// what each makes, the CLI login it works through and the setting that holds its default size.
func cmdComputeProviders(c *cli.Context) error {
	names := ProviderNames()
	chosen := c.Store.Setting("compute.provider", "")
	if c.JSON {
		var rows []map[string]any
		for _, n := range names {
			rows = append(rows, map[string]any{"name": n, "kind": providerNamed(n).Kind(), "auth": providerAuth[n],
				"size_setting": store.ComputeSizeSetting(n), "chosen": n == chosen})
		}
		return cli.WriteJSON(c.Out, map[string]any{"providers": names, "detail": rows, "setting": chosen})
	}
	if len(names) == 0 {
		c.Printf("no compute provider is built in\n")
		return nil
	}
	for _, n := range names {
		mark := ""
		if n == chosen {
			mark = "  (setting compute.provider)"
		}
		c.Printf("%-10s %s; auth: %s; size: %s%s\n", n, providerNamed(n).Kind(), providerAuth[n], store.ComputeSizeSetting(n), mark)
	}
	return nil
}

// providerAuth names the login each provider works through - on this machine, as the daemon's user; PULSE
// keeps no cloud credential of its own.
var providerAuth = map[string]string{
	"azure":     "az login",
	"aws":       "aws configure or aws sso login",
	"gcp":       "gcloud auth login",
	"multipass": "multipassd running locally",
}
