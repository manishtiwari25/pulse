package runner

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func statusTruthProject(t *testing.T, s *store.Store) row {
	t.Helper()
	if _, err := s.DB.Exec("INSERT INTO projects(slug, name, path, added_at) VALUES('proj', 'proj', ?, ?)", t.TempDir(), testNow); err != nil {
		t.Fatal(err)
	}
	return projectBySlug(s.DB, "proj")
}

func TestReflectIdlePartStatusTruth(t *testing.T) {
	for _, source := range []string{"reflect:proj:" + testNow[:10], "onboard-reflect:proj"} {
		for _, status := range []string{"scheduled", "queued", "leased", "running", "blocked", "done", "failed", "cancelled"} {
			t.Run(source+"/"+status, func(t *testing.T) {
				s := newStore(t)
				statusTruthProject(t, s)
				if _, err := s.DB.Exec(`INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-reflect', 'proj', 'reflect', 'Daily reflect', ?, ?, ?)`, status, source, testNow); err != nil {
					t.Fatal(err)
				}
				if got, want := reflectIdlePart(s, "proj", testNow), "reflect "+status+" today"; got != want {
					t.Errorf("idle diagnostic = %q, want %q", got, want)
				}
				planned, err := planJobs(s, "proj", testNow, "daemon")
				if err != nil || len(planned) != 0 {
					t.Fatalf("an existing daily attempt must not be scheduled again: %v (%v)", planned, err)
				}
				if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND kind = 'reflect'"); n != 1 {
					t.Fatalf("daily attempts = %d, want 1", n)
				}
			})
		}
	}
}

func TestReflectIdlePartStatusTruthDue(t *testing.T) {
	for _, tc := range []struct {
		name, project, kind, source, created string
	}{
		{name: "no reflect"},
		{"previous daily attempt", "proj", "reflect", "reflect:proj:2026-09-05", "2026-09-05T10:00:00Z"},
		{"previous onboarding attempt", "proj", "reflect", "onboard-reflect:proj", "2026-09-05T10:00:00Z"},
		{"another project", "other", "reflect", "reflect:proj:" + testNow[:10], testNow},
		{"another kind", "proj", "task", "reflect:proj:" + testNow[:10], testNow},
		{"unrelated source", "proj", "reflect", "manual-reflect", testNow},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := newStore(t)
			statusTruthProject(t, s)
			if tc.source != "" {
				if _, err := s.DB.Exec(`INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-other', ?, ?, 'Earlier work', 'done', ?, ?)`, tc.project, tc.kind, tc.source, tc.created); err != nil {
					t.Fatal(err)
				}
			}
			if got := reflectIdlePart(s, "proj", testNow); got != "reflect due" {
				t.Fatalf("idle diagnostic = %q, want reflect due", got)
			}
			planned, err := planJobs(s, "proj", testNow, "daemon")
			if err != nil || len(planned) != 1 || planned[0].str("kind") != "reflect" {
				t.Fatalf("a due reflect must still be scheduled: %v (%v)", planned, err)
			}
		})
	}
}

func TestReflectIdlePartStatusTruthLatestAttempt(t *testing.T) {
	for _, created := range []string{testNow, "2026-09-06T09:00:00Z"} {
		t.Run(created, func(t *testing.T) {
			s := newStore(t)
			if _, err := s.DB.Exec(`INSERT INTO jobs(id, project, kind, title, status, source, created_at) VALUES
				('J-first', 'proj', 'reflect', 'First attempt', 'done', 'onboard-reflect:proj', ?),
				('J-latest', 'proj', 'reflect', 'Latest attempt', 'failed', ?, ?)`,
				created, "reflect:proj:"+testNow[:10], testNow); err != nil {
				t.Fatal(err)
			}
			if got := reflectIdlePart(s, "proj", testNow); got != "reflect failed today" {
				t.Fatalf("latest attempt diagnostic = %q, want reflect failed today", got)
			}
		})
	}
}
