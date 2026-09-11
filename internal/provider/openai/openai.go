// Package openai implements an OpenAI-compatible chat-completions provider for
// the native harness.
package openai

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const (
	name  = "openai"
	label = "OpenAI-compatible"

	defaultBaseURL = "https://api.openai.com/v1"
	modelsCacheTTL = 5 * time.Minute
	statusCacheTTL = 10 * time.Minute
)

var (
	secretsMu    sync.Mutex
	knownSecrets []string
)

func init() {
	provider.Register(name, label, New)
}

// New opens the OpenAI-compatible provider using local settings, environment
// and private provider state.
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
	p := &openai{
		home:     o.Home,
		http:     o.HTTP,
		env:      o.Env,
		settings: o.Settings,
		log:      o.Log,
	}
	p.base = p.configuredBase()
	return p, nil
}

type openai struct {
	home     string
	http     *http.Client
	env      func(string) string
	settings map[string]string
	log      func(string, ...any)
	base     string

	mu             sync.Mutex
	models         []provider.Model
	modelsAt       time.Time
	refusedStored  bool
	refusedMessage string
	refusedAt      time.Time
	loggedOut      bool
}

type storedLogin struct {
	APIKey  string `json:"api_key"`
	BaseURL string `json:"base_url,omitempty"`
	User    string `json:"user,omitempty"`
	SavedAt string `json:"saved_at"`
}

type tokenSource struct {
	token  string
	source string
	user   string
	stored bool
}

func (p *openai) Name() string  { return name }
func (p *openai) Label() string { return label }

func (p *openai) LoginWithToken(ctx context.Context, token string) (provider.Login, error) {
	token = strings.TrimSpace(token)
	if token == "" {
		return provider.Login{}, providerError(0, "invalid_token", "OpenAI API key is empty", false)
	}
	rememberSecret(token)
	if err := p.validateKey(ctx, token); err != nil {
		return provider.Login{}, err
	}
	login := provider.Login{LoggedIn: true, User: keyUser(token), Source: "token login"}
	if err := p.writeStored(storedLogin{
		APIKey:  token,
		BaseURL: p.base,
		User:    login.User,
		SavedAt: time.Now().UTC().Format(time.RFC3339),
	}); err != nil {
		return provider.Login{}, err
	}
	p.mu.Lock()
	p.refusedStored, p.refusedMessage, p.refusedAt = false, "", time.Time{}
	p.loggedOut = false
	p.mu.Unlock()
	return login, nil
}

func (p *openai) Logout() error {
	err := os.Remove(p.loginPath())
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	p.mu.Lock()
	p.models = nil
	p.modelsAt = time.Time{}
	p.refusedStored, p.refusedMessage, p.refusedAt = false, "", time.Time{}
	p.loggedOut = true
	p.mu.Unlock()
	return nil
}

func (p *openai) Status(ctx context.Context) (provider.Login, error) {
	if s, ok, err := p.readStored(); err != nil {
		return provider.Login{}, err
	} else if ok && strings.TrimSpace(s.APIKey) != "" {
		rememberSecret(s.APIKey)
		user := strings.TrimSpace(s.User)
		if user == "" {
			user = keyUser(s.APIKey)
		}
		login := provider.Login{LoggedIn: true, User: user, Source: "token login"}
		p.mu.Lock()
		refused := p.refusedStored && time.Since(p.refusedAt) < statusCacheTTL
		msg := p.refusedMessage
		p.mu.Unlock()
		if refused {
			login.LoggedIn = false
			login.Detail = msg
		}
		return login, nil
	}
	src, err := p.sourceNoFile()
	if err != nil {
		return provider.Login{}, err
	}
	if src.token == "" {
		return provider.Login{LoggedIn: false, Detail: "Paste an OpenAI-compatible API key in Settings -> Harness or run pulse harness login --provider openai --with-token"}, nil
	}
	login := provider.Login{LoggedIn: true, User: src.user, Source: src.source}
	p.mu.Lock()
	loggedOut := p.loggedOut
	p.mu.Unlock()
	if loggedOut {
		login.Detail = "Stored OpenAI-compatible login removed; still using the environment API key."
	}
	return login, nil
}

