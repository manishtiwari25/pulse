use std::collections::BTreeMap;
use std::time::Duration;

/// Go: func landingHeld(state string) bool
pub(crate) fn landing_held(state: String) -> bool {
    matches!(
        state.as_str(),
        "deferred" | "locked" | "stopped" | "unreadable" | "blocked"
    )
}

/// Go: func tendLandings(s *store.Store, projects []row, actor, now string, prev map[string]string) (lines []string, cur map[string]string)
pub(crate) fn tend_landings(
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let ctx = pulse_bus::CancellationToken::new();
    tend_landings_context(&ctx, s, projects, actor, now, prev)
}

/// Go: func tendLandingsContext(ctx context.Context, s *store.Store, projects []row, actor, now string, prev map[string]string) (lines []string, cur map[string]string)
pub(crate) fn tend_landings_context(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut lines = Vec::new();
    let mut cur = BTreeMap::new();
    let mut swept = BTreeMap::new();
    let mut waiting = BTreeMap::new();
    let mut states = BTreeMap::new();
    for proj in projects {
        if ctx.is_cancelled() {
            break;
        }
        let slug = proj.str("slug");
        swept.insert(slug.clone(), true);
        let mut landed = false;
        for lr in crate::land::land_pending_context(
            ctx,
            s,
            proj.clone(),
            actor.clone(),
            now.clone(),
            true,
            0,
            false,
        ) {
            if lr.state == "landed" {
                landed = true;
            }
            if !landing_held(lr.state.clone()) {
                lines.push(format!("landing {}: {}", lr.state, lr.note));
                continue;
            }
            let n = if let Ok(conn) = s.conn() {
                crate::land::pending_landings(&conn, slug.clone()).len() as i32
            } else {
                0
            };
            waiting.insert(slug.clone(), n);
            states.insert(slug.clone(), lr.state.clone());
            cur.insert(
                slug.clone(),
                format!(
                    "{n} pending landing {}; {}: {}",
                    crate::util::plural(n, "job"),
                    lr.state,
                    lr.note
                ),
            );
        }
        let (line, note) =
            if crate::landing_context::landing_stop(ctx, s, proj.str("path")).is_empty() {
                push_landed(s, proj.clone(), actor.clone(), now.clone(), landed)
            } else {
                (String::new(), String::new())
            };
        if !line.is_empty() {
            lines.push(line);
        }
        if !note.is_empty() {
            cur.entry(slug.clone())
                .and_modify(|v| {
                    v.push_str("; ");
                    v.push_str(&note);
                })
                .or_insert(note);
            states.entry(slug.clone()).or_insert("push-failed".into());
        }
    }
    for (slug, note) in &prev {
        if !swept.contains_key(slug) {
            cur.insert(slug.clone(), note.clone());
        }
    }
    if let Ok(conn) = s.conn() {
        for (slug, note) in &cur {
            if !swept.contains_key(slug) || prev.get(slug) == Some(note) {
                continue;
            }
            lines.push(format!("landing held {slug}: {note}"));
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.held",
                slug,
                serde_json::json!({"project": slug, "waiting": waiting.get(slug).copied().unwrap_or_default(), "state": states.get(slug).cloned().unwrap_or_default(), "note": note}),
            );
        }
        for (slug, note) in &prev {
            if cur.contains_key(slug) || !swept.contains_key(slug) {
                continue;
            }
            lines.push(format!("landing resumed {slug}"));
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.resumed",
                slug,
                serde_json::json!({"project": slug, "was": note}),
            );
        }
    }
    record_landing_holds(s, cur.clone(), now);
    (lines, cur)
}

/// Go: func recordLandingHolds(s *store.Store, holds map[string]string, now string)
pub(crate) fn record_landing_holds(
    s: &pulse_store::Store,
    holds: BTreeMap<String, String>,
    now: String,
) -> () {
    let out = holds
        .into_iter()
        .map(|(slug, note)| (slug, serde_json::json!({"at": now, "note": note})))
        .collect::<BTreeMap<_, _>>();
    let Ok(data) = serde_json::to_string(&out) else {
        return;
    };
    if let Ok(conn) = s.conn() {
        crate::util::exec1(
            &conn,
            "INSERT INTO meta (key, value) VALUES ('landing', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            &[&data],
        );
    }
}

/// Go: func pushOn(s *store.Store) bool
pub(crate) fn push_on(s: &pulse_store::Store) -> bool {
    s.setting("landing.push", "off") == "main"
}

