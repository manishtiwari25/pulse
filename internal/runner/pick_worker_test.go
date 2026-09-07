package runner

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/bus"
)

// Workers spawned under an earlier worker_model are never picked again: joinAgent would
// refuse their name with the current model and the dispatch would be lost every tick.
func TestPickWorkerSkipsWorkersOfAnotherModel(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	setSetting(t, s, "quorum", "1")
	setSetting(t, s, "runner_cmd", "copilot")
	setSetting(t, s, "worker_model", "gpt-5-mini")
	var old []string
	for i := 0; i < 2; i++ {
		r, err := spawnAgent(s, "copilot", "gpt-5-mini", "worker")
		if err != nil {
			t.Fatal(err)
		}
		old = append(old, r.str("name"))
	}
	setSetting(t, s, "worker_model", "gpt-6-astra")
	seen := map[string]bool{}
	for i := 0; i < 3; i++ {
		name, err := pickWorker(s, testNow, "", nil)
		if err != nil {
			t.Fatal(err)
		}
		for _, o := range old {
			if name == o {
				t.Fatalf("picked %s, a worker of the old model", name)
			}
		}
		a := agentRow(s.DB, name)
		if a == nil || a.str("model") != "gpt-6-astra" {
			t.Fatalf("picked %s = %v, want a gpt-6-astra worker", name, a)
		}
		seen[name] = true
	}
	// The pool is refilled to quorum+1 = 2 current-model workers; the old ones stay active but idle.
	if len(seen) < 1 {
		t.Fatalf("no current-model worker picked: %v", seen)
	}
	cur := mustOne(s.DB, "SELECT COUNT(*) AS n FROM agents WHERE status = 'active' AND kind = 'worker' AND model = 'gpt-6-astra'").int("n")
	if cur != 2 {
		t.Fatalf("%d current-model workers, want the pool of 2", cur)
	}
	n := mustOne(s.DB, "SELECT COUNT(*) AS n FROM agents WHERE status = 'active' AND kind = 'worker'").int("n")
	if n != 4 {
		t.Fatalf("%d active workers, want 2 old + 2 current", n)
	}
	// With the setting back, the old workers are eligible again and nothing new is spawned.
	setSetting(t, s, "worker_model", "gpt-5-mini")
	name, err := pickWorker(s, testNow, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	if name != old[0] && name != old[1] {
		t.Fatalf("picked %s, want one of the gpt-5-mini workers %v", name, old)
	}
}
