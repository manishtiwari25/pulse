package runner

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

// loginGateTimeout bounds the provider status check the daemon makes each tick.
const loginGateTimeout = 20 * time.Second

// loginGate is localLoginGate as the daemon calls it; tests swap it (TestMain).
var loginGate = localLoginGate

// localLoginGate is why this machine starts no worker right now, or "" when it may.
//
// With the native harness and no provider login here, every launch would die
// at once with a login fault (run.go) and the job would queue again, tick
// after tick. Instead the local slots stay closed, the daemon says so as its
// idle reason, and the console's Harness card carries the same words. Remote
// executors keep their own login checks (ADR-034). A check that fails for
// another reason (network, provider bug) does not close the machine: the run
// itself reports that.
func localLoginGate(s *store.Store) string {
	if !s.HarnessNative() {
		return ""
	}
	p, err := localProvider(s.Home, s.Setting("provider", provider.Default))
	if err != nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), loginGateTimeout)
	defer cancel()
	login, err := p.Status(ctx)
	if err != nil || login.LoggedIn {
		return ""
	}
	detail := strings.TrimSpace(login.Detail)
	if detail == "" {
		detail = "sign in with GitHub in the console (Settings -> Harness) or run `pulse harness login`"
	}
	return fmt.Sprintf("workers on this machine wait for a %s login: %s", p.Label(), detail)
}

var (
	localProviderMu sync.Mutex
	localProviders  = map[string]provider.Provider{}
)

// localProvider keeps one provider per home and name so its status caches
// (a refused token, a looked-up user) survive from tick to tick.
func localProvider(home, name string) (provider.Provider, error) {
	key := home + "\x00" + name
	localProviderMu.Lock()
	defer localProviderMu.Unlock()
	if p, ok := localProviders[key]; ok {
		return p, nil
	}
	p, err := provider.Open(name, provider.Options{Home: home})
	if err != nil {
		return nil, err
	}
	localProviders[key] = p
	return p, nil
}

// loginWanted: once a day, while jobs wait behind the login gate, one finding on the brain's board
// (ADR-034: a measurement, never a question).
func loginWanted(s *store.Store, brain row, queued int, gate, now, actor string) row {
	if brain == nil || queued <= 0 || gate == "" {
		return nil
	}
	if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = 'login-wanted' AND date LIKE ?",
		brain.str("slug"), now[:10]+"%") != nil {
		return nil
	}
	summary := trunc(fmt.Sprintf("sign-in wanted: %d job(s) wait; %s", queued, gate), 230)
	body := fmt.Sprintf("The daemon on this machine holds %d queued job(s) because the built-in harness has no provider login "+
		"here: %s. Nothing runs on this machine until someone signs in - Settings -> Harness -> Sign in with GitHub in the "+
		"console, or `pulse harness login` in a terminal; `pulse settings set harness copilot-cli` keeps the legacy copilot "+
		"CLI path meanwhile. Remote executors keep working. This post is a measurement, not a request to act (%s).\n",
		queued, gate, now)
	post, err := createPost(s, now, postOpts{Kind: "finding", Project: brain.str("slug"), Area: "runner", Thread: "login-wanted",
		Summary: summary, Body: body, Agent: actorAgent(s, actor), Date: now})
	if err != nil {
		return nil
	}
	eventAt(s.DB, now, actor, "capacity.login-wanted", brain.str("slug"), map[string]any{"queued": queued, "post": post.str("id")})
	return post
}
