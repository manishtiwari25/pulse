package tally

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/parity"
)

// The snapshot goldens: every read command against /tmp/pulse-parity, Go vs
// the Python oracle, byte for byte after trailing-whitespace trimming.

var snapshotProjects = []string{"amistio-template", "amistio", "project-light", "commons"}

func TestParityRecordsSnapshot(t *testing.T) {
	home := parity.Snapshot(t)
	parity.RequireOracle(t)
	st := openReadOnly(t, home)
	for _, slug := range snapshotProjects {
		for _, area := range []string{"", "runner", "evolution"} {
			args := []string{"--project", slug, "--now", parity.FixedNow}
			if area != "" {
				args = append(args, "--area", area)
			}
			want := parity.Oracle(t, home, append([]string{"records"}, args...)...)
			got, err := runCLI(t, st, "tally", args...)
			if err != nil {
				t.Fatalf("tally %v: %v", args, err)
			}
			parity.Same(t, "records "+slug+" area="+area, want, got)
			// the alias prints the same bytes
			got2, _ := runCLI(t, st, "records", args...)
			if got2 != got {
				t.Errorf("records alias differs from tally for %s", slug)
			}
		}
	}
}

func TestParityRecordsThresholdFlags(t *testing.T) {
	home := parity.Snapshot(t)
	parity.RequireOracle(t)
	st := openReadOnly(t, home)
	args := []string{"--project", "amistio-template", "--now", parity.FixedNow, "--quorum", "3", "--expiry-days", "7", "--stale-days", "2",
		"--amend-quorum", "1", "--amend-majority", "1/2"}
	want := parity.Oracle(t, home, append([]string{"records"}, args...)...)
	got, err := runCLI(t, st, "tally", args...)
	if err != nil {
		t.Fatal(err)
	}
	parity.Same(t, "records with flags", want, got)
}

func TestParityBoardSnapshot(t *testing.T) {
	home := parity.Snapshot(t)
	parity.RequireOracle(t)
	st := openReadOnly(t, home)
	for _, slug := range snapshotProjects {
		for _, area := range []string{"", "runner"} {
			args := []string{"--project", slug, "--now", parity.FixedNow}
			if area != "" {
				args = append(args, "--area", area)
			}
			want := parity.Oracle(t, home, append([]string{"board"}, args...)...)
			got, err := runCLI(t, st, "board", args...)
			if err != nil {
				t.Fatalf("board %v: %v", args, err)
			}
			parity.Same(t, "board "+slug+" area="+area, want, got)
		}
	}
	// with an agent: own cards and voted cards leave the ballot counts
	for _, agent := range []string{"aurora", "fable-scale-09051203", "no-such-agent"} {
		args := []string{"--project", "amistio", "--now", parity.FixedNow, "--agent", agent}
		want := parity.Oracle(t, home, append([]string{"board"}, args...)...)
		got, err := runCLI(t, st, "board", args...)
		if err != nil {
			t.Fatalf("board %v: %v", args, err)
		}
		parity.Same(t, "board --agent "+agent, want, got)
	}
}

func TestParityQueueBallotProposalsSnapshot(t *testing.T) {
	home := parity.Snapshot(t)
	parity.RequireOracle(t)
	st := openReadOnly(t, home)
	for _, slug := range snapshotProjects {
		args := []string{"--project", slug, "--now", parity.FixedNow}
		want := parity.Oracle(t, home, append([]string{"queue"}, args...)...)
		got, err := runCLI(t, st, "queue", args...)
		if err != nil {
			t.Fatalf("queue %s: %v", slug, err)
		}
		parity.Same(t, "queue "+slug, want, got)

		for _, agent := range []string{"", "aurora", "borealis", "helix-0905"} {
			bargs := args
			if agent != "" {
				bargs = append(append([]string{}, args...), "--agent", agent)
			}
			want = parity.Oracle(t, home, append([]string{"ballot"}, bargs...)...)
			got, err = runCLI(t, st, "ballot", bargs...)
			if err != nil {
				t.Fatalf("ballot %v: %v", bargs, err)
			}
			parity.Same(t, "ballot "+slug+" agent="+agent, want, got)
		}

		if slug != "commons" {
			want = parity.Oracle(t, home, "proposals", "--project", slug)
			got, err = runCLI(t, st, "proposals", "--project", slug)
			if err != nil {
				t.Fatalf("proposals %s: %v", slug, err)
			}
			parity.Same(t, "proposals "+slug, want, got)
		}
	}
	want := parity.Oracle(t, home, "proposals")
	got, err := runCLI(t, st, "proposals")
	if err != nil {
		t.Fatal(err)
	}
	parity.Same(t, "proposals (all)", want, got)
}

func TestParityShowHashSnapshot(t *testing.T) {
	home := parity.Snapshot(t)
	parity.RequireOracle(t)
	st := openReadOnly(t, home)
	// a spread of posts: one per kind plus one with an origin_project
	rows, err := st.DB.Query("SELECT id FROM posts WHERE id IN (SELECT MIN(id) FROM posts GROUP BY kind) " +
		"UNION SELECT id FROM posts WHERE origin_project != '' ORDER BY id LIMIT 40")
	if err != nil {
		t.Fatal(err)
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			t.Fatal(err)
		}
		ids = append(ids, id)
	}
	rows.Close()
	if len(ids) == 0 {
		t.Skip("snapshot has no posts")
	}
	for _, id := range ids {
		want := parity.Oracle(t, home, "show", id)
		got, err := runCLI(t, st, "show", id)
		if err != nil {
			t.Fatalf("show %s: %v", id, err)
		}
		parity.Same(t, "show "+id, want, got)
	}
	arows, err := st.DB.Query("SELECT id FROM posts WHERE kind = 'amendment' ORDER BY id")
	if err != nil {
		t.Fatal(err)
	}
	var amendments []string
	for arows.Next() {
		var id string
		_ = arows.Scan(&id)
		amendments = append(amendments, id)
	}
	arows.Close()
	for _, id := range amendments {
		want, code, _ := parity.OracleRun(t, home, "hash", id)
		got, err := runCLI(t, st, "hash", id)
		if code != 0 {
			if err == nil {
				t.Errorf("hash %s: oracle failed (%d) but Go printed %q", id, code, got)
			}
			continue
		}
		if err != nil {
			t.Fatalf("hash %s: %v", id, err)
		}
		parity.Same(t, "hash "+id, want, got)
	}
	// unknown post: both refuse
	_, code, _ := parity.OracleRun(t, home, "show", "nope")
	_, err = runCLI(t, st, "show", "nope")
	if code == 0 || err == nil {
		t.Errorf("show nope: oracle %d, go err %v", code, err)
	}
	if err != nil && !strings.Contains(err.Error(), "No post") {
		t.Errorf("show nope: %v", err)
	}
}
