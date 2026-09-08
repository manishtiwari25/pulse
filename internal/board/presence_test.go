package board

import (
	"encoding/json"
	"errors"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func presenceExec(t *testing.T, s *store.Store, name, kind, run, job string, generation int) ExecutionIdentity {
	t.Helper()
	if _, _, err := JoinAgent(s, JoinInput{Name: name, Kind: kind, Session: run}); err != nil {
		t.Fatal(err)
	}
	presenceSQL(t, s, `INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until,generation)
		VALUES(?,'commons','task','presence','leased',?,?,?,?)`, job, testNow, name, "2026-09-07T10:00:00Z", generation)
	presenceSQL(t, s, `INSERT INTO runs(id,job_id,agent,project,started_at,status,executor)
		VALUES(?,?,?,'commons',?,'running','local')`, run, job, name, testNow)
	return ExecutionIdentity{Agent: name, Session: run, RunID: run, JobID: job, Generation: generation, Executor: "local"}
}

func presenceSQL(t *testing.T, s *store.Store, query string, args ...any) {
	t.Helper()
	if _, err := s.DB.Exec(query, args...); err != nil {
		t.Fatal(err)
	}
}

func presenceTime() time.Time {
	at, _ := time.Parse(time.RFC3339Nano, testNow)
	return at
}

func requirePresence(t *testing.T, s *store.Store, agent string, at time.Time, want string) AgentPresence {
	t.Helper()
	rows, err := AgentPresences(s, at.Format(time.RFC3339Nano))
	if err != nil {
		t.Fatal(err)
	}
	p, ok := rows[agent]
	if !ok || p.Presence != want {
		t.Fatalf("%s at %s: %+v, want %s", agent, at.Format(time.RFC3339Nano), p, want)
	}
	return p
}

func recordBeat(t *testing.T, s *store.Store, id ExecutionIdentity, at time.Time) {
	t.Helper()
	if accepted, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: id, At: at.Format(time.RFC3339Nano)}); err != nil || !accepted {
		t.Fatalf("heartbeat: accepted=%v, err=%v", accepted, err)
	}
}

func TestHeartbeatBoundariesReturnAndBoundedMetadata(t *testing.T) {
	_, s := newHome(t)
	id := presenceExec(t, s, "quiet", "worker", "run-quiet", "job-quiet", 7)
	presenceSQL(t, s, `INSERT INTO posts(id,project,area,thread,kind,agent,date,summary,body,created_at)
		VALUES('old-post','commons','agents','presence','finding','quiet',?,'old finding','history',?)`, testNow, testNow)
	before, _ := s.Agent(id.Agent)
	jobBefore, _ := JobByID(s, id.JobID)
	var eventCount int
	if err := s.DB.QueryRow("SELECT count(*) FROM events").Scan(&eventCount); err != nil {
		t.Fatal(err)
	}
	base := presenceTime()
	recordBeat(t, s, id, base)
	for _, delta := range []time.Duration{0, HeartbeatGrace - time.Nanosecond, HeartbeatGrace, HeartbeatGrace + time.Nanosecond} {
		want := "online"
		if delta >= HeartbeatGrace {
			want = "offline"
		}
		p := requirePresence(t, s, id.Agent, base.Add(delta), want)
		if p.LastHeartbeatAt != testNow || p.PresenceSession != id.Session {
			t.Fatalf("heartbeat was lost at boundary: %+v", p)
		}
	}
	for n := 1; n <= 20; n++ {
		recordBeat(t, s, id, base.Add(time.Duration(n)*HeartbeatInterval))
	}
	requirePresence(t, s, id.Agent, base.Add(20*HeartbeatInterval), "online")
	var beats, events, posts, runs int
	for query, dst := range map[string]*int{
		"SELECT count(*) FROM meta WHERE key LIKE 'agent-heartbeat:%'": &beats,
		"SELECT count(*) FROM events":                                  &events,
		"SELECT count(*) FROM posts":                                   &posts,
		"SELECT count(*) FROM runs":                                    &runs,
	} {
		if err := s.DB.QueryRow(query).Scan(dst); err != nil {
			t.Fatal(err)
		}
	}
	after, _ := s.Agent(id.Agent)
	jobAfter, _ := JobByID(s, id.JobID)
	if beats != 1 || events != eventCount || posts != 1 || runs != 1 ||
		!reflect.DeepEqual(before, after) || !reflect.DeepEqual(jobBefore, jobAfter) {
		t.Fatalf("heartbeat changed history, score, identity or lease: beats=%d events=%d/%d posts=%d runs=%d\n%+v\n%+v",
			beats, events, eventCount, posts, runs, before, after)
	}
}

