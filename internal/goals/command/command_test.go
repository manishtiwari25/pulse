package command

import (
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/goals"
	"github.com/manishtiwari25/pulse/internal/store"
)

func commandStore(t *testing.T) *store.Store {
	t.Helper()
	for _, key := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_PROJECT", "PULSE_SESSION"} {
		t.Setenv(key, "")
	}
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	s, err := store.Open(store.Options{Home: home, Create: true, Now: "2026-09-13T06:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	if _, err := s.DB.Exec("INSERT INTO projects (slug,name,path,added_at) VALUES ('demo','Demo',?,?)", filepath.Join(home, "missing"), s.Now()); err != nil {
		t.Fatal(err)
	}
	return s
}

func goalCommand(t *testing.T, c *cli.Context, args ...string) (string, error) {
	t.Helper()
	cmd, ok := cli.Lookup("goal")
	if !ok || !cmd.NoStore {
		t.Fatal("goal must choose a read-only store for reads")
	}
	var out bytes.Buffer
	c.Out, c.Err = &out, io.Discard
	fs := flag.NewFlagSet("goal", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	cmd.Flags(fs)
	if err := fs.Parse(args); err != nil {
		return "", err
	}
	err := cmd.Run(c, fs, fs.Args())
	return out.String(), err
}

func TestGoalCommandCanonicalSlug(t *testing.T) {
	s := commandStore(t)
	const slug = "customer-support-ticket-management-syste"
	if _, err := s.DB.Exec("UPDATE projects SET slug = ? WHERE slug = 'demo'", slug); err != nil {
		t.Fatal(err)
	}
	ctx := &cli.Context{Home: s.Home, Project: slug, JSON: true}
	out, err := goalCommand(t, ctx, "show")
	if err != nil {
		t.Fatalf("CLI cannot read a canonical registered slug: %v", err)
	}
	var got goals.View
	if err := json.Unmarshal([]byte(out), &got); err != nil || got.Project != slug || got.Goal.Source != goals.Generated {
		t.Fatalf("canonical slug fallback: %s, %v", out, err)
	}
	want := "Make /api/search return correct ticket results."
	if _, err := goalCommand(t, ctx, "--text", want, "set"); err != nil {
		t.Fatal(err)
	}
	out, err = goalCommand(t, ctx, "show")
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal([]byte(out), &got); err != nil || got.Project != slug || got.Goal.Source != goals.Owner || got.Goal.Text != want {
		t.Fatalf("CLI lost current owner direction for canonical slug: %s, %v", out, err)
	}
}

func TestGoalCommandReadSetResetAndReopen(t *testing.T) {
	s := commandStore(t)
	ctx := &cli.Context{Home: s.Home, Project: "demo", JSON: true}
	out, err := goalCommand(t, ctx)
	if err != nil {
		t.Fatal(err)
	}
	var initial goals.View
	if err := json.Unmarshal([]byte(out), &initial); err != nil || initial.Project != "demo" || !initial.CanEdit ||
		initial.Goal.Source != goals.Generated {
		t.Fatalf("initial goal JSON: %s, %v", out, err)
	}
	var n int
	if err := s.DB.QueryRow("SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'").Scan(&n); err != nil || n != 0 {
		t.Fatal("read command persisted a legacy fallback")
	}
	out, err = goalCommand(t, ctx, "--text", "  Make search useful and correct.  ", "set")
	if err != nil {
		t.Fatal(err)
	}
	var saved goals.View
	if err := json.Unmarshal([]byte(out), &saved); err != nil || saved.Goal.Text != "Make search useful and correct." ||
		saved.Goal.Source != goals.Owner || saved.Goal.UpdatedBy != store.HumanAgent {
		t.Fatalf("set goal JSON: %s, %v", out, err)
	}
	out, err = goalCommand(t, ctx, "show")
	if err != nil || !strings.Contains(out, saved.Goal.Text) {
		t.Fatalf("reopened command lost goal: %s, %v", out, err)
	}
	out, err = goalCommand(t, ctx, "reset")
	if err != nil {
		t.Fatal(err)
	}
	var reset goals.View
	if err := json.Unmarshal([]byte(out), &reset); err != nil || reset.Goal.Text != initial.Goal.Text ||
		reset.Goal.Source != goals.Generated || reset.Goal.UpdatedBy != store.HumanAgent {
		t.Fatalf("reset goal JSON: %s, %v", out, err)
	}
}

func TestGoalCommandRejectsWorkersAndInvalidModes(t *testing.T) {
	s := commandStore(t)
	for _, actor := range []string{"aurora", "demo-lead"} {
		for _, args := range [][]string{{"--text", "Overwrite direction.", "set"}, {"reset"}} {
			if _, err := goalCommand(t, &cli.Context{Store: s, Agent: actor, Project: "demo"}, args...); err == nil {
				t.Fatalf("agent %s could mutate a goal", actor)
			}
		}
	}
	for _, args := range [][]string{
		{"set"}, {"--text", "", "set"}, {"--text", "Ignored text.", "reset"}, {"--text", "Ignored text.", "show"},
		{"set", "reset"}, {"unknown"},
	} {
		if _, err := goalCommand(t, &cli.Context{Store: s, Project: "demo"}, args...); err == nil {
			t.Fatalf("invalid command accepted: %v", args)
		}
	}
	t.Setenv("PULSE_AGENT", "aurora")
	ctx := &cli.Context{Home: t.TempDir(), Project: "demo", Agent: "human"}
	if _, err := goalCommand(t, ctx, "--text", "Impersonate owner.", "set"); err == nil ||
		!strings.Contains(err.Error(), "Only the owner") {
		t.Fatalf("CLI did not guard impersonation before opening the store: %v", err)
	}
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_JOB", "job-worker")
	if _, err := goalCommand(t, ctx, "reset"); err == nil || !strings.Contains(err.Error(), "Only the owner") {
		t.Fatalf("worker job impersonated owner: %v", err)
	}
	if _, err := os.Stat(filepath.Join(ctx.Home, store.StoreName)); !os.IsNotExist(err) {
		t.Fatal("rejected command created a store")
	}
}

func TestGoalCommandProjectSelectionAndReadOnlyMissingStore(t *testing.T) {
	s := commandStore(t)
	t.Setenv("PULSE_PROJECT", "demo")
	t.Setenv("PULSE_AGENT", "reader")
	out, err := goalCommand(t, &cli.Context{Home: s.Home, JSON: true})
	if err != nil || !strings.Contains(out, `"project": "demo"`) || !strings.Contains(out, `"can_edit": false`) {
		t.Fatalf("project-scoped agent read: %s, %v", out, err)
	}
	home := t.TempDir()
	if _, err := goalCommand(t, &cli.Context{Home: home}, "show"); err == nil {
		t.Fatal("missing store read succeeded")
	}
	if _, err := os.Stat(filepath.Join(home, store.StoreName)); !os.IsNotExist(err) {
		t.Fatal("goal read created a store")
	}
}
