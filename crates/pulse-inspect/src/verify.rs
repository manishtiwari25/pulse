use crate::register::{current_project, json_output, no_args, or_dash, py_repr, store};
use anyhow::{Result, anyhow};
use pulse_cli::Context;
use pulse_cli::flags::FlagSet;
use rusqlite::params_from_iter;
use serde::{Deserialize, Serialize};
use std::io::{Seek, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signer {
    pub agent: String,
    pub signing_pubkey: String,
    pub id: String,
    pub job_id: String,
    pub project: String,
}

impl Signer {
    pub fn line(&self) -> String {
        format!(
            "{}@pulse.local namespaces=\"git\" {} {}/{}",
            self.agent, self.signing_pubkey, self.project, self.id
        )
    }
}

pub fn signers(s: &pulse_store::Store, project: &str) -> Result<Vec<Signer>> {
    let mut q =
        "SELECT agent, signing_pubkey, id, job_id, project FROM runs WHERE signing_pubkey != ''"
            .to_string();
    let mut params = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        params.push(project.to_string());
    }
    q.push_str(" ORDER BY started_at");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        Ok(Signer {
            agent: r.get(0)?,
            signing_pubkey: r.get(1)?,
            id: r.get(2)?,
            job_id: r.get(3)?,
            project: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn run_signers(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let signers = signers(store(c)?, &c.project)?;
    if c.json {
        return json_output(c, &signers);
    }
    if signers.is_empty() {
        writeln!(
            c.err,
            "# no run has a signing key yet (ADR-022); runs made before it carry none"
        )?;
    }
    for signer in signers {
        c.println(signer.line())?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedRun {
    pub id: String,
    pub agent: String,
    pub executor: String,
    pub status: String,
    pub started_at: String,
    pub signature: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub commit: String,
    pub author: String,
    pub author_email: String,
    pub pulse_agent: String,
    pub pulse_job: String,
    pub pulse_executor: String,
    pub runs: Vec<VerifiedRun>,
    pub verified: bool,
    pub verdict: String,
}

impl Verification {
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "commit    {}\nauthor    {} <{}>\ntrailers  Pulse-Agent: {}  Pulse-Job: {}  Pulse-Executor: {}\n",
            self.commit,
            self.author,
            self.author_email,
            or_dash(&self.pulse_agent),
            or_dash(&self.pulse_job),
            or_dash(&self.pulse_executor)
        ));
        for r in &self.runs {
            out.push_str(&format!(
                "run       {}  agent {}  executor {}  status {}  started {}  signature {}\n",
                r.id, r.agent, r.executor, r.status, r.started_at, r.signature
            ));
        }
        out.push_str(&format!("verdict   {}\n", self.verdict));
        out
    }
}

pub fn git_output(root: &str, args: &[String]) -> Result<(String, String, i32)> {
    let mut cmd = Command::new("git");
    cmd.arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .envs(
            pulse_gitenv::strip_hook(std::env::vars().map(|(k, v)| format!("{k}={v}")).collect())
                .into_iter()
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                }),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            return Err(anyhow!("git in {}: context deadline exceeded", root));
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.into()),
        }
    }
    let out = child.wait_with_output()?;
    Ok((
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(1),
    ))
}