func TestHeartbeatStopIsFinalAndDoesNotExtendLastBeat(t *testing.T) {
	_, s := newHome(t)
	id := presenceExec(t, s, "quiet", "session", "run-quiet", "job-quiet", 1)
	base := presenceTime()
	if ok, err := StopHeartbeat(s, id, base); err != nil || ok {
		t.Fatalf("a failed launch acquired heartbeat evidence: %v %v", ok, err)
	}
	recordBeat(t, s, id, base)
	if ok, err := StopHeartbeat(s, id, base.Add(time.Second)); err != nil || !ok {
		t.Fatalf("stop: %v %v", ok, err)
	}
	p := requirePresence(t, s, id.Agent, base.Add(time.Second), "offline")
	if p.LastHeartbeatAt != testNow {
		t.Fatalf("stop fabricated a beat: %+v", p)
	}
	if ok, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: id, At: base.Add(2 * time.Second).Format(time.RFC3339Nano)}); err != nil || ok {
		t.Fatalf("a stopped execution resumed: %v %v", ok, err)
	}
	presenceSQL(t, s, "UPDATE runs SET status='done' WHERE id=?", id.RunID)
	presenceSQL(t, s, "UPDATE jobs SET status='done' WHERE id=?", id.JobID)
	requirePresence(t, s, id.Agent, base.Add(2*time.Second), "offline")
}

func TestHeartbeatFencesGenerationsAndHistoricalSessions(t *testing.T) {
	_, s := newHome(t)
	old := presenceExec(t, s, "quiet", "worker", "run-old", "job-old", 1)
	base := presenceTime()
	recordBeat(t, s, old, base)
	presenceSQL(t, s, "UPDATE jobs SET generation=2 WHERE id=?", old.JobID)
	if ok, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: old, At: base.Add(time.Second).Format(time.RFC3339Nano)}); err != nil || ok {
		t.Fatalf("stale generation was accepted: %v %v", ok, err)
	}
	requirePresence(t, s, old.Agent, base.Add(time.Second), "offline")
	current := presenceExec(t, s, "quiet", "worker", "run-new", "job-new", 3)
	// Both runs have the same deterministic timestamp: insertion breaks the tie,
	// not posts, joined-session order, or an old session's newer heartbeat.
	requirePresence(t, s, current.Agent, base.Add(time.Second), "unknown")
	recordBeat(t, s, current, base.Add(2*time.Second))
	raw, _, _ := s.Meta(heartbeatPrefix + current.Agent)
	if ok, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: old, At: base.Add(3 * time.Second).Format(time.RFC3339Nano)}); err != nil || ok {
		t.Fatalf("old run replaced current evidence: %v %v", ok, err)
	}
	if ok, err := StopHeartbeat(s, old, base.Add(3*time.Second)); err != nil || ok {
		t.Fatalf("old run stopped current evidence: %v %v", ok, err)
	}
	after, _, _ := s.Meta(heartbeatPrefix + current.Agent)
	if raw != after {
		t.Fatal("historical session changed current metadata")
	}
	if p := requirePresence(t, s, current.Agent, base.Add(3*time.Second), "online"); p.PresenceSession != current.Session {
		t.Fatalf("wrong session: %+v", p)
	}
	presenceSQL(t, s, "UPDATE agents SET status='pruned',pruned_reason='no useful output' WHERE name=?", current.Agent)
	requirePresence(t, s, current.Agent, base.Add(3*time.Second), "pruned")
	if ok, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: current, At: base.Add(4 * time.Second).Format(time.RFC3339Nano)}); err != nil || ok {
		t.Fatalf("pruned identity acquired a heartbeat: %v %v", ok, err)
	}
}

