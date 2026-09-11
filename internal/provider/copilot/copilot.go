// Package copilot implements the GitHub Copilot model provider for the native harness.
package copilot

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const (
	name  = "copilot"
	label = "GitHub Copilot"

	// clientID is the well-known GitHub Copilot OAuth app used by Copilot editor plugins.
	clientID = "Iv1.b507a08c87ecfe98"

	editorVersion       = "vscode/1.99.0"
	editorPluginVersion = "copilot-chat/0.26.7"
	userAgent           = "GitHubCopilotChat/0.26.7"
	integrationID       = "vscode-chat"
	intentConversation  = "conversation-panel"

	statusCacheTTL = 10 * time.Minute
	modelsCacheTTL = 10 * time.Minute
	tokenSkew      = 60 * time.Second
)

var (
	githubBase = "https://api.github.com"
	oauthBase  = "https://github.com"
	apiBase    = "https://api.githubcopilot.com"

	tokenPatterns = []*regexp.Regexp{
		regexp.MustCompile(`gh[opusr]_[A-Za-z0-9]{20,}`),
		regexp.MustCompile(`github_pat_[A-Za-z0-9_]{20,}`),
		regexp.MustCompile(`Bearer [A-Za-z0-9._-]{16,}`),
		regexp.MustCompile(`tid=[A-Za-z0-9]+(?:;[A-Za-z0-9_=:.+-]+)*`),
	}
	legacyTokenPattern = regexp.MustCompile(`^[A-Fa-f0-9]{40}$`)
	secretsMu          sync.Mutex
	knownSecrets       []string
)

func init() {
	provider.Register(name, label, New)
}

// New opens the Copilot provider using only local defaults.
func New(o provider.Options) (provider.Provider, error) {
	if o.HTTP == nil {
		o.HTTP = &http.Client{Timeout: 120 * time.Second}
	}
	if o.Env == nil {
		o.Env = os.Getenv
	}
	if o.Home == "" {
		if h := o.Env("PULSE_HOME"); strings.TrimSpace(h) != "" {
			o.Home = strings.TrimSpace(h)
		} else if h, err := os.UserHomeDir(); err == nil {
			o.Home = filepath.Join(h, ".pulse")
		} else {
			return nil, err
		}
	}
	if o.Log == nil {
		o.Log = func(string, ...any) {}
	}
	return &copilot{
		home: o.Home,
		http: o.HTTP,
		env:  o.Env,
		log:  o.Log,
	}, nil
}

type copilot struct {
	home string
	http *http.Client
	env  func(string) string
	log  func(string, ...any)

	mu             sync.Mutex
	githubToken    string
	copilotToken   string
	copilotBase    string
	copilotExpires time.Time
	copilotPlan    string
	userToken      string
	userName       string
	userAt         time.Time
	models         []provider.Model
	modelsAt       time.Time
	loggedOut      bool
	// deniedToken remembers a GitHub token Copilot refused, so the console's
	// status polling does not ask GitHub again for statusCacheTTL.
	deniedToken string
	deniedMsg   string
	deniedAt    time.Time
}

type storedLogin struct {
	GitHubToken string `json:"github_token"`
	User        string `json:"user"`
	Source      string `json:"source"`
	SavedAt     string `json:"saved_at"`
}

type tokenSource struct {
	token  string
	source string
	user   string
}

func (p *copilot) Name() string  { return name }
func (p *copilot) Label() string { return label }

func (p *copilot) BeginDeviceLogin(ctx context.Context) (provider.DeviceFlow, error) {
	form := "client_id=" + clientID + "&scope=read:user"
	var out struct {
		DeviceCode       string `json:"device_code"`
		UserCode         string `json:"user_code"`
		VerificationURI  string `json:"verification_uri"`
		Interval         int    `json:"interval"`
		ExpiresIn        int    `json:"expires_in"`
		Error            string `json:"error"`
		ErrorDescription string `json:"error_description"`
	}
	err := p.doJSON(ctx, http.MethodPost, oauthBase+"/login/device/code", strings.NewReader(form), map[string]string{
		"Accept":       "application/json",
		"Content-Type": "application/x-www-form-urlencoded",
	}, &out)
	if err != nil {
		return provider.DeviceFlow{}, err
	}
	if out.Error != "" {
		return provider.DeviceFlow{}, providerError(0, out.Error, out.ErrorDescription, false)
	}
	return provider.DeviceFlow{
		DeviceCode:      out.DeviceCode,
		UserCode:        out.UserCode,
		VerificationURI: out.VerificationURI,
		Interval:        out.Interval,
		ExpiresIn:       out.ExpiresIn,
	}, nil
}

