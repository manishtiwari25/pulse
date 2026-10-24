#![allow(dead_code)]

use crate::test_helpers::{TEST_NOW, new_store, run};
use rusqlite::params;
use std::path::{Path, PathBuf};

pub(crate) use crate::test_helpers_daemon::set_setting;

pub(crate) fn sweep_store() -> crate::test_helpers::TestStore {
    let s = new_store();
    set_setting(&s, "daemon.sweep_grace_minutes", "0");
    set_setting(&s, "daemon.sweep_branch_days", "0");
    s
}

pub(crate) fn write_sweep_wal(s: &pulse_store::Store) {
    let mut conn = s.conn().unwrap();
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0").unwrap();
    let tx = conn.transaction().unwrap();
    let payload = "x".repeat(8192);
    for _ in 0..1200 {
        if let Err(err) = tx.execute(
            "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'test', 'checkpoint.write', 'wal', ?)",
            params![TEST_NOW, payload],
        ) {
            tx.rollback().unwrap();
            panic!("{err}");
        }
    }
    tx.commit().unwrap();
    let wal = format!("{}-wal", s.path);
    let st = std::fs::metadata(&wal);
    assert!(
        st.as_ref().is_ok_and(|m| m.len() != 0),
        "test did not create a WAL: {:?}",
        st.err()
    );
}

pub(crate) fn sweep_job(s: &pulse_store::Store, status: &str, branch: &str) -> String {
    let (job, _) = crate::jobs::add_job(
        s,
        "proj".into(),
        "task".into(),
        format!("t-{}", crate::util::new_id("x", TEST_NOW)),
        "b".into(),
        1,
        0,
        "test".into(),
        pulse_store::HUMAN_AGENT.into(),
        false,
    )
    .unwrap();
    let finished_at = if matches!(status, "done" | "failed" | "cancelled") {
        crate::util::plus_hours(TEST_NOW, -2.0)
    } else {
        String::new()
    };
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = ?, branch = ?, finished_at = ? WHERE id = ?",
            params![status, branch, finished_at, job.str("id")],
        )
        .unwrap();
    job.str("id")
}

pub(crate) fn sweep_run(s: &pulse_store::Store, job_id: &str, status: &str) -> String {
    let id = crate::util::new_id("run", TEST_NOW);
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'w', 'proj', ?, ?)",
            params![id, job_id, TEST_NOW, status],
        )
        .unwrap();
    id
}

pub(crate) fn mkdir_file(path: impl AsRef<Path>, body: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

pub(crate) fn swept_paths(items: &[crate::sweep::Swept], kind: &str) -> Vec<String> {
    items
        .iter()
        .filter(|x| x.kind == kind)
        .map(|x| x.path.clone())
        .collect()
}

pub(crate) fn held_why(items: &[crate::sweep::Swept], path: impl AsRef<Path>) -> String {
    let path = path.as_ref().display().to_string();
    held_why_text(items, &path)
}

pub(crate) fn held_why_text(items: &[crate::sweep::Swept], path: &str) -> String {
    items
        .iter()
        .find(|x| x.path == path)
        .map(|x| x.why.clone())
        .unwrap_or_default()
}

pub(crate) fn project_rows(s: &pulse_store::Store) -> Vec<crate::util::Row> {
    crate::board::project_rows(&*s.conn().unwrap(), false)
}

pub(crate) fn onboard_repo(name: &str, mut files: Vec<(&str, String)>) -> String {
    let dir = tempfile::tempdir().unwrap().keep().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    run(&dir, &["git", "init", "-q", "-b", "main"]);
    files.push(("README.md", format!("# {name}\n")));
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    run(&dir, &["git", "add", "-A"]);
    run(&dir, &["git", "commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap().display().to_string()
}

pub(crate) fn simple_repo(name: &str) -> String {
    onboard_repo(name, vec![])
}

pub(crate) fn path_join(base: &str, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(PathBuf::from(base), |p, part| p.join(part))
}
