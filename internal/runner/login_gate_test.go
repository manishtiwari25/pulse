package runner

import (
	"bytes"
	"context"
	"errors"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

// gateFakeProvider answers Status from a per-home state so tests steer the login gate.
type gateFakeProvider struct{ state *gateFakeState }

type gateFakeState struct {
	mu    sync.Mutex
	login provider.Login
	err   error
	calls int
}

var (
	gateFakeOnce   sync.Once
	gateFakeStates sync.Map
)

func (p *gateFakeProvider) Name() string  { return "gatefake" }
func (p *gateFakeProvider) Label() string { return "Gate Fake" }
func (p *gateFakeProvider) Chat(context.Context, provider.Request) (provider.Response, error) {
	return provider.Response{}, errors.New("not used")
}
func (p *gateFakeProvider) Models(context.Context) ([]provider.Model, error) { return nil, nil }
func (p *gateFakeProvider) Status(context.Context) (provider.Login, error) {
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	p.state.calls++
	return p.state.login, p.state.err
}

// gateStore is a store whose provider setting names the fake; the returned state steers its answers.
func gateStore(t *testing.T) (*store.Store, *gateFakeState) {
	t.Helper()
	gateFakeOnce.Do(func() {
		provider.Register("gatefake", "Gate Fake", func(o provider.Options) (provider.Provider, error) {
			stateAny, _ := gateFakeStates.LoadOrStore(o.Home, &gateFakeState{})
			return &gateFakeProvider{state: stateAny.(*gateFakeState)}, nil
		})
	})
	s := newStore(t)
	// The choice rule only knows real providers; the row is written directly, as the hub tests do.
	exec1(s.DB, "INSERT INTO settings(name, value, updated_at, updated_by) VALUES ('provider', 'gatefake', ?, 'test')", testNow)
	stateAny, _ := gateFakeStates.LoadOrStore(s.Home, &gateFakeState{})
	return s, stateAny.(*gateFakeState)
}

func TestLocalLoginGateClosesOnlyForNativeWithoutLogin(t *testing.T) {
	s, state := gateStore(t)
	state.login = provider.Login{LoggedIn: true, User: "octocat", Source: "device login"}
	if got := localLoginGate(s); got != "" {
		t.Fatalf("signed in but gated: %q", got)
	}
	state.login = provider.Login{LoggedIn: false, Detail: "the editor login token on this machine is stale (GitHub rejected it): sign in with GitHub"}
	got := localLoginGate(s)
	if !strings.HasPrefix(got, "workers on this machine wait for a Gate Fake login: ") || !strings.Contains(got, "token on this machine is stale") {
		t.Fatalf("not signed in: %q", got)
	}
	state.login = provider.Login{LoggedIn: false}
	if got := localLoginGate(s); !strings.Contains(got, "Settings -> Harness") || !strings.Contains(got, "pulse harness login") {
		t.Fatalf("no detail should still name the fix: %q", got)
	}
	// A check that fails for another reason does not close the machine: the run reports it.
	state.err = errors.New("network down")
	if got := localLoginGate(s); got != "" {
		t.Fatalf("status error gated the machine: %q", got)
	}
	state.err = nil
	// The legacy harness has its own login; the gate stays out of its way.
	setSetting(t, s, "harness", "copilot-cli")
	if got := localLoginGate(s); got != "" {
		t.Fatalf("copilot-cli harness gated: %q", got)
	}
	// One provider per home: its caches must survive between ticks.
	setSetting(t, s, "harness", "native")
	before := state.calls
	localLoginGate(s)
	localLoginGate(s)
	if state.calls != before+2 {
		t.Fatalf("status calls = %d, want %d", state.calls, before+2)
	}
	if a, _ := localProvider(s.Home, "gatefake", nil); a != mustProvider(t, s.Home) {
		t.Fatal("localProvider did not reuse the instance")
	}
}

func mustProvider(t *testing.T, home string) provider.Provider {
	t.Helper()
	p, err := localProvider(home, "gatefake", nil)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestExecutorSlotsHoldThisMachineWhenGated(t *testing.T) {
	s := newStore(t)
	if slots := executorSlots(s, testNow, nil, false); len(slots) == 0 || slots[0] != localExecutor {
		t.Fatalf("open gate: %v", slots)
	}
	if slots := executorSlots(s, testNow, nil, true); len(slots) != 0 {
		t.Fatalf("closed gate still offers this machine: %v", slots)
	}
}

func TestDaemonIdlesBehindLoginGateAndPostsOnce(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	exec1(s.DB, "UPDATE projects SET brain = 1 WHERE slug = 'proj'")
	job, _, err := addJob(s, "proj", "task", "say ok", "echo ok and report", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	prev := loginGate
	loginGate = func(*store.Store) string {
		return "workers on this machine wait for a Gate Fake login: sign in with GitHub (Settings -> Harness)"
	}
	t.Cleanup(func() { loginGate = prev })
	var out bytes.Buffer
	for i := 0; i < 2; i++ {
		if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
			t.Fatalf("daemon = %s\n%s", result, out.String())
		}
	}
	if j := jobRow(s.DB, job.str("id")); j.str("status") != "queued" {
		t.Fatalf("job %s, want queued behind the gate\n%s", j.str("status"), out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs"); n != 0 {
		t.Fatalf("%d run(s) started behind the gate\n%s", n, out.String())
	}
	if !strings.Contains(out.String(), "idle: workers on this machine wait for a Gate Fake login") {
		t.Fatalf("idle reason missing:\n%s", out.String())
	}
	posts := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'login-wanted'")
	if len(posts) != 1 || !strings.Contains(posts[0].str("summary"), "sign-in wanted: 1 job(s) wait") || !strings.Contains(posts[0].str("body"), "pulse harness login") {
		t.Fatalf("login-wanted posts: %v", posts)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'capacity.login-wanted'") != 1 {
		t.Fatal("expected one capacity.login-wanted event")
	}
	if raw, ok, _ := s.Meta("capacity"); !ok || !strings.Contains(raw, `"held":"workers on this machine wait for a Gate Fake login`) {
		t.Fatalf("capacity heartbeat lacks the gate: %s", raw)
	}
	// The gate opens: the same job runs on this machine.
	loginGate = func(*store.Store) string { return "" }
	out.Reset()
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	if j := jobRow(s.DB, job.str("id")); j.str("status") != "done" {
		t.Fatalf("job %s after the gate opened\n%s", j.str("status"), out.String())
	}
}
