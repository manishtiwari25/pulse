pub const RECORDS_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS records (
  id TEXT PRIMARY KEY,                -- ADR-027 | PLAN-026 | WU-022 | FEAT-016 | MEM-product | WF-board | SKILL-pulse-plan | GUIDE-new-repo | CTX-stack | ARCH-overview | EVO-constitution | INSTR-agents | USAGE-log | TPL-adr | PROMPT-eli5
  project TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('adr','plan','wu','feature','memory','workflow','skill','guide','context','architecture','evolution','instruction','template','usage','prompt','role','agent','brief','kind')),
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
);"####,
    r####"CREATE TABLE IF NOT EXISTS record_revisions (
  record_id TEXT NOT NULL REFERENCES records(id),
  rev INTEGER NOT NULL,
  frontmatter TEXT NOT NULL,          -- JSON object of the frontmatter keys
  body TEXT NOT NULL,                 -- Markdown after the frontmatter
  author TEXT NOT NULL, at TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (record_id, rev)
);"####,
    r####"CREATE INDEX IF NOT EXISTS records_kind ON records (project, kind, number);"####,
    r####"CREATE TRIGGER IF NOT EXISTS record_revisions_immutable_update BEFORE UPDATE ON record_revisions BEGIN SELECT RAISE(ABORT, 'record revisions are immutable'); END;"####,
    r####"CREATE TRIGGER IF NOT EXISTS record_revisions_immutable_delete BEFORE DELETE ON record_revisions BEGIN SELECT RAISE(ABORT, 'record revisions are immutable'); END;"####,
    r####"CREATE TRIGGER IF NOT EXISTS records_no_delete BEFORE DELETE ON records BEGIN SELECT RAISE(ABORT, 'records are never deleted; set status instead'); END;"####,
    r####"CREATE TABLE IF NOT EXISTS record_drafts (
  record_id TEXT PRIMARY KEY REFERENCES records(id),
  base_rev INTEGER NOT NULL,          -- the revision the draft was written against
  body TEXT NOT NULL,
  author TEXT NOT NULL, at TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  job_id TEXT NOT NULL DEFAULT ''     -- the job whose run filed it, when one did
);"####,
];
/// Bumped when a statement above changes shape; `Store::widen_record_kinds`
/// rebuilds an older `records` table whose `kind` CHECK lacks the agent-pack
/// kinds (role, agent, brief, kind), which the list must keep in step with
/// `RECORD_KINDS`.
pub const RECORDS_VERSION: &str = "6";
