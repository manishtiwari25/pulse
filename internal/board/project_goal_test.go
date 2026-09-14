package board

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/goals"
)

func TestProjectGoalCanonicalSlugRegistration(t *testing.T) {
	_, s := newHome(t)
	const name = "customer-support-ticket-management-system"
	slug := Slugify(name)
	if slug != "customer-support-ticket-management-syste" || len(slug) != 40 {
		t.Fatalf("unexpected canonical slug: %q", slug)
	}
	for _, want := range []string{slug, slug + "-2"} {
		dir := gitRepo(t)
		p, created, err := RegisterProject(s, dir, name, "", false, "", "human", "")
		if err != nil || p == nil || !created || p.Slug != want {
			t.Fatalf("valid registry name failed registration: %+v, %t, %v", p, created, err)
		}
		goal, err := goals.Get(s, p.Slug)
		if err != nil || goal.Source != goals.Generated || !strings.Contains(goal.Text, "real product defects") {
			t.Fatalf("canonical slug has no usable default: %+v, %v", goal, err)
		}
		if _, err := goals.Normalize(goal.Text); err != nil {
			t.Fatalf("generated text is not safe to share: %v", err)
		}
		owner, err := goals.Set(s, p.Slug, "Improve correct ticket results with the existing checks.", "human")
		if err != nil {
			t.Fatal(err)
		}
		if _, err := Init(s, dir, "", "", false, "", "human"); err != nil {
			t.Fatal(err)
		}
		if got, err := goals.Get(s, p.Slug); err != nil || got != owner {
			t.Fatalf("reinit lost the canonical slug's owner goal: %+v, %v", got, err)
		}
	}
}

func TestProjectGoalRegistrationReinitAndRestore(t *testing.T) {
	_, s := newHome(t)
	dir := gitRepo(t)
	result, err := Init(s, dir, "Goal Project", "", false, "", "human")
	if err != nil {
		t.Fatal(err)
	}
	p := result.Project
	goal, err := goals.Get(s, p.Slug)
	if err != nil || goal.Source != goals.Generated || goal.UpdatedBy != goals.GeneratedBy {
		t.Fatalf("new registration did not get a generated goal: %+v, %v", goal, err)
	}
	var settings int
	if err := s.DB.QueryRow("SELECT count(*) FROM settings WHERE name = ?", "project.goal."+p.Slug).Scan(&settings); err != nil || settings != 1 {
		t.Fatalf("initial goal not persisted: %d, %v", settings, err)
	}
	owner, err := goals.Set(s, p.Slug, "Keep imports useful and reliable with measured regression coverage.", "human")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Init(s, dir, "Different Name", "", false, "go test ./...", "human"); err != nil {
		t.Fatal(err)
	}
	if got, err := goals.Get(s, p.Slug); err != nil || got != owner {
		t.Fatalf("reinit replaced owner goal: %+v, %v", got, err)
	}
	if _, err := RemoveProject(s, p.Slug, "human", false); err != nil {
		t.Fatal(err)
	}
	if _, err := Init(s, dir, "", "", false, "", "human"); err != nil {
		t.Fatal(err)
	}
	if got, err := goals.Get(s, p.Slug); err != nil || got != owner {
		t.Fatalf("restore replaced owner goal: %+v, %v", got, err)
	}
}

func TestProjectGoalLegacyRegistrationPreservesExplicitValue(t *testing.T) {
	_, s := newHome(t)
	dir := gitRepo(t)
	if _, err := s.DB.Exec("INSERT INTO projects (slug,name,path,added_at) VALUES ('legacy','Legacy',?,?)", dir, testNow); err != nil {
		t.Fatal(err)
	}
	goal, err := goals.Get(s, "legacy")
	if err != nil || goal.Source != goals.Generated {
		t.Fatalf("legacy fallback: %+v, %v", goal, err)
	}
	owner, err := goals.Set(s, "legacy", "Reduce user-facing search defects using current checks.", "human")
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := RegisterProject(s, dir, "", "", false, "", "human", ""); err != nil {
		t.Fatal(err)
	}
	if got, err := goals.Get(s, "legacy"); err != nil || got != owner {
		t.Fatalf("legacy refresh inferred over owner: %+v, %v", got, err)
	}
}
