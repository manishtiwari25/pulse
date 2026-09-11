// Package provider is the model port of the native harness (internal/harness): one interface every model
// vendor implements - a chat call that understands tool calls, the model list, the login state - plus the
// optional logins (a device flow, a pasted token) and a registry by name. Nothing vendor-specific lives here;
// each vendor is its own subpackage (provider/copilot first) and registers itself in init.
//
// Secrets never reach the store: a provider keeps what it must under <home>/private/providers/<name>.json
// (0700/0600), and every error a provider returns is redacted before it leaves the package.
package provider

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"sort"
	"strings"
	"sync"
)

// Role is the author of a Message.
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
)

// Message is one turn of the conversation, in the OpenAI chat shape every vendor understands.
type Message struct {
	Role       Role       `json:"role"`
	Content    string     `json:"content,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`   // an assistant message asking for tools
	ToolCallID string     `json:"tool_call_id,omitempty"` // a tool message answering one call
	Name       string     `json:"name,omitempty"`         // the tool a tool message answers for
}

// ToolCall is one tool the assistant asked for; Arguments is the JSON object the model produced.
type ToolCall struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// Tool is one tool the harness offers the model; Parameters is a JSON schema object.
type Tool struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Parameters  json.RawMessage `json:"parameters"`
}

// Request is one chat call.
type Request struct {
	Model       string
	Messages    []Message
	Tools       []Tool
	MaxTokens   int      // 0 means the vendor's default
	Temperature *float64 // nil means the vendor's default
	// Initiator says who caused this call: "user" for the first call of a run, "agent" for every call
	// that follows a tool result. Vendors that bill per user request read it (Copilot's X-Initiator).
	Initiator string
}

// Usage is what one call cost.
type Usage struct {
	PromptTokens     int     `json:"prompt_tokens"`
	CompletionTokens int     `json:"completion_tokens"`
	Cost             float64 `json:"cost"` // vendor units when known (Copilot: premium requests), else 0
}

// Add sums another call's usage into u.
func (u *Usage) Add(o Usage) {
	u.PromptTokens += o.PromptTokens
	u.CompletionTokens += o.CompletionTokens
	u.Cost += o.Cost
}

// Response is the assistant's answer to one Request.
type Response struct {
	Message      Message
	FinishReason string // stop, tool_calls, length, content_filter, or the vendor's word
	Usage        Usage
	Model        string // the model that answered, when the vendor says
	RequestID    string
}

// Model is one entry of a vendor's model list.
type Model struct {
	ID            string  `json:"id"`
	Name          string  `json:"name"`
	Vendor        string  `json:"vendor"`
	Chat          bool    `json:"chat"`
	ToolCalls     bool    `json:"tool_calls"`
	Premium       float64 `json:"premium"` // billing multiplier when the vendor reports one; 0 means unknown
	Preview       bool    `json:"preview"`
	ContextWindow int     `json:"context_window"`
	MaxOutput     int     `json:"max_output"`
	Default       bool    `json:"default"` // the vendor's own default model, when it names one
}

// Login is the login state of a provider on this machine.
type Login struct {
	LoggedIn  bool   `json:"logged_in"`
	User      string `json:"user,omitempty"`
	Source    string `json:"source,omitempty"` // device login, token login, environment, editor login, ...
	Plan      string `json:"plan,omitempty"`
	ExpiresAt string `json:"expires_at,omitempty"`
	Detail    string `json:"detail,omitempty"` // one redacted line for the console when something is off
}

// DeviceFlow is a started device login: the owner opens VerificationURI and types UserCode.
type DeviceFlow struct {
	DeviceCode      string `json:"device_code"`
	UserCode        string `json:"user_code"`
	VerificationURI string `json:"verification_uri"`
	Interval        int    `json:"interval"`   // seconds between polls
	ExpiresIn       int    `json:"expires_in"` // seconds the code stays valid
}

// Device login poll answers.
const (
	DevicePending  = "pending"
	DeviceComplete = "complete"
	DeviceExpired  = "expired"
	DeviceDenied   = "denied"
	DeviceSlowDown = "slow_down"
)

