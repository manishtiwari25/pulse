#![allow(unused_variables, dead_code, unused_imports)]
use crate::proc::Outcome;
use crate::util::Row;
use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
pub(crate) trait Executor {
    fn name(&self) -> String;
    fn remote(&self) -> bool;
    fn clone(&self) -> String;
    fn key_path(&self) -> String;
    fn started(&self) -> bool;
    fn worker_dirs(&self) -> Vec<String>;
    fn prepare(&mut self, cancel: &pulse_bus::CancellationToken) -> Result<String>;
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome>;
    fn result_text(&self) -> Option<String>;
    fn commit_count(&self, base: &str) -> i32;
    fn dirty(&self) -> String;
    fn owner_paths_touched(&self, base: &str) -> Vec<String>;
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String);
    fn install_pre_commit_hook(
        &self,
        verify_cmd: &str,
        recipe: Option<&crate::recipe::Recipe>,
        pins: &[String],
        pulse_home: &str,
        project: &str,
        agent: &str,
    ) -> Result<()>;
    fn fetch_source(&self) -> String;
    fn fetch_env(&self) -> Vec<String>;
    /// Removes the run's clone and whatever the agent left registered on it;
    /// returns the leftovers so the run can write them down against the agent.
    fn cleanup(&self) -> Vec<crate::tidy::Leftover>;
}
#[derive(Debug, Clone, Default)]
pub(crate) struct RunLog(Arc<Mutex<Vec<String>>>);
impl RunLog {
    pub(crate) fn add(&self, line: impl Into<String>) {
        self.0.lock().expect("run log").push(line.into())
    }
    pub(crate) fn put(&self, line: &str) {
        self.0.lock().expect("run log").push(line.into())
    }
    pub(crate) fn text(&self) -> String {
        self.0
            .lock()
            .expect("run log")
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}
impl Write for RunLog {
    fn write(&mut self, p: &[u8]) -> std::io::Result<usize> {
        for line in String::from_utf8_lossy(p)
            .trim_end_matches('\n')
            .split('\n')
        {
            self.put(line);
        }
        Ok(p.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn tail_str(s: &str, n: usize) -> String {
    crate::executor::tail(s, n)
}
fn ssh_options() -> Vec<String> {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ClearAllForwardings=yes",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LocalExec {
    pub(crate) row: Row,
    #[serde(skip)]
    pub(crate) s: Option<pulse_store::Store>,
    pub(crate) home: String,
    pub(crate) job: Row,
    pub(crate) project: Row,
    pub(crate) agent: String,
    #[serde(skip)]
    pub(crate) log: RunLog,
    pub(crate) root: String,
    pub(crate) clone_dir: String,
    pub(crate) key: String,
    pub(crate) begun: bool,
    pub(crate) last_out: Option<String>,
}
pub(crate) fn new_local_exec(
    r: Row,
    s: &pulse_store::Store,
    home: &str,
    job: Row,
    project: Row,
    agent: &str,
    log: &mut RunLog,
) -> LocalExec {
    let job_id = job.str("id");
    let root = project.str("path");
    LocalExec {
        row: r,
        s: Some(s.clone()),
        home: home.into(),
        job,
        project,
        agent: agent.into(),
        log: log.clone(),
        root,
        clone_dir: std::path::Path::new(home)
            .join("work")
            .join(&job_id)
            .display()
            .to_string(),
        key: std::path::Path::new(home)
            .join("work")
            .join(format!("{job_id}.key"))
            .display()
            .to_string(),
        ..Default::default()
    }
}
impl LocalExec {
    pub(crate) fn name(&self) -> String {
        self.row.str("name")
    }
    pub(crate) fn branch_to_clone(&self) -> String {
        let d = self.project.str("default_branch");
        if !d.is_empty() && d != "HEAD" {
            d
        } else {
            String::new()
        }
    }
}
impl Executor for LocalExec {
    fn name(&self) -> String {
        self.row.str("name")
    }
    fn remote(&self) -> bool {
        false
    }
    fn clone(&self) -> String {
        self.clone_dir.clone()
    }
    fn key_path(&self) -> String {
        self.key.clone()
    }
    fn started(&self) -> bool {
        self.begun
    }
    fn worker_dirs(&self) -> Vec<String> {
        worker_dirs(&self.home)
    }
    fn prepare(&mut self, cancel: &pulse_bus::CancellationToken) -> Result<String> {
        preflight_run_key(&self.key)?;
        // A clone left by an earlier attempt goes, with whatever still runs in it.
        if Path::new(&self.clone_dir).exists() {
            crate::proc::sweep_job_dir(&self.clone_dir, &mut |line| self.log.add(line));
            let _ =
                crate::retention::discard_dir(Path::new(&self.home), Path::new(&self.clone_dir));
        }
        let mut args = vec![
            "git".to_string(),
            "clone".to_string(),
            "-q".to_string(),
            "--no-tags".to_string(),
        ];
        let branch = self.branch_to_clone();
        if !branch.is_empty() {
            args.extend(["-b".to_string(), branch]);
        }
        args.extend([self.root.clone(), self.clone_dir.clone()]);
        let out = crate::proc::run_group(
            cancel,
            &args,
            "",
            &crate::util::git_env(&[]),
            &[],
            Duration::from_secs(300),
        )?;
        self.log.add(format!(
            "$ git clone -q --no-tags\n{}{}",
            out.stdout, out.stderr
        ));
        if out.exit_code != 0 {
            return Err(
                crate::util::fail(1, format!("clone failed: {}", out.stderr.trim())).into(),
            );
        }
        let _ = crate::util::git(&self.clone_dir, &["remote", "remove", "origin"]);
        crate::keys::configure_clone_identity(
            &self.clone_dir,
            &self.agent,
            &self.key,
            &crate::keys::provenance_hook(&self.agent, &self.job.str("id"), &self.name()),
        )?;
        Ok(
            crate::util::git_check(&self.clone_dir, &["rev-parse", "HEAD"])?
                .stdout
                .trim()
                .to_string(),
        )
    }
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome> {
        preflight_run_key(&self.key)?;
        let worker_env = worker_env_with_cli_path(&crate::proc::environ(), env);
        crate::tmpspace::preflight_tmp_space(
            &crate::tmpspace::worker_tmp_dir(&worker_env),
            crate::tmpspace::MIN_TMP_SPACE,
        )?;
        self.begun = true;
        let budget = if minutes <= 0 {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(minutes as u64 * 60)
        };
        let id = self.execution(env.clone());
        let observer = self.observer(id);
        let out = crate::proc::run_group_observed(
            cancel,
            cmd,
            &self.clone_dir,
            &worker_env,
            &[],
            budget,
            observer.as_ref(),
        )
        .map_err(|e| {
            let setting = self
                .s
                .as_ref()
                .map(|s| s.setting("runner_cmd", ""))
                .unwrap_or_default();
            let exec_err = e.to_string().to_lowercase();
            crate::util::fail(
                1,
                format!(
                    "worker command not found: {} (setting runner_cmd={}): {exec_err}",
                    cmd.first().cloned().unwrap_or_default(),
                    setting
                ),
            )
        })?;
        self.last_out = Some(out.stdout.clone());
        Ok(out)
    }
    fn result_text(&self) -> Option<String> {
        self.last_out.clone()
    }
    fn commit_count(&self, base: &str) -> i32 {
        crate::util::git(
            &self.clone_dir,
            &["rev-list", "--count", &format!("{base}..HEAD")],
        )
        .stdout
        .trim()
        .parse()
        .unwrap_or(0)
    }
    fn dirty(&self) -> String {
        crate::util::git(
            &self.clone_dir,
            &["status", "--porcelain", "--untracked-files=no"],
        )
        .stdout
        .trim()
        .to_string()
    }
    fn owner_paths_touched(&self, base: &str) -> Vec<String> {
        crate::brief::owner_paths_touched(self.clone_dir.clone(), base.to_string())
    }
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String) {
        let secs = (minutes.max(1) as u64 * 60).max(60);
        let run = self.beat_during("verify", || {
            crate::proc::run_group(
                cancel,
                &["/bin/sh".into(), "-c".into(), cmd.into()],
                &self.clone_dir,
                &crate::proc::environ(),
                &[],
                Duration::from_secs(secs),
            )
        });
        match run {
            Ok(out) if out.timeout => (None, String::new()),
            Ok(out) => (
                Some(out.exit_code),
                format!(
                    "{}{}",
                    tail_str(&out.stdout, 20000),
                    tail_str(&out.stderr, 20000)
                ),
            ),
            Err(e) => (Some(127), e.to_string()),
        }
    }
    fn install_pre_commit_hook(
        &self,
        verify_cmd: &str,
        recipe: Option<&crate::recipe::Recipe>,
        pins: &[String],
        pulse_home: &str,
        project: &str,
        agent: &str,
    ) -> Result<()> {
        if verify_cmd.is_empty() {
            return Ok(());
        }
        crate::keys::write_pre_commit_hook(
            &self.clone_dir,
            &crate::keys::pre_commit_hook_script(
                verify_cmd,
                recipe,
                pins,
                &self.clone_dir,
                pulse_home,
                project,
                agent,
            ),
        )
    }
    fn fetch_source(&self) -> String {
        self.clone_dir.clone()
    }
    fn fetch_env(&self) -> Vec<String> {
        crate::util::git_env(&[])
    }
    fn cleanup(&self) -> Vec<crate::tidy::Leftover> {
        // Servers and watchers the worker left behind go before their
        // directory does; the directory itself is renamed into the trash in
        // an instant and deleted later by the low-priority remover.
        crate::proc::sweep_job_dir(&self.clone_dir, &mut |line| self.log.add(line));
        let left = crate::tidy::clear_leftovers(&self.clone_dir);
        let home = Path::new(&self.home);
        let _ = crate::retention::discard_dir(home, Path::new(&self.clone_dir));
        crate::retention::empty_trash_later(home);
        crate::keys::remove_run_key(&self.key);
        left
    }
}
pub(crate) fn preflight_run_key(path: &str) -> Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| crate::util::fail(1, format!("setup-error: the run's signing key {path} is missing ({e}); the worker was not started so no work is lost (ADR-022)")))?;
    if meta.len() == 0 {
        return Err(crate::util::fail(1, format!("setup-error: the run's signing key {path} is empty; the worker was not started so no work is lost (ADR-022)")).into());
    }
    Ok(())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SshExec {
    pub(crate) local: LocalExec,
    pub(crate) job_dir: String,
    pub(crate) bundle: String,
}
static SHA_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{40,64}$").unwrap());

pub(crate) fn new_ssh_exec(
    r: Row,
    s: &pulse_store::Store,
    home: &str,
    job: Row,
    project: Row,
    agent: &str,
    log: &mut RunLog,
) -> SshExec {
    SshExec {
        local: {
            let mut l = new_local_exec(r.clone(), s, home, job.clone(), project, agent, log);
            let job_dir = format!(
                "{}/{}",
                r.str("workdir").trim_end_matches('/'),
                job.str("id")
            );
            l.clone_dir = format!("{job_dir}/clone");
            l
        },
        job_dir: format!(
            "{}/{}",
            r.str("workdir").trim_end_matches('/'),
            job.str("id")
        ),
        bundle: std::path::Path::new(home)
            .join("work")
            .join(format!("{}.bundle", job.str("id")))
            .display()
            .to_string(),
    }
}
impl SshExec {
    pub(crate) fn name(&self) -> String {
        self.local.name()
    }
    pub(crate) fn call(
        &self,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<(i32, String, String)> {
        let (rc, out, err_text) = crate::executor::ssh_run(&self.local.row, args, stdin, timeout)?;
        let shown = if args.len() > 4 { &args[..4] } else { args };
        self.local.log.add(format!(
            "$ ssh {} {}\n{}{}",
            self.local.row.str("address"),
            crate::util::shell_join(shown),
            tail_str(&out, 4000),
            tail_str(&err_text, 4000)
        ));
        let src = if err_text.is_empty() { &out } else { &err_text };
        if rc != 0 && src.contains("verb not allowed") {
            let verb = args.first().map(String::as_str).unwrap_or("?");
            return Err(crate::executor::ExecutorFault {
                msg: format!(
                    "hands on {} refuse `{}` ({}) - they predate this hub; refresh them with `pulse pair <admin@host> --name {}`",
                    self.name(),
                    verb,
                    tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 120),
                    self.name()
                ),
            }
            .into());
        }
        Ok((rc, out, err_text))
    }
    pub(crate) fn preflight_remote_run_key(&self, when: &str) -> Result<()> {
        let (rc, out, err) = self.call(
            &[
                "verify".into(),
                self.local.job.str("id"),
                "1".into(),
                "--".into(),
                remote_key_check_cmd(&self.remote_key_path()),
            ],
            &[],
            Duration::from_secs(120),
        )?;
        remote_key_check_result(&self.remote_key_path(), &self.name(), when, rc, &out, &err)
    }
    pub(crate) fn remote_key_path(&self) -> String {
        format!("{}/run.key", self.job_dir)
    }
    pub(crate) fn preflight_remote_cli(&self, path: &str) -> Result<()> {
        let (rc, out, err) = self.call(
            &[
                "verify".into(),
                self.local.job.str("id"),
                "1".into(),
                "--".into(),
                remote_cli_check_cmd(path),
            ],
            &[],
            Duration::from_secs(120),
        )?;
        remote_cli_check_result(path, &self.name(), rc, &out, &err)
    }
}
impl Executor for SshExec {
    fn name(&self) -> String {
        self.local.name()
    }
    fn remote(&self) -> bool {
        true
    }
    fn clone(&self) -> String {
        self.local.clone_dir.clone()
    }
    fn key_path(&self) -> String {
        self.local.key_path()
    }
    fn started(&self) -> bool {
        self.local.started()
    }
    fn worker_dirs(&self) -> Vec<String> {
        vec![self.job_dir.clone(), "/tmp".into()]
    }
    fn prepare(&mut self, cancel: &pulse_bus::CancellationToken) -> Result<String> {
        let _ = cancel;
        let mut branch = self.local.branch_to_clone();
        if branch.is_empty() {
            let head =
                crate::util::git(&self.local.root, &["symbolic-ref", "--short", "-q", "HEAD"]);
            if head.ok() {
                branch = head.stdout.trim().to_string();
            }
        }
        let reference = if branch.is_empty() {
            "HEAD".to_string()
        } else {
            branch
        };
        let made = crate::util::git_timeout(
            &self.local.root,
            Duration::from_secs(300),
            &["bundle", "create", "-q", &self.bundle, &reference],
        );
        if !made.ok() {
            return Err(
                crate::util::fail(1, format!("bundle failed: {}", made.stderr.trim())).into(),
            );
        }
        let data = std::fs::read(&self.bundle)?;
        let (rc, out, err_text) = self.call(
            &[
                "prepare".into(),
                self.local.job.str("id"),
                reference,
                self.local.agent.clone(),
            ],
            &data,
            Duration::from_secs(600),
        )?;
        let src = if err_text.is_empty() { &out } else { &err_text };
        if rc != 0 {
            return Err(crate::util::fail(
                1,
                format!(
                    "remote prepare failed on {} (exit {rc}): {}",
                    self.name(),
                    tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 300)
                ),
            )
            .into());
        }
        let base = out
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string();
        if !SHA_RE.is_match(&base) {
            return Err(crate::util::fail(
                1,
                format!(
                    "remote prepare on {} returned no base commit: {}",
                    self.name(),
                    crate::py_repr(tail_str(&out, 200))
                ),
            )
            .into());
        }
        let key_data = std::fs::read(&self.local.key)?;
        let (rc, out, err_text) = self.call(
            &["key".into(), self.local.job.str("id"), self.name()],
            &key_data,
            Duration::from_secs(120),
        )?;
        if rc != 0 {
            let src = if err_text.is_empty() { out } else { err_text };
            return Err(crate::executor::ExecutorFault {
                msg: format!(
                    "remote key failed on {} (exit {rc}): {} - the machine cannot take a signing key; probe it with `pulse executor probe {}`",
                    self.name(),
                    tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 300),
                    self.name()
                ),
            }
            .into());
        }
        self.preflight_remote_run_key("after upload")?;
        Ok(base)
    }
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome> {
        preflight_run_key(&self.local.key)?;
        self.preflight_remote_run_key("before the worker starts")?;
        if let Ok((0, out, _)) = self.call(&["probe".into()], &[], Duration::from_secs(120)) {
            let fields = out
                .split_whitespace()
                .filter_map(|f| f.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>();
            if fields.get("pulse").map(String::as_str) == Some("missing") {
                let looked = match fields.get("pulse_lookup").map(String::as_str) {
                    Some("") | None | Some("-") => "the executor user's login PATH".to_string(),
                    Some(v) => format!("PATH={}", v.replace("%20", " ")),
                };
                return Err(crate::executor::ExecutorFault {
                    msg: format!(
                        "pulse is not on PATH on {} (hands probe reported pulse=missing, looked in {looked}) - install the pulse CLI for the executor user or fix its login PATH; probe it with `pulse executor probe {}`",
                        self.name(),
                        self.name()
                    ),
                }
                .into());
            }
        }
        if let Some(cli) = env.get("PULSE_CLI").filter(|s| !s.is_empty()) {
            self.preflight_remote_cli(cli)?;
        }
        let budget = if minutes <= 0 {
            "1s".to_string()
        } else {
            minutes.to_string()
        };
        let mut args = vec!["run".to_string(), self.local.job.str("id"), budget];
        for (k, v) in env {
            args.push(format!("{k}={v}"));
        }
        args.push("--".into());
        args.extend(cmd.iter().cloned());
        self.local.begun = true;
        let (rc, out, err_text) = self.call(
            &args,
            &[],
            Duration::from_secs((minutes.max(1) * 60 + 120) as u64),
        )?;
        self.local.last_out = Some(out.clone());
        if rc == crate::executor::REMOTE_EXIT_TIMEOUT {
            return Ok(Outcome {
                timeout: true,
                exit_code: -1,
                stdout: out,
                stderr: err_text,
            });
        }
        Ok(Outcome {
            timeout: false,
            exit_code: rc,
            stdout: out,
            stderr: err_text,
        })
    }
    fn result_text(&self) -> Option<String> {
        match self.call(
            &["result".into(), self.local.job.str("id")],
            &[],
            Duration::from_secs(120),
        ) {
            Ok((0, out, _)) => Some(out),
            _ => self.local.last_out.clone(),
        }
    }
    fn commit_count(&self, base: &str) -> i32 {
        match self.call(
            &["count".into(), self.local.job.str("id"), base.into()],
            &[],
            Duration::from_secs(120),
        ) {
            Ok((0, out, _)) => out
                .split_whitespace()
                .last()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            _ => 0,
        }
    }
    fn dirty(&self) -> String {
        match self.call(
            &["dirty".into(), self.local.job.str("id")],
            &[],
            Duration::from_secs(120),
        ) {
            Ok((0, out, _)) => out.trim().to_string(),
            _ => String::new(),
        }
    }
    fn owner_paths_touched(&self, base: &str) -> Vec<String> {
        let Ok((0, out, _)) = self.call(
            &["changed".into(), self.local.job.str("id"), base.into()],
            &[],
            Duration::from_secs(120),
        ) else {
            return Vec::new();
        };
        let mut hit = Vec::new();
        for f in out.split_whitespace() {
            if crate::keys::OWNER_PATHS
                .iter()
                .any(|p| f == *p || f.starts_with(p))
            {
                hit.push(f.to_string());
            }
        }
        hit.sort();
        hit
    }
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String) {
        let _ = cancel;
        let budget = if minutes <= 0 {
            "1s".to_string()
        } else {
            minutes.to_string()
        };
        let call = self.local.beat_during("verify", || {
            self.call(
                &[
                    "verify".into(),
                    self.local.job.str("id"),
                    budget,
                    "--".into(),
                    cmd.into(),
                ],
                &[],
                Duration::from_secs((minutes.max(1) * 60 + 120) as u64),
            )
        });
        match call {
            Ok((crate::executor::REMOTE_EXIT_TIMEOUT, out, err)) => (
                None,
                format!("{}{}", tail_str(&out, 20000), tail_str(&err, 20000)),
            ),
            Ok((rc, out, err)) => (
                Some(rc),
                format!("{}{}", tail_str(&out, 20000), tail_str(&err, 20000)),
            ),
            Err(e) => (Some(-1), e.to_string()),
        }
    }
    fn install_pre_commit_hook(
        &self,
        verify_cmd: &str,
        recipe: Option<&crate::recipe::Recipe>,
        pins: &[String],
        pulse_home: &str,
        project: &str,
        agent: &str,
    ) -> Result<()> {
        if verify_cmd.is_empty() {
            return Ok(());
        }
        let script = crate::keys::pre_commit_hook_script(
            verify_cmd,
            recipe,
            pins,
            &self.local.clone_dir,
            pulse_home,
            project,
            agent,
        );
        let (rc, out, err) = self.call(
            &["hook".into(), self.local.job.str("id")],
            script.as_bytes(),
            Duration::from_secs(120),
        )?;
        if rc != 0 {
            let src = if err.is_empty() { out } else { err };
            return Err(crate::util::fail(
                1,
                format!(
                    "remote pre-commit hook install on {} failed (exit {rc}): {}",
                    self.name(),
                    tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 300)
                ),
            )
            .into());
        }
        Ok(())
    }
    fn fetch_source(&self) -> String {
        match crate::executor::parse_ssh_address(&self.local.row.str("address")) {
            Ok((user, host, port)) => {
                format!("ssh://{user}@{host}:{port}{}", self.local.clone_dir)
            }
            Err(_) => format!("ssh://{}", self.local.clone_dir),
        }
    }
    fn fetch_env(&self) -> Vec<String> {
        crate::proc::env_with(
            &crate::util::git_env(&[]),
            &BTreeMap::from([(
                "GIT_SSH_COMMAND".to_string(),
                crate::util::shell_join(
                    &std::iter::once("ssh".to_string())
                        .chain(ssh_options())
                        .collect::<Vec<_>>(),
                ),
            )]),
        )
    }
    fn cleanup(&self) -> Vec<crate::tidy::Leftover> {
        let _ = std::fs::remove_file(&self.bundle);
        crate::keys::remove_run_key(&self.local.key);
        match self.call(
            &["clean".into(), self.local.job.str("id")],
            &[],
            Duration::from_secs(120),
        ) {
            Ok((_, out, _)) => crate::tidy::parse_leftover_lines(&out),
            Err(err) => {
                self.local.log.add(format!(
                    "remote clean skipped: {err} (the next probe reports leftovers)"
                ));
                Vec::new()
            }
        }
    }
}
pub(crate) fn remote_key_check_result(
    path: &str,
    host: &str,
    when: &str,
    rc: i32,
    out: &str,
    err_text: &str,
) -> Result<()> {
    if rc == 0 {
        return Ok(());
    }
    let src = if err_text.is_empty() { out } else { err_text };
    Err(crate::util::fail(1, format!("setup-error: the run's signing key {path} on {host} is missing or empty {when} (exit {rc}): {}; the worker was not started so no work is lost (ADR-022)", tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 300))).into())
}
pub(crate) fn remote_key_check_cmd(path: &str) -> String {
    format!("test -s {}", crate::util::shell_quote(path))
}
pub(crate) fn remote_cli_check_cmd(path: &str) -> String {
    format!("test -x {}", crate::util::shell_quote(path))
}
pub(crate) fn remote_cli_check_result(
    path: &str,
    host: &str,
    rc: i32,
    out: &str,
    err_text: &str,
) -> Result<()> {
    if rc == 0 {
        return Ok(());
    }
    let src = if err_text.is_empty() { out } else { err_text };
    Err(crate::util::fail(1, format!("setup-error: PULSE_CLI {path} on {host} is missing or not executable (exit {rc}): {}; the worker was not started so no work is lost - stage the pulse binary there or fix the recipe's PULSE_CLI", tail_str(&src.split_whitespace().collect::<Vec<_>>().join(" "), 300))).into())
}
pub(crate) fn make_executor(
    r: Row,
    s: &pulse_store::Store,
    home: &str,
    job: Row,
    project: Row,
    agent: &str,
    log: &mut RunLog,
) -> Result<Box<dyn Executor>> {
    if r.str("name") == crate::executor::LOCAL_EXECUTOR || r.str("kind") == "local" {
        return Ok(Box::new(new_local_exec(
            r, s, home, job, project, agent, log,
        )));
    }
    if r.str("kind") == "ssh" {
        return Ok(Box::new(new_ssh_exec(r, s, home, job, project, agent, log)));
    }
    Err(crate::util::fail(
        2,
        format!(
            "Executor {} has unknown kind {:?}",
            r.str("name"),
            r.str("kind")
        ),
    )
    .into())
}
pub(crate) fn worker_dirs(home: &str) -> Vec<String> {
    let mut dirs = vec![home.to_string()];
    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.display().to_string()))
    {
        dirs.push(parent);
    }
    dirs.push(std::env::temp_dir().display().to_string());
    if Path::new("/tmp").is_dir() {
        dirs.push("/tmp".into());
    }
    let mut out = Vec::new();
    for d in dirs {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}
