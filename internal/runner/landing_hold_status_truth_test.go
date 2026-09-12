package runner

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func TestLandingHoldStatusTruth(t *testing.T) {
	for _, n := range []int{0, 1, 208} {
		t.Run(fmt.Sprint(n), func(t *testing.T) {
			s := newStore(t)
			project := statusTruthProject(t, s)
			setSetting(t, s, "landing", "agents")
			setSetting(t, s, "stop", "1")
			// STOP keeps this reporting test out of Git. Duplicate metadata is
			// deliberately not evidence of distinct refs or verified patches.
			for i := 0; i < n; i++ {
				if _, err := s.DB.Exec(`INSERT INTO jobs(id, project, kind, title, status, branch, created_at, finished_at)
					VALUES(?, 'proj', 'task', 'Pending work', 'done', 'pulse/test/same-ref', ?, ?)`,
					fmt.Sprintf("J-%03d", i), testNow, testNow); err != nil {
					t.Fatal(err)
				}
			}
			lines, held := tendLandings(s, []row{project}, "daemon", testNow, nil)
			if n == 0 {
				if len(lines) != 0 || len(held) != 0 {
					t.Fatalf("no jobs must mean no hold: %q %v", lines, held)
				}
				return
			}
			noun := "jobs"
			if n == 1 {
				noun = "job"
			}
			want := fmt.Sprintf("%d pending landing %s; stopped: %s", n, noun, runnerStopped(s))
			if len(lines) != 1 || lines[0] != "landing held proj: "+want || held["proj"] != want {
				t.Errorf("hold must count job rows, not branches: lines=%q held=%v, want %q", lines, held, want)
			}
			var event struct {
				Waiting int    `json:"waiting"`
				State   string `json:"state"`
				Note    string `json:"note"`
			}
			var detail string
			if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = 'land.held' AND subject = 'proj'").Scan(&detail); err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal([]byte(detail), &event); err != nil {
				t.Fatal(err)
			}
			if event.Waiting != n || event.State != "stopped" || event.Note != want {
				t.Errorf("hold event = %+v, want %d pending landing jobs and note %q", event, n, want)
			}
			raw, exists, err := s.Meta("landing")
			if err != nil || !exists || !strings.Contains(raw, want) {
				t.Errorf("hold projection = %q, exists=%v err=%v, want note %q", raw, exists, err, want)
			}
			lines, again := tendLandings(s, []row{project}, "daemon", testNow, held)
			if len(lines) != 0 || again["proj"] != held["proj"] ||
				count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held'") != 1 {
				t.Fatalf("unchanged hold must remain quiet: %q %v", lines, again)
			}
			if got := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'done' AND branch = 'pulse/test/same-ref'"); got != n {
				t.Fatalf("reporting changed pending metadata: %d rows, want %d", got, n)
			}
		})
	}
}