func TestHeartbeatMissingMalformedAndAmbiguousEvidence(t *testing.T) {
	_, s := newHome(t)
	id := presenceExec(t, s, "quiet", "worker", "run-quiet", "job-quiet", 1)
	base := presenceTime()
	good := ExecutionHeartbeat{ExecutionIdentity: id, At: testNow}
	encode := func(h ExecutionHeartbeat) string {
		b, _ := json.Marshal(h)
		return string(b)
	}
	cases := map[string]string{"absent": "", "broken": "{", "null": "null", "empty": "{}", "array": "[]"}
	for name, alter := range map[string]func(*ExecutionHeartbeat){
		"missing time":     func(h *ExecutionHeartbeat) { h.At = "" },
		"malformed time":   func(h *ExecutionHeartbeat) { h.At = "yesterday" },
		"future":           func(h *ExecutionHeartbeat) { h.At = base.Add(time.Hour).Format(time.RFC3339Nano) },
		"before launch":    func(h *ExecutionHeartbeat) { h.At = base.Add(-time.Second).Format(time.RFC3339Nano) },
		"wrong session":    func(h *ExecutionHeartbeat) { h.Session = "old-session" },
		"wrong executor":   func(h *ExecutionHeartbeat) { h.Executor = "elsewhere" },
		"wrong job":        func(h *ExecutionHeartbeat) { h.JobID = "job-old" },
		"wrong agent":      func(h *ExecutionHeartbeat) { h.Agent = "other" },
		"no generation":    func(h *ExecutionHeartbeat) { h.Generation = 0 },
		"malformed stop":   func(h *ExecutionHeartbeat) { h.StoppedAt = "not a time" },
		"future stop":      func(h *ExecutionHeartbeat) { h.StoppedAt = base.Add(time.Hour).Format(time.RFC3339Nano) },
		"stop before beat": func(h *ExecutionHeartbeat) { h.StoppedAt = base.Add(-time.Second).Format(time.RFC3339Nano) },
	} {
		h := good
		alter(&h)
		cases[name] = encode(h)
	}
	for name, raw := range cases {
		t.Run(name, func(t *testing.T) {
			presenceSQL(t, s, "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", heartbeatPrefix+id.Agent, raw)
			p := requirePresence(t, s, id.Agent, base, "unknown")
			if p.LastHeartbeatAt != "" || p.PresenceSession != "" {
				t.Fatalf("invalid evidence advertised as current: %+v", p)
			}
			requirePresence(t, s, id.Agent, base.Add(HeartbeatGrace), "offline")
		})
	}
	presenceSQL(t, s, "UPDATE meta SET value='{}' WHERE key=?", heartbeatPrefix+id.Agent)
	recordBeat(t, s, id, base.Add(time.Second))
	requirePresence(t, s, id.Agent, base.Add(time.Second), "online")
}

