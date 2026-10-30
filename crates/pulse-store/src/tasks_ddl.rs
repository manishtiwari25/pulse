use crate::Column;

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
  read_by TEXT NOT NULL DEFAULT '[]', -- JSON list of agent names
  choices TEXT NOT NULL DEFAULT '[]', -- JSON list of answers the owner can pick: [{"label": "...", "action": "reply|done|amend"}]
  job_id TEXT NOT NULL DEFAULT ''     -- set when a worker's finished job reports back into the task's conversation
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
    r####"CREATE TABLE IF NOT EXISTS schedules (
  id TEXT PRIMARY KEY,                -- SCHED-001
  project TEXT NOT NULL,
  title TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'task',
  brief TEXT NOT NULL DEFAULT '',
  tier INTEGER NOT NULL DEFAULT 1,
  priority INTEGER NOT NULL DEFAULT 0,
  every_minutes INTEGER NOT NULL,     -- the cadence
  at_time TEXT NOT NULL DEFAULT '',   -- HH:MM UTC anchor for daily and longer cadences
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused','ended')),
  next_at TEXT NOT NULL,              -- when the next job is due
  last_at TEXT NOT NULL DEFAULT '',   -- when the schedule last fired
  last_job TEXT NOT NULL DEFAULT '',  -- the job it queued then
  runs INTEGER NOT NULL DEFAULT 0,    -- jobs queued so far
  max_runs INTEGER NOT NULL DEFAULT 0, -- 0: no end
  note TEXT NOT NULL DEFAULT '',      -- why it is paused or ended
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  script TEXT NOT NULL DEFAULT '',    -- check/watch: repo-relative path of the script on the default branch
  job_kind TEXT NOT NULL DEFAULT 'task', -- the kind of job a red check or a firing watch queues
  state TEXT NOT NULL DEFAULT '',     -- check: green/red; watch: ok/broken; '' until the first run
  state_since TEXT NOT NULL DEFAULT '',
  last_rc INTEGER,                    -- exit code of the last script run, NULL until one ran
  last_output TEXT NOT NULL DEFAULT '', -- redacted tail of the last script run
  fails INTEGER NOT NULL DEFAULT 0,   -- consecutive non-zero exits
  agent TEXT NOT NULL DEFAULT '',     -- the custom agent its jobs run as; '' lets the runner choose
  skill TEXT NOT NULL DEFAULT '',     -- the skill every job it queues follows; '' for none
  origin TEXT NOT NULL DEFAULT ''     -- '' when set by hand; repo:<path>#<name> when the project's .pulse/pulse.yaml keeps it
);"####,
    r####"CREATE INDEX IF NOT EXISTS schedules_project_status ON schedules (project, status, next_at);"####,
    r####"CREATE TRIGGER IF NOT EXISTS schedules_no_delete BEFORE DELETE ON schedules BEGIN SELECT RAISE(ABORT, 'schedules are never deleted: end instead'); END;"####,
    r####"CREATE TABLE IF NOT EXISTS leads (
  id TEXT PRIMARY KEY,                -- LEAD-001
  project TEXT NOT NULL,
  role TEXT NOT NULL,                 -- rsi, docs, security, monitor, or a custom word
  agent TEXT NOT NULL,                -- the lead agent: <slug>-lead-<role>
  title TEXT NOT NULL,                -- what it owns, in a few words
  brief TEXT NOT NULL DEFAULT '',     -- the standing brief of every job it takes; '' means the built-in one
  every_minutes INTEGER NOT NULL,     -- how often it sits down to look
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused','ended')),
  next_at TEXT NOT NULL,              -- when its next sitting is due
  last_at TEXT NOT NULL DEFAULT '',   -- when it last sat down
  last_job TEXT NOT NULL DEFAULT '',  -- the job that sitting queued
  runs INTEGER NOT NULL DEFAULT 0,    -- sittings so far
  note TEXT NOT NULL DEFAULT '',      -- why it is paused or ended
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);"####,
    r####"CREATE INDEX IF NOT EXISTS leads_project_status ON leads (project, status, next_at);"####,
    r####"CREATE TRIGGER IF NOT EXISTS leads_no_delete BEFORE DELETE ON leads BEGIN SELECT RAISE(ABORT, 'leads are never deleted: end instead'); END;"####,
    r####"CREATE TABLE IF NOT EXISTS task_suggestions (
  id TEXT PRIMARY KEY,                -- SUG-001
  project TEXT NOT NULL,
  title TEXT NOT NULL,
  brief TEXT NOT NULL DEFAULT '',     -- what to do and why, with the evidence
  priority INTEGER NOT NULL DEFAULT 0,
  suggested_by TEXT NOT NULL,         -- the agent (or the owner) who suggested it
  status TEXT NOT NULL DEFAULT 'suggested' CHECK (status IN ('suggested','reviewing','accepted','declined','owner','withdrawn')),
  reviewer TEXT NOT NULL DEFAULT '',  -- the agent whose verdict stands
  verdict TEXT NOT NULL DEFAULT '',   -- the reviewer's reason
  job_id TEXT NOT NULL DEFAULT '',    -- the latest review job
  reviews INTEGER NOT NULL DEFAULT 0, -- review jobs that ended without a verdict
  task_id TEXT NOT NULL DEFAULT '',   -- the task an accepted suggestion became
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  reviewed_at TEXT NOT NULL DEFAULT ''
);"####,
    r####"CREATE INDEX IF NOT EXISTS task_suggestions_project_status ON task_suggestions (project, status);"####,
    r####"CREATE TRIGGER IF NOT EXISTS task_suggestions_no_delete BEFORE DELETE ON task_suggestions BEGIN SELECT RAISE(ABORT, 'task suggestions are never deleted: decline or withdraw instead'); END;"####,
];

/// Columns added after the schedules table shipped: the script kinds
/// (`check`, `watch`), the agent the jobs run as, the skill they follow and
/// the repository file that keeps the schedule. `ensure_records` adds
/// each one that is missing, so an older store gains them on first open.
pub const SCHEDULE_COLUMNS: &[Column] = &[
    Column {
        table: r####"schedules"####,
        name: r####"script"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"job_kind"####,
        ddl: r####"TEXT NOT NULL DEFAULT 'task'"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"state"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"state_since"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"last_rc"####,
        ddl: r####"INTEGER"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"last_output"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"fails"####,
        ddl: r####"INTEGER NOT NULL DEFAULT 0"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"agent"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"skill"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
    Column {
        table: r####"schedules"####,
        name: r####"origin"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
];

/// Columns added after the messages table shipped: the answers a message
/// offers the owner to pick from instead of writing. `ensure_records` adds
/// each one that is missing, so an older store gains them on first open.
pub const MESSAGE_COLUMNS: &[Column] = &[
    Column {
        table: r####"messages"####,
        name: r####"choices"####,
        ddl: r####"TEXT NOT NULL DEFAULT '[]'"####,
    },
    Column {
        table: r####"messages"####,
        name: r####"job_id"####,
        ddl: r####"TEXT NOT NULL DEFAULT ''"####,
    },
];
