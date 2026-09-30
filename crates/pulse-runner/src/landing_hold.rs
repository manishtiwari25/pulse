#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]
#![allow(
    clippy::type_complexity,
    clippy::unused_unit,
    reason = "Go-port signatures intentionally mirror internal/runner"
)]

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
    tend_landings_pass(ctx, s, projects, actor, now, prev, true)
}

/// The landing pass behind `tend_landings_context`; the daemon runs it with
/// `experiments` false and measures experiments on its own lane.
pub(crate) fn tend_landings_pass(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
    experiments: bool,
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
        for lr in crate::land::land_pending_pass(
            ctx,
            s,
            proj.clone(),
            actor.clone(),
            now.clone(),
            true,
            0,
            false,
            experiments,
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
    let Ok(data) = pulse_store::gojson::to_string(&out) else {
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
            let why = pulse_cred::redact(&e.to_string(), &[]).replace(
                ": No such file or directory (os error 2)",
                ": no such file or directory",
            );
            return (Vec::new(), Box::new(|_, _| {}), why);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;
    use std::collections::BTreeMap;

    #[test]
    fn landing_pushes_main_when_asked() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let before = bare_ref(&bare, "refs/heads/main");

        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(cur.is_empty(), "hold notes with push off: {cur:?}");
        assert_eq!(bare_ref(&bare, "refs/heads/main"), before);
        assert!(push_owed(&s, "proj".into()));

        set_setting(&s, "landing.push", "main");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert!(
            lines[0].starts_with("landing pushed proj: main -> origin/main (1 commit, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes after a good push: {cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed' AND subject = 'proj'",
                &[]
            ),
            1
        );
        assert!(!push_owed(&s, "proj".into()));

        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert!(
            lines.is_empty() && cur.is_empty(),
            "quiet tick said {lines:?} {cur:?}"
        );

        push_branch(&s, &proj, &repo, "pulse/worker/2");
        let (lines, _) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1].starts_with("landing pushed proj:"),
            "lines {lines:?}"
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
    }

    #[test]
    fn landing_push_failure_is_a_note_not_an_undo() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        let other_dir = tempfile::tempdir().expect("other");
        let other = other_dir.path().join("other");
        run(
            ".",
            &["git", "clone", "-q", &bare, &other.display().to_string()],
        );
        std::fs::write(other.join("other.txt"), "other\n").expect("other");
        run(other.display().to_string(), &["git", "add", "-A"]);
        run(
            other.display().to_string(),
            &["git", "commit", "-q", "-m", "someone else"],
        );
        run(
            other.display().to_string(),
            &["git", "push", "-q", "origin", "main"],
        );
        let remote_head = bare_ref(&bare, "refs/heads/main");

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1]
                .starts_with("landing held proj: 1 commit landed but not pushed; push failed: "),
            "lines {lines:?}"
        );
        assert!(
            cur["proj"].contains("push failed"),
            "note {:?}",
            cur.get("proj")
        );
        assert_eq!(bare_ref(&bare, "refs/heads/main"), remote_head);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND json_extract(detail, '$.state') = 'push-failed'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'",
                &[]
            ),
            1
        );

        let (lines, cur2) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            cur.clone(),
        );
        assert!(
            lines.is_empty() && cur2["proj"] == cur["proj"],
            "repeat tick said {lines:?}, note {:?} -> {:?}",
            cur.get("proj"),
            cur2.get("proj")
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held'",
                &[]
            ),
            1
        );

        run(
            &repo,
            &[
                "git",
                "pull",
                "-q",
                "--no-rebase",
                "--no-edit",
                "origin",
                "main",
            ],
        );
        let (lines, cur3) = tend_landings(&s, vec![proj], "daemon".into(), TEST_NOW.into(), cur2);
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(
            lines[0].starts_with("landing pushed proj: main -> origin/main ("),
            "lines {lines:?}"
        );
        assert_eq!(lines[1], "landing resumed proj");
        assert!(cur3.is_empty(), "notes after recovery: {cur3:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        assert!(!push_owed(&s, "proj".into()));
    }

    #[test]
    fn landing_push_skips_without_upstream() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo) = new_bare_repo(&s);
        set_setting(&s, "landing.push", "main");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert_eq!(
            lines[1],
            "landing held proj: landed but not pushed; push skipped: main has no upstream (git branch --set-upstream-to=origin/main main)"
        );
        assert!(
            cur["proj"].contains("no upstream"),
            "note {:?}",
            cur.get("proj")
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn landing_auto_stash_lands() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        let branch = "pulse/aurora/auto";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);

        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "auto stash".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        std::fs::write(
            std::path::Path::new(&dir).join("README.md"),
            b"# proj\n\nowner's work\n",
        )
        .unwrap();

        let projects = vec![proj];
        let (lines, held) = tend_landings(
            &s,
            projects,
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let joined = lines.join("\n");
        if !held.is_empty() || !joined.contains(&format!("landing landed: {branch}")) {
            panic!("expected landing to succeed; lines={lines:?} held={held:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed' AND subject = ?",
            &[&branch],
        );
        if n != 1 {
            panic!("land.landed events: {n}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM jobs WHERE id = ? AND status = 'done' AND branch = ''",
            &[&j.id],
        );
        if n != 1 {
            panic!("job not finished: {:?}", job_row(&s, &j.id));
        }

        let stash_list = run(&dir, &["git", "stash", "list"]);
        if !stash_list.contains(&format!("pulse auto-stash before landing {}", j.id)) {
            panic!("stash list missing expected message: {stash_list:?}");
        }
        let status = run(
            &dir,
            &["git", "status", "--porcelain", "--untracked-files=no"],
        );
        if status.trim() != "" {
            panic!("checkout not clean after landing: {status:?}");
        }
    }

    #[test]
    fn landing_moves_default_by_ref_when_checkout_is_parked_elsewhere() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        let branch = "pulse/aurora/by-ref";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        // The owner parks the checkout on their own branch, with edits in progress.
        run(&dir, &["git", "checkout", "-q", "-b", "owner-work", "main"]);
        std::fs::write(
            std::path::Path::new(&dir).join("README.md"),
            b"# proj\n\nowner's work\n",
        )
        .unwrap();
        let main_before = run(&dir, &["git", "rev-parse", "main"]).trim().to_string();

        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "land by ref".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        // With main checked out in a second worktree the landing waits: moving the
        // branch under that tree would show the landed change as local edits there.
        let parked = std::path::Path::new(&dir)
            .parent()
            .unwrap()
            .join("main-worktree")
            .display()
            .to_string();
        run(&dir, &["git", "worktree", "add", "-q", &parked, "main"]);
        let (lines, _) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let want = format!(
            "landing held proj: 1 pending landing job; deferred: main is checked out at {parked} while the checkout of {dir} is not on main"
        );
        if lines.len() != 1 || lines[0] != want {
            panic!("expected deferral; lines={lines:?}");
        }
        run(&dir, &["git", "worktree", "remove", "--force", &parked]);

        let (lines, held) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let joined = lines.join("\n");
        if !held.is_empty() || !joined.contains(&format!("landing landed: {branch}")) {
            panic!("expected landing to succeed; lines={lines:?} held={held:?}");
        }
        let main_after = run(&dir, &["git", "rev-parse", "main"]).trim().to_string();
        if main_after == main_before {
            panic!("main did not move");
        }
        let landed_file = run(&dir, &["git", "show", "main:feature.txt"]);
        if landed_file.trim() != "feature" {
            panic!("main lacks the landed file: {landed_file:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM jobs WHERE id = ? AND status = 'done' AND branch = ''",
            &[&j.id],
        );
        if n != 1 {
            panic!("job not finished: {:?}", job_row(&s, &j.id));
        }
        let detail = must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'land.landed' AND subject = ?",
            &[&branch],
        )
        .str("detail");
        if !detail.contains(&format!("\"before\": \"{main_before}\"")) {
            panic!("landing evidence lacks the ref transition: {detail}");
        }
        // The owner's tree is exactly as they left it.
        let head = run(&dir, &["git", "rev-parse", "--abbrev-ref", "HEAD"]);
        if head.trim() != "owner-work" {
            panic!("checkout moved: {head:?}");
        }
        let readme = std::fs::read_to_string(std::path::Path::new(&dir).join("README.md")).unwrap();
        if !readme.contains("owner's work") {
            panic!("owner's edits lost: {readme:?}");
        }
        let stash_list = run(&dir, &["git", "stash", "list"]);
        if stash_list.trim() != "" {
            panic!("unexpected stash created: {stash_list:?}");
        }
        let body = must_one(
            &s,
            "SELECT body FROM posts WHERE kind = 'result' AND project = 'proj' ORDER BY date DESC LIMIT 1",
            &[],
        )
        .str("body");
        if !body.contains("by ref (the checkout was on another branch and was not touched)") {
            panic!("result post does not say how it landed: {body}");
        }
    }

    fn done_job_on_branch(s: &pulse_store::Store, dir: &str, branch: &str, file: &str) -> String {
        run(dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(dir).join(file), b"x\n").unwrap();
        run(dir, &["git", "add", "-A"]);
        run(dir, &["git", "commit", "-q", "-m", "work"]);
        run(dir, &["git", "checkout", "-q", "main"]);
        let (j, _) = pulse_board::add_job(
            s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: format!("work on {branch}"),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );
        j.id
    }

    #[test]
    fn landing_holds_when_the_default_branch_fails_verify_itself() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        // The verify command needs a tool that is not installed on this machine.
        let tool = std::path::Path::new(&dir)
            .parent()
            .unwrap()
            .join("tool-installed")
            .display()
            .to_string();
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&format!("cat {tool}")],
        );
        let proj = job_row_project(&s);
        let branch = "pulse/aurora/env-rot";
        let job_id = done_job_on_branch(&s, &dir, branch, "feature.txt");

        let (lines, held) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let note = held.get("proj").cloned().unwrap_or_default();
        if lines.len() != 1
            || !note.contains("main itself fails")
            || !note.contains("No such file or directory")
            || !note.contains("waits until 2026-09-06T10:30:00Z")
        {
            panic!("expected a base-red hold; lines={lines:?} held={held:?}");
        }
        let r = job_row(&s, &job_id);
        if r.str("status") != "done" || r.str("branch") != branch {
            panic!("held job lost its work: {r:?}");
        }
        if run(&dir, &["git", "branch", "--list", branch])
            .trim()
            .is_empty()
        {
            panic!("branch removed");
        }
        for (action, want) in [
            ("land.base-red", 1),
            ("land.failed-verify", 0),
            ("job.requeue", 0),
        ] {
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&action],
            );
            if n != want {
                panic!("{action}: {n} events, want {want}");
            }
        }

        // Within the wait the daemon does not merge or verify again; the note stands.
        let again = crate::land::land_pending(&s, proj.clone(), "daemon", TEST_NOW, true, 0, false);
        if again.len() != 1 || again[0].state != "deferred" || !note.ends_with(&again[0].note) {
            panic!("expected the standing hold: {again:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.base-red'",
            &[],
        );
        if n != 1 {
            panic!("the hold re-ran verify: {n} base-red events");
        }

        // The tool is installed; after the wait the branch lands untouched.
        std::fs::write(&tool, b"ok\n").unwrap();
        let later = crate::util::plus_minutes(TEST_NOW, 31);
        let (lines, held) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            later,
            BTreeMap::new(),
        );
        if !held.is_empty()
            || !lines
                .join("\n")
                .contains(&format!("landing landed: {branch}"))
        {
            panic!("expected landing after the wait; lines={lines:?} held={held:?}");
        }
        if crate::land::landing_retry_hold(&s, "proj", TEST_NOW).is_some() {
            panic!("landing kept the retry hold");
        }

        // A branch that breaks a green main is still requeued.
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = 'test ! -f broken.txt' WHERE slug = 'proj'",
            &[],
        );
        let proj = job_row_project(&s);
        let bad = "pulse/aurora/breaks-main";
        let bad_id = done_job_on_branch(&s, &dir, bad, "broken.txt");
        let (lines, held) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        if !held.is_empty() || !lines.join("\n").contains("failed-verify") {
            panic!("expected a requeue; lines={lines:?} held={held:?}");
        }
        let r = job_row(&s, &bad_id);
        if r.str("status") != "queued" || !r.str("branch").is_empty() {
            panic!("breaking branch not requeued: {r:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.base-red'",
            &[],
        );
        if n != 1 {
            panic!("a green base was called red: {n} base-red events");
        }
    }

    fn job_row_project(s: &pulse_store::Store) -> crate::util::Row {
        crate::board::project_by_slug(&s.conn().expect("conn"), "proj".into())
    }

    #[test]
    fn landing_runs_the_projects_setup_in_the_worktree_before_verify() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        // main's recipe installs the "dependency" the check needs; the worktree starts
        // without it, exactly like a fresh clone starts without node_modules or .venv.
        let recipe_dir = std::path::Path::new(&dir).join("docs/context");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(
            recipe_dir.join("environment.md"),
            b"---\nrecipe: 1\nsetup:\n  - printf installed > .deps\n---\n",
        )
        .unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "recipe"]);
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = 'test -f .deps && test -f feature.txt' WHERE slug = 'proj'",
            &[],
        );
        let proj = job_row_project(&s);
        let branch = "pulse/aurora/needs-deps";
        let job_id = done_job_on_branch(&s, &dir, branch, "feature.txt");

        let (lines, held) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        if !held.is_empty()
            || !lines
                .join("\n")
                .contains(&format!("landing landed: {branch}"))
        {
            panic!("expected the landing after setup; lines={lines:?} held={held:?}");
        }
        if job_row(&s, &job_id).str("status") != "done" {
            panic!("landed job changed status");
        }
        // `run` panics when git fails: main must now carry the branch's file.
        run(&dir, &["git", "cat-file", "-e", "main:feature.txt"]);
        if std::path::Path::new(&dir).join(".deps").exists() {
            panic!("setup ran in the checkout instead of the worktree");
        }

        // A setup that fails on this machine defers the landing: nothing is charged to the
        // branch, the job keeps its work, and the event names the recipe and the failure.
        std::fs::write(
            recipe_dir.join("environment.md"),
            b"---\nrecipe: 1\nsetup:\n  - echo no such tool here >&2; exit 3\n---\n",
        )
        .unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "recipe breaks"]);
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = 'test -f other.txt' WHERE slug = 'proj'",
            &[],
        );
        let proj = job_row_project(&s);
        let other = "pulse/aurora/other";
        let other_id = done_job_on_branch(&s, &dir, other, "other.txt");
        let (lines, held) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let note = held.get("proj").cloned().unwrap_or_default();
        if lines.len() != 1
            || !note
                .contains("deferred: setup from docs/context/environment.md failed on this machine")
            || !note.contains("no such tool here")
        {
            panic!("expected a setup deferral; lines={lines:?} held={held:?}");
        }
        let r = job_row(&s, &other_id);
        if r.str("status") != "done" || r.str("branch") != other {
            panic!("deferred job lost its work: {r:?}");
        }
        if run(&dir, &["git", "branch", "--list", other])
            .trim()
            .is_empty()
        {
            panic!("branch removed");
        }
        let ev = must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'land.deferred' AND subject = ?",
            &[&other],
        );
        let d: serde_json::Value = serde_json::from_str(&ev.str("detail")).unwrap();
        if d["setup"] != "docs/context/environment.md" || d["rc"] != 3 {
            panic!("land.deferred detail: {d}");
        }
        for action in ["land.failed-verify", "land.base-red", "job.requeue"] {
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&action],
            );
            if n != 0 {
                panic!("{action}: {n} events, want 0");
            }
        }
    }

    #[test]
    fn landing_auto_stash_off_defers() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        set_setting(&s, "landing.auto_stash", "off");

        let branch = "pulse/aurora/manual";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);

        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "manual stash".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );
        std::fs::write(
            std::path::Path::new(&dir).join("README.md"),
            b"# proj\n\nowner's work\n",
        )
        .unwrap();

        let projects = vec![proj];
        let (lines, held) = tend_landings(
            &s,
            projects,
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let want = format!(
            "landing held proj: 1 pending landing job; deferred: the checkout of {dir} has uncommitted changes"
        );
        if lines.len() != 1 || lines[0] != want {
            panic!("expected deferral; lines={lines:?} held={held:?}");
        }
        let r = job_row(&s, &j.id);
        if r.str("branch") != branch {
            panic!("deferred job lost its branch: {r:?}");
        }
        let stash_list = run(&dir, &["git", "stash", "list"]);
        if stash_list.trim() != "" {
            panic!("unexpected stash created: {stash_list:?}");
        }
    }

    #[test]
    fn landing_auto_stash_failed_blocked() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        let branch = "pulse/aurora/conflict";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);

        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "stash fails".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        run(&dir, &["git", "checkout", "-q", "-b", "side", "main"]);
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), b"side\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "side commit"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), b"main\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "main commit"]);
        let mut merge = std::process::Command::new(pulse_gitenv::git_program());
        merge.args(["-C", &dir, "merge", "side"]);
        merge.env_clear();
        for kv in crate::proc::environ() {
            if let Some((key, value)) = kv.split_once('=') {
                merge.env(key, value);
            }
        }
        let _ = merge.output();

        let projects = vec![proj];
        let (lines, held) = tend_landings(
            &s,
            projects,
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        if lines.len() != 1
            || !lines[0].contains("blocked")
            || !lines[0].contains("could not be auto-stashed")
        {
            panic!("expected blocked with auto-stash reason; lines={lines:?} held={held:?}");
        }
        if !held.get("proj").is_some_and(|h| h.contains("blocked")) {
            panic!("expected hold to say blocked: {held:?}");
        }
        let r = job_row(&s, &j.id);
        if r.str("branch") != branch {
            panic!("blocked job lost its branch: {r:?}");
        }
    }

    #[test]
    fn landing_push_uses_the_credential_alone() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (record, _env) = fake_ssh();
        let (proj, repo, bare) = push_fixture(&s);
        run(
            &repo,
            &[
                "git",
                "remote",
                "set-url",
                "--push",
                "origin",
                &format!("ssh://localhost{bare}"),
            ],
        );
        let _env_1 = pulse_testutil::setenv("GIT_SSH_COMMAND", "false");
        set_setting(&s, "landing.push", "main");
        set_setting(
            &s,
            "push.credential",
            &format!("file://{}", key_file(0o600)),
        );
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1].starts_with("landing pushed proj: main -> origin/main (1 commit, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes after a good push: {cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        let text = std::fs::read_to_string(&record).expect("fake ssh was not used");
        for want in [
            "-oBatchMode=yes\n",
            "-oIdentitiesOnly=yes\n",
            "-oIdentityAgent=none\n",
            "-i\n",
        ] {
            assert!(
                text.contains(want),
                "ssh args lack {:?}:\n{}",
                want.trim(),
                text
            );
        }
        let fields = text.trim().lines().collect::<Vec<_>>();
        let key_path = fields
            .windows(2)
            .find_map(|w| (w[0] == "-i").then_some(w[1]))
            .unwrap_or_default();
        assert!(
            key_path.starts_with(
                &std::path::Path::new(&s.home)
                    .join("tmp")
                    .join("push-")
                    .display()
                    .to_string()
            ),
            "key written at {key_path:?}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{record}.mode"))
                .unwrap()
                .trim(),
            "600"
        );
        assert!(
            std::fs::read_to_string(format!("{record}.head"))
                .unwrap()
                .starts_with("-----BEGIN OPENSSH PRIVATE KEY")
        );
        assert!(
            std::fs::metadata(key_path).is_err(),
            "key still on disk after the push"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed' AND subject = 'proj'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn push_credential_env_defaults_to_gits_own() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (_, repo) = new_bare_repo(&s);
        let _env = pulse_testutil::setenv("GIT_SSH_COMMAND", "");
        let (env, cleanup, why) = super::push_credential_env(&s, repo.clone());
        cleanup("", &[]);
        assert_eq!(why, "");
        assert_eq!(env, vec!["GIT_SSH_COMMAND=ssh -oBatchMode=yes"]);
        let _env = pulse_testutil::setenv("GIT_SSH_COMMAND", "ssh -i /owner/key");
        let (env, cleanup, why) = super::push_credential_env(&s, repo.clone());
        cleanup("", &[]);
        assert_eq!(why, "");
        assert!(env.is_empty(), "owner ssh command was replaced: {env:?}");
        set_setting(
            &s,
            "push.credential",
            &format!("file://{}", key_file(0o600)),
        );
        let _env_job = pulse_testutil::setenv("PULSE_JOB", "job-1");
        let (env, cleanup, why) = super::push_credential_env(&s, repo.clone());
        cleanup("", &[]);
        assert!(env.is_empty(), "worker resolved credential: {env:?}");
        assert!(why.contains("never resolved inside a run"), "why {why:?}");
    }

    #[test]
    fn landing_push_credential_failure_holds_without_fallback() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        let before = bare_ref(&bare, "refs/heads/main");
        set_setting(&s, "landing.push", "main");
        set_setting(&s, "push.credential", "file:///nonexistent/pulse/push_key");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert_eq!(
            lines[1],
            "landing held proj: 1 commit landed but not pushed; push credential: /nonexistent/pulse/push_key: no such file or directory"
        );
        assert!(
            cur["proj"].contains("push credential:"),
            "note {:?}",
            cur.get("proj")
        );
        assert_eq!(bare_ref(&bare, "refs/heads/main"), before);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND json_extract(detail, '$.state') = 'push-failed'",
                &[]
            ),
            1
        );

        set_setting(
            &s,
            "push.credential",
            &format!("file://{}", key_file(0o644)),
        );
        let (lines, cur2) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            cur,
        );
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert!(
            lines[0].contains("push credential: ") && lines[0].contains("readable by others"),
            "lines {lines:?}"
        );
        assert_eq!(bare_ref(&bare, "refs/heads/main"), before);

        set_setting(&s, "push.credential", "");
        let (lines, cur3) = tend_landings(&s, vec![proj], "daemon".into(), TEST_NOW.into(), cur2);
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(
            lines[0].starts_with("landing pushed proj: main -> origin/main (1 commit, "),
            "lines {lines:?}"
        );
        assert_eq!(lines[1], "landing resumed proj");
        assert!(cur3.is_empty(), "notes after recovery: {cur3:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
    }

    #[test]
    fn landing_hold_said_once_per_change() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        set_setting(&s, "landing.auto_stash", "off");
        for name in ["one", "two"] {
            run(
                &dir,
                &[
                    "git",
                    "checkout",
                    "-q",
                    "-b",
                    &format!("pulse/aurora/{name}"),
                    "main",
                ],
            );
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{name}.txt")),
                format!("{name}\n"),
            )
            .unwrap();
            run(&dir, &["git", "add", "-A"]);
            run(&dir, &["git", "commit", "-q", "-m", &format!("add {name}")]);
            run(&dir, &["git", "checkout", "-q", "main"]);
            let (j, _) = pulse_board::add_job(
                &s,
                pulse_board::JobInput {
                    project: "proj".into(),
                    kind: "task".into(),
                    title: format!("t {name}"),
                    brief: "b".into(),
                    tier: 1,
                    created_by: "aurora".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
                &[&TEST_NOW, &format!("pulse/aurora/{name}"), &j.id],
            );
        }
        std::fs::write(
            std::path::Path::new(&dir).join("README.md"),
            "# proj\n\nowner's work in progress\n",
        )
        .unwrap();
        let (lines, held) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let want = format!(
            "landing held proj: 2 pending landing jobs; deferred: the checkout of {dir} has uncommitted changes"
        );
        assert_eq!(lines, vec![want.clone()]);
        assert_eq!(held["proj"], want.trim_start_matches("landing held proj: "));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND subject = 'proj'",
                &[]
            ),
            1
        );
        let (lines, held2) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            held.clone(),
        );
        assert!(
            lines.is_empty() && held2["proj"] == held["proj"],
            "second tick: {lines:?} {held2:?}"
        );
        let (lines, held3) =
            tend_landings(&s, vec![], "daemon".into(), TEST_NOW.into(), held2.clone());
        assert!(
            lines.is_empty() && held3["proj"] == held["proj"],
            "unswept tick: {lines:?} {held3:?}"
        );
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "owner's work"]);
        let (lines, held4) = tend_landings(&s, vec![proj], "daemon".into(), TEST_NOW.into(), held3);
        let joined = lines.join("\n");
        assert!(
            held4.is_empty()
                && lines.len() == 3
                && joined.contains("landing landed: pulse/aurora/one")
                && joined.contains("landing landed: pulse/aurora/two")
                && lines[2] == "landing resumed proj",
            "after commit: {lines:?} {held4:?}"
        );
    }

    #[test]
    fn daemon_logs_landing_hold_once() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        set_setting(&s, "landing.auto_stash", "off");
        set_setting(&s, "jobs_per_day", "20");
        run(
            &dir,
            &["git", "checkout", "-q", "-b", "pulse/aurora/x", "main"],
        );
        std::fs::write(std::path::Path::new(&dir).join("x.txt"), "x\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add x"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "x".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = 'pulse/aurora/x' WHERE id = ?",
            &[&TEST_NOW, &j.id],
        );
        std::fs::write(
            std::path::Path::new(&dir).join("README.md"),
            "# proj\n\nedited\n",
        )
        .unwrap();
        let out = crate::test_helpers::OutBuf::new();
        let result = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ran'".into(),
                max_loops: 1,
                out: Some(out.writer()),
                ..Default::default()
            },
        );
        assert_eq!(result, "done");
        let expected = format!(
            "landing held proj: 1 pending landing job; deferred: the checkout of {dir} has uncommitted changes"
        );
        let observed_log = out.string();
        assert_eq!(
            observed_log.matches(&expected).count(),
            1,
            "daemon log:\n{observed_log}"
        );
        assert!(!observed_log.contains("landing deferred"));
        assert_eq!(job_row(&s, &j.id).str("branch"), "pulse/aurora/x");
    }

    #[test]
    fn landing_hold_status_truth() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for n in [0, 1, 208] {
            let s = new_store();
            let (project, _) = new_repo(&s);
            set_setting(&s, "landing", "agents");
            set_setting(&s, "stop", "1");
            for i in 0..n {
                exec1(
                    &s,
                    "INSERT INTO jobs(id, project, kind, title, status, branch, created_at, finished_at) VALUES(?, 'proj', 'task', 'Pending work', 'done', 'pulse/test/same-ref', ?, ?)",
                    &[&format!("J-{i:03}"), &TEST_NOW, &TEST_NOW],
                );
            }
            let (lines, held) = tend_landings(
                &s,
                vec![project.clone()],
                "daemon".into(),
                TEST_NOW.into(),
                BTreeMap::new(),
            );
            if n == 0 {
                assert!(
                    lines.is_empty() && held.is_empty(),
                    "no jobs must mean no hold: {lines:?} {held:?}"
                );
                continue;
            }
            let noun = if n == 1 { "job" } else { "jobs" };
            let want = format!(
                "{n} pending landing {noun}; stopped: {}",
                crate::run::runner_stopped(&s)
            );
            assert_eq!(lines, vec![format!("landing held proj: {want}")]);
            assert_eq!(held["proj"], want);
            let event = must_one(
                &s,
                "SELECT detail FROM events WHERE action = 'land.held' AND subject = 'proj'",
                &[],
            );
            let detail: serde_json::Value = serde_json::from_str(&event.str("detail")).unwrap();
            assert_eq!(detail["waiting"], serde_json::json!(n));
            assert_eq!(detail["state"], "stopped");
            assert_eq!(detail["note"], want);
            let (lines, again) = tend_landings(
                &s,
                vec![project],
                "daemon".into(),
                TEST_NOW.into(),
                held.clone(),
            );
            assert!(
                lines.is_empty() && again["proj"] == held["proj"],
                "unchanged hold must remain quiet: {lines:?} {again:?}"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held'",
                    &[]
                ),
                1
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE status = 'done' AND branch = 'pulse/test/same-ref'",
                    &[]
                ),
                n
            );
        }
    }

    #[test]
    fn plain_landings_go_before_experiments() {
        let _serial = crate::test_helpers::serial();
        skip_slow();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");

        // The experiment finished first, so it heads the queue.
        let experiment = "evolve/slow";
        run(&dir, &["git", "checkout", "-q", "-b", experiment, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("exp.txt"), b"exp\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "experiment"]);
        let plain = "pulse/aurora/plain";
        run(&dir, &["git", "checkout", "-q", "-b", plain, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES ('exp-first', 'proj', 'experiment', 'Experiment', 'done', ?, ?, ?)",
            &[&experiment, &TEST_NOW, &TEST_NOW],
        );
        let later = crate::util::plus_minutes(TEST_NOW, 5.0);
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES ('plain-second', 'proj', 'task', 'Plain', 'done', ?, ?, ?)",
            &[&plain, &later, &later],
        );

        let plain_landed_first = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let (seen, n, s_measure) = (plain_landed_first.clone(), calls.clone(), s.clone());
        let old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, _project, _repo, _ref, _opts| {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let row = crate::jobs::job_row(&*s_measure.conn().unwrap(), "plain-second".into());
                seen.store(
                    row.str("branch").is_empty(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                anyhow::bail!("fake measurement fails")
            }));
        let ctx = pulse_bus::CancellationToken::new();

        // The daemon's pass leaves experiments to the lane.
        let (lines, held) = tend_landings_pass(
            &ctx,
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
            false,
        );
        let joined = lines.join("\n");
        assert!(
            held.is_empty() && joined.contains(&format!("landing landed: {plain}")),
            "plain branch behind an experiment did not land; lines={lines:?} held={held:?}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the plain pass measured an experiment"
        );
        let job = crate::jobs::job_row(&*s.conn().unwrap(), "exp-first".into());
        assert_eq!(
            job.str("branch"),
            experiment,
            "the pass touched the experiment"
        );

        // The CLI's pass measures experiments after the plain branches.
        exec1(
            &s,
            "UPDATE jobs SET branch = ? WHERE id = 'plain-second'",
            &[&plain],
        );
        run(&dir, &["git", "branch", plain, "main"]);
        let (lines, _) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the full pass did not measure the experiment: {lines:?}"
        );
        assert!(
            plain_landed_first.load(std::sync::atomic::Ordering::SeqCst),
            "the experiment was measured before the plain branch landed: {lines:?}"
        );
        let _ = pulse_evolve::set_measure(old);
    }
}
