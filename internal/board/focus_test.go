package board

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func execFocusSQL(t *testing.T, s *store.Store, q string, args ...any) {
	t.Helper()
	if _, err := s.DB.Exec(q, args...); err != nil {
		t.Fatal(err)
	}
}

func TestAgentFocusLeasedJobAndSourceThread(t *testing.T) {
	_, s := newHome(t)
	addFocusProject(t, s, "proj")
	joinFocusAgent(t, s, "aurora")
	joinFocusAgent(t, s, "borealis")
	handoff := postFocus(t, s, "p1", "proj", "handoff", "area", "thread", "borealis", "handoff work", "")
	execFocusSQL(t, s, `INSERT INTO jobs(id,project,kind,title,source,status,created_at,lease_agent,lease_until)
		VALUES('job-1','proj','improve','Continue work',?,'leased',?,'aurora','2026-09-06T12:00:00Z')`, "handoff:"+handoff.ID, testNow)
	execFocusSQL(t, s, `INSERT INTO events(at,actor,action,subject,detail) VALUES('2026-09-06T10:05:00Z','aurora','job.lease','job-1','{}')`)

	focuses, err := AgentFocuses(s, testNow)
	if err != nil {
		t.Fatal(err)
	}
	f := focuses["aurora"]
	if f.JobID != "job-1" || f.JobKind != "improve" || f.JobTitle != "Continue work" || f.Project != "proj" ||
		f.Source != "handoff:"+handoff.ID || f.Thread != "area/thread" || f.Since != "2026-09-06T10:05:00Z" {
		t.Fatalf("leased focus: %+v", f)
	}
}

func TestAgentFocusRunningRunExecutorAndClaims(t *testing.T) {
	_, s := newHome(t)
	addFocusProject(t, s, "proj")
	joinFocusAgent(t, s, "aurora")
	execFocusSQL(t, s, `INSERT INTO jobs(id,project,kind,title,source,status,created_at,lease_agent,lease_until)
		VALUES('job-1','proj','task','Do it','','leased',?,'aurora','2026-09-06T12:00:00Z')`, testNow)
	execFocusSQL(t, s, `INSERT INTO runs(id,job_id,agent,project,started_at,status,executor)
		VALUES('run-1','job-1','aurora','proj','2026-09-06T10:07:00Z','running','ssh-one')`)
	postFocus(t, s, "claim-1", "proj", "claim", "claims", "active", "aurora", "mine", "2026-09-06T11:00:00Z")
	postFocus(t, s, "claim-old", "proj", "claim", "claims", "old", "aurora", "old", "2026-09-06T09:00:00Z")

	focuses, err := AgentFocuses(s, testNow)
	if err != nil {
		t.Fatal(err)
	}
	f := focuses["aurora"]
	if f.RunID != "run-1" || f.Executor != "ssh-one" || f.Since != "2026-09-06T10:07:00Z" {
		t.Fatalf("running focus: %+v", f)
	}
	if len(f.Claims) != 1 || f.Claims[0].PostID != "claim-1" || f.Claims[0].Thread != "claims/active" {
		t.Fatalf("claims: %+v", f.Claims)
	}
}

func TestAgentFocusClaimOnlyAndProjectFilter(t *testing.T) {
	_, s := newHome(t)
	addFocusProject(t, s, "proj")
	addFocusProject(t, s, "other")
	joinFocusAgent(t, s, "aurora")
	joinFocusAgent(t, s, "borealis")
	postFocus(t, s, "claim-1", "proj", "claim", "area", "thread", "aurora", "mine", "2026-09-06T11:00:00Z")
	postFocus(t, s, "claim-2", "other", "claim", "area", "other", "aurora", "other", "2026-09-06T11:00:00Z")
	execFocusSQL(t, s, `INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until)
		VALUES('job-2','other','task','Other','leased',?,'borealis','2026-09-06T12:00:00Z')`, testNow)

	focuses, err := AgentFocuses(s, testNow)
	if err != nil {
		t.Fatal(err)
	}
	if f := focuses["aurora"]; f.JobID != "" || f.Thread != "area/other" || len(f.Claims) != 2 {
		t.Fatalf("claim-only focus: %+v", f)
	}
	project, err := ProjectFocus(s, "proj", testNow)
	if err != nil {
		t.Fatal(err)
	}
	if len(project) != 1 || project[0].Agent != "aurora" || len(project[0].Claims) != 1 || project[0].Claims[0].Project != "proj" {
		t.Fatalf("project focus: %+v", project)
	}
}

func joinFocusAgent(t *testing.T, s *store.Store, name string) {
	t.Helper()
	if _, _, err := JoinAgent(s, JoinInput{Name: name, Kind: "worker"}); err != nil {
		t.Fatal(err)
	}
}

func addFocusProject(t *testing.T, s *store.Store, slug string) {
	t.Helper()
	execFocusSQL(t, s, `INSERT INTO projects(slug,name,path,added_at) VALUES(?,?,?,?)`, slug, slug, "/repo/"+slug, testNow)
}

func postFocus(t *testing.T, s *store.Store, id, project, kind, area, thread, agent, summary, expires string) *Post {
	t.Helper()
	p, err := CreatePost(s, PostInput{PostID: id, Project: project, Kind: kind, Area: area, Thread: thread, Agent: agent, Summary: summary, Expires: expires})
	if err != nil {
		t.Fatal(err)
	}
	return p
}
