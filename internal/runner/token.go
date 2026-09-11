package runner

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The executor token and the login source (ADR-034 rules 1 and 2). The owner places one fine-grained token
// on the hub with `pulse executor token --with-token`; it is accepted only when GitHub validates it and it
// reaches no repository, and it lives in <store home>/private/executor-token (0700/0600), never in the
// store. loginSource hands that token - or the hub's own `gh auth token` when it too reaches no repository -
// to the daemon for placing a login on an executor. Neither token is ever written to an event, a log line
// or a post.

const (
	executorTokenFile   = "executor-token"
	ghSourceCacheMins   = 10
	loginSourceToken    = "executor token"
	loginSourceHubLogin = "hub login"
	loginSourceProvider = "provider login"
	loginSourceNone     = "none"
)

// executorTokenPath is <store home>/private/executor-token; the store home is the directory of pulse.sqlite.
func executorTokenPath(s *store.Store) string {
	return filepath.Join(s.Home, "private", executorTokenFile)
}

// ghAuthToken asks the hub's gh for its token; tests stub it. "" with no error means gh has no login.
var ghAuthToken = func() (string, error) {
	out, err := exec.Command("gh", "auth", "token").Output()
	if err != nil {
		if errors.Is(err, exec.ErrNotFound) {
			return "", nil
		}
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return "", nil
		}
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

// checkExecutorToken is the whole acceptance rule: classification, GET /user, then zero repositories.
func checkExecutorToken(token string) (tokenFacts, error) {
	if _, err := classifyToken(token); err != nil {
		return tokenFacts{}, err
	}
	facts := githubTokenFacts(token, "")
	if facts.OK == nil {
		return facts, fail(1, "%s", facts.Note)
	}
	if !*facts.OK {
		return facts, fail(1, "%s", facts.Note)
	}
	if ok, note := githubNoRepos(token, ""); !ok {
		return facts, fail(1, "%s", note)
	}
	return facts, nil
}

// executorTokenSet is `pulse executor token --with-token`: stdin only, checked, then written 0600.
func executorTokenSet(s *store.Store, now string, token io.Reader, isTTY bool, actor string) ([]string, error) {
	if isTTY {
		return nil, fail(2, "the token must be piped, not typed")
	}
	raw, _ := io.ReadAll(token)
	tok := strings.TrimSpace(string(raw))
	facts, err := checkExecutorToken(tok)
	if err != nil {
		return nil, err
	}
	path := executorTokenPath(s)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	_ = os.Chmod(filepath.Dir(path), 0o700)
	if err := os.WriteFile(path, append([]byte(tok), '\n'), 0o600); err != nil {
		return nil, err
	}
	_ = os.Chmod(path, 0o600)
	ghSource.forget()
	lines := []string{facts.Note, "the token reaches no repository", "executor token stored in " + path}
	if warn := expiryWarning(facts.Expires, now, loginExpiryWarn); warn != "" {
		lines = append(lines, "warning: "+warn)
	}
	eventAt(s.DB, now, actor, "executor.token", "hub", map[string]any{"ok": true, "login": facts.User, "expires": facts.Expires})
	if proj := findingProject(s); proj != "" {
		summary := "executor token placed on the hub"
		if facts.User != "" {
			summary += " for " + facts.User
		}
		_, _ = createPost(s, now, postOpts{Kind: "finding", Project: proj, Area: "runner", Thread: "executors", Summary: summary,
			Body: fmt.Sprintf("The owner placed a fine-grained GitHub token that reaches no repository (account %s%s). The daemon "+
				"places it on a drained executor whose login was refused and on every machine compute provisions, so they work "+
				"without an owner step (ADR-034). `pulse executor token --remove` withdraws it.\n", orDash(facts.User), expiresText(facts.Expires)),
			Agent: actorAgent(s, actor), Date: now})
	}
	return lines, nil
}

// executorTokenRemove is `pulse executor token --remove`.
func executorTokenRemove(s *store.Store, now, actor string) ([]string, error) {
	path := executorTokenPath(s)
	err := os.Remove(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	ghSource.forget()
	if errors.Is(err, os.ErrNotExist) {
		return []string{"no executor token on file"}, nil
	}
	eventAt(s.DB, now, actor, "executor.token", "hub", map[string]any{"ok": false, "removed": true})
	return []string{"executor token removed from " + path}, nil
}

// executorTokenReport is `pulse executor token`: presence, the account it was accepted for and its expiry,
// read from the event that recorded the acceptance - never the token itself.
func executorTokenReport(s *store.Store, now string) []string {
	path := executorTokenPath(s)
	if _, err := os.Stat(path); err != nil {
		return []string{"no executor token on file; place one with `pulse executor token --with-token` (stdin) - a fine-grained token with only the Copilot Requests permission"}
	}
	lines := []string{"executor token on file at " + path}
	r := mustOne(s.DB, "SELECT detail FROM events WHERE action = 'executor.token' AND subject = 'hub' ORDER BY seq DESC LIMIT 1")
	if r != nil {
		d := jsonMap(r.str("detail"))
		login, _ := d["login"].(string)
		expires, _ := d["expires"].(string)
		if login != "" {
			lines = append(lines, "account "+login)
		}
		if expires != "" {
			lines = append(lines, "expires "+expires)
		}
		if warn := expiryWarning(expires, now, loginExpiryWarn); warn != "" {
			lines = append(lines, "WARNING "+warn)
		}
	}
	return lines
}

func orDash(s string) string {
	if s == "" {
		return "-"
	}
	return s
}

func expiresText(expires string) string {
	if expires == "" {
		return ""
	}
	return ", expires " + expires
}

// findingProject is where the daemon's own findings go: the brain, else the first active project.
func findingProject(s *store.Store) string {
	if b := brainProject(s.DB); b != nil {
		return b.str("slug")
	}
	if r := mustOne(s.DB, "SELECT slug FROM projects WHERE status = 'active' ORDER BY slug LIMIT 1"); r != nil {
		return r.str("slug")
	}
	return ""
}

// ghSourceCache remembers, for ghSourceCacheMins, what gh answered and whether that token passed the
// checks, so neither gh nor GitHub is asked on every tick. The token lives in memory only, nowhere else.
type ghSourceCache struct {
	mu    sync.Mutex
	token string
	ok    bool
	at    time.Time
	// what gh last answered and when, so a hub without a login is not asked on every request either
	fetched   string
	fetchedAt time.Time
}

var (
	ghSource       ghSourceCache
	providerSource ghSourceCache
)

func (c *ghSourceCache) forget() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.token, c.ok, c.at = "", false, time.Time{}
	c.fetched, c.fetchedAt = "", time.Time{}
}

// fetch runs ghAuthToken at most once per ghSourceCacheMins.
func (c *ghSourceCache) fetch() (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := wallClock()
	if !c.fetchedAt.IsZero() && now.Sub(c.fetchedAt) < ghSourceCacheMins*time.Minute {
		return c.fetched, nil
	}
	tok, err := ghAuthToken()
	if err != nil {
		return "", err
	}
	c.fetched, c.fetchedAt = tok, now
	return tok, nil
}

// passes says whether tok is an acceptable source, asking GitHub only when the cache is cold or stale.
func (c *ghSourceCache) passes(tok string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := wallClock()
	if c.token == tok && !c.at.IsZero() && now.Sub(c.at) < ghSourceCacheMins*time.Minute {
		return c.ok
	}
	_, err := checkExecutorToken(tok)
	c.token, c.ok, c.at = tok, err == nil, now
	return c.ok
}

// loginSource (ADR-034 rule 2): the executor token on file, else (native) the stored provider login or
// (legacy) the hub's `gh auth token` when the source reaches no repository, else none. The file is trusted
// after classifyToken alone (it was checked when placed); borrowed sources are checked against GitHub and
// cached per process for ghSourceCacheMins.
func loginSource(s *store.Store) ([]byte, string, error) {
	data, err := os.ReadFile(executorTokenPath(s))
	if err == nil {
		tok := strings.TrimSpace(string(data))
		if _, cerr := classifyToken(tok); cerr == nil {
			return []byte(tok), loginSourceToken, nil
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, loginSourceNone, err
	}
	if s.HarnessNative() {
		p, err := localProvider(s.Home, s.Setting("provider", provider.Default))
		if err != nil {
			return nil, loginSourceNone, err
		}
		exporter, ok := p.(provider.LoginExporter)
		if !ok {
			return nil, loginSourceNone, nil
		}
		tok, _, err := exporter.ExportLogin()
		if err != nil {
			if isNoStoredLogin(err) {
				return nil, loginSourceNone, nil
			}
			return nil, loginSourceNone, err
		}
		tok = strings.TrimSpace(tok)
		if tok != "" && providerSource.passes(tok) {
			return []byte(tok), loginSourceProvider, nil
		}
		return nil, loginSourceNone, nil
	}
	tok, err := ghSource.fetch()
	if err != nil {
		return nil, loginSourceNone, err
	}
	if tok != "" && ghSource.passes(tok) {
		return []byte(tok), loginSourceHubLogin, nil
	}
	return nil, loginSourceNone, nil
}

func isNoStoredLogin(err error) bool {
	var perr *provider.Error
	return errors.As(err, &perr) && perr.Code == "no_stored_login"
}

// hasLoginSource is the yes/no the compute gate asks (ADR-034 rule 6).
func hasLoginSource(s *store.Store) bool {
	tok, _, _ := loginSource(s)
	return len(tok) > 0
}

// tokenReader wraps a token for executorLogin, which reads it from a stream.
func tokenReader(tok []byte) io.Reader { return bytes.NewReader(tok) }
