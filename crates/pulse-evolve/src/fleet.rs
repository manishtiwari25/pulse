//! Port of internal/evolve/fleet.go.

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

/// Go: func fleetPaths(home string, f *Fleet) {
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

/// Go: func PrepareFleet(ctx context.Context, s *store.Store, project, ref string, n int, actor string) (fleet *Fleet, retErr error) {
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

/// Go: func ListFleets(s *store.Store, project string) ([]Fleet, error) {
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

/// Go: func getFleet(s *store.Store, id string) (*Fleet, error) {
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

/// Go: func VerifyFleet(ctx context.Context, s *store.Store, id string) (*FleetVerification, error) {
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

/// Go: func CleanFleet(s *store.Store, id, actor string) error {
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

/// Go: func archivedForJob(s *store.Store, project, job string) (*Archived, error) {
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
    #[test]
    fn fleet_prepare_verify_store_results_and_clean() {}
    #[test]
    fn fleet_refuses_project_and_symlink_destinations() {}
    #[test]
    fn fleet_detects_reattached_clone() {}
}
