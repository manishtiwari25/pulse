use crate::test_helpers::*;
use crate::util::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryMetaNote {
    pub(crate) at: String,
    pub(crate) note: String,
}

pub(crate) fn set_recover_setting(s: &pulse_store::Store, key: &str, value: &str) {
    s.set_setting(crate::util::HUMAN_AGENT, key, value)
        .expect("set recover setting");
}

pub(crate) fn insert_recover_failed_job(
    s: &pulse_store::Store,
    id: &str,
    kind: &str,
    source: &str,
    result: &str,
    finished: &str,
) -> Row {
    let conn = s.conn().expect("conn");
    conn.execute(
        "INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by,
\t\tlease_agent, attempts, finished_at, result, branch)
\t\tVALUES (?, 'proj', ?, 1, ?, ?, ?, 7, 'failed', ?, 'tester', 'w1', 3, ?, ?, ?)",
        rusqlite::params![
            id,
            kind,
            format!("Original {id}"),
            format!("original brief {id}"),
            source,
            crate::util::plus_hours(finished, -1.0),
            finished,
            result,
            format!("pulse/w1/{id}")
        ],
    )
    .expect("insert failed job");
    crate::jobs::job_row(&conn, id.to_string())
}

pub(crate) fn insert_recover_run(
    s: &pulse_store::Store,
    job_id: &str,
    run_id: &str,
    status: &str,
    summary: &str,
    log_path: &str,
    branch: &str,
) -> Row {
    let conn = s.conn().expect("conn");
    conn.execute(
        "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, branch, summary, log, executor)
\t\tVALUES (?, ?, 'w1', 'proj', ?, ?, ?, ?, ?, ?, 'local')",
        rusqlite::params![
            run_id,
            job_id,
            crate::util::plus_hours(TEST_NOW, -2.0),
            crate::util::plus_hours(TEST_NOW, -1.0),
            status,
            branch,
            summary,
            log_path
        ],
    )
    .expect("insert run");
    crate::util::query_one(&conn, "SELECT * FROM runs WHERE id = ?", &[&run_id])
        .expect("query run")
        .unwrap_or_default()
}

pub(crate) fn recover_meta(s: &pulse_store::Store) -> BTreeMap<String, RecoveryMetaNote> {
    let conn = s.conn().expect("conn");
    let r = crate::util::query_one(&conn, "SELECT value FROM meta WHERE key = 'recovery'", &[])
        .expect("query recovery meta")
        .unwrap_or_default();
    if r.0.is_empty() {
        return BTreeMap::new();
    }
    serde_json::from_str(&r.str("value")).expect("decode recovery meta")
}

pub(crate) fn recover_detail(s: &pulse_store::Store, job_id: &str) -> BTreeMap<String, Value> {
    let conn = s.conn().expect("conn");
    let r = crate::util::query_one(
        &conn,
        "SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &job_id],
    )
    .expect("query recovery detail")
    .unwrap_or_default();
    assert!(!r.0.is_empty(), "no recovery event for {job_id}");
    crate::util::json_map(&r.str("detail"))
}
