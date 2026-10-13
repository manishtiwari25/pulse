#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::Result;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::Duration;
use std::time::SystemTime;
pub(crate) const EXECUTOR_TOKEN_FILE: &str = "executor-token";
pub(crate) const SOURCE_CACHE_MINS: u64 = 10;
pub(crate) const LOGIN_SOURCE_TOKEN: &str = "executor token";
pub(crate) const LOGIN_SOURCE_PROVIDER: &str = "provider login";
pub(crate) const LOGIN_SOURCE_NONE: &str = "none";
pub(crate) fn executor_token_path(s: &pulse_store::Store) -> String {
    std::path::Path::new(&s.home)
        .join("private")
        .join(EXECUTOR_TOKEN_FILE)
        .display()
        .to_string()
}
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
    PROVIDER_SOURCE.lock().unwrap().forget();
    let mut lines = vec![
        facts.note.clone(),
        "the token reaches no repository".into(),
        format!("executor token stored in {path}"),
    ];
    let warn =
        crate::executor::expiry_warning(&facts.expires, now, crate::executor::LOGIN_EXPIRY_WARN);
    if !warn.is_empty() {
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
    let proj = finding_project(s);
    if !proj.is_empty() {
        let mut summary = String::from("executor token placed on the hub");
        if !facts.user.is_empty() {
            summary += &format!(" for {}", facts.user);
        }
        let _ = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: proj,
                area: "runner".into(),
                thread: "executors".into(),
                summary,
                body: format!(
                    "The owner placed a fine-grained GitHub token that reaches no repository (account {}{}). The daemon \
                     places it on a drained executor whose login was refused and on every machine compute provisions, so they work \
                     without an owner step (ADR-034). `pulse executor token --remove` withdraws it.\n",
                    or_dash(&facts.user),
                    expires_text(&facts.expires)
                ),
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: now.to_string(),
                ..Default::default()
            },
        );
    }
    Ok(lines)
}
pub(crate) fn executor_token_remove(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
) -> Result<Vec<String>> {
    let path = executor_token_path(s);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            PROVIDER_SOURCE.lock().unwrap().forget();
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
        let login = d.get("login").and_then(|v| v.as_str()).unwrap_or("");
        let expires = d.get("expires").and_then(|v| v.as_str()).unwrap_or("");
        if !login.is_empty() {
            lines.push(format!("account {login}"));
        }
        if !expires.is_empty() {
            lines.push(format!("expires {expires}"));
        }
        let warn =
            crate::executor::expiry_warning(expires, now, crate::executor::LOGIN_EXPIRY_WARN);
        if !warn.is_empty() {
            lines.push(format!("WARNING {warn}"));
        }
    }
    lines
}
pub(crate) fn or_dash(s: &str) -> String {
    if s.is_empty() { "-".into() } else { s.into() }
}
pub(crate) fn expires_text(expires: &str) -> String {
    if expires.is_empty() {
        String::new()
    } else {
        format!(", expires {expires}")
    }
}
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
/// Remembers GitHub's verdict on the last borrowed token for SOURCE_CACHE_MINS.
#[derive(Debug, Default)]
pub(crate) struct SourceCache {
    pub(crate) token: String,
    pub(crate) ok: bool,
    pub(crate) at: Option<SystemTime>,
}
pub(crate) static PROVIDER_SOURCE: LazyLock<Mutex<SourceCache>> =
    LazyLock::new(|| Mutex::new(SourceCache::default()));
