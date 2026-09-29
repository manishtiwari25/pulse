#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const ISO_LAYOUT: &str = "%Y-%m-%dT%H:%M:%SZ";
pub(crate) const HUMAN_AGENT: &str = pulse_store::HUMAN_AGENT;
pub(crate) const COMMONS_SLUG: &str = pulse_store::COMMONS_SLUG;

/// Go: func contains(list []string, s string) bool
pub(crate) fn contains(list: &[String], s: &str) -> bool {
    list.iter().any(|x| x == s)
}

/// Go: func fail(code int, format string, args ...any) error
pub(crate) fn fail(code: i32, message: impl Into<String>) -> pulse_cli::Error {
    pulse_cli::errorf(code, &message.into(), "", "")
}

/// Go: func stampOf(iso string) string
pub(crate) fn stamp_of(iso: &str) -> String {
    iso.replace(['-', ':'], "")
}

/// Go: func parseISO(value string) (time.Time, error)
pub(crate) fn parse_iso(value: &str) -> Result<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(value, ISO_LAYOUT)
        .map(|n| n.and_utc())
        .map_err(|_| {
            anyhow!(fail(
                2,
                format!("Not a UTC time like 2026-09-05T08:00:00Z: {value:?}")
            ))
        })
}

/// Go: func mustISO(value string) time.Time
pub(crate) fn must_iso(value: &str) -> DateTime<Utc> {
    parse_iso(value).unwrap_or_else(|_| DateTime::<Utc>::UNIX_EPOCH)
}

/// Go: func formatISO(t time.Time) string
pub(crate) fn format_iso(t: DateTime<Utc>) -> String {
    t.format(ISO_LAYOUT).to_string()
}

/// Go: func plusHours(iso string, hours float64) string
pub(crate) fn plus_hours(iso: &str, hours: f64) -> String {
    format_iso(must_iso(iso) + chrono::Duration::milliseconds((hours * 3_600_000.0) as i64))
}

/// Go: func plusMinutes(iso string, minutes float64) string
pub(crate) fn plus_minutes(iso: &str, minutes: f64) -> String {
    format_iso(must_iso(iso) + chrono::Duration::milliseconds((minutes * 60_000.0) as i64))
}

/// Go: func dayNumber(value string) int
pub(crate) fn day_number(value: &str) -> i32 {
    if value.len() < 10 {
        return 0;
    }
    let Ok(d) = NaiveDate::parse_from_str(&value[..10], "%Y-%m-%d") else {
        return 0;
    };
    chrono::Datelike::num_days_from_ce(&d)
}

static NON_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

/// Go: func slugify(name string, limit int) string
pub(crate) fn slugify(name: &str, limit: usize) -> String {
    let mut slug = NON_SLUG
        .replace_all(&name.to_lowercase(), "-")
        .trim_matches('-')
        .to_string();
    if slug.len() > limit {
        slug.truncate(limit);
    }
    slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "x".to_string()
    } else {
        slug
    }
}

/// Go: func sha256Text(text string) string
pub(crate) fn sha256_text(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Go: func trunc(text string, n int) string
pub(crate) fn trunc(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > n && n > 0 {
        chars[..n - 1].iter().collect::<String>() + "~"
    } else {
        text.to_string()
    }
}

/// Go: func plural(n int, word string) string
pub(crate) fn plural(n: i32, word: &str) -> String {
    if n == 1 {
        word.into()
    } else {
        format!("{word}s")
    }
}

/// Go: func isInside(path, root string) bool
pub(crate) fn is_inside(path: &str, root: &str) -> bool {
    let p = std::fs::canonicalize(path).or_else(|_| std::env::current_dir().map(|d| d.join(path)));
    let r = std::fs::canonicalize(root).or_else(|_| std::env::current_dir().map(|d| d.join(root)));
    match (p, r) {
        (Ok(p), Ok(r)) => p == r || p.starts_with(r),
        _ => false,
    }
}

/// Go: func newID(prefix, now string) string
pub(crate) fn new_id(prefix: &str, now: &str) -> String {
    let b: [u8; 3] = rand::random();
    format!("{prefix}-{}-{}", stamp_of(now), hex::encode(b))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GitResult {
    pub(crate) code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    #[serde(skip)]
    pub(crate) err: Option<String>,
}
impl GitResult {
    /// Go: func (g gitResult) ok() bool
    pub(crate) fn ok(&self) -> bool {
        self.err.is_none() && self.code == 0
    }
}

/// Go: func gitTimeout(repo string, timeout time.Duration, args ...string) gitResult
pub(crate) fn git_timeout(repo: &str, timeout: Duration, args: &[&str]) -> GitResult {
    git_timeout_env(repo, timeout, &[], args)
}

/// Go: func gitTimeoutEnv(repo string, timeout time.Duration, env []string, args ...string) gitResult
pub(crate) fn git_timeout_env(
    repo: &str,
    timeout: Duration,
    env: &[String],
    args: &[&str],
) -> GitResult {
    let mut full: Vec<String> = Vec::new();
    if !repo.is_empty() {
        full.extend(["-C".into(), repo.into()]);
    }
    full.extend(args.iter().map(|s| s.to_string()));
    let out = Command::new("git")
        .args(&full)
        .envs(git_env(env).into_iter().filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        }))
        .output();
    match out {
        Ok(o) => GitResult {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
            err: None,
        },
        Err(e) => GitResult {
            code: -1,
            err: Some(e.to_string()),
            ..GitResult::default()
        },
    }
}

