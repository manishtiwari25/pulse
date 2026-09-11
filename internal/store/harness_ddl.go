package store

// HarnessDDL is owned by the native harness work. It is additive so older
// schema-3 stores gain run telemetry without changing their legacy rows.
var HarnessDDL = []string{
	`CREATE TABLE IF NOT EXISTS harness_sessions (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL DEFAULT '',
  job_id TEXT NOT NULL DEFAULT '',
  agent TEXT NOT NULL DEFAULT '',
  project TEXT NOT NULL DEFAULT '',
  executor TEXT NOT NULL DEFAULT '',
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
)`,
	`CREATE INDEX IF NOT EXISTS harness_sessions_project ON harness_sessions(project, started_at)`,
	`CREATE INDEX IF NOT EXISTS harness_sessions_status ON harness_sessions(status, started_at)`,
	`CREATE TABLE IF NOT EXISTS harness_steps (
  session_id TEXT NOT NULL,
  n INTEGER NOT NULL,
  at TEXT NOT NULL,
  kind TEXT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  command TEXT NOT NULL DEFAULT '',
  exit_code INTEGER,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  model TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (session_id, n)
)`,
}

// HarnessColumns are additive columns for stores that already created an older
// harness_sessions table before the current harness schema.
var HarnessColumns = []Column{
	{Table: "harness_sessions", Name: "executor", DDL: "TEXT NOT NULL DEFAULT ''"},
}
