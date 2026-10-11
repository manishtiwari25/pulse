use anyhow::{Result, anyhow};
use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

pub(crate) static MANAGED_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9_-]{1,100}$").unwrap());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fleet {
    pub id: String,
    pub project: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub workers: i64,
    pub paths: Vec<String>,
    pub at: String,
    pub created_by: String,
    pub cleaned_at: String,
    #[serde(skip)]
    pub(crate) keys: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetWorker {
    pub worker: i64,
    pub path: String,
    pub detached: bool,
    pub no_remote: bool,
    pub no_tags: bool,
    pub stripped: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub problem: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetJob {
    pub id: String,
    pub status: String,
    pub result: String,
    pub branch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetVerification {
    pub fleet: Option<Fleet>,
    pub ok: bool,
    pub workers: Vec<FleetWorker>,
    pub jobs: Vec<FleetJob>,
}

pub(crate) fn fleet_paths(home: &str, f: &mut Fleet) {
    f.paths.clear();
    if !f.cleaned_at.is_empty() {
        return;
    }
    for i in 1..=f.workers {
        f.paths.push(
            Path::new(home)
                .join("fleet")
                .join(&f.id)
                .join(format!("w{i}"))
                .display()
                .to_string(),
        );
    }
}

pub fn prepare_fleet(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    ref_: &str,
    n: i64,
    actor: &str,
) -> Result<Fleet> {
    if !(1..=256).contains(&n) {
        return Err(crate::fixtures::input_error(
            "Cannot prepare fleet",
            "choose 1-256 workers per wave",
            "pulse fleet prepare --project P --n 1",
        ));
    }
    let p = crate::fixtures::valid_project(s, project)?;
    let (repo, commit) = crate::clones::resolve_revision(cancel, &p.path, ref_)?;
    let base = crate::clones::fleet_root(s, true)?;
    if crate::private::within(&base, &repo) {
        return Err(crate::fixtures::input_error(
            "Cannot prepare fleet",
            "the fleet directory must be outside the source checkout",
            "pulse --home DIR fleet prepare --project P --n 1",
        ));
    }
    let keys = crate::clones::project_answer_keys(s, project)?;
    let id = pulse_board::new_id(s, "fleet");
    let root = Path::new(&base).join(&id);
    fs::create_dir(&root)?;
    let mut fleet = Fleet {
        id: id.clone(),
        project: project.to_string(),
        ref_: commit.clone(),
        workers: n,
        at: s.now(),
        created_by: actor.to_string(),
        keys: keys.clone(),
        ..Default::default()
    };
    fleet_paths(
        Path::new(&base)
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .as_ref(),
        &mut fleet,
    );
    let result = (|| -> Result<()> {
        for clone in &fleet.paths {
            crate::clones::stripped_clone(cancel, &repo, &commit, clone, &keys)?;
        }
        let encoded = pulse_store::gojson::to_string(&keys)?;
        let tx = s.begin()?;
        tx.execute(
            "INSERT INTO evolution_fleets (id, project, ref, workers, stripped_json, at, created_by)
		VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, project, commit, n, encoded, fleet.at, actor],
        )?;
        tx.event(
            actor,
            "evolution.fleet.prepared",
            &id,
            serde_json::json!({"project": project, "workers": n, "ref": commit}),
        )?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = crate::clones::remove_managed(s, &id);
        return Err(e);
    }
    Ok(fleet)
}

pub fn list_fleets(s: &pulse_store::Store, project: &str) -> Result<Vec<Fleet>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, project, ref, workers, stripped_json, at, created_by, cleaned_at
		FROM evolution_fleets WHERE (? = '' OR project = ?) ORDER BY at DESC, id",
    )?;
    let mut rows = stmt.query(params![project, project])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut f = Fleet {
            id: row.get(0)?,
            project: row.get(1)?,
            ref_: row.get(2)?,
            workers: row.get(3)?,
            at: row.get(5)?,
            created_by: row.get(6)?,
            cleaned_at: row.get(7)?,
            ..Default::default()
        };
        let keys: String = row.get(4)?;
        if !MANAGED_ID.is_match(&f.id) || f.workers < 1 || f.workers > 256 {
            return Err(anyhow!("fleet record is malformed"));
        }
        f.keys = serde_json::from_str(&keys).map_err(|_| anyhow!("fleet record is malformed"))?;
        for key in &f.keys {
            crate::fixtures::relative_path(key)
                .map_err(|_| anyhow!("fleet strip list is malformed"))?;
        }
        fleet_paths(&s.home, &mut f);
        out.push(f);
    }
    Ok(out)
}