/// Go: func gitEnv(extra []string) []string
pub(crate) fn git_env(extra: &[String]) -> Vec<String> {
    let mut e = pulse_gitenv::env();
    e.extend_from_slice(extra);
    e
}
/// Go: func stripHookGitEnv(env []string) []string
pub(crate) fn strip_hook_git_env(env: &[String]) -> Vec<String> {
    pulse_gitenv::strip_hook(env.to_vec())
}
/// Go: func git(repo string, args ...string) gitResult
pub(crate) fn git(repo: &str, args: &[&str]) -> GitResult {
    git_timeout(repo, Duration::from_secs(60), args)
}
/// Go: func gitCheck(repo string, args ...string) (gitResult, error)
pub(crate) fn git_check(repo: &str, args: &[&str]) -> Result<GitResult> {
    let r = git(repo, args);
    if r.ok() {
        Ok(r)
    } else {
        Err(anyhow!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo,
            r.stderr.trim()
        ))
    }
}
/// Go: func gitBranch(repo string) string
pub(crate) fn git_branch(repo: &str) -> String {
    git_timeout(
        repo,
        Duration::from_secs(10),
        &["rev-parse", "--abbrev-ref", "HEAD"],
    )
    .stdout
    .trim()
    .to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Row(pub(crate) BTreeMap<String, Value>);
impl Row {
    pub(crate) fn new() -> Self {
        Self(BTreeMap::new())
    }
    pub(crate) fn insert(&mut self, k: impl Into<String>, v: Value) {
        self.0.insert(k.into(), v);
    }
    /// Go: func (r row) str(key string) string
    pub(crate) fn str(&self, key: &str) -> String {
        match self.0.get(key) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(v) => v.to_string(),
        }
    }
    /// Go: func (r row) int(key string) int
    pub(crate) fn int(&self, key: &str) -> i32 {
        match self.0.get(key) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
            Some(Value::String(s)) => s.parse().unwrap_or(0),
            Some(Value::Bool(b)) => *b as i32,
            _ => 0,
        }
    }
    /// Go: func (r row) float(key string) float64
    pub(crate) fn float(&self, key: &str) -> f64 {
        match self.0.get(key) {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::String(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }
}
impl std::ops::Deref for Row {
    type Target = BTreeMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Row {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub(crate) trait Querier {
    fn conn(&self) -> &rusqlite::Connection;
}
impl Querier for rusqlite::Connection {
    fn conn(&self) -> &rusqlite::Connection {
        self
    }
}
impl Querier for pulse_store::PooledConn {
    fn conn(&self) -> &rusqlite::Connection {
        self
    }
}
impl Querier for pulse_store::Tx {
    fn conn(&self) -> &rusqlite::Connection {
        self
    }
}

fn sql_value(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef::*;
    match v {
        Null => Value::Null,
        Integer(i) => json!(i),
        Real(f) => json!(f),
        Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
        Blob(b) => Value::String(String::from_utf8_lossy(b).to_string()),
    }
}

/// Go: func queryRows(q querier, query string, args ...any) ([]row, error)
pub(crate) fn query_rows(
    q: &(impl Querier + ?Sized),
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Row>> {
    let mut stmt = q.conn().prepare(query)?;
    let names = stmt
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(rr) = rows.next()? {
        let mut r = Row::new();
        for (i, c) in names.iter().enumerate() {
            r.insert(c.clone(), sql_value(rr.get_ref(i)?));
        }
        out.push(r);
    }
    Ok(out)
}
/// Go: func queryOne(q querier, query string, args ...any) (row, error)
pub(crate) fn query_one(
    q: &(impl Querier + ?Sized),
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Option<Row>> {
    Ok(query_rows(q, query, args)?.into_iter().next())
}
/// Go: func mustRows(q querier, query string, args ...any) []row
pub(crate) fn must_rows(
    q: &(impl Querier + ?Sized),
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Vec<Row> {
    query_rows(q, query, args).unwrap()
}
/// Go: func mustOne(q querier, query string, args ...any) row
pub(crate) fn must_one(
    q: &(impl Querier + ?Sized),
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Row {
    query_one(q, query, args).unwrap().unwrap_or_default()
}
/// Go: func exec1(q querier, query string, args ...any)
pub(crate) fn exec1(q: &(impl Querier + ?Sized), query: &str, args: &[&dyn rusqlite::ToSql]) {
    q.conn().execute(query, args).unwrap();
}
/// Go: func eventAt(q querier, at, actor, action, subject string, detail any)
pub(crate) fn event_at(
    q: &(impl Querier + ?Sized),
    at: &str,
    actor: &str,
    action: &str,
    subject: &str,
    detail: Value,
) {
    let text = if detail.is_null() {
        String::new()
    } else if let Some(s) = detail.as_str() {
        s.to_string()
    } else {
        py_json(&detail)
    };
    exec1(
        q,
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        &[&at, &actor, &action, &subject, &text],
    );
}
/// Go: func recover1(err *error)
pub(crate) fn recover1() {}
/// Go: func jsonList(text string) []string
pub(crate) fn json_list(text: &str) -> Vec<String> {
    if text.is_empty() {
        vec![]
    } else {
        serde_json::from_str(text).unwrap_or_default()
    }
}
/// Go: func jsonMap(text string) map[string]any
pub(crate) fn json_map(text: &str) -> BTreeMap<String, Value> {
    if text.is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_str(text).unwrap_or_default()
    }
}
/// Go: func pyJSON(v any) string
pub(crate) fn py_json<T: serde::Serialize>(v: T) -> String {
    pulse_store::py_json(v).unwrap_or_default()
}
/// Go: func fileExists(path string) bool
pub(crate) fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}
/// Go: func readFile(path string) string
pub(crate) fn read_file(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}
/// Go: func homeDir() string
pub(crate) fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}
static SAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9@%+=:,./_-]+$").unwrap());
/// Go: func shellQuote(s string) string
pub(crate) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        "''".into()
    } else if SAFE.is_match(s) {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
/// Go: func shellJoin(args []string) string
pub(crate) fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}
/// Go: func splitShell(s string) []string
pub(crate) fn split_shell(s: &str) -> Vec<String> {
    shlex_split(s)
}
fn shlex_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < b.len() && b[i] as char != '\'' {
                    cur.push(b[i] as char);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < b.len() && b[i] as char != '"' {
                    if b[i] as char == '\\'
                        && i + 1 < b.len()
                        && "\"\\$`\n".contains(b[i + 1] as char)
                    {
                        i += 1;
                    }
                    cur.push(b[i] as char);
                    i += 1;
                }
            }
            '\\' if i + 1 < b.len() => {
                in_word = true;
                i += 1;
                cur.push(b[i] as char)
            }
            ' ' | '\t' | '\n' => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                in_word = true;
                cur.push(c)
            }
        }
        i += 1;
    }
    if in_word {
        out.push(cur)
    }
    out
}
/// Go: func settingInt(s *store.Store, key string) int
pub(crate) fn setting_int(s: &pulse_store::Store, key: &str) -> i32 {
    s.setting_int(key) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_env_drops_hook_variables() {
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("GIT_INDEX_FILE", "/elsewhere/index");
            std::env::set_var("GIT_PREFIX", "sub/");
            std::env::set_var("GIT_AUTHOR_NAME", "someone else");
            std::env::set_var("GIT_DIR", "/elsewhere/.git");
            std::env::set_var("KEEP_ME", "yes");
        }
        let env = git_env(&["EXTRA=1".into()]);
        for kv in &env {
            assert!(!pulse_gitenv::hook_var(kv), "gitEnv leaked {kv:?}");
        }
        for want in ["KEEP_ME=yes", "EXTRA=1", "GIT_TERMINAL_PROMPT=0"] {
            assert!(
                env.iter().any(|kv| kv == want),
                "gitEnv missing {want:?}: {env:?}"
            );
        }
    }

    #[test]
    fn git_env_takes_no_optional_locks_and_never_prompts() {
        let env = format!(
            "\n{}\n",
            git_env(&["GIT_SSH_COMMAND=ssh -i key".into()]).join("\n")
        );
        for want in [
            "\nGIT_TERMINAL_PROMPT=0\n",
            "\nGIT_OPTIONAL_LOCKS=0\n",
            "\nGIT_SSH_COMMAND=ssh -i key\n",
        ] {
            assert!(env.contains(want), "git env lacks {want:?}:\n{env}");
        }
    }
}
