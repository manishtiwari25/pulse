package store

import (
	"database/sql"
	"path/filepath"
	"testing"
)

func TestPutGetRecord(t *testing.T) {
	home := t.TempDir()
	path := filepath.Join(home, StoreName)
	if err := Create(path); err != nil {
		t.Fatal(err)
	}
	s, err := Open(Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	n, err := s.NextRecordNumber("proj", "adr")
	if err != nil || n != 1 {
		t.Fatalf("next number = %d, %v", n, err)
	}
	rec := Record{ID: "ADR-001", Project: "proj", Kind: "adr", Number: 1, Slug: "first", Title: "First", Status: "Proposed", Areas: []string{"runner"}}
	rev, err := s.PutRecord(rec, map[string]any{"title": "First"}, "# First\n", "tester", "")
	if err != nil || rev != 1 {
		t.Fatalf("put: rev=%d err=%v", rev, err)
	}
	rec.Status = "Accepted"
	rev, err = s.PutRecord(rec, nil, "# First\n\nMore.\n", "tester", "amended")
	if err != nil || rev != 2 {
		t.Fatalf("second put: rev=%d err=%v", rev, err)
	}
	r, v, err := s.GetRecord("ADR-001", 0)
	if err != nil {
		t.Fatal(err)
	}
	if r.CurrentRev != 2 || r.Status != "Accepted" || v.Rev != 2 || v.Body != "# First\n\nMore.\n" || len(r.Areas) != 1 {
		t.Fatalf("head/revision mismatch: %+v %+v", r, v)
	}
	if _, v1, err := s.GetRecord("ADR-001", 1); err != nil || v1.Frontmatter["title"] != "First" {
		t.Fatalf("rev 1: %+v %v", v1, err)
	}
	revs, err := s.Revisions("ADR-001")
	if err != nil || len(revs) != 2 {
		t.Fatalf("revisions: %d %v", len(revs), err)
	}
	list, err := s.ListRecords(RecordFilter{Project: "proj", Kind: "adr", Area: "runner"})
	if err != nil || len(list) != 1 {
		t.Fatalf("list: %d %v", len(list), err)
	}
	if err := s.SetRecordStatus("ADR-001", "Retired", "human"); err != nil {
		t.Fatal(err)
	}
	if _, _, err := s.GetRecord("ADR-999", 0); err != ErrNoRecord {
		t.Fatalf("expected ErrNoRecord, got %v", err)
	}
	var n2 int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action IN ('record.created','record.revised','record.status')").Scan(&n2); err != nil || n2 != 3 {
		t.Fatalf("events: %d %v", n2, err)
	}
	if _, err := s.DB.Exec("DELETE FROM record_revisions WHERE record_id = 'ADR-001'"); err == nil {
		t.Fatal("revisions must be immutable")
	}
}

