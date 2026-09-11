package inspect

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/runner"
)

func TestDaemonIntervalFallback(t *testing.T) {
	for _, tc := range []struct {
		name string
		raw  string
		want int
	}{
		{"no-heartbeat", "", runner.DefaultDaemonIntervalSeconds},
		{"unreadable", "not json", runner.DefaultDaemonIntervalSeconds},
		{"missing-interval", "{}", runner.DefaultDaemonIntervalSeconds},
		{"null-interval", `{"interval":null}`, runner.DefaultDaemonIntervalSeconds},
		{"zero-interval", `{"interval":0}`, runner.DefaultDaemonIntervalSeconds},
		{"custom-interval", `{"interval":7}`, 7},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := temporaryStore(t)
			if tc.raw != "" {
				execSQL(t, s, "INSERT INTO meta (key, value) VALUES ('daemon', ?)", tc.raw)
			}
			got, err := daemonIntervalSeconds(s)
			if err != nil || got != tc.want {
				t.Fatalf("interval = %d, %v; want %d", got, err, tc.want)
			}
		})
	}
}

func TestDaemonHeartbeatDefaultInterval(t *testing.T) {
	for _, tc := range []struct {
		state string
		extra int
	}{
		{"alive", 0},
		{"silent", 1},
	} {
		t.Run(tc.state, func(t *testing.T) {
			s := temporaryStore(t)
			now, err := time.Parse(time.RFC3339, s.Now())
			if err != nil {
				t.Fatal(err)
			}
			age := time.Duration(2*runner.DefaultDaemonIntervalSeconds+60+tc.extra) * time.Second
			beat, err := json.Marshal(daemonBeat{At: now.Add(-age).Format(time.RFC3339)})
			if err != nil {
				t.Fatal(err)
			}
			execSQL(t, s, "INSERT INTO meta (key, value) VALUES ('daemon', ?)", string(beat))
			got, err := DaemonStatus(s)
			if err != nil || !strings.HasPrefix(got, tc.state+" ") {
				t.Fatalf("daemon status = %q, %v; want %s", got, err, tc.state)
			}
		})
	}
}

func TestStatusRunnerCountsAndCapacity(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO projects(slug,name,path,status,brain,added_at) VALUES
		('active','Active','/missing-active','active',0,?),
		('inactive','Inactive','/missing-inactive','inactive',0,?)`, parityNow, parityNow)
	for _, run := range []struct{ id, mode, status, at string }{
		{"today", "real", "done", parityNow},
		{"dry", "dry-run", "done", parityNow},
		{"error", "real", "error", parityNow},
		{"yesterday", "real", "done", "2026-09-05T23:59:59Z"},
		{"running", "real", "running", "2026-09-06T18:50:00Z"},
		{"stale-running", "real", "running", "2026-09-06T01:00:00Z"},
	} {
		execSQL(t, s, `INSERT INTO runs(id,job_id,agent,project,started_at,status,mode)
			VALUES (?,'job','worker','active',?,?,?)`, run.id, run.at, run.status, run.mode)
	}
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('capacity','{"queued":3,"slots":2,"inflight":2,"starved_since":"2026-09-06T18:29:01Z"}'),
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":""}')`)
	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(report.Projects) != 1 || report.Projects[0].Slug != "active" || report.RunsToday != 3 || report.DailyCap != 2 {
		t.Fatalf("wrong status counts: %+v", report)
	}
	if report.Executors != "local active (1/1 busy)" {
		t.Fatalf("stale running row counted as busy: %q", report.Executors)
	}
	if report.Daemon != "alive pid 123, last loop 60s ago, 4 loop(s), interval 30s" {
		t.Fatalf("wrong heartbeat: %q", report.Daemon)
	}
	if report.Capacity != "3 job(s) queued, 2 slot(s), 2 in flight; every slot busy for 30 min - another machine would help (pulse pair; pulse record show WF-executors)" {
		t.Fatalf("wrong capacity: %q", report.Capacity)
	}
	// The daemon's login gate travels in the same heartbeat (`held`).
	execSQL(t, s, `UPDATE meta SET value = '{"queued":3,"slots":0,"inflight":0,"starved_since":"","held":"workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)"}' WHERE key = 'capacity'`)
	if held, err := CapacityStatus(s); err != nil || held != "3 job(s) queued, 0 slot(s), 0 in flight; workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)" {
		t.Fatalf("held capacity: %q %v", held, err)
	}
	if hold, err := CapacityHold(s); err != nil || hold != "workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)" {
		t.Fatalf("capacity hold: %q %v", hold, err)
	}
	execSQL(t, s, `UPDATE meta SET value = '{"queued":3,"slots":2,"inflight":2,"starved_since":"2026-09-06T18:29:01Z"}' WHERE key = 'capacity'`)
	if hold, err := CapacityHold(s); err != nil || hold != "" {
		t.Fatalf("capacity hold without held: %q %v", hold, err)
	}
	if !strings.Contains(report.Harness, "native \u00b7 GitHub Copilot \u00b7") || !strings.Contains(report.Text(), "\nharness    "+report.Harness+"\n") {
		t.Fatalf("harness line missing: %q\n%s", report.Harness, report.Text())
	}
	assertParity(t, invoke(t, s, false, "", "status"), oracle(t, s.Home, true, "status"))
	asJSON := invoke(t, s, true, "", "status")
	var decoded StatusReport
	if err := json.Unmarshal([]byte(asJSON.out), &decoded); err != nil || decoded.Text() != report.Text() {
		t.Fatalf("status JSON lost report information: %+v %v", asJSON, err)
	}
}