func (p *openai) ExportLogin() (string, provider.Login, error) {
	s, ok, err := p.readStored()
	if err != nil {
		return "", provider.Login{}, err
	}
	if !ok || strings.TrimSpace(s.APIKey) == "" {
		return "", provider.Login{}, providerError(0, "no_stored_login", "no stored OpenAI-compatible login; paste an API key in Settings -> Harness or run pulse harness login --provider openai --with-token", false)
	}
	token := strings.TrimSpace(s.APIKey)
	rememberSecret(token)
	user := strings.TrimSpace(s.User)
	if user == "" {
		user = keyUser(token)
	}
	return token, provider.Login{LoggedIn: true, User: user, Source: "stored"}, nil
}

func (p *openai) Models(ctx context.Context) ([]provider.Model, error) {
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
	models, err := p.fetchModels(ctx, src.token)
	if err != nil {
		if isUnauthorized(err) && src.stored {
			p.rememberStoredRefusal("the stored OpenAI-compatible API key was refused; sign in again")
		}
		return nil, err
	}
	p.mu.Lock()
	p.models = append([]provider.Model(nil), models...)
	p.modelsAt = time.Now()
	p.mu.Unlock()
	return models, nil
}

func (p *openai) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
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
	backoffs := []time.Duration{time.Second, 3 * time.Second, 9 * time.Second}
	for attempt := 0; attempt < len(backoffs); attempt++ {
		resp, err := p.chatOnce(ctx, src.token, body)
		if err == nil {
			return resp, nil
		}
		if isUnauthorized(err) && src.stored {
			p.rememberStoredRefusal("the stored OpenAI-compatible API key was refused; sign in again")
		}
		if !isRetryable(err) || attempt == len(backoffs)-1 {
			return provider.Response{}, err
		}
		wait := retryDelay(err, backoffs[attempt])
		p.logRedacted("openai chat retry in %s after %s", wait, err.Error())
		if !sleep(ctx, wait) {
			return provider.Response{}, ctx.Err()
		}
	}
	return provider.Response{}, providerError(0, "retry_exhausted", "OpenAI-compatible chat retry loop exhausted", true)
}

func (p *openai) validateKey(ctx context.Context, token string) error {
	if _, err := p.fetchModels(ctx, token); err != nil {
		if isUnauthorized(err) {
			return providerError(http.StatusUnauthorized, "unauthorized", "the key was refused", false)
		}
		return err
	}
	return nil
}

func (p *openai) fetchModels(ctx context.Context, token string) ([]provider.Model, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, p.base+"/models", nil)
	if err != nil {
		return nil, err
	}
	p.headers(req, token)
	res, err := p.http.Do(req)
	if err != nil {
		return nil, providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode == http.StatusUnauthorized || res.StatusCode == http.StatusForbidden {
		return nil, providerError(res.StatusCode, "unauthorized", "the key was refused", false)
	}
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return nil, httpProviderError(res, false)
	}
	var body modelsResponse
	if err := json.NewDecoder(res.Body).Decode(&body); err != nil {
		return nil, providerError(0, "decode", err.Error(), false)
	}
	return mapModels(body), nil
}

func (p *openai) chatOnce(ctx context.Context, token string, body []byte) (provider.Response, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, p.base+"/chat/completions", bytes.NewReader(body))
	if err != nil {
		return provider.Response{}, err
	}
	p.headers(req, token)
	req.Header.Set("Content-Type", "application/json")
	res, err := p.http.Do(req)
	if err != nil {
		return provider.Response{}, providerError(0, "network", err.Error(), true)
	}
	defer res.Body.Close()
	if res.StatusCode == http.StatusUnauthorized || res.StatusCode == http.StatusForbidden {
		return provider.Response{}, providerError(res.StatusCode, "unauthorized", "the key was refused", false)
	}
	if res.StatusCode < 200 || res.StatusCode > 299 {
		return provider.Response{}, httpProviderError(res, false)
	}
	var out chatResponse
	if err := json.NewDecoder(res.Body).Decode(&out); err != nil {
		return provider.Response{}, providerError(0, "decode", err.Error(), false)
	}
	resp := out.providerResponse()
	resp.RequestID = res.Header.Get("x-request-id")
	if resp.RequestID == "" {
		resp.RequestID = out.ID
	}
	return resp, nil
}