pub(crate) fn worker_command(
    s: &pulse_store::Store,
    kind: &str,
    brief_text: &str,
    clone: &str,
    worker_cmd: &str,
    dirs: &[String],
    remote: bool,
) -> (Vec<String>, bool) {
    if !worker_cmd.is_empty() {
        return (crate::util::split_shell(worker_cmd), true);
    }
    if s.harness_native() {
        let bin = if remote {
            "pulse".to_string()
        } else {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "pulse".into())
        };
        let mut cmd = vec![
            bin,
            "harness".into(),
            "run".into(),
            "-p".into(),
            brief_text.into(),
            "-C".into(),
            clone.into(),
            "--max-steps".into(),
            s.setting("harness.max_steps", ""),
            "--step-timeout".into(),
            s.setting("harness.step_timeout", ""),
            "--output-limit".into(),
            s.setting("harness.output_limit", ""),
            "--max-cost".into(),
            s.setting("harness.max_cost", ""),
            "--keep-observations".into(),
            s.setting("harness.keep_observations", ""),
        ];
        if crate::util::setting_int(s, "minutes_per_job") > 1 {
            cmd.extend([
                "--minutes".into(),
                (crate::util::setting_int(s, "minutes_per_job") - 1).to_string(),
            ]);
        }
        let model = s.run_model(kind);
        if !model.is_empty() {
            cmd.extend(["--model".into(), model]);
        }
        return (cmd, false);
    }
    let mut cmd = vec![
        crate::executor::runner_cmd(s),
        "-p".into(),
        brief_text.into(),
        "-C".into(),
        clone.into(),
        "--allow-all-tools".into(),
    ];
    for p in crate::brief::DENIED_TOOLS {
        cmd.extend(["--deny-tool".into(), (*p).into()]);
    }
    for d in dirs {
        cmd.extend(["--add-dir".into(), d.clone()]);
    }
    cmd.extend([
        "--disable-builtin-mcps".into(),
        "--no-ask-user".into(),
        "-s".into(),
        "--max-ai-credits".into(),
        s.setting("max_ai_credits", ""),
        "--autopilot".into(),
    ]);
    let model = s.run_model(kind);
    if !model.is_empty() {
        cmd.extend(["--model".into(), model]);
    }
    (cmd, false)
}
pub(crate) fn local_cli(home: &str) -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| local_cli_fallback(home))
}
pub(crate) fn local_cli_fallback(home: &str) -> String {
    for cand in [
        std::env::var("PULSE_CLI").unwrap_or_default(),
        std::path::Path::new(home)
            .join("bin")
            .join("pulse")
            .display()
            .to_string(),
    ] {
        if cand.is_empty() {
            continue;
        }
        if std::fs::metadata(&cand).is_ok_and(|m| m.is_file()) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&cand).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
                    return cand;
                }
            }
        }
    }
    String::new()
}
pub(crate) fn worker_env_with_cli_path(
    base: &[String],
    extra: &BTreeMap<String, String>,
) -> Vec<String> {
    let Some(cli) = extra.get("PULSE_CLI").filter(|s| !s.is_empty()) else {
        return crate::proc::env_with(base, extra);
    };
    let dir = std::path::Path::new(cli)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let path = extra.get("PATH").cloned().unwrap_or_else(|| {
        base.iter()
            .find_map(|kv| kv.strip_prefix("PATH=").map(ToOwned::to_owned))
            .unwrap_or_default()
    });
    if std::env::split_paths(&path).any(|p| p.display().to_string() == dir) {
        return crate::proc::env_with(base, extra);
    }
    let mut merged = extra.clone();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let old = extra.get("PATH").cloned().unwrap_or_else(|| {
        base.iter()
            .find_map(|kv| kv.strip_prefix("PATH=").map(ToOwned::to_owned))
            .unwrap_or_default()
    });
    merged.insert(
        "PATH".into(),
        if old.is_empty() {
            dir
        } else {
            format!("{dir}{sep}{old}")
        },
    );
    crate::proc::env_with(base, &merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_env(env: &[String], key: &str) -> String {
        env.iter()
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(ToOwned::to_owned))
            .unwrap_or_default()
    }

    #[test]
    fn run_log_clone_shares_lines() {
        let _serial = crate::test_helpers::serial();
        let log = RunLog::default();
        let clone = log.clone();
        clone.add("presence unavailable: test");
        assert_eq!(log.text(), "presence unavailable: test");
    }

    #[test]
    fn preflight_run_key_missing_is_setup_error() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("job.key").display().to_string();
        let err = preflight_run_key(&missing).unwrap_err().to_string();
        assert!(
            err.contains("setup-error") && err.contains(&missing),
            "{err}"
        );
        std::fs::write(&missing, []).unwrap();
        let err = preflight_run_key(&missing).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
        std::fs::write(&missing, b"k").unwrap();
        preflight_run_key(&missing).unwrap();
    }

    #[test]
    fn preflight_run_key_remote_check_cmd() {
        let _serial = crate::test_helpers::serial();
        let _deadline =
            crate::test_helpers_daemon::test_deadline(std::time::Duration::from_secs(60));
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("run.key");
        if std::process::Command::new("/bin/sh")
            .args(["-c", &remote_key_check_cmd(key.to_str().unwrap())])
            .status()
            .unwrap()
            .success()
        {
            panic!("missing remote key must fail test -s");
        }
        std::fs::write(&key, []).unwrap();
        if std::process::Command::new("/bin/sh")
            .args(["-c", &remote_key_check_cmd(key.to_str().unwrap())])
            .status()
            .unwrap()
            .success()
        {
            panic!("empty remote key must fail test -s");
        }
        std::fs::write(&key, b"k").unwrap();
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &remote_key_check_cmd(key.to_str().unwrap())])
            .status()
            .unwrap();
        if !status.success() {
            panic!("non-empty remote key must pass: {status}");
        }
        let cmd = remote_key_check_cmd("/w/job 1/run.key");
        assert!(
            cmd.contains("'/w/job 1/run.key'"),
            "path must be shell-quoted, got {cmd:?}"
        );
    }

    #[test]
    fn preflight_run_key_remote_result() {
        let _serial = crate::test_helpers::serial();
        remote_key_check_result("/w/j/run.key", "box", "before the worker starts", 0, "", "")
            .unwrap();
        let err = remote_key_check_result(
            "/w/j/run.key",
            "box",
            "before the worker starts",
            1,
            "",
            "no such file",
        )
        .unwrap_err()
        .to_string();
        for want in [
            "setup-error",
            "/w/j/run.key",
            "box",
            "before the worker starts",
            "no such file",
        ] {
            assert!(err.contains(want), "{err:?} should contain {want:?}");
        }
    }

    #[test]
    fn preflight_run_key_after_make_run_key() {
        let _serial = crate::test_helpers::serial();
        let path = tempfile::tempdir()
            .unwrap()
            .path()
            .join("run.key")
            .display()
            .to_string();
        if let Err(err) = crate::keys::make_run_key(&path, "juniper", "run-1") {
            if err.to_string().contains("ssh-keygen is not installed") {
                return;
            }
            panic!("{err}");
        }
        preflight_run_key(&path).unwrap();
    }

    #[test]
    fn preflight_remote_cli_check_cmd() {
        let _serial = crate::test_helpers::serial();
        let _deadline =
            crate::test_helpers_daemon::test_deadline(std::time::Duration::from_secs(60));
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("pulse");
        if std::process::Command::new("/bin/sh")
            .args(["-c", &remote_cli_check_cmd(cli.to_str().unwrap())])
            .status()
            .unwrap()
            .success()
        {
            panic!("missing PULSE_CLI must fail test -x");
        }
        std::fs::write(&cli, b"#!/bin/sh\n").unwrap();
        if std::process::Command::new("/bin/sh")
            .args(["-c", &remote_cli_check_cmd(cli.to_str().unwrap())])
            .status()
            .unwrap()
            .success()
        {
            panic!("non-executable PULSE_CLI must fail test -x");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &remote_cli_check_cmd(cli.to_str().unwrap())])
            .status()
            .unwrap();
        if !status.success() {
            panic!("executable PULSE_CLI must pass: {status}");
        }
        let cmd = remote_cli_check_cmd("/usr/local/bin/my pulse");
        assert!(
            cmd.contains("'/usr/local/bin/my pulse'"),
            "path must be shell-quoted, got {cmd:?}"
        );
    }

    #[test]
    fn preflight_remote_cli_result() {
        let _serial = crate::test_helpers::serial();
        remote_cli_check_result("/usr/local/bin/pulse", "box", 0, "", "").unwrap();
        let err = remote_cli_check_result("/usr/local/bin/pulse", "box", 1, "", "no such file")
            .unwrap_err()
            .to_string();
        for want in [
            "setup-error",
            "PULSE_CLI",
            "/usr/local/bin/pulse",
            "box",
            "no such file",
            "not started",
        ] {
            assert!(err.contains(want), "{err:?} should contain {want:?}");
        }
    }

    #[test]
    fn local_exec_worker_path() {
        let _serial = crate::test_helpers::serial();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let base = vec![format!("PATH=/usr/bin{sep}/bin"), "HOME=/h".into()];
        let mut extra = BTreeMap::new();
        extra.insert("PULSE_CLI".into(), "/opt/pulse/bin/pulse".into());
        let got = worker_env_with_cli_path(&base, &extra);
        assert_eq!(
            lookup_env(&got, "PATH"),
            format!("/opt/pulse/bin{sep}/usr/bin{sep}/bin")
        );
        assert_eq!(lookup_env(&got, "PULSE_CLI"), "/opt/pulse/bin/pulse");

        extra.insert("PULSE_CLI".into(), "/usr/bin/pulse".into());
        let got = worker_env_with_cli_path(&base, &extra);
        assert_eq!(lookup_env(&got, "PATH"), format!("/usr/bin{sep}/bin"));

        let mut extra = BTreeMap::new();
        extra.insert("X".into(), "1".into());
        let got = worker_env_with_cli_path(&base, &extra);
        assert_eq!(lookup_env(&got, "PATH"), format!("/usr/bin{sep}/bin"));

        let mut extra = BTreeMap::new();
        extra.insert("PULSE_CLI".into(), "/opt/p/pulse".into());
        let got = worker_env_with_cli_path(&["HOME=/h".into()], &extra);
        assert_eq!(lookup_env(&got, "PATH"), "/opt/p");
    }

    #[test]
    fn ssh_exec_worker_path() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("pulse");
        std::fs::write(&cli, b"#!/bin/sh\necho ok\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut out = Vec::new();
        let mut h = crate::hands::new_hands(std::io::empty(), &mut out, Vec::<u8>::new());
        let mut extra = BTreeMap::new();
        extra.insert("PULSE_CLI".to_string(), cli.display().to_string());
        let rc = h.with_timeout(
            Duration::from_secs(60),
            &dir.path().display().to_string(),
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf %s \"$PATH\"".to_string(),
            ],
            &extra,
        );
        assert_eq!(rc, 0, "out={}", String::from_utf8_lossy(&out));
        assert!(
            String::from_utf8_lossy(&out).starts_with(
                &(dir.path().display().to_string() + if cfg!(windows) { ";" } else { ":" })
            ),
            "remote worker PATH not prefixed with dir(PULSE_CLI): {:?}",
            String::from_utf8_lossy(&out)
        );
    }

    #[test]
    fn local_exec_default_pulse_cli() {
        let _serial = crate::test_helpers::serial();
        let exe = std::env::current_exe().unwrap();
        let exe = exe.display().to_string();
        let mut got = BTreeMap::from([("PULSE_JOB".to_string(), "j".to_string())]);
        crate::run::worker_cli_env(
            &mut got,
            &local_cli(&tempfile::tempdir().unwrap().path().display().to_string()),
        );
        assert_eq!(got.get("PULSE_CLI").map(String::as_str), Some(exe.as_str()));
        let env = worker_env_with_cli_path(&["PATH=/usr/bin".into()], &got);
        assert_eq!(lookup_env(&env, "PULSE_CLI"), exe);
        assert!(
            lookup_env(&env, "PATH").starts_with(
                &(std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .display()
                    .to_string()
                    + if cfg!(windows) { ";" } else { ":" })
            )
        );
        let keep = worker_env_with_cli_path(
            &["PATH=/usr/bin".into()],
            &BTreeMap::from([("PULSE_CLI".to_string(), "/opt/pulse/bin/pulse".to_string())]),
        );
        assert_eq!(lookup_env(&keep, "PULSE_CLI"), "/opt/pulse/bin/pulse");
    }

    #[test]
    fn cli_on_path() {
        let _serial = crate::test_helpers::serial();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("pulse");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let other = tempfile::tempdir().unwrap();
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(
            crate::run::cli_on_path(
                &other.path().display().to_string(),
                &exe.display().to_string()
            ),
            format!("{}{sep}{}", bin.path().display(), other.path().display())
        );
        assert_eq!(
            crate::run::cli_on_path("", &exe.display().to_string()),
            bin.path().display().to_string()
        );
        assert_eq!(
            crate::run::cli_on_path(
                &format!("{}{sep}{}", bin.path().display(), other.path().display()),
                &exe.display().to_string()
            ),
            ""
        );
        assert_eq!(
            crate::run::cli_on_path(&other.path().display().to_string(), ""),
            ""
        );
    }

    #[test]
    fn cli_on_path_executable_dir() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let exe = std::env::current_exe().unwrap();
        let inherited = tempfile::tempdir().unwrap();
        let _env_1 = pulse_testutil::setenv("PATH", inherited.path());
        let mut env = BTreeMap::new();
        crate::run::worker_cli_env(&mut env, &exe.display().to_string());
        assert_eq!(
            env.get("PULSE_CLI").map(String::as_str),
            Some(exe.to_str().unwrap())
        );
        let got = worker_env_with_cli_path(&crate::proc::environ(), &env);
        let path = lookup_env(&got, "PATH");
        let dirs = std::env::split_paths(&path).collect::<Vec<_>>();
        assert!(dirs.len() >= 2 && dirs[0] == exe.parent().unwrap());
        assert_eq!(dirs.last().unwrap(), inherited.path());
    }

    #[test]
    fn cli_on_path_fallback() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        let _env_2 = pulse_testutil::setenv("PULSE_CLI", "");
        assert_eq!(local_cli_fallback(&home.path().display().to_string()), "");
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let home_cli = bin.join("pulse");
        std::fs::write(&home_cli, b"#!/bin/sh\n").unwrap();
        assert_eq!(local_cli_fallback(&home.path().display().to_string()), "");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&home_cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            local_cli_fallback(&home.path().display().to_string()),
            home_cli.display().to_string()
        );
        let other_dir = tempfile::tempdir().unwrap();
        let other = other_dir.path().join("pulse");
        std::fs::write(&other, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _env_3 = pulse_testutil::setenv("PULSE_CLI", &other);
        assert_eq!(
            local_cli_fallback(&home.path().display().to_string()),
            other.display().to_string()
        );
        let p = crate::run::cli_on_path(
            &tempfile::tempdir().unwrap().path().display().to_string(),
            &other.display().to_string(),
        );
        assert_eq!(
            std::env::split_paths(&p).next().unwrap(),
            other.parent().unwrap()
        );
        assert_eq!(
            local_cli(&home.path().display().to_string()),
            std::env::current_exe().unwrap().display().to_string()
        );
    }

    #[test]
    fn cli_on_path_worker_process() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let exe = std::env::current_exe().unwrap();
        let inherited = tempfile::tempdir().unwrap();
        let _env_4 = pulse_testutil::setenv("PATH", inherited.path());
        let s = crate::test_helpers::new_store();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("run.key");
        std::fs::write(&key, b"key").unwrap();
        let mut e = LocalExec {
            s: Some((*s).clone()),
            job: {
                let mut r = Row::new();
                r.insert("id", serde_json::json!("job-cli"));
                r
            },
            project: Row::new(),
            clone_dir: dir.path().display().to_string(),
            key: key.display().to_string(),
            ..Default::default()
        };
        let mut env = BTreeMap::new();
        crate::run::worker_cli_env(&mut env, &exe.display().to_string());
        let out = e
            .run_worker(
                &pulse_bus::CancellationToken::new(),
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf 'PATH=%s\\n' \"$PATH\"".into(),
                ],
                &env,
                1,
            )
            .unwrap();
        assert_eq!(out.exit_code, 0, "{out:?}");
        let got = out
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("PATH="))
            .unwrap_or("");
        let dirs = std::env::split_paths(got).collect::<Vec<_>>();
        assert!(!dirs.is_empty() && dirs[0] == exe.parent().unwrap());
        assert_eq!(dirs.last().unwrap(), inherited.path());
    }

    fn model_arg(cmd: &[String]) -> String {
        cmd.windows(2)
            .find_map(|w| (w[0] == "--model").then(|| w[1].clone()))
            .unwrap_or_default()
    }

    fn set_setting(s: &pulse_store::Store, key: &str, value: &str) {
        s.set_setting(crate::util::HUMAN_AGENT, key, value).unwrap();
    }

    fn insert_test_project(s: &pulse_store::Store) -> Row {
        let root = std::path::Path::new(&s.home)
            .join("exec-test-project")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = root.display().to_string();
        let now = s.now();
        s.conn()
            .unwrap()
            .execute("INSERT OR REPLACE INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES ('proj', 'Proj', ?, '', 0, '', 'main', 'active', ?, ?, ?)", rusqlite::params![root, now, crate::util::HUMAN_AGENT, now])
            .unwrap();
        let mut project = Row::new();
        project.insert("slug", serde_json::json!("proj"));
        project.insert("name", serde_json::json!("Proj"));
        project.insert("path", serde_json::json!(root));
        project
    }

    fn insert_git_project(s: &pulse_store::Store) -> Row {
        let root = std::path::Path::new(&s.home)
            .join("exec-git-project")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let run_git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@x")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@x")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run_git(&["init", "-q", "-b", "main"]);
        run_git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("README.md"), "# proj\n").unwrap();
        run_git(&["add", "-A"]);
        run_git(&["commit", "-q", "-m", "init"]);
        let root = std::fs::canonicalize(root).unwrap().display().to_string();
        let now = s.now();
        s.conn()
            .unwrap()
            .execute("INSERT OR REPLACE INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES ('proj', 'Proj', ?, '', 0, '', 'main', 'active', ?, ?, ?)", rusqlite::params![root, now, crate::util::HUMAN_AGENT, now])
            .unwrap();
        let mut project = Row::new();
        project.insert("slug", serde_json::json!("proj"));
        project.insert("name", serde_json::json!("Proj"));
        project.insert("path", serde_json::json!(root));
        project
    }

    #[test]
    fn worker_command_model_follows_the_job_kind() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        set_setting(&s, "worker_model", "heavy-test");
        set_setting(&s, "light_model", "light-test");
        for kind in pulse_board::JOB_KINDS {
            let (cmd, custom) = worker_command(&s, kind, "brief", "/tmp/clone", "", &[], false);
            assert!(!custom, "{kind}: a custom command without worker_cmd");
            let want = if pulse_store::IMPLEMENTATION_JOB_KINDS.contains(kind) {
                "heavy-test"
            } else {
                "light-test"
            };
            assert_eq!(model_arg(&cmd), want, "{kind}");
        }
        let (cmd, custom) = worker_command(
            &s,
            "task",
            "brief",
            "/tmp/clone",
            "sh -c 'echo hi'",
            &[],
            false,
        );
        assert!(custom);
        assert_eq!(model_arg(&cmd), "");
    }

    #[test]
    fn light_model_falls_back_to_worker_model() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        assert_eq!(s.run_model("task"), "");
        assert_eq!(s.run_model("lead"), "");
        set_setting(&s, "worker_model", "heavy-test");
        for kind in [
            "task", "lead", "answer", "ballot", "reflect", "onboard", "clean",
        ] {
            assert_eq!(s.run_model(kind), "heavy-test", "{kind}");
        }
        assert_eq!(s.light_model(), "heavy-test");
        assert_eq!(
            model_arg(&crate::executor::login_check_command(&s)),
            "heavy-test"
        );
        set_setting(&s, "light_model", "light-test");
        assert_eq!(s.light_model(), "light-test");
        assert_eq!(
            model_arg(&crate::executor::login_check_command(&s)),
            "light-test"
        );
        assert_eq!(s.run_model("task"), "heavy-test");
    }

    #[test]
    fn worker_command_native_and_legacy_argv() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        set_setting(&s, "worker_model", "heavy-test");
        let (cmd, custom) = worker_command(&s, "task", "brief text", "/tmp/clone", "", &[], false);
        assert!(!custom);
        let exe = std::env::current_exe().unwrap().display().to_string();
        let want = vec![
            exe,
            "harness".into(),
            "run".into(),
            "-p".into(),
            "brief text".into(),
            "-C".into(),
            "/tmp/clone".into(),
            "--max-steps".into(),
            "60".into(),
            "--step-timeout".into(),
            "300".into(),
            "--output-limit".into(),
            "20000".into(),
            "--max-cost".into(),
            "0".into(),
            "--keep-observations".into(),
            "5".into(),
            "--minutes".into(),
            "9".into(),
            "--model".into(),
            "heavy-test".into(),
        ];
        assert_eq!(cmd, want);
        let (cmd, custom) = worker_command(&s, "lead", "brief text", "/tmp/clone", "", &[], true);
        assert!(
            !custom
                && cmd.len() >= 3
                && cmd[0] == "pulse"
                && cmd[1] == "harness"
                && cmd[2] == "run"
        );
        set_setting(&s, "harness", "copilot-cli");
        let (cmd, custom) = worker_command(
            &s,
            "task",
            "brief text",
            "/tmp/clone",
            "",
            &["/tmp".into(), "/work".into()],
            false,
        );
        assert!(!custom);
        let joined = cmd.join(" ");
        for want in [
            "copilot",
            "-p brief text",
            "-C /tmp/clone",
            "--allow-all-tools",
            "--add-dir /tmp",
            "--add-dir /work",
            "--disable-builtin-mcps",
            "--no-ask-user",
            "--autopilot",
            "--model heavy-test",
        ] {
            assert!(joined.contains(want), "legacy argv lacks {want:?}: {cmd:?}");
        }
    }

    #[test]
    fn login_fault_matches_native_harness_errors() {
        let _serial = crate::test_helpers::serial();
        assert!(crate::executor::login_fault(
            "not logged in: sign in with GitHub"
        ));
        assert!(crate::executor::login_fault("not_logged_in"));
    }

    #[test]
    fn local_run_worker_reports_exec_error() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("run.key");
        std::fs::write(&key, b"key").unwrap();
        let mut job = Row::new();
        job.insert("id", serde_json::json!("job-nf"));
        let mut project = Row::new();
        project.insert("slug", serde_json::json!("proj"));
        let mut e = LocalExec {
            s: Some((*s).clone()),
            job,
            project,
            log: RunLog::default(),
            clone_dir: dir.path().display().to_string(),
            key: key.display().to_string(),
            ..Default::default()
        };
        let missing = dir
            .path()
            .join("no-such-worker-binary")
            .display()
            .to_string();
        let err = e
            .run_worker(
                &pulse_bus::CancellationToken::new(),
                std::slice::from_ref(&missing),
                &BTreeMap::new(),
                1,
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&format!("worker command not found: {missing}")),
            "{err}"
        );
        assert!(err.contains("no such file or directory"), "{err}");
    }

    #[test]
    fn preflight_run_key_local_prepare_fails_early() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let project = insert_test_project(&s);
        let home = tempfile::tempdir().unwrap();
        let mut r = Row::new();
        r.insert("name", serde_json::json!("local"));
        let mut job = Row::new();
        job.insert("id", serde_json::json!("job-prep"));
        let mut e = new_local_exec(
            r,
            &s,
            &home.path().display().to_string(),
            job,
            project,
            "cinder",
            &mut RunLog::default(),
        );
        std::fs::create_dir_all(std::path::Path::new(&e.key).parent().unwrap()).unwrap();
        let err = e
            .prepare(&pulse_bus::CancellationToken::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("setup-error") && err.contains(&e.key), "{err}");
        assert!(!e.started());
    }

    #[test]
    fn pulse_on_path() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            crate::recipe::recipe_pulse_cli(&crate::recipe::Recipe::default()),
            ""
        );
        let (r, problems) = crate::recipe::parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("PULSE_CLI", "/opt/pulse/bin/pulse ")]),
            "",
        ));
        assert!(problems.is_empty(), "{problems:?}");
        let r = r.unwrap();
        assert_eq!(crate::recipe::recipe_pulse_cli(&r), "/opt/pulse/bin/pulse");
        assert!(crate::hands::HANDS_ENV_RE.is_match(&format!(
            "PULSE_CLI={}",
            crate::recipe::recipe_pulse_cli(&r)
        )));
        let (plain, _) = crate::recipe::parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("OTHER", "x")]),
            "",
        ));
        assert_eq!(crate::recipe::recipe_pulse_cli(&plain.unwrap()), "");
    }

    #[test]
    fn remote_pulse_cli() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            crate::run::remote_pulse_cli(None),
            crate::pair::PAIR_PULSE_PATH
        );
        let (r, problems) = crate::recipe::parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("PULSE_CLI", "/opt/pulse/bin/pulse")]),
            "",
        ));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            crate::run::remote_pulse_cli(r.as_ref()),
            "/opt/pulse/bin/pulse"
        );
    }

    static PULSE_REMOTE_RAN: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn fake_pulse_missing_ssh(
        _cancel: &pulse_bus::CancellationToken,
        argv: &[String],
        _stdin: &[u8],
        _timeout: Duration,
    ) -> Result<(i32, String, String)> {
        let args = argv
            .last()
            .and_then(|s| s.strip_prefix("b64 "))
            .and_then(|s| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.decode(s).ok()
            })
            .map(|v| {
                v.split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(|p| String::from_utf8_lossy(p).to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| argv.to_vec());
        match args.first().map(String::as_str) {
            Some("verify") => Ok((0, String::new(), String::new())),
            Some("probe") => Ok((
                0,
                "hands=ok pulse=missing pulse_lookup=/usr/bin:/bin heartbeat=1".into(),
                String::new(),
            )),
            Some("run") => {
                PULSE_REMOTE_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok((0, String::new(), String::new()))
            }
            _ => Ok((0, String::new(), String::new())),
        }
    }

    #[test]
    fn pulse_on_path_remote_preflight() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("run.key");
        std::fs::write(&key, b"key").unwrap();
        let mut row = Row::new();
        row.insert("name", serde_json::json!("hands"));
        row.insert("address", serde_json::json!("runner@example.invalid"));
        let mut job = Row::new();
        job.insert("id", serde_json::json!("job-remote"));
        let local = LocalExec {
            s: Some((*s).clone()),
            row,
            job,
            clone_dir: dir.path().join("clone").display().to_string(),
            key: key.display().to_string(),
            ..Default::default()
        };
        let mut e = SshExec {
            local,
            job_dir: dir.path().display().to_string(),
            bundle: String::new(),
        };
        let old = crate::executor::run_ssh_hook();
        crate::executor::set_run_ssh(fake_pulse_missing_ssh);
        PULSE_REMOTE_RAN.store(false, std::sync::atomic::Ordering::SeqCst);
        let err = e
            .run_worker(
                &pulse_bus::CancellationToken::new(),
                &["copilot".into()],
                &BTreeMap::new(),
                1,
            )
            .unwrap_err();
        crate::executor::restore_run_ssh(old);
        assert!(
            err.downcast_ref::<crate::executor::ExecutorFault>()
                .is_some(),
            "{err}"
        );
        let text = err.to_string();
        assert!(text.contains("pulse is not on PATH"), "{text}");
        assert!(text.contains("looked in PATH=/usr/bin:/bin"), "{text}");
        assert!(!PULSE_REMOTE_RAN.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn run_job_records_the_model_of_each_run_on_the_agent() {
        let _serial = crate::test_helpers::serial();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = crate::test_helpers::new_store();
        let project = insert_git_project(&s);
        crate::test_helpers::join(&s, "aurora");
        let args_file = std::path::Path::new(&s.home).join("worker-args");
        let script = std::path::Path::new(&s.home).join("runner.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho 'SUMMARY: looked and left the tree as it was'\n",
                args_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        set_setting(&s, "runner_cmd", &script.display().to_string());
        set_setting(&s, "harness", "copilot-cli");
        set_setting(&s, "worker_model", "heavy-test");
        set_setting(&s, "light_model", "light-test");

        for (kind, want) in [
            ("answer", "light-test"),
            ("task", "heavy-test"),
            ("reflect", "light-test"),
        ] {
            let (job, _) = crate::jobs::add_job(
                &s,
                project.str("slug"),
                kind.to_string(),
                format!("{kind} job"),
                "look around".to_string(),
                1,
                0,
                format!("test:{kind}"),
                crate::util::HUMAN_AGENT.to_string(),
                false,
            )
            .unwrap();
            let job = crate::jobs::lease_job(
                &s,
                "aurora".to_string(),
                crate::test_helpers::TEST_NOW.to_string(),
                crate::jobs::LeaseOpts {
                    job_id: job.str("id"),
                    ..Default::default()
                },
            )
            .unwrap();
            let run = crate::run::run_job(
                &s,
                job,
                "aurora",
                crate::test_helpers::TEST_NOW,
                crate::run::RunOpts::default(),
            )
            .unwrap();
            assert!(!run.0.is_empty(), "{kind}");
            let args = std::fs::read_to_string(&args_file).unwrap();
            let argv = args
                .trim_end_matches('\n')
                .lines()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(model_arg(&argv), want, "{kind}: {args}");
            let conn = s.conn().unwrap();
            let agent = crate::board::agent_row(&conn, "aurora".to_string());
            assert_eq!(agent.str("model"), want, "{kind}: {agent:?}");
            let _ = std::fs::remove_file(&args_file);
        }
    }

    #[test]
    fn lead_runs_with_the_light_model() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = crate::test_helpers::new_store();
        insert_test_project(&s);
        set_setting(&s, "worker_model", "heavy-test");
        let (lead, created) = crate::lead::ensure_lead(&s, "proj".to_string()).unwrap();
        assert!(created, "lead: {lead:?}");
        assert_eq!(lead.as_ref().map(|a| a.model.as_str()), Some("heavy-test"));
        set_setting(&s, "light_model", "light-test");
        let (cmd, _) = worker_command(&s, "lead", "brief", "/tmp/clone", "", &[], false);
        assert_eq!(model_arg(&cmd), "light-test");
        let (lead2, _) = crate::board::join_agent(
            &s,
            "other-lead".to_string(),
            String::new(),
            s.run_model("lead"),
            String::new(),
            String::new(),
            crate::lead::LEAD_KIND.to_string(),
            String::new(),
        )
        .unwrap();
        assert_eq!(lead2.str("model"), "light-test");
    }

    #[test]
    fn pick_worker_keeps_workers_of_every_model() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = crate::test_helpers::new_store();
        set_setting(&s, "quorum", "1");
        set_setting(&s, "runner_cmd", "copilot");
        set_setting(&s, "harness", "copilot-cli");
        set_setting(&s, "worker_model", "gpt-5-mini");
        let mut old = Vec::new();
        for _ in 0..2 {
            let r = crate::board::spawn_agent(
                &s,
                "copilot".to_string(),
                "gpt-5-mini".to_string(),
                "worker".to_string(),
            )
            .unwrap();
            old.push(r.str("name"));
        }
        set_setting(&s, "worker_model", "gpt-6-astra");
        set_setting(&s, "light_model", "gpt-5-mini");
        for _ in 0..3 {
            let name = crate::run::pick_worker(&s, crate::test_helpers::TEST_NOW, "", &[]).unwrap();
            assert!(
                old.iter().any(|want| want == &name),
                "picked {name}, want one of the pool {old:?}"
            );
        }
        let conn = s.conn().unwrap();
        let n = crate::util::must_one(
            &conn,
            "SELECT COUNT(*) AS n FROM agents WHERE status = 'active' AND kind = 'worker'",
            &[],
        )
        .int("n");
        assert_eq!(n, 2);
    }
}
