use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(crate) const LANDING_EVIDENCE_MAX_BYTES: usize = 2 << 20;
pub(crate) const ERR_LANDING_EVIDENCE_TOO_LARGE: &str = "landing evidence too large";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CappedBuffer {
    pub(crate) buffer: Vec<u8>,
    pub(crate) limit: i32,
    pub(crate) exceeded: bool,
}

/// Go: func landingRefTransition(ctx context.Context, root, ref, target, action string) (string, string)
pub(crate) fn landing_ref_transition(
    ctx: &pulse_bus::CancellationToken,
    root: String,
    r#ref: String,
    target: String,
    action: String,
) -> (String, String) {
    let (raw, reason) = limited_git_output(
        ctx,
        root,
        Duration::from_secs(10),
        64 << 10,
        &[
            "reflog".into(),
            "show".into(),
            "--max-count=32".into(),
            "--format=%H%x00%gs%x00".into(),
            r#ref,
        ],
    );
    if !reason.is_empty() {
        return (String::new(), String::new());
    }
    let fields = String::from_utf8_lossy(&raw)
        .split('\0')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut matched = None;
    let mut i = 0;
    while i + 1 < fields.len() {
        if fields[i + 1].starts_with(&format!("{action}: ")) {
            if matched.is_some() {
                return (String::new(), String::new());
            }
            matched = Some(i);
        }
        i += 2;
    }
    let Some(m) = matched else {
        return (String::new(), String::new());
    };
    if m + 3 >= fields.len() {
        return (String::new(), String::new());
    }
    let after = fields[m].trim().to_string();
    let before = fields[m + 2].trim().to_string();
    if after != target || !landing_object_id(before.clone()) {
        return (String::new(), String::new());
    }
    (before, after)
}

/// Go: func landingObjectID(value string) bool
pub(crate) fn landing_object_id(value: String) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Go: func experimentArchive(s *store.Store, project, branch, run string) (row, error)
pub(crate) fn experiment_archive(
    s: &pulse_store::Store,
    project: String,
    branch: String,
    run: String,
) -> anyhow::Result<crate::util::Row> {
    if run.is_empty() {
        return Ok(crate::util::Row::new());
    }
    let conn = s.conn()?;
    Ok(crate::util::query_one(
        &conn,
        "SELECT id, note, kept FROM evolution_archive
		WHERE project = ? AND branch = ? AND run_id = ?
		ORDER BY at DESC, CAST(SUBSTR(id, 3) AS INTEGER) DESC LIMIT 1",
        &[&project, &branch, &run],
    )?
    .unwrap_or_default())
}

/// Go: func landingCodeDeliveryEvidence(ctx context.Context, root, before, after string) codedelivery.Evidence
pub(crate) fn landing_code_delivery_evidence(
    ctx: &pulse_bus::CancellationToken,
    root: String,
    before: String,
    after: String,
) -> pulse_codedelivery::Evidence {
    if before.is_empty() || after.is_empty() {
        return pulse_codedelivery::unavailable_evidence(
            &before,
            &after,
            pulse_codedelivery::REASON_MISSING_TRANSITION,
        );
    }
    if before == after {
        return pulse_codedelivery::summarize_name_status(
            &[],
            &before,
            &after,
            pulse_codedelivery::DEFAULT_CODE_PATH_LIMIT,
            pulse_codedelivery::DEFAULT_FILE_LIMIT,
            None,
        )
        .unwrap_or_else(|_| {
            pulse_codedelivery::unavailable_evidence(
                &before,
                &after,
                pulse_codedelivery::REASON_EVIDENCE_MALFORMED,
            )
        });
    }
    let (name_status, reason) = limited_git_output(
        ctx,
        root.clone(),
        Duration::from_secs(60),
        LANDING_EVIDENCE_MAX_BYTES as i32,
        &[
            "diff".into(),
            "--name-status".into(),
            "-z".into(),
            "--find-renames".into(),
            format!("{before}..{after}"),
        ],
    );
    if !reason.is_empty() {
        return pulse_codedelivery::unavailable_evidence(&before, &after, &reason);
    }
    let (numstat, reason) = limited_git_output(
        ctx,
        root,
        Duration::from_secs(60),
        LANDING_EVIDENCE_MAX_BYTES as i32,
        &[
            "diff".into(),
            "--numstat".into(),
            "-z".into(),
            "--no-renames".into(),
            format!("{before}..{after}"),
        ],
    );
    if !reason.is_empty() {
        return pulse_codedelivery::unavailable_evidence(&before, &after, &reason);
    }
    let (binaries, truncated) = match pulse_codedelivery::parse_numstat_binary_paths(
        &numstat,
        pulse_codedelivery::DEFAULT_FILE_LIMIT,
    ) {
        Ok(v) => v,
        Err(_) => {
            return pulse_codedelivery::unavailable_evidence(
                &before,
                &after,
                pulse_codedelivery::REASON_EVIDENCE_MALFORMED,
            );
        }
    };
    if truncated {
        return pulse_codedelivery::unavailable_evidence(
            &before,
            &after,
            pulse_codedelivery::REASON_EVIDENCE_TOO_LARGE,
        );
    }
    match pulse_codedelivery::summarize_name_status(
        &name_status,
        &before,
        &after,
        pulse_codedelivery::DEFAULT_CODE_PATH_LIMIT,
        pulse_codedelivery::DEFAULT_FILE_LIMIT,
        Some(&binaries),
    ) {
        Ok(e) if !e.changes_truncated => e,
        Ok(_) => pulse_codedelivery::unavailable_evidence(
            &before,
            &after,
            pulse_codedelivery::REASON_EVIDENCE_TOO_LARGE,
        ),
        Err(_) => pulse_codedelivery::unavailable_evidence(
            &before,
            &after,
            pulse_codedelivery::REASON_EVIDENCE_MALFORMED,
        ),
    }
}