func TestDaemonHeartbeatStates(t *testing.T) {
	s := temporaryStore(t)
	for _, tc := range []struct{ raw, want string }{
		{"not json", "unreadable heartbeat"},
		{`{"pid":42,"at":"2026-09-06T18:00:00Z","note":"exited"}`, "exited 2026-09-06T18:00:00Z (pid 42)"},
		{`{"pid":42,"at":"2026-09-06T18:49:00Z","interval":300,"loops":2}`, "alive pid 42, last loop 660s ago, 2 loop(s), interval 300s"},
		{`{"pid":42,"at":"2026-09-06T18:48:59Z","interval":300,"loops":2}`, "silent pid 42, last loop 661s ago, 2 loop(s), interval 300s"},
		{`{"at":"2026-09-06T19:00:00Z"}`, "alive pid None, last loop 0s ago, 0 loop(s), interval Nones"},
	} {
		execSQL(t, s, `INSERT INTO meta(key,value) VALUES ('daemon',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value`, tc.raw)
		got, err := DaemonStatus(s)
		if err != nil || got != tc.want {
			t.Fatalf("%s: got %q, %v; want %q", tc.raw, got, err, tc.want)
		}
		assertParity(t, invoke(t, s, false, "", "status"), oracle(t, s.Home, true, "status"))
	}
}

func TestStatusPrintsDroppedBusEvents(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":"","bus":{"subscribers":2,"dropped":7,"deduped":3}}')`)
	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	if report.Bus.Subscribers != 2 || report.Bus.Dropped != 7 || report.Bus.Deduped != 3 {
		t.Fatalf("bus status: %+v", report.Bus)
	}
	if !strings.Contains(report.Text(), "events     7 event(s) dropped for slow subscribers since the daemon started\n") {
		t.Fatalf("dropped line missing:\n%s", report.Text())
	}
}

