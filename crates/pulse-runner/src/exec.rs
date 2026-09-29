#![allow(unused_variables, dead_code, unused_imports)]
use crate::proc::Outcome;
use crate::util::Row;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
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
    pub(crate) home: String,
    pub(crate) job: Row,
    pub(crate) project: Row,
    pub(crate) agent: String,
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
    LocalExec {
        row: r,
        home: home.into(),
        job,
        project,
        agent: agent.into(),
        ..Default::default()
    }
}
impl LocalExec {
    pub(crate) fn branch_to_clone(&self) -> String {
        todo!("runner::exec::branch_to_clone")
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
        todo!("runner::exec::local_prepare")
    }
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome> {
        todo!("runner::exec::local_run_worker")
    }
    fn result_text(&self) -> Option<String> {
        self.last_out.clone()
    }
    fn commit_count(&self, base: &str) -> i32 {
        todo!("runner::exec::local_commit_count")
    }
    fn dirty(&self) -> String {
        todo!("runner::exec::local_dirty")
    }
    fn owner_paths_touched(&self, base: &str) -> Vec<String> {
        todo!("runner::exec::local_owner_paths_touched")
    }
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String) {
        todo!("runner::exec::local_verify")
    }
    fn install_pre_commit_hook(
        &self,
        verify_cmd: &str,
        recipe: Option<&crate::recipe::Recipe>,
        pins: &[String],
    ) -> Result<()> {
        todo!("runner::exec::local_install_pre_commit_hook")
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
    todo!("runner::exec::preflight_run_key")
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
        local: new_local_exec(r, s, home, job, project, agent, log),
        ..Default::default()
    }
}
impl SshExec {
    /// Go: func (e *sshExec) call(args []string, stdin []byte, timeout time.Duration) (int, string, string, error)
    pub(crate) fn call(
        &self,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<(i32, String, String)> {
        todo!("runner::exec::ssh_call")
    }
    pub(crate) fn preflight_remote_run_key(&self, when: &str) -> Result<()> {
        todo!("runner::exec::preflight_remote_run_key")
    }
    pub(crate) fn remote_key_path(&self) -> String {
        format!("{}/run.key", self.job_dir)
    }
    pub(crate) fn preflight_remote_cli(&self, path: &str) -> Result<()> {
        todo!("runner::exec::preflight_remote_cli")
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
        todo!("runner::exec::ssh_prepare")
    }
    fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<Outcome> {
        todo!("runner::exec::ssh_run_worker")
    }
    fn result_text(&self) -> Option<String> {
        todo!("runner::exec::ssh_result_text")
    }
    fn commit_count(&self, base: &str) -> i32 {
        todo!("runner::exec::ssh_commit_count")
    }
    fn dirty(&self) -> String {
        todo!("runner::exec::ssh_dirty")
    }
    fn owner_paths_touched(&self, base: &str) -> Vec<String> {
        todo!("runner::exec::ssh_owner_paths_touched")
    }
    fn verify(
        &self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &str,
        minutes: i32,
    ) -> (Option<i32>, String) {
        todo!("runner::exec::ssh_verify")
    }
    fn install_pre_commit_hook(
        &self,
        verify_cmd: &str,
        recipe: Option<&crate::recipe::Recipe>,
        pins: &[String],
    ) -> Result<()> {
        todo!("runner::exec::ssh_install_pre_commit_hook")
    }
    fn fetch_source(&self) -> String {
        todo!("runner::exec::ssh_fetch_source")
    }
    fn fetch_env(&self) -> Vec<String> {
        todo!("runner::exec::ssh_fetch_env")
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
    todo!("runner::exec::remote_key_check_result")
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
    todo!("runner::exec::remote_cli_check_result")
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
    todo!("runner::exec::make_executor")
}
/// Go: func workerDirs(home string) []string
pub(crate) fn worker_dirs(home: &str) -> Vec<String> {
    vec![format!("{home}/.cache"), "/tmp".into()]
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
    todo!("runner::exec::worker_command")
}
/// Go: func localCLI(home string) string
pub(crate) fn local_cli(home: &str) -> String {
    todo!("runner::exec::local_cli")
}
/// Go: func localCLIFallback(home string) string
pub(crate) fn local_cli_fallback(home: &str) -> String {
    todo!("runner::exec::local_cli_fallback")
}
/// Go: func workerEnvWithCLIPath(base []string, extra map[string]string) []string
pub(crate) fn worker_env_with_cli_path(
    base: &[String],
    extra: &BTreeMap<String, String>,
) -> Vec<String> {
    crate::proc::env_with(base, extra)
}
