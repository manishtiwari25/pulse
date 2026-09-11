package harness

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func obs(content string) provider.Message {
	return provider.Message{Role: provider.RoleTool, ToolCallID: "call-1", Name: "bash", Content: content}
}

func TestPruneHistoryKeepNCollapsesOlderObservations(t *testing.T) {
	msgs := []provider.Message{
		{Role: provider.RoleSystem, Content: "system"},
		{Role: provider.RoleUser, Content: "task"},
		obs(formatObservation(Observation{Output: "a\nb\nc\nd\ne\nf", ExitCode: 0}, 0)),
		{Role: provider.RoleAssistant, Content: "still full"},
		obs(formatObservation(Observation{Output: "new\noutput", ExitCode: 0}, 0)),
	}
	out, changed := pruneHistory(msgs, 1, 1)
	if changed != 1 {
		t.Fatalf("changed = %d, want 1", changed)
	}
	if !strings.Contains(out[2].Content, "characters elided") || !strings.Contains(out[2].Content, "<returncode>0</returncode>") {
		t.Fatalf("older observation not collapsed: %q", out[2].Content)
	}
	if out[3].Content != "still full" || out[4].Content != msgs[4].Content {
		t.Fatalf("newer or assistant message changed: %+v", out)
	}
}

func TestPruneHistoryMarkerIdempotence(t *testing.T) {
	msgs := []provider.Message{
		{Role: provider.RoleSystem, Content: "system"},
		{Role: provider.RoleUser, Content: "task"},
		obs("<returncode>0</returncode>\n<output>\na\n[10 characters elided - rerun the command to see them]\nz\n</output>"),
		obs(formatObservation(Observation{Output: "new", ExitCode: 0}, 0)),
	}
	out, changed := pruneHistory(msgs, 1, 1)
	if changed != 0 || out[2].Content != msgs[2].Content {
		t.Fatalf("marker was not idempotent: changed=%d content=%q", changed, out[2].Content)
	}
}

func TestPruneHistoryLeavesShortOutputsUnchanged(t *testing.T) {
	for _, output := range []string{"a\nb\nc", "a\nb\nc\nd"} {
		msgs := []provider.Message{
			{Role: provider.RoleSystem, Content: "system"},
			{Role: provider.RoleUser, Content: "task"},
			obs(formatObservation(Observation{Output: output, ExitCode: 0}, 0)),
			obs(formatObservation(Observation{Output: "new", ExitCode: 0}, 0)),
		}
		out, changed := pruneHistory(msgs, 1, 1)
		if changed != 0 {
			t.Fatalf("%q changed = %d, want 0", output, changed)
		}
		if out[2].Content != msgs[2].Content {
			t.Fatalf("%q changed content to %q", output, out[2].Content)
		}
	}
}

func TestPruneHistoryLevel2OneLiners(t *testing.T) {
	msgs := []provider.Message{
		{Role: provider.RoleSystem, Content: "system"},
		{Role: provider.RoleUser, Content: "task"},
		obs(formatObservation(Observation{Output: "a\nb\nc\nd", ExitCode: 7}, 0)),
		obs(formatObservation(Observation{Output: "new", ExitCode: 0}, 0)),
	}
	out, changed := pruneHistory(msgs, 1, 2)
	if changed != 1 {
		t.Fatalf("changed = %d, want 1", changed)
	}
	want := "<returncode>7</returncode>\n<output>\n[output elided - rerun the command to see it]\n</output>"
	if out[2].Content != want {
		t.Fatalf("level 2 = %q, want %q", out[2].Content, want)
	}
}

func TestPruneHistoryPreservesToolCallIDsAndFirstTwoMessages(t *testing.T) {
	msgs := []provider.Message{
		{Role: provider.RoleSystem, Content: "<returncode>0</returncode>\n<output>\nsystem\n</output>"},
		{Role: provider.RoleUser, Content: "<returncode>0</returncode>\n<output>\ntask\n</output>"},
		{Role: provider.RoleTool, ToolCallID: "call-99", Name: "bash", Content: formatObservation(Observation{Output: "a\nb\nc\nd\ne", ExitCode: 0}, 0)},
		obs(formatObservation(Observation{Output: "new", ExitCode: 0}, 0)),
	}
	out, changed := pruneHistory(msgs, 1, 1)
	if changed != 1 {
		t.Fatalf("changed = %d, want 1", changed)
	}
	if out[0].Content != msgs[0].Content || out[1].Content != msgs[1].Content {
		t.Fatalf("first two messages changed: %+v", out[:2])
	}
	if out[2].ToolCallID != "call-99" || out[2].Name != "bash" || out[2].Role != provider.RoleTool {
		t.Fatalf("tool pairing changed: %+v", out[2])
	}
}

func TestPruneHistoryRecognizesFencedModeUserObservations(t *testing.T) {
	msgs := []provider.Message{
		{Role: provider.RoleSystem, Content: "system"},
		{Role: provider.RoleUser, Content: "task"},
		{Role: provider.RoleUser, Content: formatObservation(Observation{Output: "a\nb\nc\nd\ne", ExitCode: 0}, 0)},
		obs(formatObservation(Observation{Output: "new", ExitCode: 0}, 0)),
	}
	out, changed := pruneHistory(msgs, 1, 1)
	if changed != 1 || !strings.Contains(out[2].Content, "characters elided") {
		t.Fatalf("fenced-mode observation not pruned: changed=%d content=%q", changed, out[2].Content)
	}
}