func TestPlannerNotesTextAndStale(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":""}'),
		('planner','{"fresh":{"at":"2026-09-06T18:59:31Z","note":"no suggestions; reflect due"},"old":{"at":"2026-09-06T18:58:59Z","note":"1 eligible suggestions, 0 plannable (1 net votes <= 0)"}}')`)

	notes, err := PlannerNotes(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 2 || notes[0].Project != "fresh" || notes[0].Stale || notes[1].Project != "old" || !notes[1].Stale {
		t.Fatalf("planner notes: %+v", notes)
	}
	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	text := report.Text()
	if !strings.Contains(text, "planner    fresh: no suggestions; reflect due\n") {
		t.Fatalf("fresh planner line missing:\n%s", text)
	}
	if !strings.Contains(text, "planner    old: 1 eligible suggestions, 0 plannable (1 net votes <= 0) (stale)\n") {
		t.Fatalf("stale planner line missing:\n%s", text)
	}
}

// The hygiene line: absent until a sweep ran, then what the last pass removed, freed, held and could not do.
func TestHygieneStatusLine(t *testing.T) {
	s := temporaryStore(t)
	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	if report.Hygiene != "" || strings.Contains(report.Text(), "hygiene") {
		t.Fatalf("hygiene line before any sweep: %q", report.Hygiene)
	}
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('hygiene','{"at":"2026-09-06T18:59:00Z","removed":{"clone":2,"scratch":5},"held":{"branch":20,"clone":8},"bytes":1503238553,"errors":1}')`)
	report, err = Status(s)
	if err != nil {
		t.Fatal(err)
	}
	want := "last sweep 2026-09-06T18:59:00Z: removed clone 2, scratch 5 (1.4 GB freed); held branch 20, clone 8; 1 error(s) - pulse sweep --dry-run shows them"
	if report.Hygiene != want {
		t.Fatalf("hygiene = %q\nwant      %q", report.Hygiene, want)
	}
	if !strings.Contains(report.Text(), "hygiene    "+want+"\n") {
		t.Fatalf("hygiene line missing:\n%s", report.Text())
	}
	execSQL(t, s, `UPDATE meta SET value = '{"at":"2026-09-06T19:09:00Z","removed":{},"held":{},"bytes":0,"errors":0}' WHERE key = 'hygiene'`)
	line, err := HygieneStatus(s)
	if err != nil || line != "last sweep 2026-09-06T19:09:00Z: nothing to remove" {
		t.Fatalf("quiet pass: %q %v", line, err)
	}
}

func TestUnfitMarksAreReadOnly(t *testing.T) {
	s := temporaryStore(t)
	raw := `{"local":{"fresh":{"tool":"tool:node@22","at":"2026-09-05T19:00:01Z"},"expired":{"tool":"uv","at":"2026-09-05T19:00:00Z"},"services":{"tool":"services","at":"2026-09-06T18:00:00Z"}}}`
	execSQL(t, s, "INSERT INTO meta(key,value) VALUES ('unfit',?)", raw)
	before := eventCount(t, s)
	got, err := ExecutorsLine(s)
	if err != nil || got != "local active (0/1 busy); lacks node@22 for fresh, needs services for services" {
		t.Fatalf("unfit report: %q, %v", got, err)
	}
	after, _, err := s.Meta("unfit")
	if err != nil || after != raw || eventCount(t, s) != before {
		t.Fatal("status must not prune or rewrite expired unfit marks")
	}
}

func TestDailyCapSpellings(t *testing.T) {
	s := temporaryStore(t)
	for _, value := range []string{"none", "off", "unlimited", "0", "3", "invalid-old-value", "-2"} {
		execSQL(t, s, "UPDATE settings SET value=? WHERE name='jobs_per_day'", value)
		got := invoke(t, s, false, "", "status")
		want := oracle(t, s.Home, true, "status")
		assertParity(t, got, want)
		if value == "3" && !strings.Contains(got.out, "runs today 0 of 3 (jobs_per_day)") {
			t.Fatalf("cap missing: %+v", got)
		}
	}
}

func TestListOutputsPreserveFixedWidths(t *testing.T) {
	s := temporaryStore(t)
	for _, row := range []struct{ id, status, project, title, result string }{
		{"short", "queued", "one", "short", ""},
		{"long", "leased", "a-project-slug-longer-than-the-column", strings.Repeat("\u754c", 65), strings.Repeat("result", 20)},
		{"done", "done", "one", "finished", "success"},
		{"blocked", "blocked", "two", "blocked", ""},
	} {
		execSQL(t, s, `INSERT INTO jobs(id,project,kind,title,status,created_at,result,lease_agent,lease_until)
			VALUES (?,?,'task',?,?,?,?,'worker','2026-09-07T19:00:00Z')`, row.id, row.project, row.title, row.status, parityNow, row.result)
	}
	for _, args := range [][]string{nil, {"--all"}} {
		assertParity(t, invoke(t, s, false, "", "jobs", args...), oracle(t, s.Home, true, append([]string{"jobs"}, args...)...))
	}
}