/// Go: func pushOwed(s *store.Store, slug string) bool
pub(crate) fn push_owed(s: &pulse_store::Store, slug: String) -> bool {
    let Ok(conn) = s.conn() else { return false };
    let r = crate::util::must_one(&conn, "SELECT COALESCE((SELECT MAX(seq) FROM events WHERE action = 'land.landed' AND json_extract(detail, '$.project') = ?), 0) AS landed,
		COALESCE((SELECT MAX(seq) FROM events WHERE action = 'land.pushed' AND subject = ?), 0) AS pushed", &[&slug, &slug]);
    !r.0.is_empty() && r.int("landed") > r.int("pushed")
}

/// Go: func pushLanded(s *store.Store, project row, actor, now string, landedNow bool) (line, note string)
pub(crate) fn push_landed(
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    landed_now: bool,
) -> (String, String) {
    if !push_on(s) {
        return (String::new(), String::new());
    }
    let slug = project.str("slug");
    let root = project.str("path");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    if !landed_now && !push_owed(s, slug.clone()) {
        return (String::new(), String::new());
    }
    let up = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &[
            "for-each-ref",
            "--format=%(upstream:remotename) %(upstream:remoteref)",
            &format!("refs/heads/{def}"),
        ],
    )
    .stdout
    .split_whitespace()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();
    if up.len() != 2 {
        return (
            String::new(),
            format!(
                "landed but not pushed; push skipped: {def} has no upstream (git branch --set-upstream-to=origin/{def} {def})"
            ),
        );
    }
    let remote = up[0].clone();
    let remote_ref = up[1].clone();
    let remote_branch = remote_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&remote_ref)
        .to_string();
    let upstream = format!("refs/remotes/{remote}/{remote_branch}");
    let n = crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &[
            "rev-list",
            "--count",
            &format!("{upstream}..refs/heads/{def}"),
        ],
    )
    .stdout
    .trim()
    .parse::<i32>()
    .unwrap_or(0);
    if n == 0 {
        return (String::new(), String::new());
    }
    let from = crate::util::git_timeout(&root, Duration::from_secs(10), &["rev-parse", &upstream])
        .stdout
        .trim()
        .to_string();
    let to = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &["rev-parse", &format!("refs/heads/{def}")],
    )
    .stdout
    .trim()
    .to_string();
    let (env, cleanup, why) = push_credential_env(s, root.clone());
    if !why.is_empty() {
        return (
            String::new(),
            format!(
                "{n} {} landed but not pushed; push credential: {why}",
                crate::util::plural(n, "commit")
            ),
        );
    }
    let res = crate::util::git_timeout_env(
        &root,
        Duration::from_secs(180),
        &env,
        &["push", &remote, &format!("refs/heads/{def}:{remote_ref}")],
    );
    cleanup("", &[]);
    if !res.ok() {
        let mut why = crate::run::squeeze(&format!("{} {}", res.stderr, res.stdout), 300);
        if let Some(e) = res.err {
            why = e;
        }
        return (
            String::new(),
            format!(
                "{n} {} landed but not pushed; push failed: {}",
                crate::util::plural(n, "commit"),
                pulse_cred::redact(&why, &[])
            ),
        );
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "land.pushed",
            &slug,
            serde_json::json!({"project": slug, "remote": remote, "branch": def, "from": from, "to": to, "commits": n}),
        );
    }
    (
        format!(
            "landing pushed {slug}: {def} -> {remote}/{remote_branch} ({n} {}, {}..{})",
            crate::util::plural(n, "commit"),
            crate::recipe::short(&from, 12),
            crate::recipe::short(&to, 12)
        ),
        String::new(),
    )
}

/// Go: func pushCredentialEnv(s *store.Store, root string) (env []string, cleanup func(), why string)
pub(crate) fn push_credential_env(
    s: &pulse_store::Store,
    root: String,
) -> (
    Vec<String>,
    Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
    String,
) {
    let ref_ = s.setting("push.credential", "").trim().to_string();
    if ref_.is_empty() {
        return (push_env(root), Box::new(|_, _| {}), String::new());
    }
    let parsed = match pulse_cred::parse(&ref_) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Box::new(|_, _| {}), e.to_string()),
    };
    let resolver = pulse_cred::new(&s.home);
    let secret = match resolver.resolve(parsed) {
        Ok(v) => v,
        Err(e) => {
            return (
                Vec::new(),
                Box::new(|_, _| {}),
                pulse_cred::redact(&e.to_string(), &[]),
            );
        }
    };
    let material = match resolver.materialize(&secret) {
        Ok(v) => v,
        Err(e) => {
            return (
                Vec::new(),
                Box::new(|_, _| {}),
                format!(
                    "could not write the key for the push: {}",
                    pulse_cred::redact(&e.to_string(), &[])
                ),
            );
        }
    };
    let env = vec![format!("GIT_SSH_COMMAND={}", material.ssh_command)];
    (env, Box::new(move |_, _| material.cleanup()), String::new())
}

/// Go: func pushEnv(root string) []string
pub(crate) fn push_env(root: String) -> Vec<String> {
    if std::env::var("GIT_SSH_COMMAND").unwrap_or_default() != ""
        || !crate::util::git_timeout(
            &root,
            Duration::from_secs(10),
            &["config", "--get", "core.sshCommand"],
        )
        .stdout
        .trim()
        .is_empty()
    {
        Vec::new()
    } else {
        vec!["GIT_SSH_COMMAND=ssh -oBatchMode=yes".into()]
    }
}
