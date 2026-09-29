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
/// Go: var ghAuthToken = func() (string, error) - asks the hub's gh for its token; tests stub it.
/// "" with no error means gh has no login.
pub(crate) static GH_AUTH_TOKEN: LazyLock<RwLock<GhAuthTokenFn>> =
    LazyLock::new(|| RwLock::new(gh_auth_token_default));

fn gh_auth_token_default() -> Result<String> {
    match std::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        // exec.ErrNotFound and *exec.ExitError both mean "no login here".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
        Ok(out) if !out.status.success() => Ok(String::new()),
        Ok(out) => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
    }
}
/// Go: func checkExecutorToken(token string) (tokenFacts, error)
pub(crate) fn check_executor_token(token: &str) -> Result<crate::executor::TokenFacts> {
    classify_executor_token(token)?;
    let facts = crate::executor::github_token_facts(token, "");
    match facts.ok {
        None => return Err(crate::util::fail(1, facts.note.clone()).into()),
        Some(false) => return Err(crate::util::fail(1, facts.note.clone()).into()),
        Some(true) => {}
    }
    let (ok, note) = crate::executor::github_no_repos(token, "");
    if !ok {
        return Err(crate::util::fail(1, note).into());
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
        let now = (*crate::run::WALL_CLOCK.read().expect("wall clock"))();
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
    if s.harness_native() {
        let provider_name = s.setting("provider", "");
        if provider_name.is_empty() {
            return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
        }
        let Ok(provider) =
            crate::login_gate::local_provider(s.home.clone(), provider_name, s.provider_settings())
        else {
            return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
        };
        if let Some(exporter) = provider.as_login_exporter() {
            match exporter.export_login() {
                Ok((tok, login)) if classify_executor_token(tok.trim()).is_ok() => {
                    let origin = if login.source.is_empty() {
                        LOGIN_SOURCE_PROVIDER.to_string()
                    } else {
                        login.source
                    };
                    return Ok((tok.trim().as_bytes().to_vec(), origin));
                }
                Ok(_) => return Ok((Vec::new(), LOGIN_SOURCE_NONE.into())),
                Err(err) if is_no_stored_login(&err) => {
                    return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
                }
                Err(_) => return Ok((Vec::new(), LOGIN_SOURCE_NONE.into())),
            }
        }
        return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
    }
    let tok = {
        let mut gh = GH_SOURCE.lock().unwrap();
        gh.fetch()?
    };
    if !tok.is_empty() {
        let mut gh = GH_SOURCE.lock().unwrap();
        let now = (*crate::run::WALL_CLOCK.read().expect("wall clock"))();
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
    let text = err.to_string();
    text.contains("no stored login")
        || text.contains("no stored GitHub Copilot login")
        || text.contains("no_stored_login")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration as StdDuration, SystemTime};

    const TEST_EXECUTOR_TOKEN: &str = "github_pat_11ATESTTOKEN0000000000_TESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTESTTEST";
    static TEST_GH_TOKEN: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));
    static TEST_CLOCK: LazyLock<Mutex<SystemTime>> =
        LazyLock::new(|| Mutex::new(SystemTime::UNIX_EPOCH));

    #[derive(Default)]
    struct FakeGitHubState {
        repos: usize,
        status: u16,
        login: String,
        calls: Vec<String>,
        tokens: Vec<String>,
    }

    fn test_gh_auth_token() -> Result<String> {
        Ok(TEST_GH_TOKEN.lock().unwrap().clone())
    }

    fn test_wall_clock() -> SystemTime {
        *TEST_CLOCK.lock().unwrap()
    }

    fn set_test_clock(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) {
        let dt = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, year, month, day, hour, min, sec)
            .unwrap();
        *TEST_CLOCK.lock().unwrap() =
            SystemTime::UNIX_EPOCH + StdDuration::from_secs(dt.timestamp() as u64);
    }

    fn install_fake_github(state: Arc<Mutex<FakeGitHubState>>) -> pulse_testutil::httptest::Server {
        pulse_testutil::httptest::Server::new(move |req| {
            let mut st = state.lock().unwrap();
            st.calls.push(format!("{}?{}", req.path(), req.query()));
            st.tokens.push(
                req.header("Authorization")
                    .unwrap_or("")
                    .trim_start_matches("Bearer ")
                    .to_string(),
            );
            let mut status = 200;
            let body = match req.path() {
                "/user" => {
                    if st.status != 0 {
                        status = st.status;
                    }
                    let login = if st.login.is_empty() {
                        "hands-bot"
                    } else {
                        &st.login
                    };
                    serde_json::json!({"login": login})
                }
                "/user/repos" => {
                    let repos = (0..st.repos)
                        .map(|_| serde_json::json!({"full_name": "acme/repo"}))
                        .collect::<Vec<_>>();
                    serde_json::Value::Array(repos)
                }
                _ => {
                    status = 404;
                    serde_json::Value::String(String::new())
                }
            };
            pulse_testutil::httptest::Response::json(status, &body).header(
                "github-authentication-token-expiration",
                "2027-01-01 00:00:00 UTC",
            )
        })
    }

    fn write_executor_token(s: &pulse_store::Store) -> String {
        let path = executor_token_path(s);
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{TEST_EXECUTOR_TOKEN}\n")).unwrap();
        path
    }

    #[test]
    fn executor_token_command() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let gh = Arc::new(Mutex::new(FakeGitHubState {
            login: "hands-bot".to_string(),
            ..Default::default()
        }));
        let srv = install_fake_github(Arc::clone(&gh));
        let _env_1 = pulse_testutil::setenv("PULSE_GITHUB_API", srv.url());
        let s = crate::test_helpers::new_store();
        let mut report = executor_token_report(&s, crate::test_helpers::TEST_NOW).join("\n");
        assert!(report.contains("no executor token on file"), "{report}");

        let mut classic: &[u8] = b"ghp_classic0000000000000000000000000000\n";
        let err = executor_token_set(
            &s,
            crate::test_helpers::TEST_NOW,
            &mut classic,
            false,
            crate::util::HUMAN_AGENT,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("classic personal access token"), "{err}");
        assert_eq!(gh.lock().unwrap().calls.len(), 0);

        gh.lock().unwrap().repos = 1;
        let mut refused: &[u8] = TEST_EXECUTOR_TOKEN.as_bytes();
        let err = executor_token_set(
            &s,
            crate::test_helpers::TEST_NOW,
            &mut refused,
            false,
            crate::util::HUMAN_AGENT,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains(crate::executor::NO_REPOSITORY_REFUSAL),
            "{err}"
        );
        assert!(!std::path::Path::new(&executor_token_path(&s)).exists());

        gh.lock().unwrap().repos = 0;
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
            out.contains("GitHub accepts the token for hands-bot"),
            "{out}"
        );
        assert!(out.contains("reaches no repository"), "{out}");
        assert!(!out.contains(TEST_EXECUTOR_TOKEN), "{out}");
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
        let joined = gh.lock().unwrap().calls.join(" ");
        assert!(joined.contains("/user?"), "{joined}");
        assert!(
            joined.contains(
                "/user/repos?per_page=1&affiliation=owner,collaborator,organization_member"
            ),
            "{joined}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(std::path::Path::new(&path).parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        report = executor_token_report(&s, crate::test_helpers::TEST_NOW).join("\n");
        assert!(report.contains("executor token on file"), "{report}");
        assert!(report.contains("account hands-bot"), "{report}");
        assert!(report.contains("expires 2027-01-01"), "{report}");
        assert!(!report.contains(TEST_EXECUTOR_TOKEN), "{report}");

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
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = crate::test_helpers::new_store();
        let gh = Arc::new(Mutex::new(FakeGitHubState::default()));
        let srv = install_fake_github(Arc::clone(&gh));
        let _env_2 = pulse_testutil::setenv("PULSE_GITHUB_API", srv.url());
        *TEST_GH_TOKEN.lock().unwrap() = String::new();
        let old_gh = *GH_AUTH_TOKEN.read().unwrap();
        *GH_AUTH_TOKEN.write().unwrap() = test_gh_auth_token;
        let old_clock = *crate::run::WALL_CLOCK.read().expect("wall clock");
        set_test_clock(2026, 9, 6, 10, 0, 0);
        *crate::run::WALL_CLOCK.write().expect("wall clock") = test_wall_clock;
        GH_SOURCE.lock().unwrap().forget();

        let (tok, origin) = login_source(&s).unwrap();
        assert!(tok.is_empty());
        assert_eq!(origin, LOGIN_SOURCE_NONE);
        assert_eq!(gh.lock().unwrap().calls.len(), 0);
        assert!(!has_login_source(&s));

        s.set_setting(crate::util::HUMAN_AGENT, "harness", "copilot-cli")
            .unwrap();
        *TEST_GH_TOKEN.lock().unwrap() = "gho_hubtoken00000000000000000000000000".to_string();
        GH_SOURCE.lock().unwrap().forget();
        let (tok, origin) = login_source(&s).unwrap();
        assert_eq!(
            String::from_utf8(tok).unwrap(),
            "gho_hubtoken00000000000000000000000000"
        );
        assert_eq!(origin, LOGIN_SOURCE_HUB_LOGIN);
        assert_eq!(gh.lock().unwrap().calls.len(), 2);
        let _ = login_source(&s).unwrap();
        let _ = login_source(&s).unwrap();
        assert_eq!(gh.lock().unwrap().calls.len(), 2);
        *TEST_CLOCK.lock().unwrap() += StdDuration::from_secs(11 * 60);
        let _ = login_source(&s).unwrap();
        assert_eq!(gh.lock().unwrap().calls.len(), 4);

        gh.lock().unwrap().repos = 1;
        *TEST_GH_TOKEN.lock().unwrap() = "gho_reaches000000000000000000000000000000".to_string();
        GH_SOURCE.lock().unwrap().forget();
        let (tok, origin) = login_source(&s).unwrap();
        assert!(tok.is_empty());
        assert_eq!(origin, LOGIN_SOURCE_NONE);

        let before = gh.lock().unwrap().calls.len();
        write_executor_token(&s);
        let (tok, origin) = login_source(&s).unwrap();
        assert_eq!(String::from_utf8(tok).unwrap(), TEST_EXECUTOR_TOKEN);
        assert_eq!(origin, LOGIN_SOURCE_TOKEN);
        assert_eq!(gh.lock().unwrap().calls.len(), before);

        std::fs::write(executor_token_path(&s), b"not a token\n").unwrap();
        let (tok, origin) = login_source(&s).unwrap();
        assert!(tok.is_empty());
        assert_eq!(origin, LOGIN_SOURCE_NONE);
        *crate::run::WALL_CLOCK.write().expect("wall clock") = old_clock;
        *GH_AUTH_TOKEN.write().unwrap() = old_gh;
        GH_SOURCE.lock().unwrap().forget();
        let _env_3 = pulse_testutil::unsetenv("PULSE_GITHUB_API");
    }
}
