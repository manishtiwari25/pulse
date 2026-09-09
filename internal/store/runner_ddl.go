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
//
// retired_vms (ADR-034) remembers a VM the daemon stopped because its
// executor's login was refused: where it is, when it was stopped and when the
// daemon may delete it (delete_after); deleted_at closes the row, note says
// why when nothing was deleted (kept by the owner, already gone).
// resource_group is the Azure scope the first providers wrote; detail (added
// later) is the machine's provider JSON (tags, ids, region or zone) so any
// provider can find its machine again at delete time. Never a credential.
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
	`CREATE TABLE IF NOT EXISTS retired_vms (
  executor TEXT NOT NULL,
  provider TEXT NOT NULL DEFAULT '',
  resource_group TEXT NOT NULL DEFAULT '',
  name TEXT NOT NULL DEFAULT '',
  deallocated_at TEXT NOT NULL DEFAULT '',
  delete_after TEXT NOT NULL DEFAULT '',
  deleted_at TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (executor)
)`,
}

// RunnerColumns are the columns the runner work added to schema-3 tables after
// the fact (ADR-034); EnsureRecords adds each one that is missing.
// executors.admin is the admin ssh address (user@host) pair, executor login
// --admin or the compute provider used, so the daemon can place a login there;
// executors.healed_at marks the one heal attempt per drain episode and is
// cleared when the executor is active again. retired_vms.detail carries the
// retired machine's provider facts for the providers that came after Azure.
var RunnerColumns = []Column{
	{Table: "executors", Name: "admin", DDL: "TEXT NOT NULL DEFAULT ''"},
	{Table: "executors", Name: "healed_at", DDL: "TEXT NOT NULL DEFAULT ''"},
	{Table: "retired_vms", Name: "detail", DDL: "TEXT NOT NULL DEFAULT '{}'"},
}
