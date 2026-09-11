package runner

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	loginReuseProviderName  = "loginreusefake"
	loginReuseProviderToken = "gho_provider000000000000000000000000"
	loginReuseGhToken       = "gho_hubtoken00000000000000000000000000"
)

type loginReuseProvider struct{ state *loginReuseState }

type loginReuseState struct {
	mu    sync.Mutex
	token string
	login provider.Login
	err   error
}

var (
	loginReuseOnce   sync.Once
	loginReuseStates sync.Map
)

func (p *loginReuseProvider) Name() string  { return loginReuseProviderName }
func (p *loginReuseProvider) Label() string { return "Login Reuse Fake" }
func (p *loginReuseProvider) Chat(context.Context, provider.Request) (provider.Response, error) {
	return provider.Response{}, errors.New("not used")
}
func (p *loginReuseProvider) Models(context.Context) ([]provider.Model, error) { return nil, nil }
func (p *loginReuseProvider) Status(context.Context) (provider.Login, error) {
	return provider.Login{}, nil
}
func (p *loginReuseProvider) ExportLogin() (string, provider.Login, error) {
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	return p.state.token, p.state.login, p.state.err
}

func loginReuseStore(t *testing.T) (*store.Store, *loginReuseState) {
	t.Helper()
	loginReuseOnce.Do(func() {
		provider.Register(loginReuseProviderName, "Login Reuse Fake", func(o provider.Options) (provider.Provider, error) {
			stateAny, _ := loginReuseStates.LoadOrStore(o.Home, &loginReuseState{})
			return &loginReuseProvider{state: stateAny.(*loginReuseState)}, nil
		})
	})
	s := newStore(t)
	exec1(s.DB, "INSERT INTO settings(name, value, updated_at, updated_by) VALUES ('provider', ?, ?, 'test')", loginReuseProviderName, testNow)
	stateAny, _ := loginReuseStates.LoadOrStore(s.Home, &loginReuseState{})
	t.Cleanup(func() {
		loginReuseStates.Delete(s.Home)
		providerSource.forget()
		ghSource.forget()
	})
	return s, stateAny.(*loginReuseState)
}

func TestNativeLoginSourceUsesProviderExporterAndCache(t *testing.T) {
	s, state := loginReuseStore(t)
	state.token = loginReuseProviderToken
	state.login = provider.Login{LoggedIn: true, User: "octocat", Source: "stored"}
	gh := &fakeGitHub{login: "octocat"}
	gh.install(t)
	oldClock := wallClock
	t.Cleanup(func() { wallClock = oldClock })
	clock := time.Date(2026, 9, 11, 7, 0, 0, 0, time.UTC)
	wallClock = func() time.Time { return clock }

	tok, origin, err := loginSource(s)
	if err != nil || string(tok) != loginReuseProviderToken || origin != loginSourceProvider {
		t.Fatalf("provider source: %q %s %v", tok, origin, err)
	}
	if !hasLoginSource(s) {
		t.Fatal("provider login did not satisfy hasLoginSource")
	}
	if n := gh.count(); n != 2 {
		t.Fatalf("GitHub checks = %d, calls %v", n, gh.calls)
	}
	loginSource(s)
	loginSource(s)
	if n := gh.count(); n != 2 {
		t.Fatalf("provider acceptance cache missed: %d", n)
	}
	clock = clock.Add(11 * time.Minute)
	loginSource(s)
	if n := gh.count(); n != 4 {
		t.Fatalf("provider acceptance cache did not expire: %d", n)
	}
	for _, tok := range gh.tokens {
		if tok != loginReuseProviderToken {
			t.Fatalf("GitHub saw token %q", tok)
		}
	}
}

func TestNativeLoginSourceNeverUsesGhToken(t *testing.T) {
	s, state := loginReuseStore(t)
	state.err = &provider.Error{Provider: loginReuseProviderName, Code: "no_stored_login", Message: "none"}
	stubGh(t, loginReuseGhToken)
	gh := &fakeGitHub{}
	gh.install(t)

	tok, origin, err := loginSource(s)
	if err != nil || tok != nil || origin != loginSourceNone {
		t.Fatalf("native gh fallback: %q %s %v", tok, origin, err)
	}
	if gh.count() != 0 {
		t.Fatalf("native mode asked GitHub about gh token: %v", gh.calls)
	}
}