func (p *copilot) PollDeviceLogin(ctx context.Context, deviceCode string) (string, provider.Login, error) {
	form := "client_id=" + clientID + "&device_code=" + deviceCode + "&grant_type=urn:ietf:params:oauth:grant-type:device_code"
	var out struct {
		AccessToken      string `json:"access_token"`
		Error            string `json:"error"`
		ErrorDescription string `json:"error_description"`
	}
	err := p.doJSON(ctx, http.MethodPost, oauthBase+"/login/oauth/access_token", strings.NewReader(form), map[string]string{
		"Accept":       "application/json",
		"Content-Type": "application/x-www-form-urlencoded",
	}, &out)
	if err != nil {
		return "", provider.Login{}, err
	}
	switch out.Error {
	case "":
	case "authorization_pending":
		return provider.DevicePending, provider.Login{}, nil
	case "slow_down":
		return provider.DeviceSlowDown, provider.Login{}, nil
	case "expired_token":
		return provider.DeviceExpired, provider.Login{}, nil
	case "access_denied":
		return provider.DeviceDenied, provider.Login{}, nil
	default:
		return "", provider.Login{}, providerError(0, out.Error, out.ErrorDescription, false)
	}
	if strings.TrimSpace(out.AccessToken) == "" {
		return "", provider.Login{}, providerError(0, "device_login", "GitHub did not return an access token", false)
	}
	user, err := p.githubUser(ctx, out.AccessToken)
	if err != nil {
		return "", provider.Login{}, err
	}
	if _, _, err := p.sessionToken(ctx, out.AccessToken, true); err != nil {
		return "", provider.Login{}, err
	}
	if err := p.writeStored(storedLogin{
		GitHubToken: out.AccessToken,
		User:        user,
		Source:      "device login",
		SavedAt:     time.Now().UTC().Format(time.RFC3339),
	}); err != nil {
		return "", provider.Login{}, err
	}
	return provider.DeviceComplete, provider.Login{LoggedIn: true, User: user, Source: "device login"}, nil
}

func (p *copilot) LoginWithToken(ctx context.Context, token string) (provider.Login, error) {
	token = strings.TrimSpace(token)
	if !looksLikeGitHubToken(token) {
		return provider.Login{}, providerError(0, "invalid_token", "GitHub token is not a supported OAuth, PAT, or legacy token shape", false)
	}
	user, err := p.githubUser(ctx, token)
	if err != nil {
		return provider.Login{}, err
	}
	if _, _, err := p.sessionToken(ctx, token, true); err != nil {
		return provider.Login{}, err
	}
	if err := p.writeStored(storedLogin{
		GitHubToken: token,
		User:        user,
		Source:      "token login",
		SavedAt:     time.Now().UTC().Format(time.RFC3339),
	}); err != nil {
		return provider.Login{}, err
	}
	p.mu.Lock()
	p.loggedOut = false
	p.mu.Unlock()
	return provider.Login{LoggedIn: true, User: user, Source: "token login"}, nil
}

func (p *copilot) Logout() error {
	err := os.Remove(p.loginPath())
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	p.mu.Lock()
	p.githubToken, p.copilotToken, p.copilotBase, p.copilotPlan = "", "", "", ""
	p.copilotExpires = time.Time{}
	p.deniedToken, p.deniedMsg, p.deniedAt = "", "", time.Time{}
	p.models = nil
	p.modelsAt = time.Time{}
	p.loggedOut = true
	p.mu.Unlock()
	return nil
}

