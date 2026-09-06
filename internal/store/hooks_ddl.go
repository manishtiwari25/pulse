package store

// HooksDDL is owned by the hooks work (internal/hooks): the hooks table (D-E).
// Every statement is its own string, all IF NOT EXISTS. Rows are never
// deleted: `pulse hook remove` sets enabled = 0.
var HooksDDL = []string{
	`CREATE TABLE IF NOT EXISTS hooks (
  id INTEGER PRIMARY KEY,
  event TEXT NOT NULL,                -- action glob, e.g. post.* or task.amended or *
  kind TEXT NOT NULL CHECK (kind IN ('script','webhook')),
  target TEXT NOT NULL,               -- script path or URL
  project TEXT NOT NULL DEFAULT '',   -- '' = every project
  enabled INTEGER NOT NULL DEFAULT 1,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_fired_at TEXT NOT NULL DEFAULT '',
  last_status TEXT NOT NULL DEFAULT '',
  fires INTEGER NOT NULL DEFAULT 0
);`,
	`CREATE TRIGGER IF NOT EXISTS hooks_no_delete BEFORE DELETE ON hooks BEGIN SELECT RAISE(ABORT, 'hooks are never deleted: pulse hook remove disables instead'); END;`,
}