pub fn verify_commit(
    s: &pulse_store::Store,
    project: &pulse_store::Project,
    refname: &str,
) -> Result<Verification> {
    let (sha, _, code) = git_output(
        &project.path,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "--end-of-options".into(),
            format!("{refname}^{{commit}}"),
        ],
    )?;
    if code != 0 {
        return Err(pulse_cli::errorf(
            2,
            &format!("pulse: no commit {} in {}", py_repr(refname), project.path),
            "",
            "",
        )
        .into());
    }
    let sha = sha.trim().to_string();
    let format = "%an%x1f%ae%x1f%(trailers:key=Pulse-Agent,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Job,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Executor,valueonly,separator=%x1d)";
    let (out, stderr, code) = git_output(
        &project.path,
        &[
            "log".into(),
            "-1".into(),
            format!("--format={format}"),
            sha.clone(),
        ],
    )?;
    if code != 0 {
        return Err(anyhow!(
            "git log failed in {}: {}",
            project.path,
            stderr.trim()
        ));
    }
    let mut fields: Vec<String> = out
        .split('\x1f')
        .map(|s| s.trim().replace('\x1d', ", "))
        .collect();
    fields.resize(5, String::new());
    let mut v = Verification {
        commit: sha.clone(),
        author: fields[0].clone(),
        author_email: fields[1].clone(),
        pulse_agent: fields[2].clone(),
        pulse_job: fields[3].clone(),
        pulse_executor: fields[4].clone(),
        runs: vec![],
        ..Default::default()
    };
    if v.pulse_job.is_empty() {
        v.verdict = "NOT verified: no Pulse-Job trailer - not a PULSE worker commit (a human's, or made before ADR-022)".into();
        return Ok(v);
    }
    struct Candidate {
        run: VerifiedRun,
        key: String,
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, agent, executor, status, started_at, signing_pubkey FROM runs
		WHERE job_id = ? AND signing_pubkey != '' ORDER BY started_at DESC",
    )?;
    let candidates: Vec<Candidate> = stmt
        .query_map([&v.pulse_job], |r| {
            Ok(Candidate {
                run: VerifiedRun {
                    id: r.get(0)?,
                    agent: r.get(1)?,
                    executor: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    ..Default::default()
                },
                key: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if candidates.is_empty() {
        v.verdict = format!(
            "NOT verified: the store has no run with a key for job {}",
            v.pulse_job
        );
        return Ok(v);
    }
    let path = std::env::temp_dir().join(format!(
        "pulse-verify-{}-{}.signers",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&path)?;
    for c in candidates {
        file.set_len(0)?;
        file.rewind()?;
        file.write_all(
            format!("{}@pulse.local namespaces=\"git\" {}\n", c.run.agent, c.key).as_bytes(),
        )?;
        file.flush()?;
        let path_text = path.display().to_string();
        let (signature, _, code) = git_output(
            &project.path,
            &[
                "-c".into(),
                "gpg.ssh.program=ssh-keygen".into(),
                "-c".into(),
                format!("gpg.ssh.allowedSignersFile={path_text}"),
                "log".into(),
                "-1".into(),
                "--format=%G?".into(),
                sha.clone(),
            ],
        )?;
        if code != 0 {
            return Err(anyhow!(
                "git signature inspection failed in {} (exit {})",
                project.path,
                code
            ));
        }
        let mut run = c.run;
        run.signature = signature.trim().to_string();
        if run.signature.is_empty() {
            run.signature = "?".into();
        }
        let ok = run.signature == "G"
            && v.author == run.agent
            && v.author_email == format!("{}@pulse.local", run.agent)
            && v.pulse_agent == run.agent;
        let id = run.id.clone();
        let agent = run.agent.clone();
        let executor = run.executor.clone();
        v.runs.push(run);
        if ok {
            v.verified = true;
            v.verdict = format!(
                "verified: signed by the key of run {id}, authored by {agent}, job {} on {executor}",
                v.pulse_job
            );
            let _ = std::fs::remove_file(&path);
            return Ok(v);
        }
    }
    v.verdict = format!(
        "NOT verified: no run of job {} signed this commit with its key as its agent",
        v.pulse_job
    );
    let _ = std::fs::remove_file(&path);
    Ok(v)
}

pub fn run_verify(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err(pulse_cli::errorf(2, "pulse: verify needs a ref", "", "").into());
    }

    let project = current_project(c)?;
    let report = verify_commit(store(c)?, &project, &args[0])?;
    if c.json {
        json_output(c, &report)?;
    } else {
        c.printf(format_args!("{}", report.text()))?;
    }
    if !report.verified {
        return Err(pulse_cli::Error {
            what: String::new(),
            why: String::new(),
            fix: String::new(),
            code: 1,
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, input: &str, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "worker")
            .env("GIT_AUTHOR_EMAIL", "worker@pulse.local")
            .env("GIT_COMMITTER_NAME", "worker")
            .env("GIT_COMMITTER_EMAIL", "worker@pulse.local")
            .env("GIT_AUTHOR_DATE", PARITY_NOW)
            .env("GIT_COMMITTER_DATE", PARITY_NOW)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if !input.is_empty() {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn empty_signers() {
        let (_dir, s) = temporary_store();
        let got = invoke(Some(s), false, "", "signers", &[]);
        assert_eq!(got.code, 0);
        assert!(got.err.contains("# no run has a signing key yet"));
        assert_eq!(got.out, "");
    }

    #[test]
    fn verify_unknown_project() {
        let (_dir, s) = temporary_store();
        let got = invoke(Some(s), false, "unknown", "verify", &["HEAD"]);
        assert_eq!(got.code, 2);
        assert!(got.err.contains("No project"));
    }

    #[test]
    fn git_output_ignores_hook_git_env() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), "", &["init", "--quiet"]);
        let tree = git(root.path(), "", &["mktree"]);
        let want = git(root.path(), "", &["commit-tree", &tree, "-m", "fixture"]);
        let hook = tempfile::tempdir().unwrap();
        git(hook.path(), "", &["init", "--quiet"]);
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("GIT_DIR", hook.path().join(".git"));
            std::env::set_var("GIT_INDEX_FILE", hook.path().join(".git/index"));
        }
        let (got, _, code) = git_output(
            root.path().to_str().unwrap(),
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("{want}^{{commit}}"),
            ],
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(got.trim(), want);
        unsafe {
            std::env::remove_var("GIT_DIR");
            std::env::remove_var("GIT_INDEX_FILE");
        }
    }

    #[test]
    fn verify_commit_parity() {
        let _guard = pulse_testutil::env_lock();
        let (_dir, s) = temporary_store();
        let root = tempfile::tempdir().unwrap();
        fixture_command(root.path(), "", "git", &["init", "--quiet"]);
        let tree = fixture_command(root.path(), "", "git", &["mktree"]);
        let key_dir = tempfile::tempdir().unwrap();
        let key = key_dir.path().join("run-key");
        let wrong_key = key_dir.path().join("other-key");
        for path in [&key, &wrong_key] {
            fixture_command(
                root.path(),
                "",
                "ssh-keygen",
                &[
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    "",
                    "-f",
                    path.to_str().unwrap(),
                ],
            );
        }
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        let other_public = std::fs::read_to_string(wrong_key.with_extension("pub")).unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO projects(slug,name,path,added_at) VALUES ('verify-project','Verify',?,?)",
            (root.path().to_str().unwrap(), PARITY_NOW),
        ).unwrap();
        for (id, job, agent, started, signing_key) in [
            (
                "run-valid",
                "job-known",
                "worker",
                "2026-09-06T18:00:00Z",
                public.trim(),
            ),
            (
                "run-retry",
                "job-known",
                "worker",
                PARITY_NOW,
                other_public.trim(),
            ),
            (
                "run-wrong-key",
                "job-wrong",
                "worker",
                PARITY_NOW,
                other_public.trim(),
            ),
            (
                "run-wrong-agent",
                "job-author",
                "other",
                PARITY_NOW,
                public.trim(),
            ),
        ] {
            s.conn().unwrap().execute(
                "INSERT INTO runs(id,job_id,agent,project,started_at,status,executor,signing_pubkey) VALUES (?,?,?,'verify-project',?,'done','local',?)",
                (id, job, agent, started, signing_key),
            ).unwrap();
        }
        let project = s.project("verify-project").unwrap().unwrap();
        for (name, trailers, signed, want_ok) in [
            ("human", "", false, false),
            (
                "unknown-job",
                "Pulse-Agent: worker\nPulse-Job: absent\nPulse-Executor: local\n",
                false,
                false,
            ),
            (
                "unsigned-worker",
                "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: local\n",
                false,
                false,
            ),
            (
                "valid",
                "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: local\n",
                true,
                true,
            ),
            (
                "wrong-key",
                "Pulse-Agent: worker\nPulse-Job: job-wrong\nPulse-Executor: local\n",
                true,
                false,
            ),
            (
                "wrong-author",
                "Pulse-Agent: other\nPulse-Job: job-author\nPulse-Executor: local\n",
                true,
                false,
            ),
            (
                "wrong-agent-trailer",
                "Pulse-Agent: other\nPulse-Job: job-known\nPulse-Executor: local\n",
                true,
                false,
            ),
            (
                "duplicate-job-trailer",
                "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Job: job-wrong\n",
                true,
                false,
            ),
            (
                "executor-trailer-is-informational",
                "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: elsewhere\n",
                true,
                true,
            ),
        ] {
            let mut arg_strings = vec![
                "-c".to_string(),
                "gpg.format=ssh".to_string(),
                "-c".to_string(),
                "gpg.ssh.program=ssh-keygen".to_string(),
                "-c".to_string(),
                format!("user.signingkey={}", key.display()),
                "commit-tree".to_string(),
            ];
            if signed {
                arg_strings.push("-S".to_string());
            }
            arg_strings.push(tree.clone());
            let arg_refs: Vec<&str> = arg_strings.iter().map(String::as_str).collect();
            let commit = fixture_command(
                root.path(),
                &format!("{name}\n\n{trailers}"),
                "git",
                &arg_refs,
            );
            let report = verify_commit(&s, &project, &commit).unwrap();
            assert_eq!(
                report.verified,
                want_ok,
                "{name}: {}\n{report:?}",
                report.text()
            );
            let got = invoke(Some(s.clone()), false, &project.slug, "verify", &[&commit]);
            let as_json = invoke(Some(s.clone()), true, &project.slug, "verify", &[&commit]);
            let decoded: Verification = serde_json::from_str(&as_json.out).unwrap();
            assert_eq!(decoded.verified, want_ok, "{name}: {as_json:?}");
            assert_eq!(decoded.commit, commit);
            assert_eq!(as_json.code, got.code);
            assert_parity(
                got,
                oracle(
                    &s.home,
                    true,
                    &["verify", &commit, "--project", &project.slug],
                ),
            );
        }
        for refname in ["does-not-exist", "--help"] {
            let got = invoke(
                Some(s.clone()),
                false,
                &project.slug,
                "verify",
                &["--", refname],
            );
            assert_eq!(got.code, 2, "{got:?}");
            if refname != "--help" {
                assert_parity(
                    got,
                    oracle(
                        &s.home,
                        true,
                        &["verify", refname, "--project", &project.slug],
                    ),
                );
            }
        }
        assert_parity(
            invoke(Some(s.clone()), false, "", "signers", &[]),
            oracle(&s.home, true, &["signers"]),
        );
    }
}
