package inspect

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/parity"
)

func TestAgentListingsKeepIdentityHistoryAndSeparateHeartbeat(t *testing.T) {
	s := temporaryStore(t)
	for name, kind := range map[string]string{"idle-worker": "worker", "idle-lead": "lead", "old-session": "session", "legacy": "legacy", "pruned": "worker"} {
		if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Kind: kind}); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := s.DB.Exec("UPDATE agents SET status='pruned',pruned_reason='no useful output for 14 days' WHERE name='pruned'"); err != nil {
		t.Fatal(err)
	}
	result := invoke(t, s, true, "", "agents", "--all")
	if result.code != 0 {
		t.Fatal(result.err)
	}
	var cards []AgentScorecard
	if err := json.Unmarshal([]byte(result.out), &cards); err != nil {
		t.Fatal(err)
	}
	want := map[string]string{"idle-worker": "idle", "idle-lead": "idle", "old-session": "unknown", "legacy": "legacy", "pruned": "pruned"}
	for _, c := range cards {
		if state, ok := want[c.Name]; ok {
			if c.Presence != state || (c.Name != "pruned" && c.Status != "active") {
				t.Errorf("%s: presence %s identity %s", c.Name, c.Presence, c.Status)
			}
			delete(want, c.Name)
		}
	}
	if len(want) != 0 {
		t.Fatalf("history missing: %v", want)
	}
	result = invoke(t, s, false, "", "agents")
	for _, text := range []string{"presence", "last heartbeat", "last useful", "old-session", "legacy", "idle-worker", "idle-lead"} {
		if result.code != 0 || !strings.Contains(result.out, text) {
			t.Errorf("listing lacks %q: %s %s", text, result.out, result.err)
		}
	}
	if strings.Contains(result.out, "pruned") {
		t.Fatal("existing --all pruning filter changed")
	}
}

func TestLegacyAgentParityOnlyRemovesAddedColumns(t *testing.T) {
	const old = "presence         pruned  worker   posts 99   jobs 2/5  last useful old  r/m\n"
	const current = "presence         pruned  worker   presence pruned  posts 99   jobs 2/5  last heartbeat 2026-09-06T19:00:00Z  last useful old  r/m\n"
	if got := parity.LegacyAgentListing(current); got != old {
		t.Fatalf("legacy identity/history changed: %q", got)
	}
	if got := parity.LegacyAgentListing(old); got != old {
		t.Fatalf("normalization removed an agent named presence: %q", got)
	}
}
