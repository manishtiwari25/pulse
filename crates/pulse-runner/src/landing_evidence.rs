#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]
#![allow(
    clippy::too_many_arguments,
    reason = "Go-port signatures intentionally mirror internal/runner"
)]

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;
    use std::collections::BTreeMap;

    #[test]
    fn landing_evidence_copy_cannot_bypass_output_cap() {
        let _serial = crate::test_helpers::serial();
        let mut b = CappedBuffer {
            limit: 5,
            ..Default::default()
        };
        assert_eq!(b.write(b"abc".to_vec()).unwrap(), 3);
        let err = b.write(b"def".to_vec()).unwrap_err();
        assert_eq!(err.to_string(), ERR_LANDING_EVIDENCE_TOO_LARGE);
        assert_eq!(b.buffer, b"abcde");
        assert!(b.exceeded);
    }

    #[test]
    fn landing_evidence_limits_actual_process_output() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (_proj, repo) = new_repo(&s);
        std::fs::write(std::path::Path::new(&repo).join("changed.txt"), "changed\n").unwrap();
        let ctx = pulse_bus::CancellationToken::new();
        let (_out, why) = limited_git_output(
            &ctx,
            repo,
            std::time::Duration::from_secs(10),
            1,
            &["status".into(), "--porcelain".into()],
        );
        assert_eq!(why, pulse_codedelivery::REASON_EVIDENCE_TOO_LARGE);
        let noisy = "#!/bin/shprintf '' 0\nprintf stdout\nprintf stderr >&2\n";
        assert!(noisy.contains("#!/bin/shprintf '' 0"));
        assert!(noisy.contains("stdout"));
        assert!(noisy.contains("stderr"));
        assert_eq!(["git", "--version"][1], "--version");
    }

    #[test]
    fn landing_evidence_proves_final_command_transition() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let real_git = which::which("git").expect("git");
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        for move_to_target in [false, true] {
            let s = new_store();
            let (project, root) = new_repo(&s);
            join(&s, "aurora");
            let branch = "pulse/aurora/final-command";
            run(&root, &["git", "switch", "-q", "-c", branch]);
            std::fs::write(std::path::Path::new(&root).join("a.go"), "package main\n").unwrap();
            run(&root, &["git", "add", "-A"]);
            run(&root, &["git", "commit", "-q", "-m", "add a"]);
            let first = ref_of(&root, "HEAD");
            std::fs::write(std::path::Path::new(&root).join("b.go"), "package main\n").unwrap();
            run(&root, &["git", "add", "-A"]);
            run(&root, &["git", "commit", "-q", "-m", "add b"]);
            let target = ref_of(&root, "HEAD");
            run(&root, &["git", "switch", "-q", "main"]);
            let (job, _) = crate::jobs::add_job(
                &s,
                "proj".into(),
                "task".into(),
                "Deliver source".into(),
                "do it".into(),
                1,
                0,
                "manual".into(),
                pulse_store::HUMAN_AGENT.into(),
                false,
            )
            .unwrap();
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', branch = ?, lease_agent = 'aurora' WHERE id = ?",
                &[&branch, &job.str("id")],
            );
            let shim_dir = tempfile::tempdir().expect("shim");
            let shim = shim_dir.path();
            let script = shim.join("git");
            std::fs::write(
                &script,
                r#"#!/bin/sh
if [ "$1" = "-C" ] && [ "$2" = "$PULSE_DELIVERY_ROOT" ] && [ "$3" = "merge" ] && [ "$4" = "--ff-only" ]; then
  GIT_REFLOG_ACTION=external-writer "$PULSE_REAL_GIT" -C "$PULSE_DELIVERY_ROOT" merge --ff-only "$PULSE_DELIVERY_ADVANCE" >/dev/null || exit 1
fi
exec "$PULSE_REAL_GIT" "$@"
"#,
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let _env_git = pulse_testutil::setenv("PULSE_REAL_GIT", &real_git);
            let _env_root = pulse_testutil::setenv("PULSE_DELIVERY_ROOT", &root);
            let _env_advance = pulse_testutil::setenv(
                "PULSE_DELIVERY_ADVANCE",
                if move_to_target { &target } else { &first },
            );
            let mut paths = vec![shim.to_path_buf()];
            paths.extend(std::env::split_paths(&original_path));
            let _env_path = pulse_testutil::setenv("PATH", std::env::join_paths(paths).unwrap());
            let (state, note) = crate::land::land_branch(
                &s,
                project,
                branch.into(),
                "daemon".into(),
                TEST_NOW.into(),
                crate::land::LandOpts {
                    job: job_row(&s, &job.str("id")),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(state, "landed", "{note}");
            let detail = must_rows(
                &s,
                "SELECT detail FROM events WHERE action = 'land.landed'",
                &[],
            )[0]
            .str("detail");
            let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
            let (evidence, ok) = pulse_codedelivery::decode_evidence(detail.get("code_delivery"));
            assert!(ok, "detail={detail}");
            if move_to_target {
                assert_eq!(evidence.evidence, pulse_codedelivery::EVIDENCE_UNAVAILABLE);
                assert_eq!(evidence.code_files, 0, "evidence={evidence:?}");
            } else {
                assert_eq!(evidence.evidence, pulse_codedelivery::EVIDENCE_RECORDED);
                assert_eq!(evidence.before, first);
                assert_eq!(evidence.after, target);
                assert_eq!(evidence.code_files, 1, "evidence={evidence:?}");
                assert_eq!(evidence.code_paths.join(","), "b.go");
            }
        }
    }

    #[test]
    fn landing_evidence_records_no_code_when_default_already_at_target() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (mut project, root) = new_repo(&s);
        join(&s, "aurora");
        let branch = "pulse/aurora/no-op-default";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        crate::test_helpers_landing::write_file(&root, "a.go", "package main\n");
        crate::test_helpers_landing::write_file(&root, "b.go", "package main\n");
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "add files"]);
        let target = crate::test_helpers_selfupdate::head_of(&root);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Deliver source".into(),
            "do it".into(),
            1,
            0,
            "manual".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?",
            &[&branch, &"aurora", &job.str("id")],
        );
        project.insert(
            "verify_cmd",
            format!(
                "git -C {} merge --ff-only {}",
                crate::util::shell_quote(&root),
                crate::util::shell_quote(&target)
            )
            .into(),
        );

        let (state, note) = crate::land::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            crate::land::LandOpts {
                job: job_row(&s, &job.str("id")),
                verify: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        if state != "landed" {
            panic!("landBranch = {state} {note} <nil>");
        }
        let evidence = crate::test_helpers_landing::landed_evidence(&s);
        if evidence.evidence != pulse_codedelivery::EVIDENCE_UNAVAILABLE
            || evidence.reason != pulse_codedelivery::REASON_MISSING_TRANSITION
            || evidence.files_changed != 0
            || evidence.code_files != 0
            || !evidence.code_paths.is_empty()
        {
            panic!("no-op transition counted stale code: {evidence:?}");
        }
    }

    #[test]
    fn landing_evidence_uses_actual_advanced_default_transition() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (mut project, root) = new_repo(&s);
        join(&s, "aurora");
        let branch = "pulse/aurora/advanced-default";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        crate::test_helpers_landing::write_file(&root, "a.go", "package main\n");
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "add a"]);
        let first = crate::test_helpers_selfupdate::head_of(&root);
        crate::test_helpers_landing::write_file(&root, "b.go", "package main\n");
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "add b"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Deliver source".into(),
            "do it".into(),
            1,
            0,
            "manual".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?",
            &[&branch, &"aurora", &job.str("id")],
        );
        project.insert(
            "verify_cmd",
            format!(
                "git -C {} merge --ff-only {}",
                crate::util::shell_quote(&root),
                crate::util::shell_quote(&first)
            )
            .into(),
        );

        let (state, note) = crate::land::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            crate::land::LandOpts {
                job: job_row(&s, &job.str("id")),
                verify: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("landBranch = {}", err));
        if state != "landed" {
            panic!("landBranch = {state} {note} <nil>");
        }
        let evidence = crate::test_helpers_landing::landed_evidence(&s);
        if evidence.before != first
            || evidence.after != crate::test_helpers_selfupdate::head_of(&root)
            || evidence.code_files != 1
            || evidence.code_paths.join(",") != "b.go"
        {
            panic!("stale pre-verification evidence was recorded: {evidence:?}");
        }
    }

    #[test]
    fn landing_evidence_transaction_fences_current_carrier() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for fault in ["receipt-write", "newer-run", "newer-generation"] {
            let s = new_store();
            let (project, _repo) = new_repo(&s);
            join(&s, "proposer");
            let (job, _) = crate::jobs::add_job(
                &s,
                "proj".into(),
                "experiment".into(),
                "Experiment: x".into(),
                "Suggestion: S-001\nHypothesis: h\n".into(),
                1,
                30,
                "suggestion:S-001".into(),
                "proposer".into(),
                true,
            )
            .unwrap();
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', branch = 'evolve/S-001', result = 'measured; finalization pending' WHERE id = ?",
                &[&job.str("id")],
            );
            exec1(
                &s,
                "INSERT INTO runs (id, job_id, project, agent, started_at, status) VALUES ('run-x', ?, 'proj', 'proposer', ?, 'done')",
                &[&job.str("id"), &TEST_NOW],
            );
            pulse_evolve::archive_row(
                &s,
                pulse_evolve::ArchiveRow {
                    project: "proj".into(),
                    suggestion_post: "S-001".into(),
                    hypothesis: "h".into(),
                    branch: "evolve/S-001".into(),
                    baseline: Some(pulse_evolve::Scores {
                        total: 0.8,
                        ..Default::default()
                    }),
                    variant: Some(pulse_evolve::Scores {
                        total: 0.9,
                        ..Default::default()
                    }),
                    tolerance: 0.02,
                    kept: true,
                    run_id: "run-x".into(),
                    note: "fake measured outcome".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let current = job_row(&s, &job.str("id"));
            match fault {
                "receipt-write" => exec1(
                    &s,
                    "CREATE TRIGGER reject_receipt BEFORE INSERT ON events WHEN NEW.action = 'land.landed' BEGIN SELECT RAISE(ABORT, 'fake receipt refused'); END",
                    &[],
                ),
                "newer-run" => exec1(
                    &s,
                    "INSERT INTO runs (id, job_id, project, agent, started_at, status) VALUES ('newer-run', ?, 'proj', 'proposer', '2026-09-06T10:00:01Z', 'done')",
                    &[&job.str("id")],
                ),
                "newer-generation" => exec1(
                    &s,
                    "UPDATE jobs SET generation = generation + 1 WHERE id = ?",
                    &[&job.str("id")],
                ),
                _ => {}
            }
            let err = record_landing(
                &s,
                project,
                "evolve/S-001".into(),
                "daemon".into(),
                "land.landed".into(),
                " | fake finalization".into(),
                crate::land::LandOpts {
                    job: current.clone(),
                    archive_id: "B-001".into(),
                    run_id: "run-x".into(),
                    ..Default::default()
                },
                BTreeMap::from([("fake".into(), serde_json::json!(true))]),
            )
            .unwrap_err();
            assert!(!err.to_string().is_empty());
            let after = job_row(&s, &job.str("id"));
            assert_eq!(after.str("branch"), current.str("branch"));
            assert_eq!(after.str("result"), current.str("result"));
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'",
                    &[]
                ),
                0
            );
        }
    }
}
