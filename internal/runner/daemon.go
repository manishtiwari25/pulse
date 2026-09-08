package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/hooks"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	dispatchTickSeconds = 15 // while runs are in flight the loop wakes this often (or when a run ends) to refill free slots
	handsWantedMinutes  = 30 // jobs waiting this long with every slot busy is the signal that another machine would help
)

// dispatch is one run in flight: the slot it occupies, the worker, and its result once the goroutine reports.
type dispatch struct {
	slot, worker string
	job          row
	run          row
	reason       string
}

func daemonHeartbeat(s *store.Store, loops, interval int, note string) {
	beat := map[string]any{"pid": os.Getpid(), "at": s.Now(), "loops": loops, "interval": interval, "note": note,
		"version": cli.Version(), "revision": cli.Revision()}
	text, _ := json.Marshal(beat)
	_, _ = s.DB.Exec("INSERT INTO meta (key, value) VALUES ('daemon', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(text))
}

func capacityNote(s *store.Store, queued, slots, inflight int, starvedSince, now string) {
	note := map[string]any{"at": now, "queued": queued, "slots": slots, "inflight": inflight, "starved_since": starvedSince}
	text, _ := json.Marshal(note)
	_, _ = s.DB.Exec("INSERT INTO meta (key, value) VALUES ('capacity', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(text))
}

// handsWanted: once a day, when jobs waited handsWantedMinutes with every slot busy, one finding on the brain's board.
func handsWanted(s *store.Store, brain row, queued, slots, minutes int, now, actor string) row {
	if brain == nil {
		return nil
	}
	if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = 'hands-wanted' AND date LIKE ?",
		brain.str("slug"), now[:10]+"%") != nil {
		return nil
	}
	summary := trunc(fmt.Sprintf("hands wanted: %d job(s) waited %d min with all %d slot(s) busy; pairing another machine "+
		"(pulse pair) or raising workers/parallel_per_project would shorten the queue", queued, minutes, slots), 230)
	body := fmt.Sprintf("The daemon on this machine found %d queued job(s) waiting for %d minutes while every executor slot (%d) was busy "+
		"(%s). More hands would shorten the queue: pair another machine with `pulse pair` (pulse record show WF-executors), "+
		"raise `workers` on a machine that has the capacity, or raise `parallel_per_project` when the wait is on one "+
		"project. The human owns machines and money; this post is a measurement, not a request to act.\n", queued, minutes, slots, now)
	post, err := createPost(s, now, postOpts{Kind: "finding", Project: brain.str("slug"), Area: "runner", Thread: "hands-wanted",
		Summary: summary, Body: body, Agent: actorAgent(s, actor), Date: now})
	if err != nil {
		return nil
	}
	eventAt(s.DB, now, actor, "capacity.hands-wanted", brain.str("slug"), map[string]any{"queued": queued, "slots": slots, "minutes": minutes, "post": post.str("id")})
	return post
}

// binaryStamp is source_stamp(): the mtime of the binary on disk; the service restarts a changed one.
func binaryStamp() string { return cli.BinaryStamp() }

type daemonOpts struct {
	Agent           string
	Interval        int
	DryRun          bool
	WorkerCmd       string
	MaxLoops        int // 0 = unlimited
	RestartOnChange bool
	Grace           time.Duration // after a stop signal, how long in-flight runs may finish before their groups are killed
	Out             io.Writer
	HubHome         string         // the hub user's home (ssh keys, known_hosts) for pairing the machines compute provisions; "" = this user's
	SelfUpdate      *selfUpdateEnv // ADR-035; nil = derive from the running binary (tests pass fakes)
}

// daemonWake is what wakes the loop before its interval is over: the owner wrote (message.sent, task.*), a
// compute request was approved, a setting changed (the stop switch), a job was queued by a lead or a CLI.
var daemonWake = []string{"message.sent", "task.created", "task.amended", "task.status", "compute.approved", "setting", "job.add"}

// runBudget bounds one run's context: the harness minutes, the setup budget, verify and landing slack.
func runBudget(s *store.Store) time.Duration {
	minutes := settingInt(s, "minutes_per_job")
	if minutes < 1 {
		minutes = 1
	}
	return time.Duration(2*minutes+setupMinutes+15) * time.Minute
}

// daemon is the keep-alive loop rebuilt on goroutines and channels (D-E): a hooks runner and a bus bridge so
// other processes' writes reach in-process subscribers, the compute worker, then per tick: land, apply passed
// amendments, tend the leads, plan, lease into free slots, one goroutine per run with a context deadline,
// results fanned in over `results`. Returns "stopped" (the stop switch: contexts cancelled, runs get the
// grace), "done", "restart", or "interrupted" (ctx cancelled: no new leases, in-flight runs finish or are
// killed after Grace).
func daemon(ctx context.Context, s *store.Store, o daemonOpts) string {
	out := o.Out
	if out == nil {
		out = os.Stdout
	}
	var outMu sync.Mutex
	say := func(format string, args ...any) {
		outMu.Lock()
		defer outMu.Unlock()
		fmt.Fprintf(out, format+"\n", args...)
	}
	interval := o.Interval
	if interval <= 0 {
		interval = 300
	}
	actor := o.Agent
	if actor == "" {
		actor = "daemon"
	}
	loops := 0
	lastPruneDay := ""
	stamp := binaryStamp()
	results := make(chan *dispatch, 64)
	inflight := map[*dispatch]bool{}
	starvedSince := ""
	refused := amendRefusals{}
	probes := &checkoutProbes{}
	defer holdCheckouts(nil)
	// loopCtx ends with the loop: hooks, bridge, compute and the wake subscription stop with it; runCtx
	// outlives it by the grace so in-flight runs may finish.
	loopCtx, stopLoop := context.WithCancel(ctx)
	defer stopLoop()
	runCtx, killRuns := context.WithCancel(context.Background())
	defer killRuns()
	hooksRunner := &hooks.Runner{Bus: bus.Default(), Store: s, Log: func(format string, a ...any) { say("hook: "+format, a...) }}
	hooksDone := make(chan struct{})
	go func() {
		defer close(hooksDone)
		hooksRunner.Run(loopCtx)
	}()
	bridgeDone := bus.Bridge(loopCtx, s, 0)
	compute := newComputeWorker(loopCtx, s, computeOpts{Actor: actor, HubHome: o.HubHome})
	wakeEvents, cancelWake := bus.Default().Subscribe(bus.Filter{Actions: daemonWake}, 32)
	defer cancelWake()
	lost := sweepLostRuns(s, s.Now())
	dry := ""
	if o.DryRun {
		dry = ", dry run"
	}
	say("daemon started %s (interval %ds%s, pid %d)", s.Now(), interval, dry, os.Getpid())
	if lost > 0 {
		say("closed %d run(s) a previous runner left in progress as lost", lost)
	}
	// ADR-035: the daemon looks after its own binary; the launcher's restore marker is read once, at start.
	selfEnv := selfUpdateEnv{Binary: pulseBinary(), GoCmd: "go", Revision: cli.Revision(), Actor: actor}
	if o.SelfUpdate != nil {
		selfEnv = *o.SelfUpdate
		selfEnv.Actor = actor
	}
	for _, l := range checkRestored(s, selfEnv) {
		say("%s", l)
	}
	report := func(d *dispatch) {
		delete(inflight, d)
		if d.run != nil {
			say("%s %s %s on %s: %s", d.run.str("status"), d.run.str("job_id"), d.run.str("agent"), d.slot, d.run.str("summary"))
		} else if d.reason != "" {
			say("idle: %s", d.reason)
		}
	}
	// reap collects the runs whose goroutines ended without waiting.
	reap := func() []*dispatch {
		var finished []*dispatch
		for {
			select {
			case d := <-results:
				report(d)
				finished = append(finished, d)
			default:
				return finished
			}
		}
	}
	waitAll := func(grace time.Duration) {
		if len(inflight) == 0 {
			return
		}
		var timer <-chan time.Time
		if grace > 0 {
			t := time.NewTimer(grace)
			defer t.Stop()
			timer = t.C
		}
		for len(inflight) > 0 {
			select {
			case d := <-results:
				report(d)
			case <-timer:
				say("grace of %s over: killing %d run(s) still in flight", grace, len(inflight))
				killRuns()
				timer = nil
			}
		}
	}
	graceFor := func(result string) time.Duration {
		if result != "interrupted" && result != "stopped" {
			return 0
		}
		if o.Grace > 0 {
			return o.Grace
		}
		return time.Duration(settingInt(s, "minutes_per_job")+5) * time.Minute
	}
	leave := func(result, note string) string {
		stopLoop()
		grace := graceFor(result)
		if len(inflight) > 0 {
			say("%s: waiting for %d run(s) in flight to finish", note, len(inflight))
			waitAll(grace)
		}
		if compute.busy() > 0 {
			say("%s: waiting for %d compute operation(s) in flight", note, compute.busy())
		}
		compute.wait(grace)
		for _, l := range compute.drain() {
			say("%s", l)
		}
		<-bridgeDone
		<-hooksDone
		beat := "exited"
		if result == "restart" {
			beat = "restarting"
		}
		daemonHeartbeat(s, loops, interval, beat)
		return result
	}
	// sleep waits d, or less when the store wakes the loop; false when ctx ended.
	sleep := func(d time.Duration) bool {
		timer := time.NewTimer(d)
		defer timer.Stop()
		for {
			select {
			case <-ctx.Done():
				return false
			case <-timer.C:
				return true
			case _, ok := <-wakeEvents:
				if !ok {
					wakeEvents = nil
					continue
				}
				return true
			case <-compute.wake:
				return true
			}
		}
	}
	for {
		loops++
		now := s.Now()
		finished := reap()
		if len(finished) > 0 && len(inflight) == 0 {
			allErr := true
			for _, d := range finished {
				if d.run != nil && d.run.str("status") != "error" {
					allErr = false
				}
			}
			if allErr && !sleep(time.Duration(interval)*time.Second) {
				return leave("interrupted", "stopping on signal")
			}
		}
		if ctx.Err() != nil {
			return leave("interrupted", "stopping on signal")
		}
		if why := runnerStopped(s); why != "" {
			say("stopped: %s", why)
			return leave("stopped", "stopping")
		}
		if o.RestartOnChange && stamp != "" && binaryStamp() != stamp {
			say("pulse changed on disk; exiting with code 3 so the service restarts it")
			return leave("restart", "restarting")
		}
		daemonHeartbeat(s, loops, interval, "")
		var started []*dispatch
		reason := ""
		var slots []string
		func() {
			defer func() {
				if r := recover(); r != nil {
					reason = fmt.Sprintf("daemon error: %v", r)
				}
			}()
			if now[:10] != lastPruneDay {
				pruned, err := pruneAgents(s, now, settingInt(s, "prune_after_days"), actor, false)
				if err != nil {
					say("prune skipped: %s", errMessage(err))
				}
				for _, pr := range pruned {
					say("pruned %s: %s", pr.Name, pr.Reason)
				}
				lastPruneDay = now[:10]
			}
			projects, _, holdLines := probes.ready(projectRows(s.DB, false), checkoutProbeTimeout)
			for _, l := range holdLines {
				say("%s", l)
			}
			for _, proj := range projects {
				// Verified branches land first, so a requeued job is planned and taken in the same loop (ADR-024).
				for _, lr := range landPending(s, proj, actor, now, true, 0, false) {
					say("landing %s: %s", lr.State, lr.Note)
				}
			}
			// ADR-035: a landing in the brain becomes the running binary; the build runs off the tick.
			if !o.DryRun {
				selfEnv.Inflight = len(inflight)
				for _, l := range tendBinary(s, now, selfEnv) {
					say("%s", l)
				}
			}
			for _, proj := range projects {
				slug := proj.str("slug")
				// D-H: passed amendments become the constitution; refusals go to the human once.
				for _, l := range autoApplyAmendments(s, slug, now, refused) {
					say("%s", l)
				}
				// D-C: the lead answers the owner and turns tasks into jobs; the planner records their progress.
				for _, l := range tendLead(s, slug, now, actor) {
					say("%s", l)
				}
				for _, l := range tendTasks(s, slug, now, actor) {
					say("%s", l)
				}
			}
			// D-J: every active project is planned before the first lease.
			for _, proj := range projects {
				jobs, err := planJobs(s, proj.str("slug"), now, actor)
				if err != nil {
					say("planning %s: %s", proj.str("slug"), errMessage(err))
				}
				for _, job := range jobs {
					say("planned %s %s: %s", job.str("kind"), job.str("id"), job.str("title"))
				}
			}
			for _, l := range compute.tick(loopCtx, now) {
				say("%s", l)
			}
			// ADR-033: the private held-out set writes itself, one sealed call per tick.
			for _, l := range tendHeldout(s, now) {
				say("%s", l)
			}
			for _, pr := range reprobeDue(s, interval, now) {
				say("executor %s is reachable again: %s", pr[0], pr[1])
			}
			// ADR-034: a drained executor heals itself or is retired; retired VMs past their grace are deleted.
			for _, l := range tendExecutors(s, now, actor) {
				say("%s", l)
			}
			busy := map[string]int{}
			for d := range inflight {
				busy[d.slot]++
			}
			slots = executorSlots(s, now, busy)
			if len(slots) == 0 {
				reason = "every executor slot is busy"
			} else {
				// Executors limited to some projects lease first so an unrestricted one cannot starve them.
				rows := map[string]row{}
				for _, name := range slots {
					if _, ok := rows[name]; !ok {
						rows[name] = executorRow(s, name)
					}
				}
				order := make([]int, len(slots))
				for i := range order {
					order[i] = i
				}
				sort.SliceStable(order, func(a, b int) bool {
					ua := rows[slots[order[a]]] != nil && rows[slots[order[a]]].str("projects") == "*"
					ub := rows[slots[order[b]]] != nil && rows[slots[order[b]]].str("projects") == "*"
					if ua != ub {
						return !ua
					}
					return order[a] < order[b]
				})
				var names []string
				for d := range inflight {
					names = append(names, d.worker)
				}
				reasons := map[string]bool{}
				launch := func(slot, worker string, job row) {
					names = append(names, worker)
					d := &dispatch{slot: slot, worker: worker, job: job}
					inflight[d] = true
					started = append(started, d)
					go func(d *dispatch) {
						dctx, cancel := context.WithTimeout(runCtx, jobBudget(s, d.job))
						defer cancel()
						defer func() {
							if r := recover(); r != nil {
								d.reason = fmt.Sprintf("daemon error on %s: %v", d.slot, r)
							}
							results <- d
						}()
						run, err := runJob(s, d.job, d.worker, now, runOpts{DryRun: o.DryRun, WorkerCmd: o.WorkerCmd, Executor: d.slot, Ctx: dctx})
						if err != nil {
							d.reason = fmt.Sprintf("daemon error on %s: %s", d.slot, errMessage(err))
							return
						}
						d.run = run
					}(d)
				}
				for _, i := range order {
					// The owner is waiting on a lead: its job goes first, run by the lead itself (D-C).
					if lead, job, why := leadJobFor(s, names, o.DryRun, now, slots[i], len(started)); job != nil {
						launch(slots[i], lead, job)
						continue
					} else if why != "" {
						reasons[why] = true
					}
					worker, err := pickWorker(s, now, o.Agent, names)
					if err != nil {
						reasons[errMessage(err)] = true
						continue
					}
					job, _, why, err := takeJob(s, worker, "", "", o.DryRun, now, slots[i], len(started))
					if err != nil {
						job, why = nil, errMessage(err)
					}
					if job == nil {
						if why != "" {
							reasons[why] = true
						}
						continue
					}
					launch(slots[i], worker, job)
				}
				reason = strings.Join(sortedKeys(boolKeys(reasons)), "; ")
				if reason == "" && len(started) == 0 {
					reason = "no queued job"
				}
			}
			queued := mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active'").int("n")
			totalSlots := len(inflight) + len(slots) - len(started)
			starved := queued > 0 && len(slots)-len(started) == 0 && len(inflight) > 0
			if starved {
				if starvedSince == "" {
					starvedSince = now
				}
			} else {
				starvedSince = ""
			}
			capacityNote(s, queued, totalSlots, len(inflight), starvedSince, now)
			if starvedSince != "" {
				waited := int(mustISO(now).Sub(mustISO(starvedSince)).Minutes())
				if waited >= handsWantedMinutes {
					if post := handsWanted(s, brainProject(s.DB), queued, totalSlots, waited, now, actor); post != nil {
						say("hands wanted: %s", post.str("summary"))
					}
				}
			}
		}()
		for _, l := range compute.drain() {
			say("%s", l)
		}
		if len(started) == 0 && len(inflight) == 0 && reason != "" {
			say("idle: %s", reason)
		} else if reason != "" && len(started) > 0 && len(started) < len(slots) {
			say("idle: %s", reason)
		}
		if o.MaxLoops > 0 && loops >= o.MaxLoops {
			return leave("done", "loop limit reached")
		}
		if len(inflight) > 0 {
			select {
			case d := <-results:
				report(d)
			case <-time.After(dispatchTickSeconds * time.Second):
			case <-compute.wake:
			case _, ok := <-wakeEvents:
				if !ok {
					wakeEvents = nil
				}
			case <-ctx.Done():
				return leave("interrupted", "stopping on signal")
			}
		} else if !sleep(time.Duration(interval) * time.Second) {
			return leave("interrupted", "stopping on signal")
		}
	}
}

func boolKeys(m map[string]bool) map[string]string {
	out := map[string]string{}
	for k := range m {
		out[k] = ""
	}
	return out
}
