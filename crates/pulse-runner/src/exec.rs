#![allow(unused_variables, dead_code, unused_imports)]
use crate::proc::Outcome;
use crate::util::Row;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
/// Go: type executor interface {...}
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
    ) -> Result<()>;
    fn fetch_source(&self) -> String;
    fn fetch_env(&self) -> Vec<String>;
    fn cleanup(&self);
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RunLog {
    pub(crate) lines: Vec<String>,
}
impl RunLog {
    /// Go: func (l *runLog) add(format string, args ...any)
    pub(crate) fn add(&mut self, line: impl Into<String>) {
        self.lines.push(line.into())
    }
    /// Go: func (l *runLog) put(line string)
    pub(crate) fn put(&mut self, line: &str) {
        self.lines.push(line.into())
    }
    /// Go: func (l *runLog) text() string
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }
}
impl Write for RunLog {
    fn write(&mut self, p: &[u8]) -> std::io::Result<usize> {
        self.lines.push(String::from_utf8_lossy(p).to_string());
        Ok(p.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// Go: func tailStr(s string, n int) string
pub(crate) fn tail_str(s: &str, n: usize) -> String {
    crate::executor::tail(s, n)
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
/// Go: func newLocalExec(r row, s *store.Store, home string, job, project row, agent string, log *runLog) *localExec
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
        let _ = std::fs::remove_dir_all(&self.clone_dir);
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
        preflight_run_key(&self.key)?;
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
        let out = crate::proc::run_group_observed(
            cancel,
            cmd,
            &self.clone_dir,
            &worker_env,
            &[],
            budget,
            None,
        )
        .map_err(|e| {
            let setting = self
                .s
                .as_ref()
                .map(|s| s.setting("runner_cmd", ""))
                .unwrap_or_default();
            crate::util::fail(
                1,
                format!(
                    "worker command not found: {} (setting runner_cmd={}): {e}",
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
        let _ = base;
        Vec::new()
    }
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String) {
        let secs = (minutes.max(1) as u64 * 60).max(60);
        match crate::proc::run_group(
            cancel,
            &["/bin/sh".into(), "-c".into(), cmd.into()],
            &self.clone_dir,
            &crate::proc::environ(),
            &[],
            Duration::from_secs(secs),
        ) {
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
    ) -> Result<()> {
        if verify_cmd.is_empty() {
            return Ok(());
        }
        crate::keys::write_pre_commit_hook(
            &self.clone_dir,
            &crate::keys::pre_commit_hook_script(verify_cmd, recipe, pins, &self.clone_dir),
        )
    }
    fn fetch_source(&self) -> String {
        self.clone_dir.clone()
    }
    fn fetch_env(&self) -> Vec<String> {
        crate::util::git_env(&[])
    }
    fn cleanup(&self) {}
}
/// Go: func preflightRunKey(path string) error
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
/// Go: func newSSHExec(r row, s *store.Store, home string, job, project row, agent string, log *runLog) *sshExec
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
    /// Go: func (e *sshExec) call(args []string, stdin []byte, timeout time.Duration) (int, string, string, error)
    pub(crate) fn call(
        &self,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<(i32, String, String)> {
        let cancel = pulse_bus::CancellationToken::new();
        match crate::executor::RUN_SSH.read().unwrap()(&cancel, args, stdin, timeout) {
            Ok(v) => Ok(v),
            Err(e) => Err(e),
        }
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
        Err(anyhow!(
            "ssh prepare unavailable until executor transport slice is filled"
        ))
    }
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome> {
        let _ = (cancel, cmd, env, minutes);
        Err(anyhow!(
            "ssh run unavailable until executor transport slice is filled"
        ))
    }
    fn result_text(&self) -> Option<String> {
        self.local.last_out.clone()
    }
    fn commit_count(&self, base: &str) -> i32 {
        let _ = base;
        0
    }
    fn dirty(&self) -> String {
        String::new()
    }
    fn owner_paths_touched(&self, base: &str) -> Vec<String> {
        let _ = base;
        Vec::new()
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
        match self.call(
            &[
                "verify".into(),
                self.local.job.str("id"),
                budget,
                "--".into(),
                cmd.into(),
            ],
            &[],
            Duration::from_secs((minutes.max(1) * 60 + 120) as u64),
        ) {
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
    ) -> Result<()> {
        if verify_cmd.is_empty() {
            return Ok(());
        }
        let script =
            crate::keys::pre_commit_hook_script(verify_cmd, recipe, pins, &self.local.clone_dir);
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
        format!("ssh://{}", self.local.clone_dir)
    }
    fn fetch_env(&self) -> Vec<String> {
        crate::util::git_env(&[])
    }
    fn cleanup(&self) {}
}
/// Go: func remoteKeyCheckResult(path, host, when string, rc int, out, errText string) error
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
/// Go: func remoteKeyCheckCmd(path string) string
pub(crate) fn remote_key_check_cmd(path: &str) -> String {
    format!("test -s {}", crate::util::shell_quote(path))
}
/// Go: func remoteCLICheckCmd(path string) string
pub(crate) fn remote_cli_check_cmd(path: &str) -> String {
    format!("test -x {}", crate::util::shell_quote(path))
}
/// Go: func remoteCLICheckResult(path, host string, rc int, out, errText string) error
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
/// Go: func makeExecutor(r row, s *store.Store, home string, job, project row, agent string, log *runLog) (executor, error)
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
/// Go: func workerDirs(home string) []string
pub(crate) fn worker_dirs(home: &str) -> Vec<String> {
    let mut dirs = vec![home.to_string()];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.display().to_string());
        }
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
/// Go: func workerCommand(s *store.Store, kind, briefText, clone, workerCmd string, dirs []string, remote bool) ([]string, bool)
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
/// Go: func localCLI(home string) string
pub(crate) fn local_cli(home: &str) -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| local_cli_fallback(home))
}
/// Go: func localCLIFallback(home string) string
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
        if let Ok(m) = std::fs::metadata(&cand) {
            if m.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if m.permissions().mode() & 0o111 != 0 {
                        return cand;
                    }
                }
            }
        }
    }
    String::new()
}
/// Go: func workerEnvWithCLIPath(base []string, extra map[string]string) []string
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
    fn preflight_run_key_missing_is_setup_error() {
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
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("run.key").display().to_string();
        assert!(
            !std::process::Command::new("/bin/sh")
                .args(["-c", &remote_key_check_cmd(&key)])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(&key, []).unwrap();
        assert!(
            !std::process::Command::new("/bin/sh")
                .args(["-c", &remote_key_check_cmd(&key)])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(&key, b"k").unwrap();
        assert!(
            std::process::Command::new("/bin/sh")
                .args(["-c", &remote_key_check_cmd(&key)])
                .status()
                .unwrap()
                .success()
        );
        assert!(remote_key_check_cmd("/w/job 1/run.key").contains("'/w/job 1/run.key'"));
    }

    #[test]
    fn preflight_run_key_remote_result() {
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
    fn preflight_remote_cli_check_cmd() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("pulse").display().to_string();
        assert!(
            !std::process::Command::new("/bin/sh")
                .args(["-c", &remote_cli_check_cmd(&cli)])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(&cli, b"#!/bin/sh\n").unwrap();
        assert!(
            !std::process::Command::new("/bin/sh")
                .args(["-c", &remote_cli_check_cmd(&cli)])
                .status()
                .unwrap()
                .success()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(
            std::process::Command::new("/bin/sh")
                .args(["-c", &remote_cli_check_cmd(&cli)])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            remote_cli_check_cmd("/usr/local/bin/my pulse").contains("'/usr/local/bin/my pulse'")
        );
    }

    #[test]
    fn preflight_remote_cli_result() {
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
}
