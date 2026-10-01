pub const EVOLVE_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS evolution_archive (
  id TEXT NOT NULL,
  project TEXT NOT NULL,
  suggestion_post TEXT NOT NULL DEFAULT '',
  hypothesis TEXT NOT NULL DEFAULT '',
  branch TEXT NOT NULL DEFAULT '',
  baseline_score REAL NOT NULL,
  variant_score REAL NOT NULL,
  tolerance REAL NOT NULL,
  kept INTEGER NOT NULL CHECK (kept IN (0, 1)),
  fixtures_json TEXT NOT NULL DEFAULT '[]',
  sandbox TEXT NOT NULL DEFAULT '',
  run_id TEXT NOT NULL DEFAULT '',
  at TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (id, project)
)"####,
    r####"CREATE INDEX IF NOT EXISTS evolution_archive_project_at ON evolution_archive (project, at)"####,
    r####"CREATE TABLE IF NOT EXISTS evolution_baselines (
  project TEXT NOT NULL,
  "commit" TEXT NOT NULL,
  fixture_hash TEXT NOT NULL,
  score REAL NOT NULL,
  fixtures_json TEXT NOT NULL,
  run_id TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (project, "commit", fixture_hash)
)"####,
    r####"CREATE INDEX IF NOT EXISTS evolution_baselines_project_at ON evolution_baselines (project, at)"####,
    r####"CREATE TRIGGER IF NOT EXISTS evolution_archive_no_update BEFORE UPDATE ON evolution_archive
BEGIN SELECT RAISE(ABORT, 'archive rows are immutable'); END"####,
    r####"CREATE TRIGGER IF NOT EXISTS evolution_archive_no_delete BEFORE DELETE ON evolution_archive
BEGIN SELECT RAISE(ABORT, 'archive rows are immutable'); END"####,
    r####"CREATE TABLE IF NOT EXISTS evolution_fixtures (
  id TEXT NOT NULL,
  project TEXT NOT NULL,
  name TEXT NOT NULL,
  request TEXT NOT NULL,
  scorer_cmd TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1,
  kind TEXT NOT NULL DEFAULT 'public' CHECK (kind IN ('public', 'signal')),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_by TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (id),
  UNIQUE (project, name)
)"####,
    r####"CREATE INDEX IF NOT EXISTS evolution_fixtures_project_enabled ON evolution_fixtures (project, enabled, name)"####,
    r####"CREATE TABLE IF NOT EXISTS evolution_fixture_keys (
  fixture_id TEXT NOT NULL REFERENCES evolution_fixtures(id),
  path TEXT NOT NULL,
  PRIMARY KEY (fixture_id, path)
)"####,
    r####"CREATE TABLE IF NOT EXISTS evolution_fleets (
  id TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  ref TEXT NOT NULL,
  workers INTEGER NOT NULL CHECK (workers > 0),
  stripped_json TEXT NOT NULL,
  at TEXT NOT NULL,
  created_by TEXT NOT NULL,
  cleaned_at TEXT NOT NULL DEFAULT ''
)"####,
    r####"CREATE INDEX IF NOT EXISTS evolution_fleets_project_at ON evolution_fleets (project, at)"####,
    r####"CREATE TRIGGER IF NOT EXISTS evolution_fixtures_no_delete BEFORE DELETE ON evolution_fixtures
BEGIN SELECT RAISE(ABORT, 'fixtures are never deleted; disable instead'); END"####,
    r####"CREATE TRIGGER IF NOT EXISTS evolution_fleets_no_delete BEFORE DELETE ON evolution_fleets
BEGIN SELECT RAISE(ABORT, 'fleet history is never deleted; clean the clones instead'); END"####,
    r####"CREATE TABLE IF NOT EXISTS evolution_cycle_leases (
  project TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  expires_at TEXT NOT NULL
)"####,
];