/// Go: func limitedGitOutput(ctx context.Context, repo string, timeout time.Duration, maxBytes int, args ...string) ([]byte, string)
pub(crate) fn limited_git_output(
    ctx: &pulse_bus::CancellationToken,
    repo: String,
    timeout: std::time::Duration,
    max_bytes: i32,
    args: &[String],
) -> (Vec<u8>, String) {
    let child = ctx.child();
    let timer = child.with_timeout(timeout);
    let mut full = Vec::new();
    if !repo.is_empty() {
        full.extend(["-C".to_string(), repo]);
    }
    full.extend(args.iter().cloned());
    let mut cmd = Command::new("git");
    cmd.args(&full);
    cmd.env_clear();
    for kv in crate::util::strip_hook_git_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    ) {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Ok(mut child_proc) = cmd.spawn() else {
        return (
            Vec::new(),
            pulse_codedelivery::REASON_GIT_UNAVAILABLE.to_string(),
        );
    };
    let mut stdout = child_proc.stdout.take().unwrap();
    let mut stderr = child_proc.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    loop {
        if timer.is_cancelled() {
            let _ = child_proc.kill();
            let _ = child_proc.wait();
            return (
                Vec::new(),
                pulse_codedelivery::REASON_GIT_UNAVAILABLE.to_string(),
            );
        }
        match child_proc.try_wait() {
            Ok(Some(st)) => {
                let out = out_t.join().unwrap_or_default();
                let err = err_t.join().unwrap_or_default();
                if out.len() > max_bytes as usize || err.len() > 4096 {
                    return (
                        Vec::new(),
                        pulse_codedelivery::REASON_EVIDENCE_TOO_LARGE.to_string(),
                    );
                }
                if !st.success() {
                    return (
                        Vec::new(),
                        pulse_codedelivery::REASON_GIT_UNAVAILABLE.to_string(),
                    );
                }
                return (out, String::new());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                return (
                    Vec::new(),
                    pulse_codedelivery::REASON_GIT_UNAVAILABLE.to_string(),
                );
            }
        }
    }
}

/// Go: func landingWorkSnapshot(s *store.Store, project, job row, o landOpts) codedelivery.WorkSnapshot
pub(crate) fn landing_work_snapshot(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    o: crate::land::LandOpts,
) -> pulse_codedelivery::WorkSnapshot {
    if job.0.is_empty() {
        return pulse_codedelivery::WorkSnapshot::default();
    }
    let mut work = pulse_codedelivery::WorkSnapshot {
        job: Some(pulse_codedelivery::JobSnapshot {
            id: job.str("id"),
            title: job.str("title"),
            kind: job.str("kind"),
            status: job.str("status"),
            source: job.str("source"),
            generation: job.int("generation"),
        }),
        ..Default::default()
    };
    let mut worker = pulse_codedelivery::WorkerSnapshot {
        name: job.str("lease_agent"),
        run_id: o.run_id,
        ..Default::default()
    };
    if worker.run_id.is_empty()
        && let Ok(conn) = s.conn()
        && let Ok(Some(row)) = crate::util::query_one(
            &conn,
            "SELECT id, agent, executor FROM runs
			WHERE job_id = ? AND project = ? AND status = 'done'
			ORDER BY finished_at DESC, started_at DESC, id DESC LIMIT 1",
            &[&job.str("id"), &project.str("slug")],
        )
    {
        worker.run_id = row.str("id");
        if worker.name.is_empty() {
            worker.name = row.str("agent");
        }
        worker.executor = row.str("executor");
    }
    if !worker.name.is_empty() {
        if let Ok(Some(agent)) = s.agent(&worker.name) {
            worker.runner = agent.runner;
            worker.model = agent.model;
        }
        work.worker = Some(worker);
    }
    work
}

