package store

import (
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
	if got := len(ExtraSettingNames()); got != len(ComputeSettings)+len(EvolveSettings) {
		t.Fatalf("ExtraSettingNames = %d", got)
	}
}
