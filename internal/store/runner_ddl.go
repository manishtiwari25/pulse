package store

// RunnerDDL is owned by the runner work (internal/runner). Every statement is
// its own string, all IF NOT EXISTS; store.EnsureRecords executes them after
// RecordsDDL.
//
// briefs keeps the text a worker was launched with (D-L: the brief travels as
// the harness prompt, never as a file in the clone), one row per run.
//
// compute_machines is the daemon's memory of the machines it provisioned for
// compute_requests (D-D): which provider, the provider's name for it, the
// executor it became and where it answers, so a later daemon can release it
// when the request's hours run out. detail is provider JSON (tags, ids);
// never a credential.
var RunnerDDL = []string{
	`CREATE TABLE IF NOT EXISTS briefs (
  run_id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  text TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (run_id)
)`,
	`CREATE TABLE IF NOT EXISTS compute_machines (
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
)`,
}
