package board

import (
	"errors"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestRegisterProjectRunsTheCreationHookOnce(t *testing.T) {
	_, s := newHome(t)
	repo := gitRepo(t)
	previous := OnProjectCreated
	t.Cleanup(func() { OnProjectCreated = previous })
	calls := []string{}
	OnProjectCreated = func(hs *store.Store, p *store.Project, actor string) error {
		if hs != s || p == nil {
			t.Fatal("the hook did not receive the registering store and project")
		}
		calls = append(calls, p.Slug+"/"+actor)
		return nil
	}
	p, created, err := RegisterProject(s, repo, "demo", "", false, "", "alice", "")
	if err != nil || !created || p.Slug != "demo" {
		t.Fatalf("register: %+v %v %v", p, created, err)
	}
	if _, created, err = RegisterProject(s, repo, "demo", "", false, "", "alice", ""); err != nil || created {
		t.Fatalf("refresh: %v %v", created, err)
	}
	if len(calls) != 1 || calls[0] != "demo/alice" {
		t.Fatalf("hook calls: %v", calls)
	}
	OnProjectCreated = nil
	if _, created, err = RegisterProject(s, gitRepo(t), "second", "", false, "", "alice", ""); err != nil || !created {
		t.Fatalf("register without a hook: %v %v", created, err)
	}
}

func TestRegisterProjectSurvivesAFailingHook(t *testing.T) {
	_, s := newHome(t)
	previous := OnProjectCreated
	t.Cleanup(func() { OnProjectCreated = previous })
	OnProjectCreated = func(*store.Store, *store.Project, string) error { return errors.New("seeding broke") }
	p, created, err := RegisterProject(s, gitRepo(t), "demo", "", false, "", "human", "")
	if err != nil || !created || p == nil {
		t.Fatalf("a hook failure failed the registration: %v", err)
	}
	if got, err := s.Project("demo"); err != nil || got == nil || got.Status != "active" {
		t.Fatalf("project after a failing hook: %+v %v", got, err)
	}
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'project.hook_failed' AND subject = 'demo' AND detail LIKE '%seeding broke%'").Scan(&n); err != nil || n != 1 {
		t.Fatalf("hook_failed events: %d (%v)", n, err)
	}
}
