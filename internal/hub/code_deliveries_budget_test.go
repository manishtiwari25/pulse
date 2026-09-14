package hub

import (
	"context"
	"errors"
	"fmt"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/codedelivery"
	"github.com/manishtiwari25/pulse/internal/tally"
)

func TestCodeDeliveryVotesRespectCancellationWhileWaitingForDatabase(t *testing.T) {
	s := additionsStore(t, false)
	s.DB.SetMaxOpenConns(1)
	conn, err := s.DB.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		_, _, _, err := tally.SuggestionVoteCountsContext(ctx, s, "demo", "suggestion")
		done <- err
	}()
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("cancelled vote query returned %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("vote query ignored request cancellation")
	}
}

func TestCodeDeliveriesDoNotReadUnrelatedPostBodies(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	_, err := s.DB.Exec(`INSERT INTO posts(id, project, kind, area, thread, date, agent, summary, body, created_at)
		VALUES ('source', 'demo', 'suggestion', 'code', 'delivery', '2026-09-01', 'author', 'Source work', '', '2026-09-01');
		INSERT INTO posts(id, project, kind, area, thread, date, agent, on_post, vote, summary, body, created_at)
		VALUES ('vote', 'demo', 'vote', 'code', 'delivery', '2026-09-02', 'voter', 'source', 'up', 'Useful', '', '2026-09-02')`)
	if err != nil {
		t.Fatal(err)
	}
	_, err = s.DB.Exec(`WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 2000)
		INSERT INTO posts(id, project, kind, area, thread, date, agent, summary, body, created_at)
		SELECT 'unrelated-' || i, 'demo', 'finding', 'other', 'other', '2026-09-01', 'observer',
		'Unrelated', replace(hex(zeroblob(4096)), '0', 'x'), '2026-09-01' FROM n`)
	if err != nil {
		t.Fatal(err)
	}
	for i := 1; i <= 3; i++ {
		sha := fmt.Sprintf("%040x", i)
		seedCodeDeliveryEvent(t, s, "demo", sha, codedelivery.Evidence{
			Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
			Before: strings.Repeat("f", 40), After: sha, FilesChanged: 1, CodeFiles: 1, CodePaths: []string{"src/source.go"},
		}, codedelivery.WorkSnapshot{Job: &codedelivery.JobSnapshot{
			ID: fmt.Sprintf("job-%d", i), Title: "Source delivery", Kind: "task", Status: "done", Source: "suggestion:source",
		}})
	}
	runtime.GC()
	var before, after runtime.MemStats
	runtime.ReadMemStats(&before)
	report, err := codedelivery.Recent(context.Background(), s, "demo", 100, 0)
	runtime.ReadMemStats(&after)
	if err != nil {
		t.Fatal(err)
	}
	if report.Summary.CodeDeliveries != 3 {
		t.Fatalf("delivery count: %+v", report.Summary)
	}
	for _, row := range report.Deliveries {
		if row.Suggestion == nil || row.Suggestion.Votes == nil || row.Suggestion.Votes.Up != 1 {
			t.Fatalf("missing vote attribution: %+v", row.Suggestion)
		}
	}
	if allocated := after.TotalAlloc - before.TotalAlloc; allocated > 8<<20 {
		t.Fatalf("small report allocated %d bytes with unrelated post bodies", allocated)
	}
}