func TestComputeSettings(t *testing.T) {
	if _, err := ValidateSetting("compute.max_vms", "-1"); err == nil {
		t.Fatal("negative max_vms accepted")
	}
	if v, err := ValidateSetting("compute.max_vms", "3"); err != nil || v != "3" {
		t.Fatalf("max_vms: %q %v", v, err)
	}
	if _, err := ValidateSetting("compute.azure.subscription", "ghp_abcdefghijklmnop"); err == nil {
		t.Fatal("secret-shaped value accepted")
	}
	if v, err := ValidateSetting("compute.azure.location", "westeurope"); err != nil || v != "westeurope" {
		t.Fatalf("location: %q %v", v, err)
	}
	if _, err := ValidateSetting("compute.provider", "aws"); err == nil {
		t.Fatal("unknown provider accepted")
	}
	if !SecretShaped("-----BEGIN RSA PRIVATE KEY-----") || SecretShaped("/usr/local/bin:/opt/homebrew/bin/some/very/long/path/that/keeps/going/on") {
		t.Fatal("SecretShaped mismatch")
	}
	home := t.TempDir()
	if err := Create(filepath.Join(home, StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := Open(Options{Home: home})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if s.Setting("compute.provider", "") != "azure" || s.SettingInt("compute.max_vms") != 0 {
		t.Fatal("compute defaults not resolved")
	}
	if err := s.SetSetting("human", "compute.auto_approve", "1"); err != nil {
		t.Fatal(err)
	}
	if s.Setting("compute.auto_approve", "") != "1" {
		t.Fatal("compute setting not stored")
	}
	if s.Setting("evolve.limits", "") != "none" || s.Setting("evolve.tolerance", "") != "0.02" {
		t.Fatal("evolve defaults not resolved")
	}
	if _, err := ValidateSetting("evolve.tolerance", "1.5"); err == nil {
		t.Fatal("tolerance > 1 accepted")
	}
	if got := len(ExtraSettingNames()); got != len(ComputeSettings)+len(EvolveSettings)+len(DaemonSettings)+len(LeadSettings) {
		t.Fatalf("ExtraSettingNames = %d", got)
	}
	// ADR-034: self-heal, self-retire and the grace before a retired VM is deleted.
	if s.Setting("compute.self_heal", "") != "1" || s.Setting("compute.self_retire", "") != "1" || s.SettingInt("compute.retire_grace_hours") != 24 {
		t.Fatal("ADR-034 defaults not resolved")
	}
	for _, name := range []string{"compute.self_heal", "compute.self_retire"} {
		if v, err := ValidateSetting(name, "0"); err != nil || v != "0" {
			t.Fatalf("%s 0: %q %v", name, v, err)
		}
		if _, err := ValidateSetting(name, "yes"); err == nil {
			t.Fatalf("%s accepted yes", name)
		}
	}
	if _, err := ValidateSetting("compute.retire_grace_hours", "0"); err == nil {
		t.Fatal("retire_grace_hours 0 accepted")
	}
	if v, err := ValidateSetting("compute.retire_grace_hours", "48"); err != nil || v != "48" {
		t.Fatalf("retire_grace_hours: %q %v", v, err)
	}
}

// TestAdditiveColumns: EnsureRecords adds executors.admin / executors.healed_at to a schema-3 store that
// predates them and creates retired_vms; a second open is a no-op (ADR-034).
func TestAdditiveColumns(t *testing.T) {
	home := t.TempDir()
	// a schema-3 store as legacy/pulse.py left it: no additive tables, no ADR-034 columns
	bare, err := sql.Open("sqlite", dsn(filepath.Join(home, StoreName), false))
	if err != nil {
		t.Fatal(err)
	}
	if err := initSchema(bare, "2026-09-08T00:00:00Z"); err != nil {
		t.Fatal(err)
	}
	if has, _ := hasColumn(bare, "executors", "admin"); has {
		t.Fatal("schema 3 already has executors.admin")
	}
	bare.Close()
	s, err := Open(Options{Home: home})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	for _, c := range RunnerColumns {
		if has, err := hasColumn(s.DB, c.Table, c.Name); err != nil || !has {
			t.Fatalf("%s.%s missing after EnsureRecords: %v", c.Table, c.Name, err)
		}
	}
	if _, err := s.DB.Exec("INSERT INTO executors (name, created_at) VALUES ('x', '2026-09-08T00:00:00Z')"); err != nil {
		t.Fatal(err)
	}
	var admin, healed string
	if err := s.DB.QueryRow("SELECT admin, healed_at FROM executors WHERE name = 'x'").Scan(&admin, &healed); err != nil || admin != "" || healed != "" {
		t.Fatalf("defaults: %q %q %v", admin, healed, err)
	}
	if _, err := s.DB.Exec("INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after) VALUES ('x', 'azure', 'rg', 'vm', 'a', 'b')"); err != nil {
		t.Fatal(err)
	}
	if !s.additiveComplete() {
		t.Fatal("additiveComplete false after EnsureRecords")
	}
	if err := s.EnsureRecords(); err != nil {
		t.Fatal(err)
	}
	if n := countEvents(t, s, "schema.records"); n != 1 {
		t.Fatalf("schema.records events = %d", n)
	}
}
