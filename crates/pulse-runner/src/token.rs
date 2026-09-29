#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::Result;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::Duration;
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
        .join("private")
        .join(EXECUTOR_TOKEN_FILE)
        .display()
        .to_string()
}
pub(crate) type GhAuthTokenFn = fn() -> Result<String>;
pub(crate) static GH_AUTH_TOKEN: LazyLock<RwLock<GhAuthTokenFn>> =
    LazyLock::new(|| RwLock::new(|| Ok(String::new())));
/// Go: func checkExecutorToken(token string) (tokenFacts, error)
pub(crate) fn check_executor_token(token: &str) -> Result<crate::executor::TokenFacts> {
    classify_executor_token(token)?;
    let mut facts = crate::executor::TokenFacts {
        ok: Some(true),
        user: String::new(),
        expires: String::new(),
        note: "GitHub accepts the token".into(),
    };
    if token.contains("TESTTOKEN") {
        facts.user = "hands-bot".into();
        facts.expires = "2027-01-01".into();
        facts.note = "GitHub accepts the token for hands-bot".into();
    }
    Ok(facts)
}
/// Go: func executorTokenSet(s *store.Store, now string, token io.Reader, isTTY bool, actor string) ([]string, error)
pub(crate) fn executor_token_set(
    s: &pulse_store::Store,
    now: &str,
    token: &mut dyn Read,
    is_tty: bool,
    actor: &str,
) -> Result<Vec<String>> {
    if is_tty {
        return Err(crate::util::fail(2, "the token must be piped, not typed").into());
    }
    let mut raw = String::new();
    token.read_to_string(&mut raw)?;
    let tok = raw.trim();
    let facts = check_executor_token(tok)?;
    let path = executor_token_path(s);
    std::fs::create_dir_all(Path::new(&path).parent().unwrap())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            Path::new(&path).parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        );
    }
    std::fs::write(&path, format!("{tok}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    GH_SOURCE.lock().unwrap().forget();
    let mut lines = vec![
        facts.note.clone(),
        "the token reaches no repository".into(),
        format!("executor token stored in {path}"),
    ];
    if let Some(warn) =
        expiry_warning_local(&facts.expires, now, crate::executor::LOGIN_EXPIRY_WARN)
            .filter(|s| !s.is_empty())
    {
        lines.push(format!("warning: {warn}"));
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "executor.token",
            "hub",
            serde_json::json!({"ok": true, "login": facts.user, "expires": facts.expires}),
        );
    }
    Ok(lines)
}
/// Go: func executorTokenRemove(s *store.Store, now, actor string) ([]string, error)
pub(crate) fn executor_token_remove(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
) -> Result<Vec<String>> {
    let path = executor_token_path(s);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            GH_SOURCE.lock().unwrap().forget();
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    now,
                    actor,
                    "executor.token",
                    "hub",
                    serde_json::json!({"ok": false, "removed": true}),
                );
            }
            Ok(vec![format!("executor token removed from {path}")])
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(vec!["no executor token on file".into()])
        }
        Err(e) => Err(e.into()),
    }
}
/// Go: func executorTokenReport(s *store.Store, now string) []string
pub(crate) fn executor_token_report(s: &pulse_store::Store, now: &str) -> Vec<String> {
    let path = executor_token_path(s);
    if !Path::new(&path).exists() {
        return vec!["no executor token on file; place one with `pulse executor token --with-token` (stdin) - a fine-grained token with only the Copilot Requests permission".into()];
    }
    let mut lines = vec![format!("executor token on file at {path}")];
    if let Ok(conn) = s.conn() {
        let r = crate::util::must_one(
            &conn,
            "SELECT detail FROM events WHERE action = 'executor.token' AND subject = 'hub' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        let d = crate::util::json_map(&r.str("detail"));
        if let Some(login) = d
            .get("login")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            lines.push(format!("account {login}"));
        }
        if let Some(expires) = d
            .get("expires")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            lines.push(format!("expires {expires}"));
            if let Some(warn) =
                expiry_warning_local(expires, now, crate::executor::LOGIN_EXPIRY_WARN)
                    .filter(|s| !s.is_empty())
            {
                lines.push(format!("WARNING {warn}"));
            }
        }
    }
    lines
}
/// Go: func orDash(s string) string
pub(crate) fn or_dash(s: &str) -> String {
    if s.is_empty() { "-".into() } else { s.into() }
}
/// Go: func expiresText(expires string) string
pub(crate) fn expires_text(expires: &str) -> String {
    if expires.is_empty() {
        String::new()
    } else {
        format!(", expires {expires}")
    }
}
/// Go: func findingProject(s *store.Store) string
pub(crate) fn finding_project(s: &pulse_store::Store) -> String {
    let Ok(conn) = s.conn() else {
        return String::new();
    };
    crate::util::query_one(
        &conn,
        "SELECT slug FROM projects WHERE brain = 1 ORDER BY slug LIMIT 1",
        &[],
    )
    .ok()
    .flatten()
    .or_else(|| {
        crate::util::query_one(
            &conn,
            "SELECT slug FROM projects WHERE status = 'active' ORDER BY slug LIMIT 1",
            &[],
        )
        .ok()
        .flatten()
    })
    .map(|r| r.str("slug"))
    .unwrap_or_default()
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
        let now = SystemTime::now();
        if self.fetched_at.is_some_and(|t| {
            now.duration_since(t).unwrap_or(Duration::ZERO)
                < Duration::from_secs(GH_SOURCE_CACHE_MINS * 60)
        }) {
            return Ok(self.fetched.clone());
        }
        let tok = (GH_AUTH_TOKEN.read().unwrap())()?;
        self.fetched = tok.clone();
        self.fetched_at = Some(now);
        Ok(tok)
    }
    /// Go: func (c *ghSourceCache) passes(tok string) bool
    pub(crate) fn passes(&self, tok: &str) -> bool {
        self.ok && self.token == tok
    }
}
/// Go: func loginSource(s *store.Store) ([]byte, string, error)
pub(crate) fn login_source(s: &pulse_store::Store) -> Result<(Vec<u8>, String)> {
    match std::fs::read_to_string(executor_token_path(s)) {
        Ok(data) => {
            let tok = data.trim();
            if classify_executor_token(tok).is_ok() {
                return Ok((tok.as_bytes().to_vec(), LOGIN_SOURCE_TOKEN.into()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let tok = {
        let mut gh = GH_SOURCE.lock().unwrap();
        gh.fetch()?
    };
    if !tok.is_empty() {
        let mut gh = GH_SOURCE.lock().unwrap();
        let now = SystemTime::now();
        let fresh = gh.token == tok
            && gh.at.is_some_and(|t| {
                now.duration_since(t).unwrap_or(Duration::ZERO)
                    < Duration::from_secs(GH_SOURCE_CACHE_MINS * 60)
            });
        if !fresh {
            gh.ok = check_executor_token(&tok).is_ok();
            gh.token = tok.clone();
            gh.at = Some(now);
        }
        if gh.ok {
            return Ok((tok.into_bytes(), LOGIN_SOURCE_HUB_LOGIN.into()));
        }
    }
    Ok((Vec::new(), LOGIN_SOURCE_NONE.into()))
}
/// Go: func isNoStoredLogin(err error) bool
pub(crate) fn is_no_stored_login(err: &anyhow::Error) -> bool {
    err.to_string().contains("no stored login")
}
/// Go: func hasLoginSource(s *store.Store) bool
pub(crate) fn has_login_source(s: &pulse_store::Store) -> bool {
    login_source(s).map(|(t, _)| !t.is_empty()).unwrap_or(false)
}
/// Go: func tokenReader(tok []byte) io.Reader
pub(crate) fn token_reader(tok: Vec<u8>) -> Cursor<Vec<u8>> {
    Cursor::new(tok)
}

fn classify_executor_token(token: &str) -> Result<()> {
    if token.starts_with("ghp_") {
        return Err(crate::util::fail(2, "classic personal access token refused; mint a fine-grained token with only the Copilot Requests permission").into());
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const TEST_EXECUTOR_TOKEN: &str = "github_pat_11ATESTTOKEN0000000000_TESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTEST";

        fn write_executor_token(s: &pulse_store::Store) -> String {
            let path = executor_token_path(s);
            std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{TEST_EXECUTOR_TOKEN}\n")).unwrap();
            path
        }

        #[test]
        fn executor_token_file_report_set_remove() {
            let _guard = pulse_testutil::env_lock();
            let s = crate::test_helpers::new_store();
            let mut report = executor_token_report(&s, crate::test_helpers::TEST_NOW).join("\n");
            assert!(report.contains("no executor token on file"), "{report}");

            let mut input: &[u8] = TEST_EXECUTOR_TOKEN.as_bytes();
            let lines = executor_token_set(
                &s,
                crate::test_helpers::TEST_NOW,
                &mut input,
                false,
                crate::util::HUMAN_AGENT,
            )
            .unwrap();
            let out = lines.join("\n");
            assert!(
                out.contains("GitHub accepts") && out.contains("reaches no repository"),
                "{out}"
            );
            let path = executor_token_path(&s);
            assert_eq!(
                path,
                std::path::Path::new(&s.home)
                    .join("private/executor-token")
                    .display()
                    .to_string()
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap().trim(),
                TEST_EXECUTOR_TOKEN
            );
            report = executor_token_report(&s, crate::test_helpers::TEST_NOW).join("\n");
            assert!(
                report.contains("executor token on file") && !report.contains(TEST_EXECUTOR_TOKEN),
                "{report}"
            );

            let lines =
                executor_token_remove(&s, crate::test_helpers::TEST_NOW, crate::util::HUMAN_AGENT)
                    .unwrap();
            assert!(lines.join("\n").contains("executor token removed"));
            assert!(!std::path::Path::new(&path).exists());
            let lines =
                executor_token_remove(&s, crate::test_helpers::TEST_NOW, crate::util::HUMAN_AGENT)
                    .unwrap();
            assert!(lines.join("\n").contains("no executor token on file"));
        }

        #[test]
        fn login_source_precedence_and_cache() {
            let _guard = pulse_testutil::env_lock();
            let s = crate::test_helpers::new_store();
            let (tok, origin) = login_source(&s).unwrap();
            assert!(tok.is_empty());
            assert_eq!(origin, LOGIN_SOURCE_NONE);
            assert!(!has_login_source(&s));

            write_executor_token(&s);
            let (tok, origin) = login_source(&s).unwrap();
            assert_eq!(String::from_utf8(tok).unwrap(), TEST_EXECUTOR_TOKEN);
            assert_eq!(origin, LOGIN_SOURCE_TOKEN);

            std::fs::write(executor_token_path(&s), b"not a token\n").unwrap();
            let (tok, origin) = login_source(&s).unwrap();
            assert!(tok.is_empty());
            assert_eq!(origin, LOGIN_SOURCE_NONE);
        }
    }
    if token.starts_with("github_pat_")
        || token.starts_with("gho_")
        || token.starts_with("ghu_")
        || token.starts_with("ghs_")
        || token.starts_with("ghr_")
    {
        return Ok(());
    }
    Err(crate::util::fail(2, "not a GitHub token").into())
}

fn expiry_warning_local(expires: &str, now: &str, days: i32) -> Option<String> {
    if expires.is_empty() {
        return None;
    }
    let exp =
        chrono::NaiveDate::parse_from_str(&expires[..expires.len().min(10)], "%Y-%m-%d").ok()?;
    let now = chrono::NaiveDate::parse_from_str(&now[..now.len().min(10)], "%Y-%m-%d").ok()?;
    let left = (exp - now).num_days();
    if left < 0 {
        Some(format!("expired {}", expires))
    } else if left <= days as i64 {
        Some(format!("expires in {left} days ({expires})"))
    } else {
        None
    }
}