pub(crate) fn get_fleet(s: &pulse_store::Store, id: &str) -> Result<Fleet> {
    if !MANAGED_ID.is_match(id) {
        return Err(crate::fixtures::input_error(
            "Cannot find fleet",
            "expected a fleet id, not a directory path",
            "pulse fleet list",
        ));
    }
    for f in list_fleets(s, "")? {
        if f.id == id {
            return Ok(f);
        }
    }
    Err(crate::fixtures::input_error(
        "No such fleet",
        "this store has no fleet with that id",
        "pulse fleet list",
    ))
}

pub fn verify_fleet(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    id: &str,
) -> Result<FleetVerification> {
    let fleet = get_fleet(s, id)?;
    let base = crate::clones::fleet_root(s, false)?;
    let mut report = FleetVerification {
        fleet: Some(fleet.clone()),
        ok: true,
        workers: Vec::new(),
        jobs: Vec::new(),
    };
    if !fleet.cleaned_at.is_empty() {
        return Err(crate::fixtures::input_error(
            "Fleet has already been cleaned",
            "there are no worker clones to inspect",
            "pulse fleet list",
        ));
    }
    let entry = fs::symlink_metadata(Path::new(&base).join(id));
    if match entry.as_ref() {
        Ok(e) => !e.is_dir() || e.file_type().is_symlink(),
        Err(_) => true,
    } {
        return Err(crate::clones::ViolationError {
            reason: "fleet root is missing or no longer its original directory".to_string(),
        }
        .into());
    }
    for (i, clone) in fleet.paths.iter().enumerate() {
        let mut w = FleetWorker {
            worker: i as i64 + 1,
            path: clone.clone(),
            ..Default::default()
        };
        let entry = fs::symlink_metadata(clone);
        if match entry.as_ref() {
            Ok(e) => !e.is_dir() || e.file_type().is_symlink(),
            Err(_) => true,
        } {
            w.problem = "clone is missing or replaced".to_string();
        } else if crate::clones::check_clone(cancel, clone, &fleet.keys).is_err() {
            w.problem = "clone violates detached, remote-free, tag-free or stripping constraints"
                .to_string();
        } else {
            w.detached = true;
            w.no_remote = true;
            w.no_tags = true;
            w.stripped = true;
        }
        if !w.problem.is_empty() {
            report.ok = false;
        }
        report.workers.push(w);
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, status, result, branch FROM jobs WHERE source = ? ORDER BY created_at, id",
    )?;
    let mut rows = stmt.query(params![format!("fleet:{id}")])?;
    while let Some(row) = rows.next()? {
        let mut j = FleetJob {
            id: row.get(0)?,
            status: row.get(1)?,
            result: row.get(2)?,
            branch: row.get(3)?,
        };
        if pulse_store::secret_shaped(&j.result) {
            j.result = "[credential-shaped result withheld]".to_string();
        }
        report.jobs.push(j);
    }
    Ok(report)
}

pub fn clean_fleet(s: &pulse_store::Store, id: &str, actor: &str) -> Result<()> {
    let fleet = get_fleet(s, id)?;
    if !fleet.cleaned_at.is_empty() {
        return Ok(());
    }
    let conn = s.conn()?;
    let active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE source = ? AND status IN ('queued', 'leased')",
        params![format!("fleet:{id}")],
        |r| r.get(0),
    )?;
    drop(conn);
    if active != 0 {
        return Err(crate::fixtures::input_error(
            "Cannot clean active fleet",
            "worker jobs are still queued or leased",
            "pulse jobs",
        ));
    }
    crate::clones::remove_managed(s, id)?;
    let tx = s.begin()?;
    let n = tx.execute(
        "UPDATE evolution_fleets SET cleaned_at = ? WHERE id = ? AND cleaned_at = ''",
        params![s.now(), id],
    )?;
    if n == 0 {
        tx.commit()?;
        return Ok(());
    }
    tx.event(
        actor,
        "evolution.fleet.cleaned",
        id,
        serde_json::json!({"project": fleet.project}),
    )?;
    tx.commit()
}

