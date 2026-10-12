pub const HARNESS_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS harness_sessions (
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
  trajectory TEXT NOT NULL DEFAULT '',
  cancel_requested_at TEXT NOT NULL DEFAULT ''
)"####,
    r####"CREATE INDEX IF NOT EXISTS harness_sessions_project ON harness_sessions(project, started_at)"####,
    r####"CREATE INDEX IF NOT EXISTS harness_sessions_status ON harness_sessions(status, started_at)"####,
    r####"CREATE TABLE IF NOT EXISTS harness_steps (
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
  finish_reason TEXT,
  choices INTEGER,
  PRIMARY KEY (session_id, n)
)"####,
];
use crate::Column;
pub const HARNESS_COLUMNS: &[Column] = &[
    Column {
        table: r####"harness_sessions"####,
        name: r####"executor"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"harness_sessions"####,
        name: r####"cancel_requested_at"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"harness_steps"####,
        name: r####"finish_reason"####,
        ddl: r####"TEXT"####,
    },
    Column {
        table: r####"harness_steps"####,
        name: r####"choices"####,
        ddl: r####"INTEGER"####,
    },
];
