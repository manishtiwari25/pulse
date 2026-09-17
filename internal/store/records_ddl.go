package store

// RecordsDDL is owned by the records work (internal/records); keep in sync.
//
// Each statement is its own string: the triggers carry semicolons inside
// BEGIN ... END, so the list must never be rebuilt by splitting on ";".
// store.EnsureRecords executes them in order (all IF NOT EXISTS, additive only)
// and tracks the result with the meta key records_version = RecordsVersion.
var RecordsDDL = []string{
	`CREATE TABLE IF NOT EXISTS records (
  id TEXT PRIMARY KEY,                -- ADR-027 | PLAN-026 | WU-022 | FEAT-016 | MEM-product | WF-board | SKILL-pulse-plan | GUIDE-new-repo | CTX-stack | ARCH-overview | EVO-constitution | INSTR-agents | USAGE-log | TPL-adr | PROMPT-eli5
  project TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('adr','plan','wu','feature','memory','workflow','skill','guide','context','architecture','evolution','instruction','template','usage','prompt')),
  number INTEGER,
  slug TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT '',
  tier INTEGER,
  areas TEXT NOT NULL DEFAULT '[]',
  tags TEXT NOT NULL DEFAULT '[]',
  delivery TEXT NOT NULL DEFAULT '',
  delivered TEXT NOT NULL DEFAULT '',
  date TEXT NOT NULL DEFAULT '',
  path TEXT NOT NULL DEFAULT '',      -- original repository path
  created_at TEXT NOT NULL, created_by TEXT NOT NULL, updated_at TEXT NOT NULL,
  current_rev INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL                -- 'store' or 'markdown:<path>@<commit>'
);`,
	`CREATE TABLE IF NOT EXISTS record_revisions (
  record_id TEXT NOT NULL REFERENCES records(id),
  rev INTEGER NOT NULL,
  frontmatter TEXT NOT NULL,          -- JSON object of the frontmatter keys
  body TEXT NOT NULL,                 -- Markdown after the frontmatter
  author TEXT NOT NULL, at TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (record_id, rev)
);`,
	`CREATE INDEX IF NOT EXISTS records_kind ON records (project, kind, number);`,
	`CREATE TRIGGER IF NOT EXISTS record_revisions_immutable_update BEFORE UPDATE ON record_revisions BEGIN SELECT RAISE(ABORT, 'record revisions are immutable'); END;`,
	`CREATE TRIGGER IF NOT EXISTS record_revisions_immutable_delete BEFORE DELETE ON record_revisions BEGIN SELECT RAISE(ABORT, 'record revisions are immutable'); END;`,
	`CREATE TRIGGER IF NOT EXISTS records_no_delete BEFORE DELETE ON records BEGIN SELECT RAISE(ABORT, 'records are never deleted; set status instead'); END;`,
	// ADR-046: at most one proposed body per record, never the record's text
	// until a human adopts it (then it is a revision and the row is gone).
	`CREATE TABLE IF NOT EXISTS record_drafts (
  record_id TEXT PRIMARY KEY REFERENCES records(id),
  base_rev INTEGER NOT NULL,          -- the revision the draft was written against
  body TEXT NOT NULL,
  author TEXT NOT NULL, at TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  job_id TEXT NOT NULL DEFAULT ''     -- the job whose run filed it, when one did
);`,
}

// RecordsVersion is the value EnsureRecords writes to meta.records_version.
// BUMP IT whenever a statement is added to RecordsDDL: EnsureRecords skips a
// store whose stamp already equals this value, so new tables only reach
// existing stores through a new version.
const RecordsVersion = "3"