impl SourceCache {
    pub(crate) fn forget(&mut self) {
        *self = Self::default()
    }
    /// Says whether tok is an acceptable source, asking GitHub only when the cache is cold or stale.
    pub(crate) fn passes(&mut self, tok: &str) -> bool {
        let now = (*crate::run::WALL_CLOCK.read().expect("wall clock"))();
        if self.token == tok
            && self.at.is_some_and(|t| {
                now.duration_since(t).unwrap_or(Duration::ZERO)
                    < Duration::from_secs(SOURCE_CACHE_MINS * 60)
            })
        {
            return self.ok;
        }
        let err = check_executor_token(tok).err();
        self.token = tok.to_string();
        self.ok = err.is_none();
        self.at = Some(now);
        self.ok
    }
}
/// ADR-034 rule 2: the executor token on file, else the stored provider login when it reaches no
/// repository, else none. The file is trusted after classifyToken alone (it was checked when
/// placed); the borrowed provider login is checked against GitHub and cached per process for
/// SOURCE_CACHE_MINS.
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
    let p = crate::login_gate::local_provider(
        s.home.clone(),
        s.setting("provider", pulse_provider::DEFAULT),
        s.provider_settings(),
    )?;
    let Some(exporter) = p.as_login_exporter() else {
        return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
    };
    let tok = match exporter.export_login() {
        Ok((tok, _)) => tok,
        Err(err) => {
            if is_no_stored_login(&err) {
                return Ok((Vec::new(), LOGIN_SOURCE_NONE.into()));
            }
            return Err(err);
        }
    };
    let tok = tok.trim();
    if !tok.is_empty() && PROVIDER_SOURCE.lock().unwrap().passes(tok) {
        return Ok((tok.as_bytes().to_vec(), LOGIN_SOURCE_PROVIDER.into()));
    }
    Ok((Vec::new(), LOGIN_SOURCE_NONE.into()))
}
pub(crate) fn is_no_stored_login(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|e| e.downcast_ref::<pulse_provider::ProviderError>())
        .any(|perr| perr.code == "no_stored_login")
}
pub(crate) fn has_login_source(s: &pulse_store::Store) -> bool {
    login_source(s).map(|(t, _)| !t.is_empty()).unwrap_or(false)
}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_bare_repo, new_store, serial};
    use crate::test_helpers_daemon::{must_one, must_rows, set_setting};
    use crate::test_helpers_hands::{TEST_EXECUTOR_TOKEN, write_executor_token};
    use crate::test_helpers_provider::{invoke, invoke_ok, with_stdin};
    use crate::test_helpers_token::{FakeGitHub, FakeGitHubState, fixed_wall_clock, time_date};
    use crate::util::json_map;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    #[test]
    fn executor_token_command() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        let gh = FakeGitHub::new(FakeGitHubState {
            login: "hands-bot".into(),
            ..Default::default()
        });
        let _gh = gh.install();

        // Report before anything is placed: no file, no token anywhere.
        let mut out = invoke_ok(&s, "executor", "", &["token"]);
        assert!(out.contains("no executor token on file"), "report: {out:?}");

        // A classic token is refused before GitHub is asked.
        with_stdin("ghp_classic0000000000000000000000000000\n", || {
            let (_, err_text, code) = invoke(&s, "executor", "", &["token", "--with-token"]);
            assert!(
                code == 2 && err_text.contains("classic personal access token"),
                "ghp_: {code} {err_text:?}"
            );
        });
        assert!(
            gh.count() == 0,
            "GitHub asked about a classic token: {:?}",
            gh.lock().calls
        );

        // A token that reaches one repository is refused with the fixed sentence; nothing is written.
        gh.lock().repos = 1;
        with_stdin(&format!("{TEST_EXECUTOR_TOKEN}\n"), || {
            let (_, err_text, code) = invoke(&s, "executor", "", &["token", "--with-token"]);
            assert!(
                code == 1 && err_text.contains(crate::executor::NO_REPOSITORY_REFUSAL),
                "one repo: {code} {err_text:?}"
            );
        });
        assert!(
            !Path::new(&executor_token_path(&s)).exists(),
            "a refused token was written"
        );

        // Zero repositories: accepted, written 0600 under private/ 0700, finding and event carry the login only.
        gh.lock().repos = 0;
        with_stdin(&format!("{TEST_EXECUTOR_TOKEN}\n"), || {
            out = invoke_ok(&s, "executor", "", &["token", "--with-token"]);
        });
        assert!(
            out.contains("GitHub accepts the token for hands-bot")
                && out.contains("reaches no repository")
                && !out.contains(TEST_EXECUTOR_TOKEN),
            "accept: {out:?}"
        );
        let path = executor_token_path(&s);
        assert!(
            path == Path::new(&s.home)
                .join("private")
                .join("executor-token")
                .display()
                .to_string(),
            "token path: {path}"
        );
        let fi = std::fs::metadata(&path);
        assert!(
            matches!(&fi, Ok(fi) if fi.permissions().mode() & 0o777 == 0o600),
            "token file: {fi:?}"
        );
        let di = std::fs::metadata(Path::new(&path).parent().unwrap()).unwrap();
        assert!(
            di.permissions().mode() & 0o777 == 0o700,
            "private dir mode: {:o}",
            di.permissions().mode()
        );
        let data = std::fs::read(&path).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&data).trim() == TEST_EXECUTOR_TOKEN,
            "token file content differs"
        );
        let joined = gh.lock().calls.join(" ");
        assert!(
            joined.contains("/user?")
                && joined.contains(
                    "/user/repos?per_page=1&affiliation=owner,collaborator,organization_member"
                ),
            "GitHub calls: {:?}",
            gh.lock().calls
        );
        let ev = must_one(
            &s,
            "SELECT * FROM events WHERE action = 'executor.token' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        let d = json_map(&ev.str("detail"));
        assert!(
            !crate::util::is_nil(&ev)
                && d.get("login") == Some(&serde_json::json!("hands-bot"))
                && d.get("ok") == Some(&serde_json::json!(true)),
            "event: {ev:?}"
        );
        let posts = must_rows(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'executors'",
            &[],
        );
        assert!(
            posts.len() == 1
                && posts[0].str("summary") == "executor token placed on the hub for hands-bot",
            "finding: {posts:?}"
        );
        for r in posts.iter().chain(std::iter::once(&ev)) {
            for col in ["summary", "body", "detail"] {
                assert!(
                    !r.str(col).contains(TEST_EXECUTOR_TOKEN),
                    "token leaked into {col}"
                );
            }
        }

        // Report: presence, account, expiry - never the token.
        out = invoke_ok(&s, "executor", "", &["token"]);
        assert!(
            out.contains("executor token on file")
                && out.contains("account hands-bot")
                && out.contains("expires 2027-01-01")
                && !out.contains(TEST_EXECUTOR_TOKEN),
            "report: {out:?}"
        );
        let (_, _, code) = invoke(&s, "executor", "", &["token", "--with-token", "--remove"]);
        assert!(code == 2, "both flags accepted: {code}");

        // Remove.
        out = invoke_ok(&s, "executor", "", &["token", "--remove"]);
        assert!(out.contains("executor token removed"), "remove: {out:?}");
        assert!(!Path::new(&path).exists(), "token file still there");
        out = invoke_ok(&s, "executor", "", &["token", "--remove"]);
        assert!(
            out.contains("no executor token on file"),
            "second remove: {out:?}"
        );
    }

    #[test]
    fn login_source_precedence() {
        let _serial = serial();
        let s = new_store();
        let gh = FakeGitHub::default();
        let _gh = gh.install();
        let _clock = fixed_wall_clock(time_date(2026, 9, 6, 10, 0, 0));

        // Nothing anywhere: none, and GitHub is not asked.
        let res = login_source(&s);
        assert!(
            matches!(&res, Ok((tok, origin)) if tok.is_empty() && origin == LOGIN_SOURCE_NONE)
                && gh.count() == 0,
            "empty: {res:?} {}",
            gh.count()
        );
        assert!(!has_login_source(&s), "has a source from nothing");

        // The file wins and is trusted without a network call.
        let before = gh.count();
        write_executor_token(&s);
        let (tok, origin) = login_source(&s).unwrap_or_default();
        assert!(
            tok.as_slice() == TEST_EXECUTOR_TOKEN.as_bytes()
                && origin == LOGIN_SOURCE_TOKEN
                && gh.count() == before,
            "file: {tok:?} {origin} ({} calls)",
            gh.count() - before
        );
        // A damaged file falls back to the provider login (none here): none.
        let _ = std::fs::write(executor_token_path(&s), b"not a token\n");
        let (tok, origin) = login_source(&s).unwrap_or_default();
        assert!(
            tok.is_empty() && origin == LOGIN_SOURCE_NONE,
            "damaged file: {tok:?} {origin}"
        );
    }
}