func (p *openai) headers(req *http.Request, token string) {
	rememberSecret(token)
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")
}

func (p *openai) tokenSource() (tokenSource, error) {
	if s, ok, err := p.readStored(); err != nil {
		return tokenSource{}, err
	} else if ok && strings.TrimSpace(s.APIKey) != "" {
		tok := strings.TrimSpace(s.APIKey)
		rememberSecret(tok)
		user := strings.TrimSpace(s.User)
		if user == "" {
			user = keyUser(tok)
		}
		return tokenSource{token: tok, source: "token login", user: user, stored: true}, nil
	}
	return p.sourceNoFile()
}

func (p *openai) sourceNoFile() (tokenSource, error) {
	if tok := strings.TrimSpace(p.env("OPENAI_API_KEY")); tok != "" {
		rememberSecret(tok)
		return tokenSource{token: tok, source: "environment", user: keyUser(tok)}, nil
	}
	return tokenSource{}, nil
}

func (p *openai) configuredBase() string {
	if p.settings != nil {
		if v := strings.TrimSpace(p.settings["provider.openai.base_url"]); v != "" {
			return trimBase(v)
		}
	}
	if v := strings.TrimSpace(p.env("OPENAI_BASE_URL")); v != "" {
		return trimBase(v)
	}
	return defaultBaseURL
}

func trimBase(base string) string {
	base = strings.TrimSpace(base)
	for strings.HasSuffix(base, "/") {
		base = strings.TrimSuffix(base, "/")
	}
	if base == "" {
		return defaultBaseURL
	}
	return base
}

func (p *openai) loginPath() string {
	return filepath.Join(p.home, "private", "providers", "openai.json")
}

func (p *openai) readStored() (storedLogin, bool, error) {
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

func (p *openai) writeStored(s storedLogin) error {
	rememberSecret(s.APIKey)
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

func (p *openai) rememberStoredRefusal(msg string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.refusedStored = true
	p.refusedMessage = msg
	p.refusedAt = time.Now()
}

func (p *openai) logRedacted(format string, a ...any) {
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
		PromptTokens     int `json:"prompt_tokens"`
		CompletionTokens int `json:"completion_tokens"`
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
			Cost:             0,
		},
		Model: r.Model,
	}
}

type modelsResponse struct {
	Data []struct {
		ID      string `json:"id"`
		OwnedBy string `json:"owned_by"`
		Created int64  `json:"created"`
	} `json:"data"`
}

func mapModels(body modelsResponse) []provider.Model {
	out := make([]provider.Model, 0, len(body.Data))
	for _, m := range body.Data {
		vendor := strings.TrimSpace(m.OwnedBy)
		if vendor == "" {
			vendor = "openai"
		}
		out = append(out, provider.Model{
			ID:            m.ID,
			Name:          m.ID,
			Vendor:        vendor,
			Chat:          true,
			ToolCalls:     true,
			ContextWindow: 0,
		})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	if len(out) == 0 {
		return out
	}
	defaultIndex := 0
	for i, m := range out {
		if isGPT4oFamily(m.ID) {
			defaultIndex = i
			break
		}
	}
	out[defaultIndex].Default = true
	return out
}

func isGPT4oFamily(id string) bool {
	id = strings.ToLower(strings.TrimSpace(id))
	return id == "gpt-4o" || strings.HasPrefix(id, "gpt-4o-")
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

func isUnauthorized(err error) bool {
	var perr *provider.Error
	return errors.As(err, &perr) && (perr.Status == http.StatusUnauthorized || perr.Status == http.StatusForbidden || perr.Code == "unauthorized")
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

func redact(s string) string {
	out := s
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
	if len(secret) < 8 {
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

func keyUser(token string) string {
	token = strings.TrimSpace(token)
	if len(token) <= 4 {
		return "…" + token
	}
	return "…" + token[len(token)-4:]
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
