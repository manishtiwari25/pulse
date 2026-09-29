pub const TASKS_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,                -- TASK-001
  project TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','planned','in_progress','review','done','cancelled')),
  priority INTEGER NOT NULL DEFAULT 0,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  current_rev INTEGER NOT NULL DEFAULT 1,
  lead TEXT NOT NULL DEFAULT '',      -- the project's lead agent that took it
  job_id TEXT NOT NULL DEFAULT ''     -- the job the lead queued for it
);"####,
    r####"CREATE INDEX IF NOT EXISTS tasks_project_status ON tasks (project, status);"####,
    r####"CREATE TABLE IF NOT EXISTS task_revisions (
  task_id TEXT NOT NULL REFERENCES tasks(id),
  rev INTEGER NOT NULL,
  title TEXT NOT NULL,
  brief TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  author TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (task_id, rev)
);"####,
    r####"CREATE TRIGGER IF NOT EXISTS task_revisions_immutable_update BEFORE UPDATE ON task_revisions BEGIN SELECT RAISE(ABORT, 'task revisions are immutable: amend the task instead'); END;"####,
    r####"CREATE TRIGGER IF NOT EXISTS task_revisions_immutable_delete BEFORE DELETE ON task_revisions BEGIN SELECT RAISE(ABORT, 'task revisions are immutable: amend the task instead'); END;"####,
    r####"CREATE TRIGGER IF NOT EXISTS tasks_no_delete BEFORE DELETE ON tasks BEGIN SELECT RAISE(ABORT, 'tasks are never deleted: cancel instead'); END;"####,
    r####"CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,                -- MSG-20260906T210000Z-1
  project TEXT NOT NULL,
  task_id TEXT,                       -- NULL when the message is about the project, not one task
  author TEXT NOT NULL,
  body TEXT NOT NULL,
  at TEXT NOT NULL,
  read_by TEXT NOT NULL DEFAULT '[]'  -- JSON list of agent names
);"####,
    r####"CREATE INDEX IF NOT EXISTS messages_project_task ON messages (project, task_id, id);"####,
    r####"CREATE TRIGGER IF NOT EXISTS messages_immutable_update BEFORE UPDATE OF id, project, task_id, author, body, at ON messages BEGIN SELECT RAISE(ABORT, 'messages are immutable: send another'); END;"####,
    r####"CREATE TRIGGER IF NOT EXISTS messages_immutable_delete BEFORE DELETE ON messages BEGIN SELECT RAISE(ABORT, 'messages are immutable: send another'); END;"####,
    r####"CREATE TABLE IF NOT EXISTS compute_requests (
  id TEXT PRIMARY KEY,                -- CR-001
  project TEXT NOT NULL,
  requested_by TEXT NOT NULL,
  size TEXT NOT NULL,
  hours INTEGER NOT NULL,
  reason TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','provisioning','ready','denied','released','failed')),
  decided_by TEXT NOT NULL DEFAULT '',
  executor TEXT NOT NULL DEFAULT '',
  words TEXT NOT NULL DEFAULT '',     -- the owner's words on approve/deny
  proposal_post TEXT NOT NULL DEFAULT '',  -- the board proposal when the request waited for the owner
  at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);"####,
    r####"CREATE INDEX IF NOT EXISTS compute_requests_project_status ON compute_requests (project, status);"####,
    r####"CREATE TRIGGER IF NOT EXISTS compute_requests_no_delete BEFORE DELETE ON compute_requests BEGIN SELECT RAISE(ABORT, 'compute requests are never deleted: deny or release instead'); END;"####,
];