func (p *copilot) Status(ctx context.Context) (provider.Login, error) {
	if s, ok, err := p.readStored(); err != nil {
		return provider.Login{}, err
	} else if ok {
		return p.storedStatus(s), nil
	}
	src, err := p.sourceNoFile()
	if err != nil {
		return provider.Login{}, err
	}
	if src.token == "" {
		return provider.Login{LoggedIn: false, Detail: "Sign in with GitHub (Settings -> Harness) or run pulse harness login"}, nil
	}
	user, err := p.cachedUser(ctx, src.token)
	if err != nil {
		// A borrowed token GitHub no longer accepts is "not signed in", not a fault.
		var perr *provider.Error
		if errors.As(err, &perr) && perr.Status == http.StatusUnauthorized {
			return provider.Login{LoggedIn: false, Source: src.source, Detail: fmt.Sprintf("the %s token on this machine is stale (GitHub rejected it): sign in with GitHub (Settings -> Harness) or run pulse harness login", src.source)}, nil
		}
		return provider.Login{}, err
	}
	login := provider.Login{LoggedIn: true, User: user, Source: src.source}
	// A borrowed token (environment, gh, copilot CLI) proves a GitHub identity,
	// not a Copilot seat; the stored login was checked when it was saved.
	plan, expires, err := p.copilotAccess(ctx, src.token)
	switch {
	case err == nil:
		login.Plan = plan
		if !expires.IsZero() {
			login.ExpiresAt = expires.UTC().Format(time.RFC3339)
		}
	case errors.Is(err, provider.ErrNotLoggedIn):
		login.LoggedIn = false
		reason := strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(err.Error(), provider.ErrNotLoggedIn.Error()), ":"))
		if reason == "" {
			reason = "Copilot refused the token"
		}
		login.Detail = fmt.Sprintf("%s (via %s): %s - sign in with GitHub (Settings -> Harness) or run pulse harness login", user, src.source, reason)
		return login, nil
	default:
		login.Detail = "Copilot access check failed: " + squeeze(redact(err.Error()), 200)
	}
	p.mu.Lock()
	loggedOut := p.loggedOut
	p.mu.Unlock()
	if loggedOut && src.source == "environment" {
		login.Detail = "Stored Copilot login removed; still using the environment token."
	}
	return login, nil
}

func (p *copilot) ExportLogin() (string, provider.Login, error) {
	s, ok, err := p.readStored()
	if err != nil {
		return "", provider.Login{}, err
	}
	if !ok || strings.TrimSpace(s.GitHubToken) == "" {
		return "", provider.Login{}, providerError(0, "no_stored_login", "no stored GitHub Copilot login; sign in with GitHub in Settings -> Harness or run pulse harness login", false)
	}
	token := strings.TrimSpace(s.GitHubToken)
	rememberSecret(token)
	return token, provider.Login{LoggedIn: true, User: s.User, Source: "stored"}, nil
}

// copilotAccess exchanges the GitHub token for a Copilot session token (cached
// until it nears expiry) and reports the plan; a refusal is remembered for
// statusCacheTTL so status polling stays quiet.
func (p *copilot) copilotAccess(ctx context.Context, githubToken string) (string, time.Time, error) {
	p.mu.Lock()
	if p.deniedToken == githubToken && time.Since(p.deniedAt) < statusCacheTTL {
		msg := p.deniedMsg
		p.mu.Unlock()
		return "", time.Time{}, fmt.Errorf("%w: %s", provider.ErrNotLoggedIn, msg)
	}
	p.mu.Unlock()
	if _, _, err := p.sessionToken(ctx, githubToken, false); err != nil {
		return "", time.Time{}, err
	}
	p.mu.Lock()
	plan, expires := p.copilotPlan, p.copilotExpires
	p.mu.Unlock()
	return plan, expires, nil
}

// copilotAccessMessage explains a refusal from the Copilot token service.
func copilotAccessMessage(status int) string {
	switch status {
	case http.StatusUnauthorized:
		return "GitHub rejected the token (expired or revoked): sign in again"
	case http.StatusNotFound:
		return "this GitHub account has no Copilot access (no seat or plan): sign in with a GitHub account that has Copilot"
	default:
		return "this token is not Copilot-enabled (tokens from gh auth login and ordinary personal access tokens never are): sign in with GitHub so PULSE holds a Copilot-enabled login"
	}
}

func (p *copilot) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
	src, err := p.tokenSource()
	if err != nil {
		return provider.Response{}, err
	}
	if src.token == "" {
		return provider.Response{}, provider.ErrNotLoggedIn
	}
	body, err := json.Marshal(openAIRequest(req))
	if err != nil {
		return provider.Response{}, err
	}
	refreshedFor401 := false
	backoffs := []time.Duration{time.Second, 3 * time.Second, 9 * time.Second}
	for attempt := 0; attempt < len(backoffs); attempt++ {
		token, base, err := p.sessionToken(ctx, src.token, false)
		if err != nil {
			return provider.Response{}, err
		}
		resp, err := p.chatOnce(ctx, base, token, body, req)
		if err == nil {
			return resp, nil
		}
		var perr *provider.Error
		if errors.As(err, &perr) && perr.Status == http.StatusUnauthorized && !refreshedFor401 {
			refreshedFor401 = true
			p.logRedacted("copilot token was refused; refreshing once")
			if _, _, ferr := p.sessionToken(ctx, src.token, true); ferr != nil {
				return provider.Response{}, ferr
			}
			attempt--
			continue
		}
		if !isRetryable(err) || attempt == len(backoffs)-1 {
			return provider.Response{}, err
		}
		wait := retryDelay(err, backoffs[attempt])
		p.logRedacted("copilot chat retry in %s after %s", wait, err.Error())
		if !sleep(ctx, wait) {
			return provider.Response{}, ctx.Err()
		}
	}
	return provider.Response{}, providerError(0, "retry_exhausted", "Copilot chat retry loop exhausted", true)
}

