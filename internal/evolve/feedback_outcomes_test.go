package evolve

import (
	"strings"
	"testing"
)

func TestFinalizedFeedbackSupportsBothClassesAndOutcomes(t *testing.T) {
	for _, class := range []string{"operating-layer", "product"} {
		for _, outcome := range []string{"kept", "discarded"} {
			t.Run(class+"/"+outcome, func(t *testing.T) {
				s := cycleReady(t)
				file := "README.md"
				if class == "product" {
					file = "value.txt"
				}
				votedSuggestion(t, s, "candidate", class, file, 2)
				planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
				if err != nil || !planned.Created {
					t.Fatalf("planned: %+v (%v)", planned, err)
				}
				if err := fakeArchiveOutcome(s, planned.JobID, outcome == "kept"); err != nil {
					t.Fatal(err)
				}
				archive, err := archivedForJob(s, "demo", planned.JobID)
				if err != nil || archive == nil {
					t.Fatalf("archive: %+v (%v)", archive, err)
				}
				action := "land.dropped"
				if archive.Kept {
					action = "land.landed"
				}
				if err := fakeFinalizeArchive(s, planned.JobID, action, archive); err != nil {
					t.Fatal(err)
				}
				for i := range 2 {
					feedback, err := PlanFeedback(s, "demo", "tester")
					if err != nil || (feedback != nil) != (i == 0) {
						t.Fatalf("feedback reconciliation %d: %+v (%v)", i, feedback, err)
					}
					if feedback != nil && !strings.Contains(feedback.Brief, "Outcome: "+outcome) {
						t.Fatalf("feedback lost its measured disposition: %s", feedback.Brief)
					}
				}
			})
		}
	}
}
