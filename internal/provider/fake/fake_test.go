package fake

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func TestScriptOrderAndExhaustion(t *testing.T) {
	p := New(Script{Replies: []Reply{
		{Content: "first"},
		{Command: "echo hi", Content: "running"},
		{Report: "SUMMARY: done"},
	}})
	req := provider.Request{Model: "fake-1", Messages: []provider.Message{{Role: provider.RoleUser, Content: "task"}}}
	resp, err := p.Chat(context.Background(), req)
	if err != nil || resp.Message.Content != "first" || len(resp.Message.ToolCalls) != 0 {
		t.Fatalf("first response = %+v %v", resp, err)
	}
	resp, err = p.Chat(context.Background(), req)
	if err != nil || resp.Message.Content != "running" || len(resp.Message.ToolCalls) != 1 || resp.Message.ToolCalls[0].ID != "call-2" {
		t.Fatalf("command response = %+v %v", resp, err)
	}
	resp, err = p.Chat(context.Background(), req)
	if err != nil || resp.Message.Content != "SUMMARY: done" {
		t.Fatalf("report response = %+v %v", resp, err)
	}
	resp, err = p.Chat(context.Background(), req)
	if err != nil || resp.Message.Content != "SUMMARY: script exhausted" {
		t.Fatalf("exhausted response = %+v %v", resp, err)
	}
	if len(p.Requests()) != 4 {
		t.Fatalf("requests = %d, want 4", len(p.Requests()))
	}
}

func TestTokensPerCharAndLoop(t *testing.T) {
	p := New(Script{Replies: []Reply{{Content: "abcd"}}, TokensPerChar: 0.5, Loop: true})
	req := provider.Request{Messages: []provider.Message{
		{Role: provider.RoleUser, Content: "123456"},
		{Role: provider.RoleAssistant, ToolCalls: []provider.ToolCall{{Arguments: `{"command":"echo"}`}}},
	}}
	for i := 0; i < 2; i++ {
		resp, err := p.Chat(context.Background(), req)
		if err != nil {
			t.Fatal(err)
		}
		if resp.Usage.PromptTokens != 12 || resp.Usage.CompletionTokens != 1 {
			t.Fatalf("usage = %+v, want prompt 12 completion 1", resp.Usage)
		}
	}
}

func TestReplyError(t *testing.T) {
	want := &provider.Error{Provider: "fake", Message: "boom"}
	p := New(Script{Replies: []Reply{{Err: want}}})
	_, err := p.Chat(context.Background(), provider.Request{})
	if !errors.Is(err, want) {
		t.Fatalf("error = %v, want %v", err, want)
	}
}

func TestRegisterIdempotentAndJSONScript(t *testing.T) {
	Register()
	Register()
	path := filepath.Join(t.TempDir(), "script.json")
	data, err := json.Marshal(Script{
		Replies: []Reply{{Report: "SUMMARY: from json", Usage: provider.Usage{PromptTokens: 3}}},
		Models:  []provider.Model{{ID: "json-model", Name: "JSON Model", Vendor: "fake", Chat: true}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	p, err := provider.Open("fake", provider.Options{Env: func(key string) string {
		if key == "PULSE_HARNESS_FAKE_SCRIPT" {
			return path
		}
		return ""
	}})
	if err != nil {
		t.Fatal(err)
	}
	models, err := p.Models(context.Background())
	if err != nil || len(models) != 1 || models[0].ID != "json-model" {
		t.Fatalf("models = %+v %v", models, err)
	}
	resp, err := p.Chat(context.Background(), provider.Request{Model: "json-model"})
	if err != nil || resp.Message.Content != "SUMMARY: from json" || resp.Usage.PromptTokens != 3 {
		t.Fatalf("response = %+v %v", resp, err)
	}
}