func (p *copilot) Models(ctx context.Context) ([]provider.Model, error) {
	p.mu.Lock()
	if len(p.models) > 0 && time.Since(p.modelsAt) < modelsCacheTTL {
		out := append([]provider.Model(nil), p.models...)
		p.mu.Unlock()
		return out, nil
	}
	p.mu.Unlock()
	src, err := p.tokenSource()
	if err != nil {
		return nil, err
	}
	if src.token == "" {
		return nil, provider.ErrNotLoggedIn
	}
	token, base, err := p.sessionToken(ctx, src.token, false)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(base, "/")+"/models", nil)
	if err != nil {
		return nil, err
	}
	p.copilotHeaders(req, token)
	res, err := p.http.Do(req)
	if err != nil {
		return nil, providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return nil, httpProviderError(res, true)
	}
	var body modelsResponse
	if err := json.NewDecoder(res.Body).Decode(&body); err != nil {
		return nil, providerError(0, "decode", err.Error(), false)
	}
	models := mapModels(body)
	p.mu.Lock()
	p.models = append([]provider.Model(nil), models...)
	p.modelsAt = time.Now()
	p.mu.Unlock()
	return models, nil
}

func (p *copilot) chatOnce(ctx context.Context, base, token string, body []byte, req provider.Request) (provider.Response, error) {
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, strings.TrimRight(base, "/")+"/chat/completions", bytes.NewReader(body))
	if err != nil {
		return provider.Response{}, err
	}
	p.copilotHeaders(httpReq, token)
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Copilot-Integration-Id", integrationID)
	httpReq.Header.Set("Openai-Intent", intentConversation)
	httpReq.Header.Set("X-Request-Id", requestID())
	httpReq.Header.Set("X-Initiator", initiator(req))
	res, err := p.http.Do(httpReq)
	if err != nil {
		return provider.Response{}, providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return provider.Response{}, httpProviderError(res, false)
	}
	var out chatResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return provider.Response{}, providerError(0, "decode", err.Error(), false)
	}
	return out.providerResponse(), nil
}

func (p *copilot) sessionToken(ctx context.Context, githubToken string, force bool) (string, string, error) {
	rememberSecret(githubToken)
	p.mu.Lock()
	if !force && p.githubToken == githubToken && p.copilotToken != "" && time.Now().Before(p.copilotExpires.Add(-tokenSkew)) {
		tok, base := p.copilotToken, p.copilotBase
		if base == "" {
			base = apiBase
		}
		p.mu.Unlock()
		return tok, base, nil
	}
	p.mu.Unlock()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(githubBase, "/")+"/copilot_internal/v2/token", nil)
	if err != nil {
		return "", "", err
	}
	req.Header.Set("Authorization", "token "+githubToken)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Editor-Version", editorVersion)
	req.Header.Set("Editor-Plugin-Version", editorPluginVersion)
	req.Header.Set("User-Agent", userAgent)
	res, err := p.http.Do(req)
	if err != nil {
		return "", "", providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	// GitHub answers 404 here for a valid token whose account has no Copilot seat.
	if res.StatusCode == http.StatusUnauthorized || res.StatusCode == http.StatusForbidden || res.StatusCode == http.StatusNotFound {
		msg := copilotAccessMessage(res.StatusCode)
		p.mu.Lock()
		p.deniedToken, p.deniedMsg, p.deniedAt = githubToken, msg, time.Now()
		p.mu.Unlock()
		return "", "", fmt.Errorf("%w: %s", provider.ErrNotLoggedIn, msg)
	}
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return "", "", httpProviderError(res, true)
	}
	var out struct {
		Token     string            `json:"token"`
		ExpiresAt int64             `json:"expires_at"`
		RefreshIn int               `json:"refresh_in"`
		Endpoints map[string]string `json:"endpoints"`
		SKU       string            `json:"sku"`
	}
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", "", providerError(0, "decode", err.Error(), false)
	}
	base := apiBase
	if out.Endpoints != nil && out.Endpoints["api"] != "" {
		base = out.Endpoints["api"]
	}
	expires := time.Unix(out.ExpiresAt, 0)
	if out.ExpiresAt == 0 && out.RefreshIn > 0 {
		expires = time.Now().Add(time.Duration(out.RefreshIn) * time.Second)
	}
	p.mu.Lock()
	p.githubToken = githubToken
	p.copilotToken = out.Token
	p.copilotBase = base
	p.copilotExpires = expires
	p.copilotPlan = out.SKU
	if p.deniedToken == githubToken {
		p.deniedToken, p.deniedMsg, p.deniedAt = "", "", time.Time{}
	}
	p.mu.Unlock()
	rememberSecret(out.Token)
	p.logRedacted("copilot session token refreshed")
	return out.Token, base, nil
}

