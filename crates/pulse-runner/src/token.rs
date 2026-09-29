#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::Result;
use std::io::{Cursor, Read};
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::SystemTime;
pub(crate) const EXECUTOR_TOKEN_FILE: &str = "executor-token";
pub(crate) const GH_SOURCE_CACHE_MINS: u64 = 10;
pub(crate) const LOGIN_SOURCE_TOKEN: &str = "executor token";
pub(crate) const LOGIN_SOURCE_HUB_LOGIN: &str = "hub login";
pub(crate) const LOGIN_SOURCE_PROVIDER: &str = "provider login";
pub(crate) const LOGIN_SOURCE_NONE: &str = "none";
/// Go: func executorTokenPath(s *store.Store) string
pub(crate) fn executor_token_path(s: &pulse_store::Store) -> String {
    std::path::Path::new(&s.home)
        .join(EXECUTOR_TOKEN_FILE)
        .display()
        .to_string()
}
pub(crate) type GhAuthTokenFn = fn() -> Result<String>;
pub(crate) static GH_AUTH_TOKEN: LazyLock<RwLock<GhAuthTokenFn>> =
    LazyLock::new(|| RwLock::new(|| Ok(String::new())));
/// Go: func checkExecutorToken(token string) (tokenFacts, error)
pub(crate) fn check_executor_token(token: &str) -> Result<crate::executor::TokenFacts> {
    todo!("runner::token::check_executor_token")
}
/// Go: func executorTokenSet(s *store.Store, now string, token io.Reader, isTTY bool, actor string) ([]string, error)
pub(crate) fn executor_token_set(
    s: &pulse_store::Store,
    now: &str,
    token: &mut dyn Read,
    is_tty: bool,
    actor: &str,
) -> Result<Vec<String>> {
    todo!("runner::token::executor_token_set")
}
/// Go: func executorTokenRemove(s *store.Store, now, actor string) ([]string, error)
pub(crate) fn executor_token_remove(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
) -> Result<Vec<String>> {
    todo!("runner::token::executor_token_remove")
}
/// Go: func executorTokenReport(s *store.Store, now string) []string
pub(crate) fn executor_token_report(s: &pulse_store::Store, now: &str) -> Vec<String> {
    todo!("runner::token::executor_token_report")
}
/// Go: func orDash(s string) string
pub(crate) fn or_dash(s: &str) -> String {
    if s.is_empty() { "-".into() } else { s.into() }
}
/// Go: func expiresText(expires string) string
pub(crate) fn expires_text(expires: &str) -> String {
    if expires.is_empty() {
        "never".into()
    } else {
        expires.into()
    }
}
/// Go: func findingProject(s *store.Store) string
pub(crate) fn finding_project(s: &pulse_store::Store) -> String {
    todo!("runner::token::finding_project")
}
#[derive(Debug, Default)]
pub(crate) struct GhSourceCache {
    pub(crate) token: String,
    pub(crate) ok: bool,
    pub(crate) at: Option<SystemTime>,
    pub(crate) fetched: String,
    pub(crate) fetched_at: Option<SystemTime>,
}
pub(crate) static GH_SOURCE: LazyLock<Mutex<GhSourceCache>> =
    LazyLock::new(|| Mutex::new(GhSourceCache::default()));
pub(crate) static PROVIDER_SOURCE: LazyLock<Mutex<GhSourceCache>> =
    LazyLock::new(|| Mutex::new(GhSourceCache::default()));
impl GhSourceCache {
    /// Go: func (c *ghSourceCache) forget()
    pub(crate) fn forget(&mut self) {
        *self = Self::default()
    }
    /// Go: func (c *ghSourceCache) fetch() (string, error)
    pub(crate) fn fetch(&mut self) -> Result<String> {
        todo!("runner::token::gh_source_cache_fetch")
    }
    /// Go: func (c *ghSourceCache) passes(tok string) bool
    pub(crate) fn passes(&self, tok: &str) -> bool {
        self.ok && self.token == tok
    }
}
/// Go: func loginSource(s *store.Store) ([]byte, string, error)
pub(crate) fn login_source(s: &pulse_store::Store) -> Result<(Vec<u8>, String)> {
    todo!("runner::token::login_source")
}
/// Go: func isNoStoredLogin(err error) bool
pub(crate) fn is_no_stored_login(err: &anyhow::Error) -> bool {
    err.to_string().contains("no stored login")
}
/// Go: func hasLoginSource(s *store.Store) bool
pub(crate) fn has_login_source(s: &pulse_store::Store) -> bool {
    login_source(s).is_ok()
}
/// Go: func tokenReader(tok []byte) io.Reader
pub(crate) fn token_reader(tok: Vec<u8>) -> Cursor<Vec<u8>> {
    Cursor::new(tok)
}