func TestLegacyLoginSourceStillUsesGhToken(t *testing.T) {
	s, _ := loginReuseStore(t)
	setSetting(t, s, "harness", "copilot-cli")
	stubGh(t, loginReuseGhToken)
	gh := &fakeGitHub{login: "hub-user"}
	gh.install(t)

	tok, origin, err := loginSource(s)
	if err != nil || string(tok) != loginReuseGhToken || origin != loginSourceHubLogin {
		t.Fatalf("legacy gh source: %q %s %v", tok, origin, err)
	}
	if gh.count() != 2 {
		t.Fatalf("legacy gh checks = %d", gh.count())
	}
}

func TestExecutorLoginWithoutTokenReusesLoginSource(t *testing.T) {
	s, state := loginReuseStore(t)
	state.token = loginReuseProviderToken
	state.login = provider.Login{LoggedIn: true, User: "octocat", Source: "stored"}
	gh := &fakeGitHub{login: "octocat"}
	gh.install(t)
	if _, err := addExecutor(s, testNow, "hands", "pulse@hands", "/srv/pulse", 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	old := runSSH
	t.Cleanup(func() { runSSH = old })
	var sent []string
	handCalls := 0
	runSSH = func(_ context.Context, argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		if strings.Contains(strings.Join(argv, " "), "pulse harness login --with-token") {
			sent = append(sent, strings.TrimSpace(string(stdin)))
			return 0, "", "", nil
		}
		if len(argv) == 0 || !strings.HasPrefix(argv[len(argv)-1], "b64 ") {
			t.Fatalf("unexpected ssh argv %v", argv)
		}
		handCalls++
		switch handCalls {
		case 1:
			return 0, "hands=ok user=pulse workdir=/srv/pulse git=2.40 harness=yes pulse=1.2.3 remotes=0 keys_ok=yes disk_free_mb=4096 jobs=- keys=- heartbeat=1\n", "", nil
		case 2:
			return 0, "", "", nil
		default:
			return 0, "SUMMARY: ok\n", "", nil
		}
	}

	out := invokeOK(t, s, "executor", "", "login", "hands", "--admin", "admin@hands")
	if len(sent) != 1 || sent[0] != loginReuseProviderToken {
		t.Fatalf("sent tokens = %#v", sent)
	}
	if !strings.Contains(out, "login source: "+loginSourceProvider) || strings.Contains(out, loginReuseProviderToken) {
		t.Fatalf("executor login output: %q", out)
	}
	ev := mustOne(s.DB, "SELECT detail FROM events WHERE action = 'executor.login' ORDER BY seq DESC LIMIT 1")
	if detail := ev.str("detail"); !strings.Contains(detail, loginSourceProvider) || strings.Contains(detail, loginReuseProviderToken) {
		t.Fatalf("event detail leaked or missed source: %q", detail)
	}
}

func TestExecutorLoginWithoutTokenExplainsMissingSource(t *testing.T) {
	s, state := loginReuseStore(t)
	state.err = &provider.Error{Provider: loginReuseProviderName, Code: "no_stored_login", Message: "none"}
	stubGh(t, loginReuseGhToken)
	gh := &fakeGitHub{}
	gh.install(t)
	if _, err := addExecutor(s, testNow, "hands", "pulse@hands", "/srv/pulse", 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	out, errText, code := invoke(t, s, "executor", "", "login", "hands", "--admin", "admin@hands")
	if code != 2 || out != "" {
		t.Fatalf("missing source code/out = %d %q", code, out)
	}
	for _, want := range []string{"Settings -> Harness", "pulse harness login", "pulse executor token --with-token", "--with-token"} {
		if !strings.Contains(errText, want) {
			t.Fatalf("missing %q in %q", want, errText)
		}
	}
	if strings.Contains(errText, loginReuseGhToken) || gh.count() != 0 {
		t.Fatalf("missing source leaked token or used gh: %q calls=%v", errText, gh.calls)
	}
}