func (p *copilot) tokenSource() (tokenSource, error) {
	if s, ok, err := p.readStored(); err != nil {
		return tokenSource{}, err
	} else if ok && strings.TrimSpace(s.GitHubToken) != "" {
		rememberSecret(strings.TrimSpace(s.GitHubToken))
		return tokenSource{token: strings.TrimSpace(s.GitHubToken), source: s.Source, user: s.User}, nil
	}
	return p.sourceNoFile()
}

func (p *copilot) sourceNoFile() (tokenSource, error) {
	for _, key := range []string{"COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"} {
		if tok := strings.TrimSpace(p.env(key)); tok != "" {
			rememberSecret(tok)
			return tokenSource{token: tok, source: "environment"}, nil
		}
	}
	// Tokens from `gh auth login` are deliberately not borrowed: the gh OAuth app
	// is not Copilot-enabled, so GitHub refuses them at the Copilot token exchange.
	if src := p.editorLogin(); src.token != "" {
		return src, nil
	}
	if src := p.copilotCLIConfig(); src.token != "" {
		return src, nil
	}
	return tokenSource{}, nil
}

// editorLogin borrows the login the Copilot editor plugins (VS Code, JetBrains,
// Neovim) keep in $XDG_CONFIG_HOME/github-copilot: apps.json, or the older
// hosts.json. Those OAuth apps are Copilot-enabled, so their tokens pass the
// Copilot token exchange while they last.
func (p *copilot) editorLogin() tokenSource {
	dir := strings.TrimSpace(p.env("XDG_CONFIG_HOME"))
	if dir == "" {
		h, err := os.UserHomeDir()
		if err != nil {
			return tokenSource{}
		}
		dir = filepath.Join(h, ".config")
	}
	for _, file := range []string{"apps.json", "hosts.json"} {
		data, err := os.ReadFile(filepath.Join(dir, "github-copilot", file))
		if err != nil {
			continue
		}
		var hosts map[string]struct {
			User  string `json:"user"`
			Token string `json:"oauth_token"`
		}
		if json.Unmarshal(data, &hosts) != nil {
			continue
		}
		keys := make([]string, 0, len(hosts))
		for k := range hosts {
			if k == "github.com" || strings.HasPrefix(k, "github.com:") {
				keys = append(keys, k)
			}
		}
		sort.Strings(keys)
		for _, k := range keys {
			tok := strings.TrimSpace(hosts[k].Token)
			if looksLikeGitHubToken(tok) {
				rememberSecret(tok)
				return tokenSource{token: tok, source: "editor login", user: hosts[k].User}
			}
		}
	}
	return tokenSource{}
}

func (p *copilot) cachedUser(ctx context.Context, token string) (string, error) {
	p.mu.Lock()
	if p.userToken == token && p.userName != "" && time.Since(p.userAt) < statusCacheTTL {
		user := p.userName
		p.mu.Unlock()
		return user, nil
	}
	p.mu.Unlock()
	user, err := p.githubUser(ctx, token)
	if err != nil {
		return "", err
	}
	p.mu.Lock()
	p.userToken, p.userName, p.userAt = token, user, time.Now()
	p.mu.Unlock()
	return user, nil
}