func TestHeartbeatRestartIdleLegacyAndPruningStaySeparate(t *testing.T) {
	home, s := newHome(t)
	id := presenceExec(t, s, "quiet", "worker", "run-quiet", "job-quiet", 1)
	base := presenceTime()
	for _, kind := range []string{"worker", "lead", "session", "legacy"} {
		if _, _, err := JoinAgent(s, JoinInput{Name: "idle-" + kind, Kind: kind}); err != nil {
			t.Fatal(err)
		}
		want := "idle"
		if kind == "session" {
			want = "unknown"
		} else if kind == "legacy" {
			want = "legacy"
		}
		requirePresence(t, s, "idle-"+kind, base, want)
	}
	recordBeat(t, s, id, base)
	reopened, err := store.Open(store.Options{Home: home, Now: base.Add(HeartbeatGrace).Format(time.RFC3339Nano)})
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	requirePresence(t, reopened, id.Agent, base.Add(HeartbeatGrace), "offline")
	j, _ := JobByID(s, id.JobID)
	if j.Status != "leased" || j.Generation != 1 || j.LeaseAgent != id.Agent {
		t.Fatalf("missed heartbeats changed lease: %+v", j)
	}
	var runState string
	if err := s.DB.QueryRow("SELECT status FROM runs WHERE id=?", id.RunID).Scan(&runState); err != nil || runState != "running" {
		t.Fatalf("missed heartbeats changed run: %q %v", runState, err)
	}
	recordBeat(t, s, id, base.Add(HeartbeatGrace))
	requirePresence(t, s, id.Agent, base.Add(HeartbeatGrace), "online")
	presenceSQL(t, s, "UPDATE agents SET last_useful_at='2026-08-01T00:00:00Z' WHERE name=?", id.Agent)
	before, err := PruneAgents(s, testNow, 14, "daemon", true)
	if err != nil || len(before) != 0 {
		t.Fatalf("in-flight pruning changed: %+v %v", before, err)
	}
	presenceSQL(t, s, "UPDATE runs SET status='done' WHERE id=?", id.RunID)
	presenceSQL(t, s, "UPDATE jobs SET status='done' WHERE id=?", id.JobID)
	requirePresence(t, s, id.Agent, base.Add(HeartbeatGrace), "idle")
	pruned, err := PruneAgents(s, testNow, 14, "daemon", false)
	if err != nil || len(pruned) != 1 || pruned[0].Name != id.Agent || !strings.Contains(pruned[0].Reason, "no useful output since 2026-08-01") {
		t.Fatalf("a recent heartbeat extended retirement: %+v %v", pruned, err)
	}
	if _, _, err := JoinAgent(s, JoinInput{Name: id.Agent}); err == nil {
		t.Fatal("pruned name became reusable")
	}
	all, err := Scorecards(s, true)
	if err != nil {
		t.Fatal(err)
	}
	for _, a := range all {
		if a.Name == id.Agent && (a.Presence != "pruned" || a.Sessions[0] != id.Session) {
			t.Fatalf("pruned history lost: %+v", a)
		}
	}
}

func TestHeartbeatConcurrentStopFencesLateObservations(t *testing.T) {
	_, s := newHome(t)
	base := presenceTime()
	for n := range 20 {
		suffix := strconv.Itoa(n)
		id := presenceExec(t, s, "quiet", "worker", "run-"+suffix, "job-"+suffix, n+1)
		recordBeat(t, s, id, base)
		start := make(chan struct{})
		stopped, observed := make(chan error, 1), make(chan error, 1)
		go func() {
			<-start
			ok, err := StopHeartbeat(s, id, base.Add(2*time.Second))
			if err == nil && !ok {
				err = errors.New("current execution was not stopped")
			}
			stopped <- err
		}()
		go func() {
			<-start
			_, err := RecordHeartbeat(s, ExecutionHeartbeat{ExecutionIdentity: id, At: base.Add(time.Second).Format(time.RFC3339Nano)})
			observed <- err
		}()
		close(start)
		if err := <-stopped; err != nil {
			t.Fatal(err)
		}
		if err := <-observed; err != nil {
			t.Fatal(err)
		}
		p := requirePresence(t, s, id.Agent, base.Add(2*time.Second), "offline")
		if p.LastHeartbeatAt != testNow && p.LastHeartbeatAt != base.Add(time.Second).Format(time.RFC3339Nano) {
			t.Fatalf("stop changed observation time: %+v", p)
		}
	}
}
