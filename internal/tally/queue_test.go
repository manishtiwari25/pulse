package tally

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestWorkWanted(t *testing.T) {
	f := newFixture(t, true)
	// a second project with a handoff nobody claimed, an old open question and an unknown-kind post
	if _, err := f.st.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES ('other', 'Other', ?, ?)", f.home+"/other", fixtureNow); err != nil {
		t.Fatal(err)
	}
	g := &fixture{st: f.st, ids: map[string]string{}}
	post := func(p fpost) string {
		p.project = "other"
		return g.post(t, p)
	}
	h := post(fpost{kind: "handoff", area: "governance", thread: "left", agent: "r/h", date: "2026-09-01T00:00:00Z", summary: "finish the export", session: "s"})
	post(fpost{kind: "handoff", area: "governance", thread: "taken", agent: "r/h", date: "2026-09-01T01:00:00Z", summary: "taken over", session: "s"})
	post(fpost{kind: "claim", area: "governance", thread: "taken", agent: "r/c", date: "2026-09-02T00:00:00Z", summary: "on it", session: "s", expires: "2026-09-30T00:00:00Z"})
	post(fpost{kind: "", area: "governance", thread: "odd", agent: "r/w", date: "2026-09-02T00:00:00Z", summary: "no kind", session: "s"})
	post(fpost{kind: "question", area: "governance", thread: "why", agent: "r/q", date: "2026-08-15T00:00:00Z", summary: "why is the export slow?", session: "s"})

	th := Thresholds{Now: fixtureNow}
	q, err := WorkWanted(f.st, nil, "", th)
	if err != nil {
		t.Fatal(err)
	}
	if len(q.Projects) != 2 || q.Projects[0].Slug != "fixture" || !q.Projects[0].Brain || q.Projects[1].Slug != "other" {
		t.Fatalf("projects: %+v", q.Projects)
	}
	if len(q.ForTheHuman) != 1 || q.ForTheHuman[0].Project != "fixture" {
		t.Errorf("for the human: %+v", q.ForTheHuman)
	}
	var kinds []string
	for _, it := range q.Work {
		kinds = append(kinds, it.Kind+"@"+it.Project)
	}
	// rank order: handoff, eligible, questions (oldest first), stale claim, hygiene
	want := "handoff@other,eligible@fixture,question@fixture,question@other,stale-claim@fixture,hygiene@other"
	if got := strings.Join(kinds, ","); got != want {
		t.Errorf("order:\n got %s\nwant %s", got, want)
	}
	if q.Work[0].ID != h || q.Work[0].AgeDays != 9 || q.Work[1].ID != f.ids["eligible"] || q.Work[1].Score != 1 {
		t.Errorf("items: %+v %+v", q.Work[0], q.Work[1])
	}
	// questions: fixture's (July) is older than other's (August), so it comes first
	if q.Work[2].AgeDays <= q.Work[3].AgeDays || q.Work[3].Thread != "governance/why" {
		t.Errorf("question order: %+v %+v", q.Work[2], q.Work[3])
	}
	last := q.Work[len(q.Work)-1]
	if last.Kind != WorkHygiene || !strings.Contains(last.Summary, "1 post(s) without a kind") || last.AgeDays != -1 {
		t.Errorf("hygiene: %+v", last)
	}

	// with an agent: ballot items appear at rank 4 and the agent's own cards do not
	q, err = WorkWanted(f.st, nil, "runner-1/model-1", th)
	if err != nil {
		t.Fatal(err)
	}
	ballots := 0
	for _, it := range q.Work {
		if it.Kind == WorkBallot {
			ballots++
			if it.ID == f.ids["amendment"] {
				t.Error("voted amendment on the ballot")
			}
		}
	}
	if ballots != 3 {
		t.Errorf("ballot items %d", ballots)
	}

	// STOP hides the loop and adds a hygiene line
	if _, err := f.st.DB.Exec("UPDATE settings SET value = '1' WHERE name = 'stop'"); err != nil {
		t.Fatal(err)
	}
	q, err = WorkWanted(f.st, []string{"fixture"}, "", th)
	if err != nil {
		t.Fatal(err)
	}
	for _, it := range q.Work {
		if it.Kind == WorkEligible {
			t.Error("eligible shown under STOP")
		}
	}
	if !q.Projects[0].Stop || !strings.Contains(WorkText(q, 0), "STOP is on") {
		t.Errorf("stop not reported:\n%s", WorkText(q, 0))
	}
	if _, err := f.st.DB.Exec("UPDATE settings SET value = '0' WHERE name = 'stop'"); err != nil {
		t.Fatal(err)
	}

	// the CLI: no --project = every project; --limit; --json; --project = the Python queue
	out, err := runCLI(t, f.st, "queue", "--now", fixtureNow, "--limit", "2")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(out, "QUEUE - work wanted across 2 projects - "+fixtureNow) || !strings.Contains(out, "showing 2") || !strings.Contains(out, "fixture *") {
		t.Errorf("queue text:\n%s", out)
	}
	out, err = runCLI(t, f.st, "queue", "--now", fixtureNow, "--json")
	if err != nil {
		t.Fatal(err)
	}
	var parsed Queue
	if err := json.Unmarshal([]byte(out), &parsed); err != nil || len(parsed.Work) != 6 || parsed.Work[0].Kind != "handoff" {
		t.Errorf("queue json: %v %d", err, len(parsed.Work))
	}
	out, err = runCLI(t, f.st, "queue", "--now", fixtureNow, "--project", "fixture")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(out, "QUEUE -") || !strings.Contains(out, "eligible") {
		t.Errorf("single-project queue changed:\n%s", out)
	}
	// an empty store: no projects, nothing wanted
	q, err = WorkWanted(f.st, []string{}, "", th)
	if err != nil || len(q.Work) != 0 || !strings.Contains(WorkText(q, 0), "none - nothing on any board") {
		t.Errorf("empty: %v\n%s", err, WorkText(q, 0))
	}
}