func (p *copilot) githubUser(ctx context.Context, token string) (string, error) {
	rememberSecret(token)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(githubBase, "/")+"/user", nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "token "+token)
	req.Header.Set("Accept", "application/json")
	res, err := p.http.Do(req)
	if err != nil {
		return "", providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return "", httpProviderError(res, false)
	}
	var out struct {
		Login string `json:"login"`
	}
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return "", providerError(0, "decode", err.Error(), false)
	}
	return out.Login, nil
}

func (p *copilot) storedStatus(s storedLogin) provider.Login {
	p.mu.Lock()
	expires, plan := p.copilotExpires, p.copilotPlan
	p.mu.Unlock()
	login := provider.Login{LoggedIn: true, User: s.User, Source: s.Source, Plan: plan}
	if !expires.IsZero() {
		login.ExpiresAt = expires.UTC().Format(time.RFC3339)
	}
	return login
}

func (p *copilot) loginPath() string {
	return filepath.Join(p.home, "private", "providers", "copilot.json")
}

func (p *copilot) readStored() (storedLogin, bool, error) {
	data, err := os.ReadFile(p.loginPath())
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return storedLogin{}, false, nil
		}
		return storedLogin{}, false, err
	}
	var s storedLogin
	if err := json.Unmarshal(data, &s); err != nil {
		return storedLogin{}, false, err
	}
	return s, true, nil
}

func (p *copilot) writeStored(s storedLogin) error {
	rememberSecret(s.GitHubToken)
	dir := filepath.Dir(p.loginPath())
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	_ = os.Chmod(dir, 0o700)
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(p.loginPath(), append(data, '\n'), 0o600); err != nil {
		return err
	}
	return os.Chmod(p.loginPath(), 0o600)
}

func (p *copilot) copilotCLIConfig() tokenSource {
	home := p.env("COPILOT_HOME")
	path := ""
	if strings.TrimSpace(home) != "" {
		path = filepath.Join(strings.TrimSpace(home), "config.json")
	} else if h, err := os.UserHomeDir(); err == nil {
		path = filepath.Join(h, ".copilot", "config.json")
	}
	if path == "" {
		return tokenSource{}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return tokenSource{}
	}
	var v any
	if json.Unmarshal(data, &v) != nil {
		return tokenSource{}
	}
	if tok := findOAuthToken(v); tok != "" {
		return tokenSource{token: tok, source: "copilot cli"}
	}
	return tokenSource{}
}

func (p *copilot) copilotHeaders(req *http.Request, token string) {
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Editor-Version", editorVersion)
	req.Header.Set("Editor-Plugin-Version", editorPluginVersion)
	req.Header.Set("User-Agent", userAgent)
}

