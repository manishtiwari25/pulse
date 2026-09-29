#![allow(unused_variables, dead_code, unused_imports)]
pub(crate) use crate::util::GitResult;
use crate::util::Row;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
pub(crate) const LOCAL_EXECUTOR: &str = "local";
pub(crate) const UNREACHABLE_AFTER: i32 = 3;
pub(crate) const SSH_EXIT_UNREACH: i32 = 255;
pub(crate) const REMOTE_EXIT_TIMEOUT: i32 = 124;
pub(crate) const LOGIN_CHECK_PROMPT: &str = "Reply with exactly the word ok and nothing else.";
pub(crate) const NATIVE_LOGIN_PROMPT: &str = "Reply with exactly one line: SUMMARY: ok";
pub(crate) const LOGIN_CHECK_MINUTES: i32 = 2;
pub(crate) const LOGIN_EXPIRY_WARN: i32 = 7;
pub(crate) const GITHUB_API: &str = "https://api.github.com";
#[derive(Debug, Clone)]
pub struct ExecutorUnreachable {
    pub msg: String,
}
impl std::fmt::Display for ExecutorUnreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorUnreachable {}
#[derive(Debug, Clone)]
pub struct ExecutorFault {
    pub msg: String,
}
impl std::fmt::Display for ExecutorFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorFault {}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorUnfit {
    pub msg: String,
    pub tool: String,
    pub project: String,
    pub detail: String,
}
impl std::fmt::Display for ExecutorUnfit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorUnfit {}
/// Go: func loginFault(text string) bool
pub(crate) fn login_fault(text: &str) -> bool {
    text.to_lowercase().contains("login") || text.contains("401")
}
/// Go: func redactTokens(text string) string
pub(crate) fn redact_tokens(text: &str) -> String {
    regex::Regex::new(r"(github_pat_|gho_|ghu_|ghp_|ghs_|ghr_)[A-Za-z0-9_]+")
        .unwrap()
        .replace_all(text, "[redacted]")
        .to_string()
}
/// Go: func faultLine(text string, limit int) string
pub(crate) fn fault_line(text: &str, limit: usize) -> String {
    cut(
        text.lines().find(|l| !l.trim().is_empty()).unwrap_or(""),
        limit,
    )
}
/// Go: func cut(s string, n int) string
pub(crate) fn cut(s: &str, n: usize) -> String {
    if s.len() > n {
        s[..n.min(s.len())].to_string()
    } else {
        s.to_string()
    }
}
/// Go: func tail(s string, n int) string
pub(crate) fn tail(s: &str, n: usize) -> String {
    let lines = s.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
/// Go: func parseSSHAddress(text string) (string, string, int, error)
pub(crate) fn parse_ssh_address(text: &str) -> Result<(String, String, i32)> {
    todo!("runner::executor::parse_ssh_address")
}
/// Go: func normalizeProjects(text string) string
pub(crate) fn normalize_projects(text: &str) -> String {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}
/// Go: func executorAllows(r row, slug string) bool
pub(crate) fn executor_allows(r: &Row, slug: &str) -> bool {
    let p = r.str("projects");
    p.is_empty() || p.split(',').any(|x| x.trim() == slug)
}
/// Go: func executorProjects(r row) []string
pub(crate) fn executor_projects(r: &Row) -> Vec<String> {
    r.str("projects")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
/// Go: func localExecutorRow(s *store.Store) row
pub(crate) fn local_executor_row(s: &pulse_store::Store) -> Row {
    todo!("runner::executor::local_executor_row")
}
/// Go: func executorRows(s *store.Store, includeLocal bool) []row
pub(crate) fn executor_rows(s: &pulse_store::Store, include_local: bool) -> Vec<Row> {
    todo!("runner::executor::executor_rows")
}
/// Go: func executorRow(s *store.Store, name string) row
pub(crate) fn executor_row(s: &pulse_store::Store, name: &str) -> Row {
    todo!("runner::executor::executor_row")
}
/// Go: func addExecutor(...)
pub(crate) fn add_executor(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    address: &str,
    workdir: &str,
    workers: i32,
    projects: &str,
    created_by: &str,
) -> Result<Row> {
    todo!("runner::executor::add_executor")
}
pub(crate) fn executor_running(s: &pulse_store::Store, name: &str, now: &str) -> i32 {
    todo!("runner::executor::executor_running")
}
pub(crate) fn executor_free_slots(
    s: &pulse_store::Store,
    r: &Row,
    now: &str,
    inflight: i32,
) -> i32 {
    todo!("runner::executor::executor_free_slots")
}
pub(crate) fn pick_executor(s: &pulse_store::Store, slug: &str, now: &str) -> Row {
    todo!("runner::executor::pick_executor")
}
pub(crate) fn set_executor_status(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    status: &str,
    actor: &str,
    note: Option<&str>,
) -> Result<Row> {
    todo!("runner::executor::set_executor_status")
}
pub(crate) fn note_executor_run(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    status: &str,
    note: &str,
    transport_error: bool,
    fault: bool,
) {
    todo!("runner::executor::note_executor_run")
}
pub(crate) fn ssh_command(r: &Row, args: &[String]) -> Result<Vec<String>> {
    todo!("runner::executor::ssh_command")
}
pub(crate) type SshRunner =
    fn(&pulse_bus::CancellationToken, &[String], &[u8], Duration) -> Result<(i32, String, String)>;
pub(crate) static RUN_SSH: LazyLock<RwLock<SshRunner>> = LazyLock::new(|| RwLock::new(run_process));
/// Go: func runProcess(ctx context.Context, argv []string, stdin []byte, timeout time.Duration) (int, string, string, error)
pub(crate) fn run_process(
    cancel: &pulse_bus::CancellationToken,
    argv: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    todo!("runner::executor::run_process")
}
pub(crate) fn ssh_run(
    r: &Row,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    todo!("runner::executor::ssh_run")
}
pub(crate) fn ssh_run_context(
    cancel: &pulse_bus::CancellationToken,
    r: &Row,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    todo!("runner::executor::ssh_run_context")
}
pub(crate) fn admin_ssh_command(
    address: &str,
    remote_command: &str,
    control_path: &str,
) -> Result<Vec<String>> {
    todo!("runner::executor::admin_ssh_command")
}
pub(crate) fn admin_ssh_run(
    address: &str,
    remote_command: &str,
    stdin: &[u8],
    timeout: Duration,
    control_path: &str,
) -> Result<(i32, String, String)> {
    todo!("runner::executor::admin_ssh_run")
}
pub(crate) const LOGIN_PREP_SCRIPT: &str = "set -e; d=\"$HOME/.copilot\"; f=\"$d/settings.json\"; umask 077; mkdir -p \"$d\"; chmod 700 \"$d\"; ";
pub(crate) static LOGIN_PREP_COMMAND: LazyLock<String> =
    LazyLock::new(|| format!("sudo -n -u pulse -H sh -c '{}'", LOGIN_PREP_SCRIPT));
pub(crate) const LOGIN_COMMAND: &str = "sudo -n -u pulse -H timeout 90 copilot login --with-token";
/// Go: func classifyToken(token string) (string, error)
pub(crate) fn classify_token(token: &str) -> Result<String> {
    todo!("runner::executor::classify_token")
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenFacts {
    pub ok: Option<bool>,
    pub user: String,
    pub expires: String,
    pub note: String,
}
pub(crate) fn github_api_base(api: &str) -> String {
    api.trim_end_matches('/').to_string()
}
pub(crate) fn github_get(token: &str, url: &str) -> Result<(u16, Vec<u8>)> {
    todo!("runner::executor::github_get")
}
pub(crate) fn github_token_facts(token: &str, api: &str) -> TokenFacts {
    todo!("runner::executor::github_token_facts")
}
pub(crate) const NO_REPOSITORY_REFUSAL: &str = "the executor token must reach no repository: mint a fine-grained token with only the Copilot Requests permission";
pub(crate) fn github_no_repos(token: &str, api: &str) -> (bool, String) {
    todo!("runner::executor::github_no_repos")
}
pub(crate) fn expiry_warning(expires: &str, now: &str, days: i32) -> String {
    todo!("runner::executor::expiry_warning")
}
pub(crate) fn login_check_command(s: &pulse_store::Store) -> Vec<String> {
    todo!("runner::executor::login_check_command")
}
pub(crate) fn runner_cmd(s: &pulse_store::Store) -> String {
    todo!("runner::executor::runner_cmd")
}
pub(crate) fn tiny_bundle() -> Result<Vec<u8>> {
    todo!("runner::executor::tiny_bundle")
}
pub(crate) fn login_check(
    s: &pulse_store::Store,
    name: &str,
    actor: &str,
    now: &str,
    apply: bool,
) -> Result<(bool, String)> {
    todo!("runner::executor::login_check")
}
pub(crate) fn set_executor_admin(s: &pulse_store::Store, name: &str, admin: &str) {
    todo!("runner::executor::set_executor_admin")
}
pub(crate) fn login_drained(r: &Row) -> bool {
    todo!("runner::executor::login_drained")
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LoginResult {
    pub(crate) ok: bool,
    pub(crate) kind: String,
    pub(crate) rc: i32,
    pub(crate) lines: Vec<String>,
}
pub(crate) fn last_lines(text: &str, n: usize) -> Vec<String> {
    text.lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(ToOwned::to_owned)
        .collect()
}
pub(crate) type PlaceLoginFn = fn(
    &pulse_store::Store,
    &str,
    &str,
    &str,
    &mut dyn Read,
    bool,
    bool,
    &str,
) -> Result<LoginResult>;
pub(crate) static PLACE_LOGIN: LazyLock<RwLock<PlaceLoginFn>> =
    LazyLock::new(|| RwLock::new(executor_login));
pub(crate) fn executor_login(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    admin_address: &str,
    token: &mut dyn Read,
    is_tty: bool,
    remove: bool,
    actor: &str,
) -> Result<LoginResult> {
    todo!("runner::executor::executor_login")
}
pub(crate) fn parse_probe(text: &str) -> BTreeMap<String, String> {
    todo!("runner::executor::parse_probe")
}
pub(crate) fn probe_executor(
    s: &pulse_store::Store,
    name: &str,
    actor: &str,
    now: &str,
) -> Result<(bool, String)> {
    todo!("runner::executor::probe_executor")
}
pub(crate) fn sweep_executor_jobs(
    s: &pulse_store::Store,
    now: &str,
    r: &Row,
    jobs: &str,
) -> Vec<String> {
    todo!("runner::executor::sweep_executor_jobs")
}
pub(crate) fn last_login_check(s: &pulse_store::Store, name: &str) -> BTreeMap<String, Value> {
    todo!("runner::executor::last_login_check")
}
pub(crate) fn login_expiry(s: &pulse_store::Store, name: &str) -> String {
    todo!("runner::executor::login_expiry")
}