pub(crate) fn archived_for_job(
    s: &pulse_store::Store,
    project: &str,
    job: &str,
) -> Result<Option<crate::archive::Archived>> {
    let conn = s.conn()?;
    let id: Option<String> = conn
        .query_row(
            "SELECT a.id FROM evolution_archive a JOIN runs r ON r.id = a.run_id
		WHERE a.project = ? AND r.project = a.project AND r.job_id = ?
		AND r.id = (SELECT latest.id FROM runs latest WHERE latest.job_id = r.job_id
			ORDER BY latest.started_at DESC, latest.id DESC LIMIT 1)
		ORDER BY a.at DESC, CAST(SUBSTR(a.id,3) AS INTEGER) DESC LIMIT 1",
            params![project, job],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    for row in crate::archive::list_archive(s, project)? {
        if row.id == id {
            return Ok(Some(row));
        }
    }
    Err(anyhow!("job archive disappeared"))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_measure::*;
    use std::path::Path;

    #[test]
    fn fleet_prepare_verify_store_results_and_clean() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        fixture(&s, "demo", "F01", "printf 1", 1.0);
        let ref_ = test_git(&repo, &["rev-parse", "HEAD"]);
        let cancel = pulse_bus::CancellationToken::new();
        let fleet = super::prepare_fleet(&cancel, &s, "demo", &ref_, 2, "human").unwrap();
        assert_eq!(fleet.paths.len(), 2);
        for (i, clone) in fleet.paths.iter().enumerate() {
            assert_eq!(Path::new(clone).file_name().unwrap(), ["w1", "w2"][i]);
            assert_eq!(test_git(clone, &["rev-parse", "HEAD"]), ref_);
            assert_eq!(test_git(clone, &["remote"]), "");
            assert_eq!(test_git(clone, &["tag", "--list"]), "");
            assert!(std::fs::metadata(Path::new(clone).join("keys/answer.txt")).is_err());
            put_file(
                Path::new(clone).join("worker-result.json"),
                "this is deliberately not JSON; never read it",
                0o600,
            );
        }
        let (job, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "demo".into(),
                kind: "task".into(),
                title: "fleet worker".into(),
                source: format!("fleet:{}", fleet.id),
                created_by: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(super::clean_fleet(&s, &fleet.id, "human").is_err());
        pulse_board::finish_job(
            &s,
            &job.id,
            "human",
            "done",
            "result recorded in the store",
            "",
            None,
        )
        .unwrap();
        let report = super::verify_fleet(&cancel, &s, &fleet.id).unwrap();
        assert!(report.ok);
        assert_eq!(report.jobs.len(), 1);
        assert_eq!(report.jobs[0].result, "result recorded in the store");
        super::clean_fleet(&s, &fleet.id, "human").unwrap();
        super::clean_fleet(&s, &fleet.id, "human").unwrap();
        assert_no_clones(&s);
        let list = super::list_fleets(&s, "demo").unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].cleaned_at.is_empty());
        assert!(list[0].paths.is_empty());
    }

    #[test]
    fn fleet_refuses_project_and_symlink_destinations() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let mut alias = s.clone();
        alias.home = Path::new(&repo).join("wrong-home").display().to_string();
        let cancel = pulse_bus::CancellationToken::new();
        assert!(super::prepare_fleet(&cancel, &alias, "demo", "HEAD", 1, "human").is_err());
        let outside = crate::test_helpers::temp_path();
        put_file(outside.join("preserve"), "untouched", 0o600);
        std::os::unix::fs::symlink(&outside, Path::new(&s.home).join("fleet")).unwrap();
        assert!(super::prepare_fleet(&cancel, &s, "demo", "HEAD", 1, "human").is_err());
        assert!(crate::clones::remove_managed(&s, "../outside").is_err());
        assert!(std::fs::metadata(outside.join("preserve")).is_ok());
    }

    #[test]
    fn fleet_detects_reattached_clone() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        test_project(&s, "demo");
        let cancel = pulse_bus::CancellationToken::new();
        let fleet = super::prepare_fleet(&cancel, &s, "demo", "HEAD", 1, "human").unwrap();
        test_git(
            &fleet.paths[0],
            &["switch", "--quiet", "-c", "worker-branch"],
        );
        let report = super::verify_fleet(&cancel, &s, &fleet.id).unwrap();
        assert!(!report.ok);
        super::clean_fleet(&s, &fleet.id, "human").unwrap();
    }
}
