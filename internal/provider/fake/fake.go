// Package fake provides a scripted provider for harness tests.
package fake

import (
	"context"
	"encoding/json"
	"os"
	"sync"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const (
	name  = "fake"
	label = "Scripted (tests)"
)

// Reply is one scripted answer from the fake provider.
type Reply struct {
	Content string         `json:"content,omitempty"`
	Command string         `json:"command,omitempty"`
	Report  string         `json:"report,omitempty"`
	Usage   provider.Usage `json:"usage,omitempty"`
	Err     error          `json:"-"`
	Model   string         `json:"model,omitempty"`
}

// Script configures the fake provider's replies and model list.
type Script struct {
	Replies       []Reply          `json:"replies,omitempty"`
	Models        []provider.Model `json:"models,omitempty"`
	TokensPerChar float64          `json:"tokens_per_char,omitempty"`
	Loop          bool             `json:"loop,omitempty"`
}

// Provider is a thread-safe scripted provider.
type Provider struct {
	mu       sync.Mutex
	script   Script
	next     int
	requests []provider.Request
}

// New returns a scripted fake provider.
func New(script Script) *Provider {
	if script.TokensPerChar == 0 {
		script.TokensPerChar = 0.25
	}
	return &Provider{script: script}
}

func (p *Provider) Name() string  { return name }
func (p *Provider) Label() string { return label }

// Chat records the request and returns the next scripted reply.
func (p *Provider) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
	_ = ctx
	p.mu.Lock()
	defer p.mu.Unlock()
	p.requests = append(p.requests, cloneRequest(req))
	reply := p.nextReply()
	if reply.Err != nil {
		return provider.Response{Model: reply.Model}, reply.Err
	}
	msg := provider.Message{Role: provider.RoleAssistant}
	finish := "stop"
	switch {
	case reply.Command != "":
		args, _ := json.Marshal(struct {
			Command string `json:"command"`
		}{Command: reply.Command})
		msg.Content = reply.Content
		msg.ToolCalls = []provider.ToolCall{{ID: "call-" + itoa(len(p.requests)), Name: "bash", Arguments: string(args)}}
		finish = "tool_calls"
	case reply.Report != "":
		msg.Content = reply.Report
	default:
		msg.Content = reply.Content
	}
	usage := reply.Usage
	if usage.PromptTokens == 0 {
		usage.PromptTokens = int(float64(requestChars(req)) * p.script.TokensPerChar)
	}
	if usage.CompletionTokens == 0 {
		usage.CompletionTokens = len(replyText(reply)) / 4
	}
	model := reply.Model
	if model == "" {
		model = req.Model
	}
	return provider.Response{Message: msg, FinishReason: finish, Usage: usage, Model: model}, nil
}

func (p *Provider) nextReply() Reply {
	if len(p.script.Replies) == 0 {
		return Reply{Report: "SUMMARY: script exhausted"}
	}
	if p.next < len(p.script.Replies) {
		reply := p.script.Replies[p.next]
		p.next++
		return reply
	}
	if p.script.Loop {
		return p.script.Replies[len(p.script.Replies)-1]
	}
	return Reply{Report: "SUMMARY: script exhausted"}
}

// Requests returns a copy of every chat request received.
func (p *Provider) Requests() []provider.Request {
	p.mu.Lock()
	defer p.mu.Unlock()
	out := make([]provider.Request, len(p.requests))
	for i, req := range p.requests {
		out[i] = cloneRequest(req)
	}
	return out
}

func (p *Provider) Models(ctx context.Context) ([]provider.Model, error) {
	_ = ctx
	if len(p.script.Models) > 0 {
		return append([]provider.Model(nil), p.script.Models...), nil
	}
	return []provider.Model{{
		ID: "fake-1", Name: "Fake 1", Vendor: "fake", Chat: true, ToolCalls: true,
		ContextWindow: 128000, Default: true,
	}}, nil
}

func (p *Provider) Status(ctx context.Context) (provider.Login, error) {
	_ = ctx
	return provider.Login{LoggedIn: true, User: "scripted", Source: "built-in"}, nil
}

func (p *Provider) Logout() error { return nil }

var registerOnce sync.Once

// Register adds the fake provider to the global registry once.
func Register() {
	registerOnce.Do(func() {
		provider.Register(name, label, func(o provider.Options) (provider.Provider, error) {
			env := o.Env
			if env == nil {
				env = os.Getenv
			}
			if path := env("PULSE_HARNESS_FAKE_SCRIPT"); path != "" {
				data, err := os.ReadFile(path)
				if err != nil {
					return nil, err
				}
				var script Script
				if err := json.Unmarshal(data, &script); err != nil {
					return nil, err
				}
				return New(script), nil
			}
			return New(defaultScript()), nil
		})
	})
}

func defaultScript() Script {
	return Script{Replies: []Reply{
		{Command: "echo hi"},
		{Report: "SUMMARY: scripted run finished."},
	}}
}

func requestChars(req provider.Request) int {
	total := 0
	for _, msg := range req.Messages {
		total += len(msg.Content)
		for _, call := range msg.ToolCalls {
			total += len(call.Arguments)
		}
	}
	return total
}

func replyText(reply Reply) string {
	return reply.Content + reply.Command + reply.Report
}

func cloneRequest(req provider.Request) provider.Request {
	req.Messages = cloneMessages(req.Messages)
	req.Tools = append([]provider.Tool(nil), req.Tools...)
	return req
}

func cloneMessages(in []provider.Message) []provider.Message {
	out := append([]provider.Message(nil), in...)
	for i := range out {
		out[i].ToolCalls = append([]provider.ToolCall(nil), out[i].ToolCalls...)
	}
	return out
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var buf [20]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	return string(buf[i:])
}
