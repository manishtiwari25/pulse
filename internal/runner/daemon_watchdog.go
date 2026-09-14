package runner

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"
)

// The loop body is meant to be quick - landing verification, measurement and the runs themselves are off
// the loop by design - so a heartbeat older than this is a loop that hangs, not one that is busy. The
// factor keeps a long owner interval honest; the floor leaves the planner's model calls (120s per request,
// retried) and the checkout probes their room.
const (
	daemonStallFactor = 6
	daemonStallFloor  = 30 * time.Minute
	// watchdogKillGrace is how long the runs in flight get to see their cancelled context before exit.
	watchdogKillGrace = 15 * time.Second
	stallDumpKeep     = 5 // dumps older than the newest this many are removed
)

// daemonStallBound is how long the loop may go without a heartbeat before the watchdog acts.
func daemonStallBound(interval int) time.Duration {
	d := time.Duration(interval) * time.Second * daemonStallFactor
	if d < daemonStallFloor {
		d = daemonStallFloor
	}
	return d
}

// daemonWatchdog restarts a daemon whose loop stopped beating. The loop cannot leave on its own when one
// of its synchronous steps blocks (a syscall macOS holds behind a consent dialog, a lock never released),
// so `pulse status` said "silent" for as long as nobody looked. Now the stall is written down - a goroutine
// dump under logs/, a note in the heartbeat row - the runs in flight are cancelled, and the process exits
// 3, the code the service launcher restarts on (ADR-035). SIGUSR1 writes the same dump without exiting,
// for looking at a slow loop while it runs.
type daemonWatchdog struct {
	mu     sync.Mutex
	beatAt time.Time
	bound  time.Duration
	logs   string
	say    func(string, ...any)
	note   func(note, dump string) // records the stall in the heartbeat row and the event log
	kill   func()                  // cancels the runs in flight
	exit   func(int)               // os.Exit unless a test says otherwise
	now    func() time.Time
	dump   func() []byte
	grace  time.Duration // the runs' time to see their cancelled context before exit
	fired  bool
}

func newDaemonWatchdog(bound time.Duration, logs string, say func(string, ...any), note func(note, dump string), kill func()) *daemonWatchdog {
	return &daemonWatchdog{beatAt: time.Now(), bound: bound, logs: logs, say: say, note: note, kill: kill,
		exit: os.Exit, now: time.Now, dump: goroutineDump, grace: watchdogKillGrace}
}

// beat is called by the loop each time it writes its heartbeat.
func (w *daemonWatchdog) beat() {
	w.mu.Lock()
	w.beatAt = w.now()
	w.mu.Unlock()
}

// stalled reports how long the loop has been silent past its bound, or 0.
func (w *daemonWatchdog) stalled() time.Duration {
	w.mu.Lock()
	defer w.mu.Unlock()
	if since := w.now().Sub(w.beatAt); since > w.bound {
		return since
	}
	return 0
}

// run checks the beat every tick until ctx ends; on a stall it dumps, notes, kills and exits once.
func (w *daemonWatchdog) run(ctx context.Context, tick time.Duration) {
	timer := time.NewTicker(tick)
	defer timer.Stop()
	usr1 := make(chan os.Signal, 1)
	signal.Notify(usr1, syscall.SIGUSR1)
	defer signal.Stop(usr1)
	for {
		select {
		case <-ctx.Done():
			return
		case <-usr1:
			path, err := w.writeDump("signal")
			if err != nil {
				w.say("watchdog: goroutine dump failed: %v", err)
			} else {
				w.say("watchdog: goroutine dump on SIGUSR1 at %s", path)
			}
		case <-timer.C:
			if since := w.stalled(); since > 0 {
				w.fire(since)
				return
			}
		}
	}
}

func (w *daemonWatchdog) fire(since time.Duration) {
	w.mu.Lock()
	if w.fired {
		w.mu.Unlock()
		return
	}
	w.fired = true
	w.mu.Unlock()
	path, err := w.writeDump("stall")
	where := path
	if err != nil {
		where = fmt.Sprintf("no dump: %v", err)
	}
	line := fmt.Sprintf("watchdog: the loop has not beaten for %s (bound %s); goroutines at %s; cancelling runs and exiting 3 so the service restarts the daemon",
		since.Round(time.Second), w.bound, where)
	w.say("%s", line)
	if w.note != nil {
		w.note("stalled "+since.Round(time.Second).String()+"; restarting", path)
	}
	if w.kill != nil {
		w.kill()
		time.Sleep(w.grace)
	}
	w.exit(3)
}

// writeDump writes every goroutine's stack to logs/daemon-<why>-<utc>.log and prunes old dumps.
func (w *daemonWatchdog) writeDump(why string) (string, error) {
	if err := os.MkdirAll(w.logs, 0o700); err != nil {
		return "", err
	}
	name := fmt.Sprintf("daemon-%s-%s.log", why, w.now().UTC().Format("20060102T150405Z"))
	path := filepath.Join(w.logs, name)
	head := fmt.Sprintf("pulse daemon pid %d, %s, %s UTC\n\n", os.Getpid(), why, w.now().UTC().Format(time.RFC3339))
	if err := os.WriteFile(path, append([]byte(head), w.dump()...), 0o600); err != nil {
		return "", err
	}
	pruneStallDumps(w.logs, stallDumpKeep)
	return path, nil
}

func goroutineDump() []byte {
	buf := make([]byte, 1<<20)
	for {
		n := runtime.Stack(buf, true)
		if n < len(buf) {
			return buf[:n]
		}
		buf = make([]byte, 2*len(buf))
	}
}

// pruneStallDumps keeps the newest `keep` daemon-*.log dumps; names sort by their UTC stamp.
func pruneStallDumps(logs string, keep int) {
	entries, err := os.ReadDir(logs)
	if err != nil {
		return
	}
	var names []string
	for _, e := range entries {
		n := e.Name()
		if !e.IsDir() && (strings.HasPrefix(n, "daemon-stall-") || strings.HasPrefix(n, "daemon-signal-")) && strings.HasSuffix(n, ".log") {
			names = append(names, n)
		}
	}
	if len(names) <= keep {
		return
	}
	// The UTC stamp is the only part that varies, so it orders the dumps oldest first.
	sort.SliceStable(names, func(a, b int) bool { return dumpStamp(names[a]) < dumpStamp(names[b]) })
	for _, n := range names[:len(names)-keep] {
		_ = os.Remove(filepath.Join(logs, n))
	}
}

func dumpStamp(name string) string {
	i := strings.LastIndex(name, "-")
	if i < 0 {
		return name
	}
	return strings.TrimSuffix(name[i+1:], ".log")
}