// Provider is what the harness needs from a model vendor.
type Provider interface {
	// Name is the registry name, e.g. "copilot".
	Name() string
	// Label is the name the console shows, e.g. "GitHub Copilot".
	Label() string
	// Chat makes one call. A missing login is ErrNotLoggedIn; vendor failures are *Error.
	Chat(ctx context.Context, req Request) (Response, error)
	// Models lists what the login may use, chat models first, sorted by ID.
	Models(ctx context.Context) ([]Model, error)
	// Status says whether this machine is logged in, without making a model call.
	Status(ctx context.Context) (Login, error)
}

// DeviceLoginer is a provider that signs in through a device flow.
type DeviceLoginer interface {
	BeginDeviceLogin(ctx context.Context) (DeviceFlow, error)
	// PollDeviceLogin answers one of the Device* words; on DeviceComplete the login is stored and returned.
	PollDeviceLogin(ctx context.Context, deviceCode string) (string, Login, error)
}

// TokenLoginer is a provider that accepts a pasted token (a CI machine, a remote hand).
type TokenLoginer interface {
	LoginWithToken(ctx context.Context, token string) (Login, error)
}

// LoginExporter hands out the login this machine holds so another machine can use it. Only a login the
// human placed through PULSE is exportable; borrowed tokens (environment, editor, other CLIs) are not.
type LoginExporter interface {
	ExportLogin() (token string, login Login, err error)
}

// Logouter forgets the stored login.
type Logouter interface {
	Logout() error
}

// ErrNotLoggedIn is the one error the harness turns into "sign in first".
var ErrNotLoggedIn = errors.New("not logged in")

// Error is a vendor failure, already redacted. Retryable marks the failures a caller may try again
// (rate limits, 5xx, network); Status is the HTTP status when there was one.
type Error struct {
	Provider  string
	Status    int
	Code      string
	Message   string
	Retryable bool
}

func (e *Error) Error() string {
	if e.Status != 0 {
		return fmt.Sprintf("%s: %s (HTTP %d)", e.Provider, e.Message, e.Status)
	}
	return fmt.Sprintf("%s: %s", e.Provider, e.Message)
}

// Options is what every provider is opened with.
type Options struct {
	// Home is the PULSE home; a provider's private files live under <Home>/private/providers.
	Home string
	// Settings are store settings the provider may read, keyed as stored
	// (for example provider.openai.base_url). Providers must work with nil.
	Settings map[string]string
	// HTTP is the client to use; nil means a default with sane timeouts.
	HTTP *http.Client
	// Env reads an environment variable; nil means os.Getenv.
	Env func(string) string
	// Log, when set, receives one redacted line per notable event (a token refresh, a retry).
	Log func(format string, a ...any)
}

// Factory opens a provider.
type Factory func(Options) (Provider, error)

var (
	registryMu sync.RWMutex
	registry   = map[string]Factory{}
	labels     = map[string]string{}
)

// Register adds a provider; a duplicate name panics like a duplicate flag would.
func Register(name, label string, f Factory) {
	registryMu.Lock()
	defer registryMu.Unlock()
	if _, dup := registry[name]; dup {
		panic("provider " + name + " registered twice")
	}
	registry[name] = f
	labels[name] = label
}

// Names lists the registered providers, sorted.
func Names() []string {
	registryMu.RLock()
	defer registryMu.RUnlock()
	out := make([]string, 0, len(registry))
	for n := range registry {
		out = append(out, n)
	}
	sort.Strings(out)
	return out
}

// LabelOf is the console name of a registered provider ("" when unknown).
func LabelOf(name string) string {
	registryMu.RLock()
	defer registryMu.RUnlock()
	return labels[name]
}

// Open builds the named provider; an unknown name lists what exists.
func Open(name string, o Options) (Provider, error) {
	registryMu.RLock()
	f, ok := registry[strings.TrimSpace(name)]
	registryMu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("unknown provider %q; known: %s", name, strings.Join(Names(), ", "))
	}
	return f(o)
}

// Default is the provider a fresh install uses.
const Default = "copilot"
