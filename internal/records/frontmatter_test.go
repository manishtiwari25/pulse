package records

import (
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestParseFrontmatter(t *testing.T) {
	text := "---\r\nid: ADR-019\r\ntitle: \"A title: with # and quotes\"\r\ntier: 2\r\nareas: [agents, 'data, storage', \"a#b\"] # comment\r\ntags:\r\n  - one\r\n  - 'it''s two'\r\ndelivered:\r\nenv: {\"MODE\": \"test\"}\r\nactive: true\r\n---\r\n\r\n# Body\r\n---\r\n"
	front, body, err := ParseFrontmatter(text)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]any{
		"id": "ADR-019", "title": "A title: with # and quotes", "tier": float64(2),
		"areas": []any{"agents", "data, storage", "a#b"}, "tags": []any{"one", "it's two"},
		"delivered": nil, "env": map[string]any{"MODE": "test"}, "active": true,
	}
	if !reflect.DeepEqual(front, want) {
		t.Fatalf("frontmatter = %#v, want %#v", front, want)
	}
	if body != "\r\n# Body\r\n---\r\n" {
		t.Fatalf("body changed: %q", body)
	}
	got, roundBody, err := ParseFrontmatter(RenderMarkdown(nil, &store.Revision{Frontmatter: front, Body: body}))
	if err != nil || !reflect.DeepEqual(got, want) || roundBody != body {
		t.Fatalf("round-trip = %#v, %q, %v", got, roundBody, err)
	}
}

func TestMultilineFrontmatter(t *testing.T) {
	tests := []struct{ name, value, want string }{
		{"folded", ">\n  a long\n  description\n\n  second paragraph\n", "a long description\nsecond paragraph\n"},
		{"literal", "|\n  a long\n  description\n\n", "a long\ndescription\n"},
		{"stripped", "|-\n  line one\n  line two\n\n", "line one\nline two"},
		{"kept", "|+\n  first\n\n\n", "first\n\n\n"},
		{"folded-indent", ">-\n  first\n    indented\n  last\n", "first\n  indented\nlast"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			front, body, err := ParseFrontmatter("---\ndescription: " + tt.value + "next: okay\n---\nBody\n")
			if err != nil || front["description"] != tt.want || front["next"] != "okay" || body != "Body\n" {
				t.Fatalf("got %#v, body %q, err %v", front, body, err)
			}
		})
	}
	front, _, err := ParseFrontmatter("---\nsetup:\n  - npm ci\n  - >-\n    go vet\n    ./...\nenv:\n  MODE: test\n  EMPTY: \"\"\n---\n")
	if err != nil || !reflect.DeepEqual(front["setup"], []any{"npm ci", "go vet ./..."}) || !reflect.DeepEqual(front["env"], map[string]any{"MODE": "test", "EMPTY": ""}) {
		t.Fatalf("structured values: %#v %v", front, err)
	}
	front, _, err = ParseFrontmatter("---\ntags:\n- one\n- two\nnext: okay\n---\n")
	if err != nil || !reflect.DeepEqual(front["tags"], []any{"one", "two"}) || front["next"] != "okay" {
		t.Fatalf("indentationless block list: %#v %v", front, err)
	}
}

func TestFrontmatterErrors(t *testing.T) {
	for _, text := range []string{
		"---\nx: y\n", "---\nnot a key\n---\n", "---\nx: a\nx: b\n---\n",
		"---\nx: [a, b\n---\n", "---\nx: 'unterminated\n---\n",
		"---\nx: [a,,b]\n---\n", "---\nx: *alias\n---\n",
		"---\n  unexpected: indentation\n---\n", "---\nx: {a: 1, a: 2}\n---\n",
	} {
		if _, _, err := ParseFrontmatter(text); err == nil {
			t.Errorf("accepted invalid frontmatter %q", text)
		}
	}
	for _, text := range []string{"# Heading\n\nBody\n", "", "not YAML\n---\nbody"} {
		front, body, err := ParseFrontmatter(text)
		if err != nil || len(front) != 0 || body != text {
			t.Errorf("no frontmatter: %#v, %q, %v", front, body, err)
		}
	}
}

func TestRealFrontmatterSamples(t *testing.T) {
	repo := sourceRepo(t)
	samples := []struct{ file, title string }{
		{"docs/decisions/ADR-019-agent-first-pulse-store-and-runner.md", "Agent-First PULSE"},
		{"docs/plans/PLAN-026-project-environment-recipes.md", "Project Environment Recipes"},
		{"docs/plans/WU-022-decision-hook.md", "Decision Hook"},
		{"docs/features/016-project-environment-recipes.md", "Project Environment Recipes"},
		{"docs/memory/product.md", "Memory - Product"},
		{"docs/workflows/board.md", "Workflow - Agents' Board"},
		{"docs/skills/pulse-plan/SKILL.md", "PULSE Plan"},
		{"docs/guides/new-repo.md", "Start a New Repository"},
		{"docs/context/environment.md", "Environment"},
		{"docs/architecture/overview.md", "PULSE Architecture Overview"},
		{"docs/evolution/constitution.md", "PULSE Evolution Constitution"},
		{"docs/usage/usage-log.md", "Archived Token Usage Log"},
		{"docs/prompts/shared/eli5.prompt.md", "Shared ELI5 Output Format"},
		{"AGENTS.md", "PULSE - Agent Operating Guide"},
	}
	for _, sample := range samples {
		t.Run(sample.file, func(t *testing.T) {
			text := gitFixture(t, repo, "show", fixtureRef+":"+sample.file)
			front, body, err := ParseFrontmatter(text)
			if err != nil {
				t.Fatal(err)
			}
			rec := store.Record{}
			if err := applyColumns(&rec, front, body); err != nil {
				t.Fatal(err)
			}
			if !strings.HasPrefix(rec.Title, sample.title) {
				t.Fatalf("title = %q, want prefix %q", rec.Title, sample.title)
			}
			f2, b2, err := ParseFrontmatter(RenderMarkdown(&rec, &store.Revision{Frontmatter: front, Body: body}))
			if err != nil || !reflect.DeepEqual(f2, front) || b2 != body {
				t.Fatalf("round-trip differs: %v", err)
			}
		})
	}
}
