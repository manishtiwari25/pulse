package store

import (
	"database/sql"
	"path/filepath"
	"testing"
)

func TestHarnessExecutorColumnFreshAndMigrated(t *testing.T) {
	fresh := openTemp(t, Options{Home: t.TempDir(), Create: true})
	if has, err := hasColumn(fresh.DB, "harness_sessions", "executor"); err != nil || !has {
		t.Fatalf("fresh executor column: %v has=%v", err, has)
	}
	fresh.Close()

	home := t.TempDir()
	db, err := sql.Open("sqlite", dsn(filepath.Join(home, StoreName), false))
	if err != nil {
		t.Fatal(err)
	}
	if err := initSchema(db, "2026-09-11T00:00:00Z"); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TABLE harness_sessions (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL DEFAULT '',
  job_id TEXT NOT NULL DEFAULT '',
  agent TEXT NOT NULL DEFAULT '',
  project TEXT NOT NULL DEFAULT '',
  provider TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  cwd TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'running',
  exit_status TEXT NOT NULL DEFAULT '',
  submission TEXT NOT NULL DEFAULT '',
  error TEXT NOT NULL DEFAULT '',
  steps INTEGER NOT NULL DEFAULT 0,
  calls INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  ended_at TEXT NOT NULL DEFAULT '',
  trajectory TEXT NOT NULL DEFAULT ''
)`); err != nil {
		t.Fatal(err)
	}
	if has, err := hasColumn(db, "harness_sessions", "executor"); err != nil || has {
		t.Fatalf("legacy executor column before migration: %v has=%v", err, has)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	migrated := openTemp(t, Options{Home: home})
	if has, err := hasColumn(migrated.DB, "harness_sessions", "executor"); err != nil || !has {
		t.Fatalf("migrated executor column: %v has=%v", err, has)
	}
	if _, err := migrated.DB.Exec("INSERT INTO harness_sessions (id, started_at, updated_at) VALUES ('hs', ?, ?)", migrated.Now(), migrated.Now()); err != nil {
		t.Fatal(err)
	}
	var executor string
	if err := migrated.DB.QueryRow("SELECT executor FROM harness_sessions WHERE id = 'hs'").Scan(&executor); err != nil || executor != "" {
		t.Fatalf("executor default = %q err=%v", executor, err)
	}
}
