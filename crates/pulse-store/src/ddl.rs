pub const DDL: &str = r####"CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS projects (
    slug TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    remote TEXT NOT NULL DEFAULT '',
    brain INTEGER NOT NULL DEFAULT 0,
    verify_cmd TEXT NOT NULL DEFAULT '',
    default_branch TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active',
    added_at TEXT NOT NULL,
    added_by TEXT NOT NULL DEFAULT '',
    last_seen TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS agents (
    name TEXT PRIMARY KEY,
    display TEXT NOT NULL DEFAULT '',
    runner TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT 'session',
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL,
    last_active_at TEXT NOT NULL DEFAULT '',
    last_useful_at TEXT NOT NULL DEFAULT '',
    sessions TEXT NOT NULL DEFAULT '[]',
    jobs_done INTEGER NOT NULL DEFAULT 0,
    jobs_failed INTEGER NOT NULL DEFAULT 0,
    posts INTEGER NOT NULL DEFAULT 0,
    pruned_at TEXT NOT NULL DEFAULT '',
    pruned_reason TEXT NOT NULL DEFAULT '',
    legacy_id TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS posts (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    area TEXT NOT NULL,
    thread TEXT NOT NULL,
    kind TEXT NOT NULL,
    agent TEXT NOT NULL,
    runner TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    session TEXT NOT NULL DEFAULT '',
    date TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT '',
    expires TEXT NOT NULL DEFAULT '',
    on_post TEXT NOT NULL DEFAULT '',
    vote TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    has_read_line INTEGER NOT NULL DEFAULT 0,
    resolution TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    links TEXT NOT NULL DEFAULT '',
    files TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    origin_project TEXT NOT NULL DEFAULT '',
    extra TEXT NOT NULL DEFAULT '{}',
    origin TEXT NOT NULL DEFAULT 'store',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS posts_thread ON posts (project, area, thread, id);
CREATE INDEX IF NOT EXISTS posts_on ON posts (on_post);
CREATE INDEX IF NOT EXISTS posts_agent ON posts (agent, date);
CREATE TRIGGER IF NOT EXISTS posts_no_update BEFORE UPDATE ON posts
BEGIN SELECT RAISE(ABORT, 'posts are immutable: post again instead'); END;
CREATE TRIGGER IF NOT EXISTS posts_no_delete BEFORE DELETE ON posts
BEGIN SELECT RAISE(ABORT, 'posts are immutable: post again instead'); END;
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    kind TEXT NOT NULL,
    tier INTEGER NOT NULL DEFAULT 1,
    title TEXT NOT NULL,
    brief TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT '',
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'queued',
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL DEFAULT '',
    lease_agent TEXT NOT NULL DEFAULT '',
    lease_until TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    finished_at TEXT NOT NULL DEFAULT '',
    result TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS jobs_status ON jobs (status, priority, created_at);
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'running',
    exit_code INTEGER,
    seconds REAL,
    clone TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    base TEXT NOT NULL DEFAULT '',
    commits INTEGER NOT NULL DEFAULT 0,
    summary TEXT NOT NULL DEFAULT '',
    log TEXT NOT NULL DEFAULT '',
    verify TEXT NOT NULL DEFAULT '',
    mode TEXT NOT NULL DEFAULT 'real',
    executor TEXT NOT NULL DEFAULT 'local',
    signing_pubkey TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS executors (
    name TEXT PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'ssh',
    address TEXT NOT NULL DEFAULT '',
    workdir TEXT NOT NULL DEFAULT '',
    workers INTEGER NOT NULL DEFAULT 1,
    projects TEXT NOT NULL DEFAULT '*',
    status TEXT NOT NULL DEFAULT 'active',
    last_probe_at TEXT NOT NULL DEFAULT '',
    probe_note TEXT NOT NULL DEFAULT '',
    errors INTEGER NOT NULL DEFAULT 0,
    backoff_minutes INTEGER NOT NULL DEFAULT 0,
    next_probe_at TEXT NOT NULL DEFAULT '',
    runs_done INTEGER NOT NULL DEFAULT 0,
    runs_failed INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS decisions (
    post_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    answer TEXT NOT NULL,
    words TEXT NOT NULL DEFAULT '',
    decided_at TEXT NOT NULL,
    decided_by TEXT NOT NULL DEFAULT 'human',
    result_post TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    actor TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
WHEN OLD.action NOT IN ('harness.step')
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
"####;

/// The one kind of event the ledger lets go: a harness session's per-step
/// notification, which belongs to the step transcript and is pruned with it
/// (`daemon.transcript_keep_days`). Everything else stays append-only.
pub const PRUNABLE_EVENT_ACTIONS: &[&str] = &["harness.step"];

/// The delete guard as the schema creates it now; `ensure_events_prunable`
/// swaps an older store's unconditional guard for this one.
pub const EVENTS_NO_DELETE_SQL: &str = "CREATE TRIGGER events_no_delete BEFORE DELETE ON events\nWHEN OLD.action NOT IN ('harness.step')\nBEGIN SELECT RAISE(ABORT, 'events are append-only'); END";
