package evolve

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestStatusDistinguishesReadinessFromProgress(t *testing.T) {
	for _, scenario := range []string{"no-eligible", "product-gate", "unsupported-card", "queued", "stop", "cap"} {
		t.Run(scenario, func(t *testing.T) {
			s := cycleReady(t)
			wantState, wantReason := "waiting", "no eligible"
			switch scenario {
			case "product-gate":
				if _, err := s.PutRecord(store.Record{ID: "EVO-evaluators-demo", Project: "demo", Kind: "evolution", Slug: "evaluators"},
					nil, "Product scope: off\n", "human", "fixture"); err != nil {
					t.Fatal(err)
				}
				votedSuggestion(t, s, "product", "product", "value.txt", 2)
				wantReason = "operating layer"
			case "unsupported-card":
				votedPostWithVotes(t, s, board.PostInput{
					Project: "demo", Area: "evolution", Thread: "incomplete", Kind: "suggestion",
					Agent: "proposer", Summary: "Not a one-variable card", Body: "Class: operating-layer\n",
				}, "up", "up")
				wantState, wantReason = "waiting", "no experiment card"
			case "queued":
				votedSuggestion(t, s, "ready", "operating-layer", "README.md", 2)
				if _, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"}); err != nil {
					t.Fatal(err)
				}
				wantState, wantReason = "queued", "in flight"
			case "stop":
				if err := s.SetSetting("human", "stop", "1"); err != nil {
					t.Fatal(err)
				}
				wantState, wantReason = "gated", "STOP"
			case "cap":
				if err := s.SetSetting("human", "evolve.limits", "cycles=0"); err != nil {
					t.Fatal(err)
				}
				wantState, wantReason = "gated", "budget"
			}
			beforeJobs, beforePosts := countRows(t, s, "jobs"), countRows(t, s, "posts")
			got, err := Status(s, "demo")
			if err != nil {
				t.Fatal(err)
			}
			data, err := json.Marshal(got)
			if err != nil {
				t.Fatal(err)
			}
			var wire struct {
				Planning struct {
					State  string `json:"state"`
					Reason string `json:"reason"`
				} `json:"planning"`
			}
			if err := json.Unmarshal(data, &wire); err != nil {
				t.Fatal(err)
			}
			if wire.Planning.State != wantState || !strings.Contains(wire.Planning.Reason, wantReason) {
				t.Fatalf("planning = %+v, want %s / %s; Level %s is readiness, not cycle progress",
					wire.Planning, wantState, wantReason, got.Level)
			}
			shown := false
			for _, row := range statusRows(got) {
				if row[0] == "Experiment planning" && strings.Contains(row[1], wantReason) {
					shown = true
				}
			}
			if !shown {
				t.Fatal("the CLI status table omitted the planning reason")
			}
			if countRows(t, s, "jobs") != beforeJobs || countRows(t, s, "posts") != beforePosts {
				t.Fatal("status changed the plan or board")
			}
			if scenario == "unsupported-card" {
				// ADR-044: the planner queues a card job for the card-less pick and status names it; no owner step.
				res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
				if err != nil || !res.CardCreated || res.CardJob == "" {
					t.Fatalf("plan = %+v (%v); want a card job", res, err)
				}
				after, err := Status(s, "demo")
				if err != nil || after.Planning.State != "carding" || after.Planning.Job != res.CardJob ||
					!strings.Contains(after.Planning.Reason, "card job "+res.CardJob) || strings.Contains(after.Planning.Reason, "owner:") {
					t.Fatalf("planning after the card job = %+v (%v)", after.Planning, err)
				}
			}
		})
	}
}

func TestReflectDoesNotRecommendUnreadyCycle(t *testing.T) {
	s := cycleReady(t)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	for _, ready := range []bool{false, true} {
		if ready {
			votedSuggestion(t, s, "ready", "operating-layer", "README.md", 2)
		}
		r, err := Reflect(ctx, s, "demo", "", "tester", "")
		if err != nil {
			t.Fatal(err)
		}
		hasCycle := strings.Contains(strings.Join(r.NextCommands, "\n"), "pulse evolve cycle")
		if r.Level != "D" || hasCycle != ready {
			t.Fatalf("ready=%v: level %s commands %v", ready, r.Level, r.NextCommands)
		}
	}
}