/// Go: func recordLanding(s *store.Store, project row, branch, actor, action, result string, o landOpts, detail map[string]any) error
pub(crate) fn record_landing(
    s: &pulse_store::Store,
    project: crate::util::Row,
    branch: String,
    actor: String,
    action: String,
    result: String,
    o: crate::land::LandOpts,
    detail: BTreeMap<String, serde_json::Value>,
) -> anyhow::Result<()> {
    let tx = s.begin()?;
    let job = o.job.clone();
    let mut detail = detail;
    detail.insert("project".into(), serde_json::json!(project.str("slug")));
    detail.insert(
        "job".into(),
        serde_json::json!(crate::land::job_id_of(job.clone())),
    );
    if !o.archive_id.is_empty() {
        if job.0.is_empty() || o.run_id.is_empty() {
            let _ = tx.rollback();
            anyhow::bail!(
                "cannot finalize archive {} without its experiment run",
                o.archive_id
            );
        }
        let kept = action == "land.landed" || action == "land.merged";
        let current: bool = tx.query_row("SELECT EXISTS (
			SELECT 1 FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
			JOIN jobs j ON j.id = r.job_id AND j.project = a.project
			WHERE a.project = ? AND a.id = ? AND a.branch = ? AND a.run_id = ? AND a.kept = ?
			AND j.id = ? AND j.kind = 'experiment' AND j.status = 'done' AND r.status = 'done'
			AND r.id = (SELECT latest.id FROM runs latest WHERE latest.job_id = j.id
				ORDER BY latest.started_at DESC, latest.id DESC LIMIT 1)
			AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = j.id AND active.status = 'running'))",
            rusqlite::params![project.str("slug"), o.archive_id, branch, o.run_id, kept, job.str("id")], |r| r.get(0))?;
        if !current {
            let _ = tx.rollback();
            anyhow::bail!(
                "experiment {} changed before archive {} could be finalized; owner review needed",
                job.str("id"),
                o.archive_id
            );
        }
        detail.insert("archive".into(), serde_json::json!(o.archive_id));
        detail.insert("run_id".into(), serde_json::json!(o.run_id));
    }
    if !job.0.is_empty() {
        let changed = tx.execute(
            "UPDATE jobs SET branch = '', result = result || ?
			WHERE id = ? AND branch = ? AND generation = ?",
            rusqlite::params![result, job.str("id"), branch, job.int("generation")],
        )?;
        if changed != 1 {
            let _ = tx.rollback();
            anyhow::bail!(
                "job {} changed before landing finalization; owner review needed",
                job.str("id")
            );
        }
    }
    tx.event(&actor, &action, &branch, serde_json::json!(detail))?;
    tx.commit()
}

/// Go: func failMissingExperiment(s *store.Store, project, job row, actor, run, archive, reason string) error
pub(crate) fn fail_missing_experiment(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    run: String,
    archive: String,
    reason: String,
) -> anyhow::Result<()> {
    let tx = s.begin()?;
    let changed = tx.execute("UPDATE jobs SET status = 'failed', branch = '', finished_at = ?, lease_until = '',
		result = result || ? WHERE id = ? AND status = 'done' AND branch = ? AND generation = ?
		AND COALESCE((SELECT r.id FROM runs r WHERE r.job_id = jobs.id
			ORDER BY r.started_at DESC, r.id DESC LIMIT 1), '') = ?
		AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = jobs.id AND active.status = 'running')",
        rusqlite::params![s.now(), format!(" | {reason}"), job.str("id"), job.str("branch"), job.int("generation"), run])?;
    if changed != 1 {
        let _ = tx.rollback();
        anyhow::bail!(
            "experiment {} changed before its missing artifact could be recorded",
            job.str("id")
        );
    }
    let detail = serde_json::json!({"project": project.str("slug"), "job": job.str("id"), "run_id": run, "archive": archive, "result": reason});
    tx.event(&actor, "job.failed", &job.str("id"), detail.clone())?;
    tx.event(&actor, "land.missing", &job.str("branch"), detail)?;
    tx.commit()
}

impl CappedBuffer {
    /// Go: func (b *cappedBuffer) Write(p []byte) (int, error)
    pub(crate) fn write(&mut self, p: Vec<u8>) -> anyhow::Result<i32> {
        let space = self.limit as isize - self.buffer.len() as isize;
        if space > 0 {
            let n = (space as usize).min(p.len());
            self.buffer.extend_from_slice(&p[..n]);
            if n == p.len() {
                return Ok(p.len() as i32);
            }
        }
        self.exceeded = true;
        Err(anyhow!(ERR_LANDING_EVIDENCE_TOO_LARGE))
    }
}
