package runner

import (
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// A fine-grained-token-shaped string no GitHub ever issued; the fake GitHub below accepts it.
const testExecutorToken = "github_pat_11ATESTTOKEN0000000000_TESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTEST"

// writeExecutorToken places the fake token where loginSource reads it (0700/0600), as the command would.
func writeExecutorToken(t *testing.T, s *store.Store) string {
	t.Helper()
	path := executorTokenPath(s)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(testExecutorToken+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

// fakeGitHub answers GET /user and GET /user/repos; repos says how many repositories the token reaches and
// status overrides /user's answer. Every request is counted.
type fakeGitHub struct {
	mu     sync.Mutex
	repos  int
	status int
	login  string
	calls  []string
	tokens []string
}

func (g *fakeGitHub) install(t *testing.T) {
	t.Helper()
	old := githubHTTP
	t.Cleanup(func() { githubHTTP = old; ghSource.forget() })
	ghSource.forget()
	githubHTTP = &http.Client{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		g.mu.Lock()
		defer g.mu.Unlock()
		g.calls = append(g.calls, r.URL.Path+"?"+r.URL.RawQuery)
		g.tokens = append(g.tokens, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
		body, status := "", 200
		switch r.URL.Path {
		case "/user":
			login := g.login
			if login == "" {
				login = "hands-bot"
			}
			body = `{"login":"` + login + `"}`
			if g.status != 0 {
				status = g.status
			}
		case "/user/repos":
			items := make([]string, 0, g.repos)
			for i := 0; i < g.repos; i++ {
				items = append(items, `{"full_name":"acme/repo"}`)
			}
			body = "[" + strings.Join(items, ",") + "]"
		default:
			status = 404
		}
		h := http.Header{}
		h.Set("github-authentication-token-expiration", "2027-01-01 00:00:00 UTC")
		return &http.Response{StatusCode: status, Header: h, Body: io.NopCloser(strings.NewReader(body)), Request: r}, nil
	})}
}

func (g *fakeGitHub) count() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return len(g.calls)
}

// withStdin runs f with os.Stdin reading text, as `... | pulse executor token --with-token` would.
func withStdin(t *testing.T, text string, f func()) {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	old := os.Stdin
	os.Stdin = r
	defer func() { os.Stdin = old; r.Close() }()
	go func() { io.WriteString(w, text); w.Close() }()
	f()
}

// stubGh makes the hub's `gh auth token` answer tok for the test.
func stubGh(t *testing.T, tok string) {
	t.Helper()
	old := ghAuthToken
	t.Cleanup(func() { ghAuthToken = old; ghSource.forget() })
	ghSource.forget()
	ghAuthToken = func() (string, error) { return tok, nil }
}

func TestExecutorTokenCommand(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	gh := &fakeGitHub{login: "hands-bot"}
	gh.install(t)

	// Report before anything is placed: no file, no token anywhere.
	out := invokeOK(t, s, "executor", "", "token")
	if !strings.Contains(out, "no executor token on file") {
		t.Fatalf("report: %q", out)
	}

	// A classic token is refused before GitHub is asked.
	withStdin(t, "ghp_classic0000000000000000000000000000\n", func() {
		_, errText, code := invoke(t, s, "executor", "", "token", "--with-token")
		if code != 2 || !strings.Contains(errText, "classic personal access token") {
			t.Fatalf("ghp_: %d %q", code, errText)
		}
	})
	if gh.count() != 0 {
		t.Fatalf("GitHub asked about a classic token: %v", gh.calls)
	}

	// A token that reaches one repository is refused with the fixed sentence; nothing is written.
	gh.repos = 1
	withStdin(t, testExecutorToken+"\n", func() {
		_, errText, code := invoke(t, s, "executor", "", "token", "--with-token")
		if code != 1 || !strings.Contains(errText, noRepositoryRefusal) {
			t.Fatalf("one repo: %d %q", code, errText)
		}
	})
	if _, err := os.Stat(executorTokenPath(s)); err == nil {
		t.Fatal("a refused token was written")
	}

	// Zero repositories: accepted, written 0600 under private/ 0700, finding and event carry the login only.
	gh.repos = 0
	withStdin(t, testExecutorToken+"\n", func() {
		out = invokeOK(t, s, "executor", "", "token", "--with-token")
	})
	if !strings.Contains(out, "GitHub accepts the token for hands-bot") || !strings.Contains(out, "reaches no repository") || strings.Contains(out, testExecutorToken) {
		t.Fatalf("accept: %q", out)
	}
	path := executorTokenPath(s)
	if path != filepath.Join(s.Home, "private", "executor-token") {
		t.Fatalf("token path: %s", path)
	}
	fi, err := os.Stat(path)
	if err != nil || fi.Mode().Perm() != 0o600 {
		t.Fatalf("token file: %v %v", fi, err)
	}
	if di, _ := os.Stat(filepath.Dir(path)); di.Mode().Perm() != 0o700 {
		t.Fatalf("private dir mode: %v", di.Mode())
	}
	data, _ := os.ReadFile(path)
	if strings.TrimSpace(string(data)) != testExecutorToken {
		t.Fatal("token file content differs")
	}
	joined := strings.Join(gh.calls, " ")
	if !strings.Contains(joined, "/user?") || !strings.Contains(joined, "/user/repos?per_page=1&affiliation=owner,collaborator,organization_member") {
		t.Fatalf("GitHub calls: %v", gh.calls)
	}
	ev := mustOne(s.DB, "SELECT * FROM events WHERE action = 'executor.token' ORDER BY seq DESC LIMIT 1")
	if d := jsonMap(ev.str("detail")); ev == nil || d["login"] != "hands-bot" || d["ok"] != true {
		t.Fatalf("event: %v", ev)
	}
	posts := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'executors'")
	if len(posts) != 1 || posts[0].str("summary") != "executor token placed on the hub for hands-bot" {
		t.Fatalf("finding: %v", posts)
	}
	for _, r := range append(posts, ev) {
		for _, col := range []string{"summary", "body", "detail"} {
			if strings.Contains(r.str(col), testExecutorToken) {
				t.Fatalf("token leaked into %s", col)
			}
		}
	}

	// Report: presence, account, expiry - never the token.
	out = invokeOK(t, s, "executor", "", "token")
	if !strings.Contains(out, "executor token on file") || !strings.Contains(out, "account hands-bot") || !strings.Contains(out, "expires 2027-01-01") || strings.Contains(out, testExecutorToken) {
		t.Fatalf("report: %q", out)
	}
	if _, _, code := invoke(t, s, "executor", "", "token", "--with-token", "--remove"); code != 2 {
		t.Fatalf("both flags accepted: %d", code)
	}

	// Remove.
	out = invokeOK(t, s, "executor", "", "token", "--remove")
	if !strings.Contains(out, "executor token removed") {
		t.Fatalf("remove: %q", out)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("token file still there")
	}
	if out = invokeOK(t, s, "executor", "", "token", "--remove"); !strings.Contains(out, "no executor token on file") {
		t.Fatalf("second remove: %q", out)
	}
}

func TestLoginSourcePrecedenceAndCache(t *testing.T) {
	s := newStore(t)
	gh := &fakeGitHub{}
	gh.install(t)
	stubGh(t, "")
	oldClock := wallClock
	t.Cleanup(func() { wallClock = oldClock })
	clock := time.Date(2026, 9, 6, 10, 0, 0, 0, time.UTC)
	wallClock = func() time.Time { return clock }

	// Nothing anywhere: none, and GitHub is not asked.
	tok, origin, err := loginSource(s)
	if tok != nil || origin != loginSourceNone || err != nil || gh.count() != 0 {
		t.Fatalf("empty: %q %s %v %d", tok, origin, err, gh.count())
	}
	if hasLoginSource(s) {
		t.Fatal("has a source from nothing")
	}

	// The hub's gh token counts only after passing the same checks; the answer is cached for ten minutes.
	setSetting(t, s, "harness", "copilot-cli")
	stubGh(t, "gho_hubtoken00000000000000000000000000")
	tok, origin, _ = loginSource(s)
	if string(tok) != "gho_hubtoken00000000000000000000000000" || origin != loginSourceHubLogin {
		t.Fatalf("gh: %q %s", tok, origin)
	}
	if n := gh.count(); n != 2 {
		t.Fatalf("GitHub calls for the hub login: %d %v", n, gh.calls)
	}
	loginSource(s)
	loginSource(s)
	if n := gh.count(); n != 2 {
		t.Fatalf("cache missed: %d calls", n)
	}
	clock = clock.Add(11 * time.Minute)
	loginSource(s)
	if n := gh.count(); n != 4 {
		t.Fatalf("cache never expired: %d calls", n)
	}

	// A hub login that reaches a repository is not a source.
	gh.repos = 1
	stubGh(t, "gho_reaches000000000000000000000000000000")
	if tok, origin, _ := loginSource(s); tok != nil || origin != loginSourceNone {
		t.Fatalf("repo-reaching gh token accepted: %q %s", tok, origin)
	}

	// The file wins over gh and is trusted without a network call.
	before := gh.count()
	writeExecutorToken(t, s)
	tok, origin, _ = loginSource(s)
	if string(tok) != testExecutorToken || origin != loginSourceToken || gh.count() != before {
		t.Fatalf("file: %q %s (%d calls)", tok, origin, gh.count()-before)
	}
	// A damaged file falls back to gh (which here is refused): none.
	os.WriteFile(executorTokenPath(s), []byte("not a token\n"), 0o600)
	if tok, origin, _ := loginSource(s); tok != nil || origin != loginSourceNone {
		t.Fatalf("damaged file: %q %s", tok, origin)
	}
}