func (p *copilot) doJSON(ctx context.Context, method, url string, body io.Reader, headers map[string]string, out any) error {
	req, err := http.NewRequestWithContext(ctx, method, url, body)
	if err != nil {
		return err
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	res, err := p.http.Do(req)
	if err != nil {
		return providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return httpProviderError(res, false)
	}
	return json.NewDecoder(res.Body).Decode(out)
}

func (p *copilot) logRedacted(format string, a ...any) {
	p.log(redact(fmt.Sprintf(format, a...)))
}

type openAIMessage struct {
	Role       provider.Role    `json:"role"`
	Content    any              `json:"content,omitempty"`
	ToolCalls  []openAIToolCall `json:"tool_calls,omitempty"`
	ToolCallID string           `json:"tool_call_id,omitempty"`
	Name       string           `json:"name,omitempty"`
}

type openAIToolCall struct {
	ID       string         `json:"id"`
	Type     string         `json:"type"`
	Function openAIFunction `json:"function"`
}

type openAIFunction struct {
	Name      string          `json:"name,omitempty"`
	Arguments string          `json:"arguments,omitempty"`
	Desc      string          `json:"description,omitempty"`
	Params    json.RawMessage `json:"parameters,omitempty"`
}

type openAITool struct {
	Type     string         `json:"type"`
	Function openAIFunction `json:"function"`
}

type openAIChatRequest struct {
	Model       string          `json:"model"`
	Messages    []openAIMessage `json:"messages"`
	Tools       []openAITool    `json:"tools,omitempty"`
	ToolChoice  string          `json:"tool_choice,omitempty"`
	MaxTokens   int             `json:"max_tokens,omitempty"`
	Temperature *float64        `json:"temperature,omitempty"`
	Stream      bool            `json:"stream"`
}

func openAIRequest(req provider.Request) openAIChatRequest {
	out := openAIChatRequest{Model: req.Model, Stream: false, MaxTokens: req.MaxTokens, Temperature: req.Temperature}
	for _, m := range req.Messages {
		om := openAIMessage{Role: m.Role, Content: m.Content, ToolCallID: m.ToolCallID, Name: m.Name}
		if m.Content == "" {
			om.Content = nil
		}
		for _, tc := range m.ToolCalls {
			om.ToolCalls = append(om.ToolCalls, openAIToolCall{
				ID:   tc.ID,
				Type: "function",
				Function: openAIFunction{
					Name:      tc.Name,
					Arguments: tc.Arguments,
				},
			})
		}
		out.Messages = append(out.Messages, om)
	}
	for _, t := range req.Tools {
		out.Tools = append(out.Tools, openAITool{
			Type: "function",
			Function: openAIFunction{
				Name:   t.Name,
				Desc:   t.Description,
				Params: t.Parameters,
			},
		})
	}
	if len(out.Tools) > 0 {
		out.ToolChoice = "auto"
	}
	return out
}

type chatResponse struct {
	ID      string `json:"id"`
	Model   string `json:"model"`
	Choices []struct {
		FinishReason string        `json:"finish_reason"`
		Message      openAIMessage `json:"message"`
	} `json:"choices"`
	Usage struct {
		PromptTokens     int     `json:"prompt_tokens"`
		CompletionTokens int     `json:"completion_tokens"`
		Cost             float64 `json:"cost"`
	} `json:"usage"`
}

func (r chatResponse) providerResponse() provider.Response {
	var msg provider.Message
	finish := ""
	if len(r.Choices) > 0 {
		c := r.Choices[0]
		finish = c.FinishReason
		msg.Role = c.Message.Role
		if s, ok := c.Message.Content.(string); ok {
			msg.Content = s
		}
		msg.ToolCallID = c.Message.ToolCallID
		msg.Name = c.Message.Name
		for _, tc := range c.Message.ToolCalls {
			msg.ToolCalls = append(msg.ToolCalls, provider.ToolCall{
				ID:        tc.ID,
				Name:      tc.Function.Name,
				Arguments: tc.Function.Arguments,
			})
		}
	}
	return provider.Response{
		Message:      msg,
		FinishReason: finish,
		Usage: provider.Usage{
			PromptTokens:     r.Usage.PromptTokens,
			CompletionTokens: r.Usage.CompletionTokens,
			Cost:             r.Usage.Cost,
		},
		Model:     r.Model,
		RequestID: r.ID,
	}
}

type modelsResponse struct {
	Data []struct {
		ID                 string `json:"id"`
		Name               string `json:"name"`
		Vendor             string `json:"vendor"`
		Version            string `json:"version"`
		ModelPickerEnabled *bool  `json:"model_picker_enabled"`
		Preview            bool   `json:"preview"`
		IsChatDefault      bool   `json:"is_chat_default"`
		Capabilities       struct {
			Type     string `json:"type"`
			Family   string `json:"family"`
			Supports struct {
				ToolCalls bool `json:"tool_calls"`
			} `json:"supports"`
			Limits struct {
				MaxContextWindowTokens int `json:"max_context_window_tokens"`
				MaxOutputTokens        int `json:"max_output_tokens"`
			} `json:"limits"`
		} `json:"capabilities"`
		Billing struct {
			IsPremium  bool    `json:"is_premium"`
			Multiplier float64 `json:"multiplier"`
		} `json:"billing"`
	} `json:"data"`
}

func mapModels(body modelsResponse) []provider.Model {
	var out []provider.Model
	for _, m := range body.Data {
		if m.ModelPickerEnabled != nil && !*m.ModelPickerEnabled {
			continue
		}
		if m.Capabilities.Type != "chat" {
			continue
		}
		out = append(out, provider.Model{
			ID:            m.ID,
			Name:          m.Name,
			Vendor:        m.Vendor,
			Chat:          true,
			ToolCalls:     m.Capabilities.Supports.ToolCalls,
			Premium:       m.Billing.Multiplier,
			Preview:       m.Preview,
			ContextWindow: m.Capabilities.Limits.MaxContextWindowTokens,
			MaxOutput:     m.Capabilities.Limits.MaxOutputTokens,
			Default:       m.IsChatDefault,
		})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

func httpProviderError(res *http.Response, defaultRetryable bool) error {
	data, _ := io.ReadAll(io.LimitReader(res.Body, 4096))
	code, msg := parseErrorBody(data)
	if msg == "" {
		msg = strings.TrimSpace(string(data))
	}
	if msg == "" {
		msg = res.Status
	}
	retryable := defaultRetryable || res.StatusCode == 429 || res.StatusCode == 500 || res.StatusCode == 502 || res.StatusCode == 503 || res.StatusCode == 504
	err := providerError(res.StatusCode, code, msg, retryable)
	if ra := res.Header.Get("Retry-After"); ra != "" {
		err.Code = strings.TrimSpace(strings.Join([]string{err.Code, "retry_after=" + ra}, " "))
	}
	return err
}

func parseErrorBody(data []byte) (string, string) {
	var body struct {
		Error   any    `json:"error"`
		Type    string `json:"type"`
		Code    string `json:"code"`
		Message string `json:"message"`
	}
	if json.Unmarshal(data, &body) != nil {
		return "", ""
	}
	code, msg := body.Code, body.Message
	switch e := body.Error.(type) {
	case string:
		if msg == "" {
			msg = e
		}
	case map[string]any:
		if code == "" {
			if s, _ := e["code"].(string); s != "" {
				code = s
			} else if s, _ := e["type"].(string); s != "" {
				code = s
			}
		}
		if msg == "" {
			if s, _ := e["message"].(string); s != "" {
				msg = s
			}
		}
	}
	if code == "" {
		code = body.Type
	}
	return code, msg
}

func providerError(status int, code, message string, retryable bool) *provider.Error {
	return &provider.Error{
		Provider:  name,
		Status:    status,
		Code:      squeeze(redact(code), 80),
		Message:   squeeze(redact(message), 300),
		Retryable: retryable,
	}
}

func isRetryable(err error) bool {
	var perr *provider.Error
	return errors.As(err, &perr) && perr.Retryable
}

func retryDelay(err error, fallback time.Duration) time.Duration {
	var perr *provider.Error
	if errors.As(err, &perr) {
		for _, part := range strings.Fields(perr.Code) {
			if strings.HasPrefix(part, "retry_after=") {
				if sec, e := strconv.Atoi(strings.TrimPrefix(part, "retry_after=")); e == nil {
					d := time.Duration(sec) * time.Second
					if d > 30*time.Second {
						return 30 * time.Second
					}
					return d
				}
			}
		}
	}
	return fallback
}

func initiator(req provider.Request) string {
	if req.Initiator == "user" || req.Initiator == "agent" {
		return req.Initiator
	}
	if len(req.Messages) > 0 && req.Messages[len(req.Messages)-1].Role == provider.RoleUser {
		return "user"
	}
	return "agent"
}

func requestID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		return strconv.FormatInt(time.Now().UnixNano(), 16)
	}
	return hex.EncodeToString(b[:])
}

func looksLikeGitHubToken(token string) bool {
	return strings.HasPrefix(token, "gho_") ||
		strings.HasPrefix(token, "ghu_") ||
		strings.HasPrefix(token, "ghp_") ||
		strings.HasPrefix(token, "github_pat_") ||
		legacyTokenPattern.MatchString(token)
}

func findOAuthToken(v any) string {
	switch x := v.(type) {
	case string:
		if looksLikeGitHubToken(x) {
			return x
		}
	case []any:
		for _, e := range x {
			if tok := findOAuthToken(e); tok != "" {
				return tok
			}
		}
	case map[string]any:
		for _, key := range []string{"authTokens", "loggedInUsers"} {
			if tok := findOAuthToken(x[key]); tok != "" {
				return tok
			}
		}
	}
	return ""
}

func redact(s string) string {
	out := s
	for _, re := range tokenPatterns {
		out = re.ReplaceAllString(out, "[redacted]")
	}
	secretsMu.Lock()
	defer secretsMu.Unlock()
	for _, secret := range knownSecrets {
		if secret != "" {
			out = strings.ReplaceAll(out, secret, "[redacted]")
		}
	}
	return out
}

func rememberSecret(secret string) {
	secret = strings.TrimSpace(secret)
	if len(secret) < 16 {
		return
	}
	secretsMu.Lock()
	defer secretsMu.Unlock()
	for _, existing := range knownSecrets {
		if existing == secret {
			return
		}
	}
	knownSecrets = append(knownSecrets, secret)
}

func squeeze(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) <= n {
		return s
	}
	if n <= 3 {
		return s[:n]
	}
	return s[:n-3] + "..."
}

func sleep(ctx context.Context, d time.Duration) bool {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}
