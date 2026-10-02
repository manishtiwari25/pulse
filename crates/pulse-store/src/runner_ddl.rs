pub const RUNNER_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS briefs (
  run_id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  text TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (run_id)
)"####,
    r####"CREATE TABLE IF NOT EXISTS compute_machines (
  request_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  name TEXT NOT NULL,
  executor TEXT NOT NULL DEFAULT '',
  address TEXT NOT NULL DEFAULT '',
  detail TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  released_at TEXT NOT NULL DEFAULT '',
  attempts INTEGER NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (request_id)
)"####,
    r####"CREATE TABLE IF NOT EXISTS retired_vms (
  executor TEXT NOT NULL,
  provider TEXT NOT NULL DEFAULT '',
  resource_group TEXT NOT NULL DEFAULT '',
  name TEXT NOT NULL DEFAULT '',
  deallocated_at TEXT NOT NULL DEFAULT '',
  delete_after TEXT NOT NULL DEFAULT '',
  deleted_at TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (executor)
)"####,
    r####"CREATE INDEX IF NOT EXISTS events_action_subject ON events (action, subject, seq)"####,
    r####"CREATE INDEX IF NOT EXISTS events_subject_seq ON events (subject, seq)"####,
    r####"CREATE INDEX IF NOT EXISTS runs_agent_recent ON runs (agent, mode, started_at DESC, id DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS runs_status_started ON runs (status, started_at DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS runs_executor_status ON runs (executor, status, started_at DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS jobs_source_status ON jobs (source, status, created_at DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS jobs_source_status_kind_id ON jobs (source, status, kind, id)"####,
    r####"CREATE INDEX IF NOT EXISTS jobs_status_finished ON jobs (status, finished_at DESC, id)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_kind_expires ON posts (kind, expires)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_kind_date ON posts (kind, date DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_project_kind ON posts (project, kind, date DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_thread_answers ON posts (project, area, thread, agent, date DESC)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_project_date ON posts (project, date)"####,
    r####"CREATE INDEX IF NOT EXISTS posts_project_area_thread_id ON posts (project, area, thread, id)"####,
    r####"CREATE INDEX IF NOT EXISTS agents_status_active ON agents (status, last_active_at DESC)"####,
];
use crate::Column;
pub const RUNNER_COLUMNS: &[Column] = &[
    Column {
        table: r####"executors"####,
        name: r####"admin"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"executors"####,
        name: r####"healed_at"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"retired_vms"####,
        name: r####"detail"####,
        ddl: r####"TEXT NOT NULL DEFAULT '{}'"####,
    },
    Column {
        table: r####"runs"####,
        name: r####"work_mb"####,
        ddl: r####"INTEGER NOT NULL DEFAULT 0"####,
    },
];
