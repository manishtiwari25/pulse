#![allow(dead_code, reason = "not every item has a caller in the crate")]
#![allow(
    clippy::type_complexity,
    clippy::unused_unit,
    reason = "signatures intentionally mirror the original runner"
)]

use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectLandingOutcome {
    pub(crate) slug: String,
    pub(crate) lines: Vec<String>,
    pub(crate) hold: Option<String>,
}

pub(crate) fn landing_held(state: String) -> bool {
    matches!(
        state.as_str(),
        "deferred" | "locked" | "stopped" | "unreadable" | "blocked"
    )
}

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

pub(crate) fn tend_landings_context(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    tend_landings_pass(ctx, s, projects, actor, now, prev, true, None)
}

/// The landing pass behind `tend_landings_context`; the daemon runs it with
/// `experiments` false and measures experiments on its own lane, and reads
/// where the pass is through `stage` when it overruns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tend_landings_pass(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
    experiments: bool,
    stage: Option<&crate::daemon_maintenance::Stage>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut lines = Vec::new();
    let mut cur = BTreeMap::new();
    let mut swept = BTreeMap::new();
    for proj in projects {
        if ctx.is_cancelled() {
            break;
        }
        let slug = proj.str("slug");
        let outcome = tend_landing_project(
            ctx,
            s,
            proj,
            actor.clone(),
            now.clone(),
            experiments,
            prev.get(&slug).cloned(),
            stage,
        );
        swept.insert(slug.clone(), true);
        lines.extend(outcome.lines);
        if let Some(note) = outcome.hold {
            cur.insert(slug, note);
        }
    }
    for (slug, note) in &prev {
        if !swept.contains_key(slug) {
            cur.insert(slug.clone(), note.clone());
        }
    }
    record_landing_holds(s, cur.clone(), now);
    (lines, cur)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tend_landing_project(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    proj: crate::util::Row,
    actor: String,
    now: String,
    experiments: bool,
    prev: Option<String>,
    stage: Option<&crate::daemon_maintenance::Stage>,
) -> ProjectLandingOutcome {
    let mut lines = Vec::new();
    let mut hold = None;
    let mut waiting = 0;
    let mut state = String::new();
    let slug = proj.str("slug");
    crate::daemon_maintenance::set_stage(stage, format!("landing {slug}"));
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
        waiting = if let Ok(conn) = s.conn() {
            crate::land::pending_landings(&conn, slug.clone()).len() as i32
        } else {
            0
        };
        state = lr.state.clone();
        hold = Some(format!(
            "{waiting} pending landing {}; {}: {}",
            crate::util::plural(waiting, "job"),
            lr.state,
            lr.note
        ));
    }
    let (push_lines, note) =
        if crate::landing_context::landing_stop(ctx, s, proj.str("path")).is_empty() {
            crate::daemon_maintenance::set_stage(stage, format!("pushing {slug}"));
            push_landed(s, proj.clone(), actor.clone(), now.clone(), landed)
        } else {
            (Vec::new(), String::new())
        };
    lines.extend(push_lines);
    if !note.is_empty() {
        hold.get_or_insert_with(String::new);
        if let Some(hold) = &mut hold {
            if !hold.is_empty() {
                hold.push_str("; ");
            }
            hold.push_str(&note);
        }
        if state.is_empty() {
            state = "push-failed".into();
        }
    }
    if let Ok(conn) = s.conn() {
        match (&prev, &hold) {
            (Some(prev), Some(note)) if prev == note => {}
            (_, Some(note)) => {
                lines.push(format!("landing held {slug}: {note}"));
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "land.held",
                    &slug,
                    serde_json::json!({"project": slug, "waiting": waiting, "state": state, "note": note}),
                );
            }
            (Some(prev), None) => {
                lines.push(format!("landing resumed {slug}"));
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "land.resumed",
                    &slug,
                    serde_json::json!({"project": slug, "was": prev}),
                );
            }
            (None, None) => {}
        }
    }
    ProjectLandingOutcome { slug, lines, hold }
}

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

/// The hold notes `record_landing_holds` last wrote, keyed by project slug,
/// so a daemon restart does not repeat an unchanged `land.held` event.
pub(crate) fn load_landing_holds(s: &pulse_store::Store) -> BTreeMap<String, String> {
    let Ok(conn) = s.conn() else {
        return BTreeMap::new();
    };
    let r = crate::util::must_one(&conn, "SELECT value FROM meta WHERE key = 'landing'", &[]);
    if r.0.is_empty() {
        return BTreeMap::new();
    }
    let Ok(data) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&r.str("value"))
    else {
        return BTreeMap::new();
    };
    data.into_iter()
        .filter_map(|(slug, v)| {
            v.get("note")
                .and_then(|n| n.as_str())
                .map(|n| (slug, n.to_string()))
        })
        .collect()
}

/// The daemon delivers after a landing: `main` pushes the default branch,
/// `pr` pushes it to `pulse/landed` and keeps a pull request open
/// (`landing_pr`).
pub(crate) fn push_on(s: &pulse_store::Store) -> bool {
    matches!(s.setting("landing.push", "off").as_str(), "main" | "pr")
}

/// A landing newer than the project's last push or pull request.
pub(crate) fn push_owed(s: &pulse_store::Store, slug: String) -> bool {
    let Ok(conn) = s.conn() else { return false };
    let r = crate::util::must_one(&conn, "SELECT COALESCE((SELECT MAX(seq) FROM events WHERE action = 'land.landed' AND json_extract(detail, '$.project') = ?), 0) AS landed,
		COALESCE((SELECT MAX(seq) FROM events WHERE action IN ('land.pushed', 'land.pr') AND subject = ?), 0) AS pushed", &[&slug, &slug]);
    !r.0.is_empty() && r.int("landed") > r.int("pushed")
}

/// The hold note's last clause when a push to the default branch did not
/// go through: the way to a pull request instead.
fn pull_request_hint(slug: &str) -> String {
    format!(
        "for a pull request instead: the task's \"Open a pull request\" in the hub, or pulse land --project {slug} --pr"
    )
}

/// Where a push goes: the default branch's upstream, the url the push uses
/// and the environment that carries the daemon's credential.
pub(crate) struct PushTarget {
    pub(crate) root: String,
    pub(crate) slug: String,
    pub(crate) def: String,
    pub(crate) remote: String,
    pub(crate) remote_ref: String,
    pub(crate) remote_branch: String,
    /// `refs/remotes/<remote>/<branch>`, the tracking ref the fetch updates.
    pub(crate) upstream: String,
    pub(crate) url: String,
    pub(crate) env: Vec<String>,
}

struct CredentialCleanup(Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>);

impl Drop for CredentialCleanup {
    fn drop(&mut self) {
        (self.0)("", &[]);
    }
}

/// `git` wanted a username or password for an http(s) url: nothing in the
/// daemon's environment answered in full (a helper may have given the
/// username alone), and there is no terminal to ask at.
pub(crate) fn no_http_credential(text: &str) -> bool {
    text.contains("could not read Username")
        || text.contains("could not read Password")
        || text.contains("unable to get password from user")
}

/// The `git@host:owner/repo.git` form of a GitHub-style http(s) url; None for
/// anything else (a port, a deeper path, a url that is not http).
pub(crate) fn ssh_form(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host = host.rsplit('@').next().unwrap_or(host);
    if host.is_empty() || host.contains(':') {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segs = path.split('/');
    let owner = segs.next().unwrap_or_default();
    let repo = segs.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || segs.next().is_some() {
        return None;
    }
    Some(format!("git@{host}:{owner}/{repo}.git"))
}

/// A url for a log line: the userinfo an https url may carry stays out.
pub(crate) fn shown_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{path}"),
        None => url.to_string(),
    }
}

/// The url a push to `remote` uses: its push url, else its fetch url, else
/// the name itself (git resolves it the same way).
fn push_url(root: &str, remote: &str) -> String {
    for args in [
        ["remote", "get-url", "--push", remote].as_slice(),
        ["remote", "get-url", remote].as_slice(),
    ] {
        let r = crate::util::git_timeout(root, Duration::from_secs(10), args);
        if r.ok() && !r.stdout.trim().is_empty() {
            return r.stdout.trim().to_string();
        }
    }
    remote.to_string()
}

fn fetch_upstream(t: &PushTarget) -> crate::util::GitResult {
    crate::util::git_timeout_env(
        &t.root,
        Duration::from_secs(120),
        &t.env,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            &t.url,
            &format!("+refs/heads/{}:{}", t.remote_branch, t.upstream),
        ],
    )
}

fn push_default(t: &PushTarget) -> crate::util::GitResult {
    crate::util::git_timeout_env(
        &t.root,
        Duration::from_secs(180),
        &t.env,
        &[
            "push",
            &t.remote,
            &format!("refs/heads/{}:{}", t.def, t.remote_ref),
        ],
    )
}

/// An http(s) push url git has no password for, when the ssh form of the same
/// repository answers to the daemon's key: the remote's push url moves to it
/// (the fetch url stays yours) and the line says so. Err is the hold note's
/// explanation of why it stayed, with the fix.
fn push_url_over_ssh(
    s: &pulse_store::Store,
    t: &mut PushTarget,
    actor: &str,
    now: &str,
) -> Result<String, String> {
    let shown = shown_url(&t.url);
    let fix = |ssh: &str| {
        format!(
            "fix: git -C {} remote set-url --push {} {ssh} with a key the daemon has (a deploy key), or a credential helper that answers for the daemon",
            t.root, t.remote
        )
    };
    let Some(ssh) = ssh_form(&t.url) else {
        return Err(format!(
            "{shown} takes a password the daemon has none of, and it is not a GitHub-style url pulse could push over ssh instead; {}",
            fix("git@HOST:OWNER/REPO.git")
        ));
    };
    let probe = crate::util::git_timeout_env(
        &t.root,
        Duration::from_secs(60),
        &t.env,
        &[
            "ls-remote",
            "--exit-code",
            &ssh,
            &format!("refs/heads/{}", t.remote_branch),
        ],
    );
    if !probe.ok() {
        let why = crate::run::squeeze(&format!("{} {}", probe.stderr, probe.stdout), 160);
        return Err(format!(
            "{shown} takes a password the daemon has none of, and {ssh} does not answer to the daemon's key ({}); {}",
            pulse_cred::redact(&why, &[]),
            fix(&ssh)
        ));
    }
    let set = crate::util::git_timeout(
        &t.root,
        Duration::from_secs(10),
        &["remote", "set-url", "--push", &t.remote, &ssh],
    );
    if !set.ok() {
        return Err(format!(
            "{shown} takes a password the daemon has none of; {ssh} answers, but git remote set-url --push failed: {}; {}",
            crate::util::trunc(set.stderr.trim(), 160),
            fix(&ssh)
        ));
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.push-url",
            &t.slug,
            serde_json::json!({"project": t.slug, "remote": t.remote, "from": shown, "to": ssh}),
        );
    }
    t.url = ssh.clone();
    Ok(format!(
        "landing push url {}: {} pushes over ssh now ({ssh}); {shown} took a password the daemon has none of",
        t.slug, t.remote
    ))
}

/// The states `land_branch` returns that time resolves on its own: the next
/// pass simply tries the sync again.
fn sync_waits(state: &str) -> bool {
    matches!(
        state,
        "deferred" | "locked" | "stopped" | "unreadable" | "blocked"
    )
}

/// What `replay_onto_squashed_upstream` did: the default branch's commit the
/// upstream is a squash of, the new tip, how many commits were copied, and
/// the line that says so.
pub(crate) struct Replayed {
    pub(crate) squash_of: String,
    pub(crate) tip: String,
    pub(crate) commits: i32,
    pub(crate) note: String,
}

/// The most commits the replay copies; a default branch further ahead of the
/// squash than this is left to the merge path and its hold line.
const REPLAY_MAX_COMMITS: usize = 2000;

/// An upstream rewritten by hand to a squash of what pulse had pushed - a
/// root commit whose tree equals a commit on the default branch - shares no
/// ancestor with the branch, so merging the two conflicts on every file and
/// the landed work waits for days (distillery, 2026-10-22: four commits held
/// behind "340 commits landed but not pushed"). The tree says what to do: the
/// commits after that point are copied onto the upstream tip along the
/// branch's first-parent line, each with its own tree, author, date and
/// message, and the branch is moved to the copy. The result's tree is the
/// branch's own, so the checkout and its index already match it and nothing
/// needs verifying again; the push that follows is a fast-forward. Returns
/// `Ok(None)` when the upstream is not that case (a shared ancestor, or no
/// commit of the branch has the upstream's tree) and the merge path applies.
pub(crate) fn replay_onto_squashed_upstream(
    t: &PushTarget,
    up_sha: &str,
    def_sha: &str,
) -> std::result::Result<Option<Replayed>, String> {
    let git = |timeout: u64, args: &[&str]| {
        crate::util::git_timeout(&t.root, Duration::from_secs(timeout), args)
    };
    if up_sha.is_empty() || def_sha.is_empty() {
        return Ok(None);
    }
    let base = git(30, &["merge-base", up_sha, def_sha]);
    if base.ok() && !base.stdout.trim().is_empty() {
        return Ok(None);
    }
    let up_tree = git(10, &["rev-parse", &format!("{up_sha}^{{tree}}")])
        .stdout
        .trim()
        .to_string();
    if up_tree.is_empty() {
        return Ok(None);
    }
    let def_ref = format!("refs/heads/{}", t.def);
    let line = git(
        60,
        &[
            "log",
            "--first-parent",
            "--format=%H %T",
            &format!("-n{}", REPLAY_MAX_COMMITS + 1),
            &def_ref,
        ],
    );
    let Some(squash_of) = line.stdout.lines().find_map(|l| {
        let (sha, tree) = l.trim().split_once(' ')?;
        (tree == up_tree).then(|| sha.to_string())
    }) else {
        return Ok(None);
    };
    let listed = git(
        60,
        &[
            "rev-list",
            "--first-parent",
            "--reverse",
            &format!("{squash_of}..{def_ref}"),
        ],
    );
    if !listed.ok() {
        return Err(crate::util::trunc(listed.stderr.trim(), 200));
    }
    let shas = listed
        .stdout
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if shas.len() > REPLAY_MAX_COMMITS {
        return Err(format!(
            "{} commits to copy, more than {REPLAY_MAX_COMMITS}",
            shas.len()
        ));
    }
    let mut parent = up_sha.to_string();
    for sha in &shas {
        let shown = git(10, &["log", "-1", "--format=%T%n%an%n%ae%n%aI", sha]);
        let mut fields = shown.stdout.lines();
        let (Some(tree), Some(name), Some(email), Some(date)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "could not read {}: {}",
                crate::recipe::short(sha, 12),
                crate::util::trunc(shown.stderr.trim(), 160)
            ));
        };
        let message = git(10, &["log", "-1", "--format=%B", sha]).stdout;
        let message = message.trim_end();
        let made = crate::util::git_timeout_env(
            &t.root,
            Duration::from_secs(30),
            &[
                format!("GIT_AUTHOR_NAME={name}"),
                format!("GIT_AUTHOR_EMAIL={email}"),
                format!("GIT_AUTHOR_DATE={date}"),
            ],
            &[
                "-c",
                "user.name=pulse",
                "-c",
                "user.email=pulse@pulse.local",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                tree,
                "-p",
                &parent,
                "-m",
                if message.is_empty() {
                    "(no message)"
                } else {
                    message
                },
            ],
        );
        if !made.ok() {
            return Err(format!(
                "copying {} failed: {}",
                crate::recipe::short(sha, 12),
                crate::util::trunc(made.stderr.trim(), 200)
            ));
        }
        parent = made.stdout.trim().to_string();
    }
    let tip = parent;
    let tip_tree = git(10, &["rev-parse", &format!("{tip}^{{tree}}")])
        .stdout
        .trim()
        .to_string();
    let def_tree = git(10, &["rev-parse", &format!("{def_sha}^{{tree}}")])
        .stdout
        .trim()
        .to_string();
    if tip_tree.is_empty() || tip_tree != def_tree {
        return Err(format!(
            "the copy's tree {} is not {}'s {}",
            crate::recipe::short(&tip_tree, 12),
            t.def,
            crate::recipe::short(&def_tree, 12)
        ));
    }
    // The old value guards the move: a branch that moved meanwhile is left
    // alone and the next pass starts over from what it finds.
    let moved = git(
        30,
        &[
            "update-ref",
            "-m",
            &format!(
                "pulse: {} commits copied onto the squashed {}/{}",
                shas.len(),
                t.remote,
                t.remote_branch
            ),
            &def_ref,
            &tip,
            def_sha,
        ],
    );
    if !moved.ok() {
        return Err(format!(
            "moving {} failed: {}",
            t.def,
            crate::util::trunc(moved.stderr.trim(), 200)
        ));
    }
    let commits = shas.len() as i32;
    let note = if commits == 0 {
        format!(
            "{} now points at it (same tree, nothing to copy); no verify needed",
            t.def
        )
    } else {
        format!(
            "{commits} {} copied onto it with their trees, authors and messages ({}..{}); the tree is unchanged, so no verify is needed and the push is a fast-forward",
            crate::util::plural(commits, "commit"),
            crate::recipe::short(def_sha, 12),
            crate::recipe::short(&tip, 12)
        )
    };
    Ok(Some(Replayed {
        squash_of,
        tip,
        commits,
        note,
    }))
}

/// Directory prefix of the hub's generated static export.
pub(crate) const HUB_OUT_PREFIX: &str = "web/hub/out/";

/// Rebuild the hub's static export from the merged `web/hub/src` after an
/// upstream sync merge conflicted only under the generated export: take the
/// upstream copy of `web/hub/{src,out,package-lock.json}`, rebuild in a
/// temporary index and working tree so a failure leaves the merge conflict
/// intact for the usual hold, and commit. Returns false when the conflict
/// reaches outside the export or any step failed.
pub(crate) fn resolve_hub_out_sync_conflict(worktree: &str, branch: &str) -> bool {
    // The sync branch is `pulse/sync/<remote>/<upstream-branch>`; the
    // upstream ref it merged is `<remote>/<upstream-branch>`.
    let upstream_ref = branch
        .strip_prefix("pulse/sync/")
        .map(str::to_string)
        .unwrap_or_default();
    let unmerged = crate::util::git_timeout(
        worktree,
        Duration::from_secs(30),
        &["diff", "--name-only", "--diff-filter=U", "-z"],
    );
    if !unmerged.ok()
        || unmerged
            .stdout
            .split('\0')
            .any(|p| !p.is_empty() && !p.starts_with(HUB_OUT_PREFIX))
    {
        return false;
    }
    let upstream = crate::util::git_timeout(
        worktree,
        Duration::from_secs(30),
        &["rev-parse", "--verify", &upstream_ref],
    );
    if !upstream.ok() {
        return false;
    }
    let upstream = upstream.stdout.trim().to_string();
    // Incidental bare-index fixup from the merge path; best effort only.
    let _ = crate::util::git_timeout(
        worktree,
        Duration::from_secs(60),
        &["checkout", &upstream, "--", ".husky/_/.gitignore"],
    );
    // Rebuild the export in place: the landing worktree's disk already holds
    // the merged web/hub/src (only generated out/ paths conflicted, so src
    // merged clean), which is exactly the tree the stamp gate names. Build
    // over it with npm, then stage the result and finish the merge commit.
    // Any failure backs out to the usual hold.
    let hub = std::path::Path::new(worktree).join("web/hub");
    let npm = |args: &[&str]| {
        std::process::Command::new("npm")
            .args(args)
            .current_dir(&hub)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_QUARANTINE_PATH")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|st| st.success())
    };
    if !npm(&["ci", "--no-audit", "--no-fund"]) || !npm(&["run", "build"]) {
        crate::util::git_timeout(worktree, Duration::from_secs(60), &["merge", "--abort"]);
        return false;
    }
    if !crate::util::git_timeout(
        worktree,
        Duration::from_secs(120),
        &["add", "-A", "--", "web/hub"],
    )
    .ok()
    {
        crate::util::git_timeout(worktree, Duration::from_secs(60), &["merge", "--abort"]);
        return false;
    }
    let msg = format!("Land {branch}: rebuild web/hub/out from the merged src");
    let committed = std::process::Command::new("git")
        .args([
            "-C",
            worktree,
            "-c",
            "user.name=pulse",
            "-c",
            "user.email=pulse@pulse.local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--no-verify",
            "-m",
            &msg,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|st| st.success());
    if !committed {
        crate::util::git_timeout(worktree, Duration::from_secs(60), &["merge", "--abort"]);
        return false;
    }
    true
}

/// The remote moved under the local default branch. Merge `<remote>/<branch>`
/// into it through the landing path - a throwaway worktree, the project's
/// check on the merged result, a fast-forward of the checkout - so the push
/// that follows is a plain fast-forward. An upstream that is a squash of the
/// branch's own history is not merged but replayed onto
/// (`replay_onto_squashed_upstream`). Returns the log line of a landed
/// sync and the hold note of anything else; a conflict or a red check is
/// remembered for this pair of shas, so it is not re-run every pass and goes
/// away when either side moves.
pub(crate) fn sync_upstream(
    s: &pulse_store::Store,
    project: crate::util::Row,
    t: &PushTarget,
    actor: String,
    now: String,
    behind: i32,
) -> (Option<String>, Option<String>) {
    let slug = t.slug.clone();
    let rev = |r: &str| {
        crate::util::git_timeout(&t.root, Duration::from_secs(10), &["rev-parse", r])
            .stdout
            .trim()
            .to_string()
    };
    let up_sha = rev(&t.upstream);
    let def_sha = rev(&format!("refs/heads/{}", t.def));
    let what = format!(
        "{}/{} has {behind} {} {} does not",
        t.remote,
        t.remote_branch,
        crate::util::plural(behind, "commit"),
        t.def
    );
    if let Some(hold) = crate::land::landing_retry_hold(s, &slug, &now) {
        return (None, Some(format!("{what}; merging them waits: {hold}")));
    }
    let key = format!("push-sync:{slug}");
    // Before the memo of a remembered conflict: a squashed upstream that
    // conflicted on an earlier pass is exactly what the replay resolves.
    match replay_onto_squashed_upstream(t, &up_sha, &def_sha) {
        Ok(Some(replayed)) => {
            if let Ok(conn) = s.conn() {
                crate::util::exec1(&conn, "DELETE FROM meta WHERE key = ?", &[&key]);
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "land.synced",
                    &slug,
                    serde_json::json!({"project": slug, "remote": t.remote, "branch": t.remote_branch, "commits": behind, "upstream": up_sha, "base": def_sha, "replayed": replayed.commits, "squash_of": replayed.squash_of, "tip": replayed.tip, "note": replayed.note}),
                );
            }
            return (
                Some(format!(
                    "landing synced {slug}: {}/{} is a squash of {}'s {}; {}",
                    t.remote,
                    t.remote_branch,
                    t.def,
                    crate::recipe::short(&replayed.squash_of, 12),
                    replayed.note
                )),
                None,
            );
        }
        Ok(None) => {}
        Err(why) => {
            return (
                None,
                Some(format!(
                    "{what}; {}/{} is a squash of {} but the replay failed: {why}",
                    t.remote, t.remote_branch, t.def
                )),
            );
        }
    }
    if let Ok(conn) = s.conn() {
        let r = crate::util::must_one(&conn, "SELECT value FROM meta WHERE key = ?", &[&key]);
        if !r.0.is_empty()
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.str("value"))
            && v["upstream"] == up_sha
            && v["base"] == def_sha
            && let Some(note) = v["note"].as_str()
        {
            return (None, Some(note.to_string()));
        }
    }
    let branch = format!("pulse/sync/{}/{}", t.remote, t.remote_branch);
    let made = crate::util::git_timeout(
        &t.root,
        Duration::from_secs(10),
        &["branch", "-f", &branch, &up_sha],
    );
    if !made.ok() {
        return (
            None,
            Some(format!(
                "{what}; could not make {branch}: {}",
                crate::util::trunc(made.stderr.trim(), 160)
            )),
        );
    }
    let subject = crate::util::git_timeout(
        &t.root,
        Duration::from_secs(10),
        &["log", "-1", "--format=%s", &up_sha],
    )
    .stdout
    .trim()
    .to_string();
    let title = format!(
        "sync {}/{}: {behind} {} ({} {})",
        t.remote,
        t.remote_branch,
        crate::util::plural(behind, "commit"),
        crate::recipe::short(&up_sha, 7),
        crate::util::trunc(&subject, 120)
    );
    let (mut state, mut note) = match crate::land::land_branch(
        s,
        project.clone(),
        branch.clone(),
        actor.clone(),
        now.clone(),
        crate::land::LandOpts {
            verify: true,
            title,
            sync: true,
            upstream: up_sha.clone(),
            ..Default::default()
        },
    ) {
        Ok(v) => v,
        Err(e) => ("failed".to_string(), e.to_string()),
    };
    // A sync never leaves a branch behind, whatever happened to it.
    crate::util::git_timeout(&t.root, Duration::from_secs(30), &["branch", "-D", &branch]);
    // The plain merge conflicted: the default branch's own commits are
    // re-applied onto the upstream and the result lands as one merge commit
    // (resync.rs), so the forge's truth reaches the checkout.
    let mut resync: Option<crate::resync::Resync> = None;
    if state == "conflict" {
        match crate::resync::resync_onto_upstream(&s.home, t, &up_sha, &def_sha, &actor) {
            Ok(Some(r)) => {
                let title = format!(
                    "sync {}/{}: {behind} {} with {}'s own {} {} re-applied ({})",
                    t.remote,
                    t.remote_branch,
                    crate::util::plural(behind, "commit"),
                    t.def,
                    r.picked.len(),
                    crate::util::plural(r.picked.len() as i32, "commit"),
                    r.tally()
                );
                let landed = crate::land::land_branch(
                    s,
                    project.clone(),
                    r.branch.clone(),
                    actor.clone(),
                    now.clone(),
                    crate::land::LandOpts {
                        verify: true,
                        title,
                        sync: true,
                        upstream: up_sha.clone(),
                        ..Default::default()
                    },
                );
                crate::util::git_timeout(
                    &t.root,
                    Duration::from_secs(30),
                    &["branch", "-D", &r.branch],
                );
                (state, note) = match landed {
                    Ok(v) => v,
                    Err(e) => ("failed".to_string(), e.to_string()),
                };
                resync = Some(r);
            }
            Ok(None) => {}
            Err(why) => {
                note = format!(
                    "{note}; re-applying {}'s own commits onto {}/{} failed too: {why}",
                    t.def, t.remote, t.remote_branch
                );
            }
        }
    }
    if state == "landed" || state == "merged" {
        let resync = resync.map(|r| crate::resync::after_landing(s, t, &r, &actor, &now));
        if let Ok(conn) = s.conn() {
            crate::util::exec1(&conn, "DELETE FROM meta WHERE key = ?", &[&key]);
            let mut detail = serde_json::json!({"project": slug, "remote": t.remote, "branch": t.remote_branch, "commits": behind, "upstream": up_sha, "base": def_sha, "note": note});
            if let Some(r) = &resync {
                detail["resync"] = r.detail();
            }
            crate::util::event_at(&conn, &now, &actor, "land.synced", &slug, detail);
        }
        let reapplied = resync
            .map(|r| {
                let jobs = r
                    .with("conflicted")
                    .iter()
                    .filter(|p| !p.job.is_empty())
                    .map(|p| p.job.clone())
                    .collect::<Vec<_>>();
                format!(
                    "; {}'s own {} {} re-applied: {}{}",
                    t.def,
                    r.picked.len(),
                    crate::util::plural(r.picked.len() as i32, "commit"),
                    r.tally(),
                    if jobs.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " for {} {}",
                            crate::util::plural(jobs.len() as i32, "job"),
                            jobs.join(", ")
                        )
                    }
                )
            })
            .unwrap_or_default();
        return (
            Some(format!(
                "landing synced {slug}: {}/{} merged into {} ({behind} {}{reapplied}; {note})",
                t.remote,
                t.remote_branch,
                t.def,
                crate::util::plural(behind, "commit")
            )),
            None,
        );
    }
    if sync_waits(&state) {
        return (None, Some(format!("{what}; merging them {state}: {note}")));
    }
    let hold = format!(
        "{what}; merging them {state}: {note}; pulse tries again when either side moves; by hand: git -C {} pull --no-rebase {} {}",
        t.root, t.remote, t.remote_branch
    );
    if let Ok(conn) = s.conn()
        && let Ok(data) = pulse_store::gojson::to_string(&serde_json::json!({
            "upstream": up_sha, "base": def_sha, "state": state, "note": hold, "at": now
        }))
    {
        crate::util::exec1(
            &conn,
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            &[&key, &data],
        );
    }
    (None, Some(hold))
}

/// Pushes the default branch to its upstream after a landing. Before the
/// push it fetches the upstream through the push url; a remote that moved is
/// merged into the default branch first (`sync_upstream`), and an http(s)
/// push url git has no password for moves to its ssh form when that answers
/// to the daemon's key (`push_url_over_ssh`). Returns the log lines and the
/// hold note, empty when nothing is owed or the push went through.
/// The tracking ref ahead of the local default branch: a fetch-and-check
/// pass is owed even with nothing to push, so a merge made on the forge
/// reaches the brain within one landing pass instead of waiting for a local
/// landing.
fn upstream_ahead_of_default(root: &str, def: &str, s: &pulse_store::Store) -> bool {
    let upstream = if crate::landing_pr::pr_mode(s) {
        "origin/pr"
    } else {
        "origin/main"
    };
    crate::util::git_timeout(
        root,
        Duration::from_secs(30),
        &[
            "rev-list",
            "--count",
            &format!("refs/heads/{def}..{upstream}"),
        ],
    )
    .stdout
    .trim()
    .parse::<i32>()
    .unwrap_or(0)
        > 0
}

fn fast_forward_failed_note(why: &str, upstream: &str) -> String {
    format!("fast-forward to {upstream} failed: {why}")
}

/// A remote merge (e.g. a human merged on the forge) reached the tracking
/// ref while the local default branch stayed put; move it with the merge's
/// own commits so the brain sees them this pass instead of waiting for a
/// local landing. Only a clean checkout of the default branch is touched: a
/// dirty or detached checkout keeps its HEAD and `sync_upstream` handles the
/// remote commits as on any other pass.
#[allow(clippy::too_many_arguments)]
fn fast_forward_default(
    s: &pulse_store::Store,
    root: &str,
    slug: &str,
    def: &str,
    name: &str,
    up_ref: &str,
    actor: &str,
    now: &str,
    behind: i32,
) -> Option<String> {
    let (on_default, clean) = {
        let head = crate::util::git_timeout(
            root,
            Duration::from_secs(10),
            &["rev-parse", "--abbrev-ref", "HEAD"],
        );
        let status = crate::util::git_timeout(
            root,
            Duration::from_secs(60),
            &["status", "--porcelain", "--untracked-files=no"],
        );
        (
            head.ok() && head.stdout.trim() == def,
            status.ok() && status.stdout.trim().is_empty(),
        )
    };
    if !on_default || !clean {
        return None;
    }
    let from = crate::util::git_timeout(root, Duration::from_secs(10), &["rev-parse", def])
        .stdout
        .trim()
        .to_string();
    let to = crate::util::git_timeout(root, Duration::from_secs(10), &["rev-parse", up_ref])
        .stdout
        .trim()
        .to_string();
    let ff = crate::util::git_timeout(
        root,
        Duration::from_secs(120),
        &["merge", "--ff-only", up_ref],
    );
    if !ff.ok() {
        let why = crate::run::squeeze(&format!("{} {}", ff.stderr, ff.stdout), 160);
        let why = pulse_cred::redact(&why, &[]);
        if let Ok(conn) = s.conn() {
            crate::util::event_at(
                &conn,
                now,
                actor,
                "land.fast-forward",
                slug,
                serde_json::json!({"project": slug, "branch": def, "upstream": name, "behind": behind, "ok": false, "why": why.clone()}),
            );
        }
        return Some(fast_forward_failed_note(&why, name));
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.fast-forward",
            slug,
            serde_json::json!({"project": slug, "branch": def, "upstream": name, "behind": behind, "ok": true, "from": crate::recipe::short(&from, 12), "to": crate::recipe::short(&to, 12)}),
        );
    }
    Some(format!(
        "landing fast-forwarded {slug}: {def} -> {name} ({behind} {})",
        crate::util::plural(behind, "commit"),
    ))
}

pub(crate) fn push_landed(
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    landed_now: bool,
) -> (Vec<String>, String) {
    let slug = project.str("slug");
    // The owner's one-time ask for a pull request counts whatever the
    // setting says; it is answered below, by a pull request or by the word
    // that nothing was ahead.
    let asked = pulse_tasks::pull_request_asked(s, &slug);
    if !push_on(s) && !asked {
        return (Vec::new(), String::new());
    }
    let root = project.str("path");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    if !landed_now
        && !asked
        && !push_owed(s, slug.clone())
        && !upstream_ahead_of_default(&root, &def, s)
    {
        return (Vec::new(), String::new());
    }
    let via_pr = asked || crate::landing_pr::pr_mode(s);
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
            Vec::new(),
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
    let ahead = |upstream: &str| {
        crate::util::git_timeout(
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
        .unwrap_or(0)
    };
    let mut n = ahead(&upstream);
    if n == 0 {
        // The local default branch has nothing the upstream lacks, so there
        // is nothing to push - but the remote can still carry a merge the
        // brain has not seen. Fetch (sync_upstream's own way) and, when the
        // branch is now behind, fast-forward it to the upstream: the merge's
        // own commits move the brain's HEAD, so nothing can conflict.
        let fetched = fetch_upstream(&PushTarget {
            root: root.clone(),
            slug: slug.clone(),
            def: def.clone(),
            remote: remote.clone(),
            remote_ref: remote_ref.clone(),
            remote_branch: remote_branch.clone(),
            upstream: upstream.clone(),
            url: push_url(&root, &remote),
            env: push_env(root.clone()),
        });
        let ff_ok = fetched.ok();
        if ff_ok {
            let behind_out = crate::util::git_timeout(
                &root,
                Duration::from_secs(30),
                &[
                    "rev-list",
                    "--count",
                    &format!("refs/heads/{def}..{upstream}"),
                ],
            );
            let behind = behind_out.stdout.trim().parse::<i32>().unwrap_or(0);
            if behind > 0
                && let Some(result) = fast_forward_default(
                    s,
                    &root,
                    &slug,
                    &def,
                    &format!("{remote}/{remote_branch}"),
                    &upstream,
                    &actor,
                    &now,
                    behind,
                )
            {
                return (vec![result], String::new());
            }
        }
        if asked && let Ok(conn) = s.conn() {
            let why = format!("{def} has nothing {remote}/{remote_branch} lacks");
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.pr-answered",
                &slug,
                serde_json::json!({"project": slug, "why": why}),
            );
            return (
                vec![format!(
                    "landing pull request {slug}: nothing to deliver, {why}"
                )],
                String::new(),
            );
        }
        return (Vec::new(), String::new());
    }
    let (env, cleanup, why) = push_credential_env(s, root.clone());
    if !why.is_empty() {
        return (
            Vec::new(),
            format!(
                "{n} {} landed but not pushed; push credential: {why}",
                crate::util::plural(n, "commit")
            ),
        );
    }
    let _cleanup = CredentialCleanup(cleanup);
    let mut t = PushTarget {
        root: root.clone(),
        slug: slug.clone(),
        def: def.clone(),
        remote: remote.clone(),
        remote_ref,
        remote_branch: remote_branch.clone(),
        upstream: upstream.clone(),
        url: push_url(&root, &remote),
        env,
    };
    let mut lines = Vec::new();
    let mut ssh_tried = false;
    let mut url_note = String::new();
    let mut conflict: Option<String> = None;
    let mut fetched = fetch_upstream(&t);
    if !fetched.ok() && no_http_credential(&fetched.stderr) {
        ssh_tried = true;
        match push_url_over_ssh(s, &mut t, &actor, &now) {
            Ok(line) => {
                lines.push(line);
                fetched = fetch_upstream(&t);
            }
            Err(why) => url_note = why,
        }
    }
    let fetch_note = if fetched.ok() {
        String::new()
    } else {
        let why = if let Some(e) = &fetched.err {
            e.clone()
        } else {
            crate::run::squeeze(&format!("{} {}", fetched.stderr, fetched.stdout), 200)
        };
        format!(
            "fetch from {} failed: {}",
            shown_url(&t.url),
            pulse_cred::redact(&why, &[])
        )
    };
    if fetched.ok() {
        let behind = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &[
                "rev-list",
                "--count",
                &format!("refs/heads/{def}..{upstream}"),
            ],
        )
        .stdout
        .trim()
        .parse::<i32>()
        .unwrap_or(0);
        if behind > 0 {
            let (line, hold) =
                sync_upstream(s, project.clone(), &t, actor.clone(), now.clone(), behind);
            lines.extend(line);
            if let Some(hold) = hold {
                if via_pr {
                    // The pull request carries the conflict to the forge,
                    // where the owner resolves it; nothing waits here.
                    conflict = Some(hold);
                } else {
                    return (
                        lines,
                        format!(
                            "{n} {} landed but not pushed; {hold}; {}",
                            crate::util::plural(n, "commit"),
                            pull_request_hint(&slug)
                        ),
                    );
                }
            } else {
                n = ahead(&upstream);
                if n == 0 {
                    return (lines, String::new());
                }
            }
        }
    }
    if via_pr {
        let (pr_lines, note) = crate::landing_pr::deliver(s, &t, &actor, &now, n, conflict);
        lines.extend(pr_lines);
        if !note.is_empty() && !fetch_note.is_empty() {
            return (lines, format!("{note}; {fetch_note}"));
        }
        return (lines, note);
    }
    let rev = |r: &str| {
        crate::util::git_timeout(&root, Duration::from_secs(10), &["rev-parse", r])
            .stdout
            .trim()
            .to_string()
    };
    let from = rev(&upstream);
    let to = rev(&format!("refs/heads/{def}"));
    let mut res = push_default(&t);
    if !res.ok() && !ssh_tried && no_http_credential(&res.stderr) {
        match push_url_over_ssh(s, &mut t, &actor, &now) {
            Ok(line) => {
                lines.push(line);
                res = push_default(&t);
            }
            Err(why) => url_note = why,
        }
    }
    if !res.ok() {
        let mut why = crate::run::squeeze(&format!("{} {}", res.stderr, res.stdout), 300);
        if let Some(e) = res.err {
            why = e;
        }
        let mut note = format!(
            "{n} {} landed but not pushed; push failed: {}",
            crate::util::plural(n, "commit"),
            pulse_cred::redact(&why, &[])
        );
        for extra in [url_note, fetch_note, pull_request_hint(&slug)] {
            if !extra.is_empty() {
                note.push_str("; ");
                note.push_str(&extra);
            }
        }
        return (lines, note);
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
    let mut line = format!(
        "landing pushed {slug}: {def} -> {remote}/{remote_branch} ({n} {}, {}..{})",
        crate::util::plural(n, "commit"),
        crate::recipe::short(&from, 12),
        crate::recipe::short(&to, 12)
    );
    if !fetch_note.is_empty() {
        line.push_str(&format!(" ({fetch_note})"));
    }
    lines.push(line);
    (lines, String::new())
}

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
    use crate::test_helpers_daemon::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    /// A repo with one committed `web/hub` skeleton and a bare `origin/main`
    /// ref (a plain local branch), so the resolver can rev-parse the upstream
    /// it derives from the `pulse/sync/<remote>/<branch>` name.
    /// Build a repo whose `origin/main` and local `main` diverged, changing
    /// only the generated `web/hub/out/index.html`, so a real merge of
    /// origin/main conflicts out/-only. Returns the sync branch name.
    fn seed_hub_sync_worktree(root: &str) -> String {
        std::fs::create_dir_all(Path::new(root).join("web/hub/src")).expect("dirs");
        std::fs::create_dir_all(Path::new(root).join("web/hub/out")).expect("dirs");
        commit_file(root, "web/hub/src/app.css", "merged src\n", "seed src");
        commit_file(root, "web/hub/out/index.html", "base out\n", "seed out");
        // A buildable hub skeleton: the resolver runs `npm ci && npm run build`.
        commit_file(
            root,
            "web/hub/package.json",
            "{\"name\":\"t\",\"version\":\"0.0.0\",\"scripts\":{\"build\":\"node -e \\\"require('fs').writeFileSync('out/index.html','rebuilt')\\\"\"}}\n",
            "seed pkg",
        );
        commit_file(
            root,
            "web/hub/package-lock.json",
            "{\"name\":\"t\",\"version\":\"0.0.0\",\"lockfileVersion\":3,\"packages\":{\"\":{\"name\":\"t\",\"version\":\"0.0.0\"}}}\n",
            "seed lock",
        );
        // Upstream moves only the generated bundle. Build it on a side
        // branch, then point the origin/main ref at it.
        run(root, &["git", "branch", "-q", "upbuild", "HEAD"]);
        commit_file(root, "web/hub/out/index.html", "local out\n", "local out");
        run(root, &["git", "checkout", "-q", "upbuild"]);
        commit_file(
            root,
            "web/hub/out/index.html",
            "upstream out\n",
            "upstream out",
        );
        let up = run(root, &["git", "rev-parse", "HEAD"]);
        let up = up.trim();
        run(root, &["git", "update-ref", "refs/remotes/origin/main", up]);
        run(root, &["git", "checkout", "-q", "main"]);
        run(root, &["git", "branch", "-qD", "upbuild"]);
        "pulse/sync/origin/main".to_string()
    }

    #[test]
    fn hub_out_only_sync_conflict_is_resolved_by_the_rebuilt_bundle() {
        let home = tempfile::tempdir().expect("home");
        let root = home.path().join("wt");
        std::fs::create_dir_all(&root).expect("root");
        run(
            root.to_str().unwrap(),
            &["git", "init", "-q", "-b", "main", "."],
        );
        let root_s = root.display().to_string();
        let branch = seed_hub_sync_worktree(&root_s);
        // The merge conflicts out/-only by construction; allow the non-zero.
        let status = std::process::Command::new("git")
            .args([
                "-C",
                &root_s,
                "merge",
                "--no-ff",
                "--no-commit",
                "origin/main",
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("merge");
        assert!(!status.success(), "fixture merge should conflict");
        let unmerged = run(&root_s, &["git", "diff", "--name-only", "--diff-filter=U"]);
        assert!(
            !unmerged.is_empty() && unmerged.lines().all(|p| p.starts_with(HUB_OUT_PREFIX)),
            "fixture conflict should sit under web/hub/out only: {unmerged:?}"
        );
        let before = run(&root_s, &["git", "rev-parse", "HEAD"]);
        assert!(
            resolve_hub_out_sync_conflict(&root_s, &branch),
            "out/-only conflict should resolve"
        );
        let head = run(&root_s, &["git", "log", "-1", "--format=%s", "HEAD"]);
        assert!(
            head.contains("rebuild web/hub/out"),
            "resolution commit, got {head:?}"
        );
        let new_head = run(&root_s, &["git", "rev-parse", "HEAD"]);
        assert_ne!(new_head, before, "a resolution commit was added");
        let src = run(&root_s, &["git", "show", "HEAD:web/hub/src/app.css"]);
        assert!(src.contains("merged src"), "merged src kept: {src:?}");
        // The rebuilt bundle carries the build's output, not either side's copy.
        let out = run(&root_s, &["git", "show", "HEAD:web/hub/out/index.html"]);
        assert!(out.contains("rebuilt"), "bundle rebuilt from src: {out:?}");
    }

    #[test]
    fn sync_conflict_outside_the_hub_export_still_holds() {
        let home = tempfile::tempdir().expect("home");
        let root = home.path().join("wt");
        std::fs::create_dir_all(&root).expect("root");
        run(
            root.to_str().unwrap(),
            &["git", "init", "-q", "-b", "main", "."],
        );
        let root_s = root.display().to_string();
        let branch = seed_hub_sync_worktree(&root_s);
        // A conflicted path outside web/hub/out alongside the out/ one: refuse
        // to resolve, leave HEAD and the merge state untouched for the hold.
        let base = run(&root_s, &["git", "merge-base", "HEAD", "origin/main"]);
        let _ = base;
        commit_file(&root_s, "src/main.rs", "fn main() {}\n", "local code");
        let before = run(&root_s, &["git", "rev-parse", "HEAD"]);
        // Force an unmerged entry outside web/hub/out.
        let mut info = String::new();
        for (stage, text) in [(1, "base\n"), (2, "ours\n"), (3, "theirs\n")] {
            let oid = run_input(&root_s, text);
            info.push_str(&format!("100644 {oid} {stage}\tsrc/main.rs\n"));
        }
        feed_git(&root_s, &["update-index", "--index-info"], &info);
        assert!(
            !resolve_hub_out_sync_conflict(&root_s, &branch),
            "a conflict outside web/hub/out must still hold"
        );
        assert_eq!(run(&root_s, &["git", "rev-parse", "HEAD"]), before);
    }

    fn run_input(root: &str, text: &str) -> String {
        let mut child = std::process::Command::new("git")
            .args(["-C", root, "hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("hash-object");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(text.as_bytes())
            .expect("write");
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn feed_git(root: &str, args: &[&str], input: &str) {
        let mut child = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write");
        assert!(child.wait().expect("wait").success());
    }

    #[test]
    fn landing_holds_survive_reload() {
        let s = new_store();
        let mut holds = BTreeMap::new();
        holds.insert("proj".to_string(), "push failed: x".to_string());
        record_landing_holds(&s, holds.clone(), "2026-01-01T00:00:00Z".to_string());
        assert_eq!(load_landing_holds(&s), holds);
        record_landing_holds(&s, BTreeMap::new(), "2026-01-01T00:00:00Z".to_string());
        assert!(load_landing_holds(&s).is_empty());
    }

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

    /// A clone of `bare` at a fresh temp path.
    fn other_clone(bare: &str) -> String {
        let dir = tempfile::tempdir().expect("other").keep().join("other");
        run(
            ".",
            &["git", "clone", "-q", bare, &dir.display().to_string()],
        );
        dir.display().to_string()
    }

    fn commit_file(repo: &str, name: &str, text: &str, msg: &str) {
        std::fs::create_dir_all(Path::new(repo).join(name).parent().unwrap()).ok();
        std::fs::write(Path::new(repo).join(name), text).expect("write");
        run(repo, &["git", "add", "-A"]);
        run(repo, &["git", "commit", "-q", "-m", msg]);
    }

    /// Rewrites the remote's main by hand to one root commit holding its
    /// current tree, the way an owner squashes a history; returns its sha.
    fn squash_remote(clone: &str, msg: &str) -> String {
        let tree = run(clone, &["git", "rev-parse", "origin/main^{tree}"])
            .trim()
            .to_string();
        let squash = run(clone, &["git", "commit-tree", &tree, "-m", msg])
            .trim()
            .to_string();
        run(
            clone,
            &[
                "git",
                "push",
                "-q",
                "-f",
                "origin",
                &format!("{squash}:refs/heads/main"),
            ],
        );
        squash
    }

    #[test]
    fn landing_syncs_a_remote_that_moved_then_pushes() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        let other = other_clone(&bare);
        commit_file(&other, "other.txt", "other\n", "someone else");
        run(&other, &["git", "push", "-q", "origin", "main"]);
        let remote_head = bare_ref(&bare, "refs/heads/main");

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1].starts_with(
                "landing synced proj: origin/main merged into main (1 commit; pulse/sync/origin/main landed on main as "
            ),
            "lines {lines:?}"
        );
        assert!(
            lines[2].starts_with("landing pushed proj: main -> origin/main (2 commits, "),
            "lines {lines:?}"
        );
        assert!(
            cur.is_empty(),
            "hold notes after a sync and a push: {cur:?}"
        );
        let local = ref_of(&repo, "refs/heads/main");
        assert_eq!(bare_ref(&bare, "refs/heads/main"), local);
        assert!(
            run(
                &repo,
                &["git", "merge-base", "--is-ancestor", &remote_head, "main"]
            )
            .is_empty()
        );
        assert!(Path::new(&repo).join("other.txt").is_file());
        assert!(Path::new(&repo).join("pulse_worker_1.txt").is_file());
        let subject = run(&repo, &["git", "log", "-1", "--format=%s", "main"]);
        assert!(
            subject.starts_with("Land pulse/sync/origin/main: sync origin/main: 1 commit ("),
            "merge subject {subject:?}"
        );
        assert!(
            run(&repo, &["git", "branch", "--list", "pulse/sync/*"])
                .trim()
                .is_empty(),
            "the sync branch stayed"
        );
        for (action, want) in [
            ("land.landed", 2),
            ("land.synced", 1),
            ("land.pushed", 1),
            ("land.held", 0),
        ] {
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                    &[&action]
                ),
                want,
                "{action}"
            );
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key = 'push-sync:proj'",
                &[]
            ),
            0
        );
        assert!(!push_owed(&s, "proj".into()));

        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert!(
            lines.is_empty() && cur.is_empty(),
            "quiet tick said {lines:?} {cur:?}"
        );
    }

    #[test]
    fn push_landed_fast_forwards_the_default_branch_to_a_human_merge() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");

        // A human merges on the forge: main gains a commit the brain lacks.
        let other = other_clone(&bare);
        commit_file(&other, "merged.txt", "merged on the forge\n", "human merge");
        run(&other, &["git", "push", "-q", "origin", "main"]);
        let remote_head = bare_ref(&bare, "refs/heads/main");
        let before = ref_of(&repo, "refs/heads/main");
        assert_ne!(remote_head, before);

        // ahead == 0, but push_landed still fetches and fast-forwards the
        // default branch so the merge reaches the brain in this pass. The
        // tracking ref is fetched first so the pass is owed without a push.
        run(&repo, &["git", "fetch", "-q", "origin", "main"]);
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert!(cur.is_empty(), "a fast-forward cannot hold: {cur:?}");
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert_eq!(
            lines[0],
            "landing fast-forwarded proj: main -> origin/main (1 commit)"
        );
        assert_eq!(ref_of(&repo, "refs/heads/main"), remote_head);
        assert_eq!(
            ref_of(&repo, "refs/remotes/origin/main"),
            remote_head,
            "the tracking ref fetched"
        );
        assert!(Path::new(&repo).join("merged.txt").is_file());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.fast-forward' AND json_extract(detail, '$.ok') = 1",
                &[]
            ),
            1
        );

        // Next pass is quiet: nothing new upstream, nothing to fast-forward.
        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert!(
            lines.is_empty() && cur.is_empty(),
            "quiet tick said {lines:?} {cur:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.fast-forward'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn push_landed_never_fast_forwards_a_dirty_checkout() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");

        let other = other_clone(&bare);
        commit_file(&other, "merged.txt", "merged on the forge\n", "human merge");
        run(&other, &["git", "push", "-q", "origin", "main"]);
        let remote_head = bare_ref(&bare, "refs/heads/main");
        let before = ref_of(&repo, "refs/heads/main");

        // The owner's checkout has uncommitted work: keep its HEAD exactly
        // where it was; sync_upstream handles the remote commits instead.
        std::fs::write(Path::new(&repo).join("wip.txt"), "work in progress\n").unwrap();
        run(&repo, &["git", "add", "wip.txt"]);
        run(&repo, &["git", "fetch", "-q", "origin", "main"]);

        let (lines, _cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(
            ref_of(&repo, "refs/heads/main"),
            before,
            "a dirty checkout's HEAD never moves"
        );
        assert_ne!(remote_head, ref_of(&repo, "refs/heads/main"));
        assert!(
            lines.iter().all(|l| !l.contains("fast-forwarded")),
            "no fast-forward line: {lines:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.fast-forward'",
                &[]
            ),
            0
        );
        assert!(Path::new(&repo).join("wip.txt").is_file(), "wip kept");
    }

    /// A merge of the upstream that conflicts for real no longer holds the
    /// push until a human pulls: the default branch's own commits are
    /// re-applied onto the upstream, the one that clashes is set aside with
    /// its patch and a job, and the merge of both sides lands and pushes.
    #[test]
    fn landing_sync_conflict_resyncs_onto_the_upstream_and_sets_the_clash_aside() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        commit_file(&repo, "README.md", "# proj\nlocal\n", "local readme");
        let local_readme = ref_of(&repo, "refs/heads/main");
        let other = other_clone(&bare);
        commit_file(&other, "README.md", "# proj\nremote\n", "remote readme");
        run(&other, &["git", "push", "-q", "origin", "main"]);
        let remote_head = bare_ref(&bare, "refs/heads/main");

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        let short = crate::recipe::short(&local_readme, 12);
        let job = must_one(
            &s,
            "SELECT id, title, brief, status, source FROM jobs WHERE source = ?",
            &[&format!("resync:proj:{short}")],
        );
        assert!(!job.0.is_empty(), "no job re-applies the set-aside commit");
        assert!(
            lines[1].starts_with(&format!(
                "landing synced proj: origin/main merged into main (1 commit; main's own 2 commits re-applied: 1 carried, 1 set aside for job {}; pulse/resync/origin/main landed on main as ",
                job.str("id")
            )),
            "lines {lines:?}"
        );
        assert!(
            lines[2].starts_with("landing pushed proj: main -> origin/main ("),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        // The merge commit: both sides as parents, the upstream's README, the
        // carried work, and a message that says where each commit went.
        let local = ref_of(&repo, "refs/heads/main");
        assert_eq!(bare_ref(&bare, "refs/heads/main"), local);
        let parents = run(&repo, &["git", "log", "-1", "--format=%P", "main"]);
        assert!(
            parents.trim().split(' ').nth(1) == Some(remote_head.as_str()),
            "parents {parents:?}"
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&repo).join("README.md")).expect("readme"),
            "# proj\nremote\n"
        );
        assert!(Path::new(&repo).join("pulse_worker_1.txt").is_file());
        let message = run(&repo, &["git", "log", "-1", "--format=%B", "main"]);
        assert!(
            message.starts_with(
                "Sync origin/main into main: main's own 2 commits re-applied (1 carried, 1 set aside)\n"
            ),
            "message {message:?}"
        );
        assert!(
            message.contains(&format!(
                "Set aside, re-applied by a job:\n- {short} (local readme) (conflicts: README.md; patch at {}/patches/proj/resync-{short}.patch)",
                s.home
            )),
            "message {message:?}"
        );
        assert!(
            message.contains("Pulse-Landed-By: daemon"),
            "message {message:?}"
        );
        let patch = std::fs::read_to_string(
            Path::new(&s.home)
                .join("patches/proj")
                .join(format!("resync-{short}.patch")),
        )
        .expect("patch");
        assert!(
            patch.contains("local readme") && patch.contains("+local"),
            "{patch}"
        );
        assert!(
            run(&repo, &["git", "branch", "--list", "pulse/*"])
                .trim()
                .is_empty(),
            "a pulse branch stayed"
        );
        assert!(
            run(&repo, &["git", "worktree", "list"]).lines().count() == 1,
            "a worktree stayed: {}",
            run(&repo, &["git", "worktree", "list"])
        );
        // The job knows what to do and where the commit still is.
        assert_eq!(job.str("status"), "queued");
        assert_eq!(
            job.str("title"),
            format!("Re-apply {short} onto the synced main: local readme")
        );
        let brief = job.str("brief");
        assert!(
            brief.contains(&format!("`git show {short}`"))
                && brief.contains("clashed on README.md"),
            "{brief}"
        );
        for (action, want) in [
            ("land.conflict", 1),
            ("land.synced", 1),
            ("land.pushed", 1),
            ("land.held", 0),
        ] {
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                    &[&action]
                ),
                want,
                "{action}"
            );
        }
        let synced = must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'land.synced'",
            &[],
        );
        let d: serde_json::Value = serde_json::from_str(&synced.str("detail")).unwrap();
        assert_eq!(d["resync"]["commits"], 2, "{d}");
        assert_eq!(d["resync"]["carried"], 1, "{d}");
        assert_eq!(d["resync"]["conflicted"], 1, "{d}");
        assert_eq!(d["resync"]["picked"][0]["sha"], local_readme, "{d}");
        assert_eq!(d["resync"]["picked"][0]["fate"], "conflicted", "{d}");
        assert_eq!(d["resync"]["picked"][0]["job"], job.str("id"), "{d}");
        assert_eq!(d["resync"]["picked"][1]["fate"], "carried", "{d}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key = 'push-sync:proj'",
                &[]
            ),
            0
        );
        let finding = must_one(
            &s,
            "SELECT summary, body FROM posts WHERE kind = 'finding' AND area = 'landing'",
            &[],
        );
        assert!(
            finding.str("summary").starts_with(&format!(
                "sync of origin/main into proj's main: 1 of main's own 2 commits set aside for a job ({short})"
            )),
            "{}",
            finding.str("summary")
        );
        assert!(
            finding
                .str("body")
                .contains(&format!("job {} re-applies it", job.str("id"))),
            "{}",
            finding.str("body")
        );
        assert!(!push_owed(&s, "proj".into()));

        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert!(
            lines.is_empty() && cur.is_empty(),
            "quiet tick said {lines:?} {cur:?}"
        );
    }

    /// What the re-apply does with each kind of commit: one the upstream
    /// lacks is carried; one the upstream already holds is dropped; one that
    /// only changed a file the upstream removed is dropped with it; one that
    /// changed such a file and more keeps the rest.
    #[test]
    fn landing_resync_carries_drops_and_trims_the_default_branchs_own_commits() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        commit_file(&repo, "docs/log.md", "v1\n", "log v1");
        run(&repo, &["git", "push", "-q", "origin", "main"]);
        commit_file(&repo, "a.txt", "a\n", "add a");
        commit_file(&repo, "docs/log.md", "v2\n", "log v2");
        commit_file(&repo, "same.txt", "same\n", "add same");
        std::fs::write(Path::new(&repo).join("docs/log.md"), "v3\n").expect("write");
        commit_file(&repo, "d.txt", "d\n", "log v3 and d");
        let other = other_clone(&bare);
        run(&other, &["git", "rm", "-q", "docs/log.md"]);
        run(&other, &["git", "commit", "-q", "-m", "remove the log"]);
        commit_file(&other, "same.txt", "same\n", "add same upstream");
        commit_file(&other, "other.txt", "other\n", "other");
        run(&other, &["git", "push", "-q", "origin", "main"]);

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(
            lines[1].starts_with(
                "landing synced proj: origin/main merged into main (3 commits; main's own 5 commits re-applied: 3 carried, 2 already upstream; pulse/resync/origin/main landed on main as "
            ),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        for f in [
            "a.txt",
            "d.txt",
            "same.txt",
            "other.txt",
            "pulse_worker_1.txt",
        ] {
            assert!(Path::new(&repo).join(f).is_file(), "{f} missing");
        }
        assert!(!Path::new(&repo).join("docs/log.md").exists());
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        let synced = must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'land.synced'",
            &[],
        );
        let d: serde_json::Value = serde_json::from_str(&synced.str("detail")).unwrap();
        let fates: Vec<(String, String, Vec<String>)> = d["resync"]["picked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["subject"].as_str().unwrap().to_string(),
                    p["fate"].as_str().unwrap().to_string(),
                    p["files"]
                        .as_array()
                        .map(|a| a.iter().map(|f| f.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            fates,
            vec![
                ("add a".to_string(), "carried".to_string(), vec![]),
                (
                    "log v2".to_string(),
                    "dropped".to_string(),
                    vec!["docs/log.md".to_string()]
                ),
                ("add same".to_string(), "dropped".to_string(), vec![]),
                (
                    "log v3 and d".to_string(),
                    "carried".to_string(),
                    vec!["docs/log.md".to_string()]
                ),
                (
                    "work on pulse/worker/1".to_string(),
                    "carried".to_string(),
                    vec![]
                ),
            ]
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source LIKE 'resync:%'",
                &[]
            ),
            0
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND area = 'landing'",
                &[]
            ),
            0
        );
        let message = run(&repo, &["git", "log", "-1", "--format=%B", "main"]);
        assert!(
            message.contains("(dropped: docs/log.md, which origin/main removed)")
                && message.contains("Already in origin/main, dropped:"),
            "message {message:?}"
        );
    }

    /// The check that guards a sync. Red when a `red-*.txt` file sits in the
    /// tree, naming it; red for a second reason when `on.txt` is there too.
    fn red_check(repo: &str) {
        commit_file(
            repo,
            "check.sh",
            "rc=0\nfor f in red-*.txt; do [ -f \"$f\" ] && { echo \"FAIL $f\"; rc=1; }; done\n[ -f red-a.txt ] && [ -f on.txt ] && { echo \"FAIL combo\"; rc=1; }\nexit $rc\n",
            "check",
        );
        run(repo, &["git", "push", "-q", "origin", "main"]);
    }

    /// The forge's head fails the project's check by itself. The sync's
    /// merged result fails the same way, so it lands all the same: holding it
    /// would only keep the checkout from holding the commit a fix must start
    /// from.
    #[test]
    fn landing_sync_carries_the_upstreams_own_red_in() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        red_check(&repo);
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = 'sh check.sh' WHERE slug = 'proj'",
            &[],
        );
        let proj = crate::board::project_by_slug(&s.conn().unwrap(), proj.str("slug"));
        let other = other_clone(&bare);
        commit_file(&other, "red-a.txt", "red\n", "breaks the check");
        run(&other, &["git", "push", "-q", "origin", "main"]);
        let remote_head = bare_ref(&bare, "refs/heads/main");

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(
            lines[1].starts_with("landing synced proj: origin/main merged into main (1 commit; ")
                && lines[1].contains(&format!(
                    "verify exit 1 on the merged result and on the upstream {} alike, so the sync carries the forge's red in for the pipeline fix",
                    crate::recipe::short(&remote_head, 12)
                )),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        assert!(Path::new(&repo).join("red-a.txt").is_file());
        let red = must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'land.upstream-red'",
            &[],
        );
        let d: serde_json::Value = serde_json::from_str(&red.str("detail")).unwrap();
        assert_eq!(d["upstream"], remote_head, "{d}");
        assert_eq!(d["failures"], serde_json::json!(["FAIL red-a.txt"]), "{d}");
        for action in ["land.failed-verify", "land.base-red", "land.held"] {
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                    &[&action]
                ),
                0,
                "{action}"
            );
        }
        // No green memo was written for the red merged tree.
        let merged_tree = run(&repo, &["git", "rev-parse", "main^{tree}"])
            .trim()
            .to_string();
        assert!(
            !crate::verify_memo::path(&s.home, "proj", &merged_tree).exists(),
            "a verify memo was written for the red merge"
        );
    }

    /// A merged result that fails for more than the upstream's own reason is
    /// the merge's fault: the sync holds as before.
    #[test]
    fn landing_sync_still_holds_when_the_merge_fails_more_than_the_upstream() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        red_check(&repo);
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = 'sh check.sh' WHERE slug = 'proj'",
            &[],
        );
        let proj = crate::board::project_by_slug(&s.conn().unwrap(), proj.str("slug"));
        commit_file(&repo, "on.txt", "on\n", "local on");
        let other = other_clone(&bare);
        commit_file(&other, "red-a.txt", "red\n", "breaks the check");
        run(&other, &["git", "push", "-q", "origin", "main"]);
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
        let note = &cur["proj"];
        assert!(
            note.contains(
                "merging them failed-verify: verify exit 1; the merge fails more than the upstream "
            ) && note.contains(" does (FAIL combo) on the merged main; branch removed; patch at "),
            "note {note:?}"
        );
        assert_eq!(bare_ref(&bare, "refs/heads/main"), remote_head);
        assert!(!Path::new(&repo).join("red-a.txt").exists());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key = 'push-sync:proj'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.upstream-red'",
                &[]
            ),
            0
        );
    }

    /// The owner squashed the remote by hand to one root commit holding what
    /// pulse had pushed. Merging that into main conflicts on every file; the
    /// landed commits are copied onto the squash instead, main moves to the
    /// copy with its tree unchanged, and the push is a fast-forward.
    #[test]
    fn landing_replays_onto_a_squashed_upstream_then_pushes() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        let pushed = ref_of(&repo, "refs/heads/main");
        commit_file(&repo, "README.md", "# proj\nlocal\n", "local readme");
        run(
            &repo,
            &[
                "git",
                "commit",
                "-q",
                "--allow-empty",
                "--author=someone <someone@example.com>",
                "-m",
                "an empty commit by someone else",
            ],
        );
        let before = ref_of(&repo, "refs/heads/main");
        // The squash: a root commit with the pushed tip's tree, force-pushed.
        let other = other_clone(&bare);
        let squash = squash_remote(&other, "Squashed history: full project state");
        assert_eq!(bare_ref(&bare, "refs/heads/main"), squash);
        assert_eq!(
            run(&other, &["git", "rev-parse", &format!("{squash}^{{tree}}")]),
            run(&repo, &["git", "rev-parse", &format!("{pushed}^{{tree}}")]),
            "the fixture's squash must hold the pushed tree"
        );

        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1].starts_with(&format!(
                "landing synced proj: origin/main is a squash of main's {}; 3 commits copied onto it with their trees, authors and messages (",
                crate::recipe::short(&pushed, 12)
            )),
            "lines {lines:?}"
        );
        assert!(
            lines[2].starts_with("landing pushed proj: main -> origin/main (3 commits, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        let now = ref_of(&repo, "refs/heads/main");
        assert_ne!(now, before);
        assert_eq!(bare_ref(&bare, "refs/heads/main"), now);
        assert!(
            run(&repo, &["git", "ls-tree", "--name-only", "main"])
                .lines()
                .any(|f| f == "pulse_worker_1.txt"),
            "the landed work is in the tree"
        );
        assert_eq!(
            run(
                &repo,
                &[
                    "git",
                    "rev-list",
                    "--max-parents=0",
                    "--first-parent",
                    "main"
                ]
            )
            .trim(),
            squash.trim(),
            "the squash is now main's root"
        );
        assert_eq!(
            run(
                &repo,
                &["git", "rev-list", "--count", "--first-parent", "main"]
            )
            .trim(),
            "4"
        );
        let author = run(&repo, &["git", "log", "-1", "--format=%an", &pushed])
            .trim()
            .to_string();
        assert_eq!(
            run(
                &repo,
                &["git", "log", "--format=%an %s", "--first-parent", "main"]
            ),
            format!(
                "{author} work on pulse/worker/1\nsomeone an empty commit by someone else\n{author} local readme\n{} Squashed history: full project state\n",
                run(&other, &["git", "log", "-1", "--format=%an", &squash]).trim()
            ),
            "authors and messages travel with the copies"
        );
        assert_eq!(
            run(&repo, &["git", "log", "-1", "--format=%cn", "main"]).trim(),
            "pulse",
            "the copies are pulse's commits"
        );
        assert!(
            run(&repo, &["git", "status", "--porcelain"])
                .trim()
                .is_empty(),
            "the checkout stays clean"
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&repo).join("README.md")).expect("readme"),
            "# proj\nlocal\n"
        );
        assert!(
            run(&repo, &["git", "branch", "--list", "pulse/sync/*"])
                .trim()
                .is_empty(),
            "no sync branch is left"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.synced' AND subject = 'proj' AND detail LIKE '%\"replayed\": 3%'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.conflict'",
                &[]
            ),
            0
        );
        assert!(!push_owed(&s, "proj".into()));
    }

    /// A conflict remembered by an earlier pass for the same pair of shas
    /// does not stop the replay: the memo is what the merge path left behind.
    #[test]
    fn landing_replays_over_a_remembered_sync_conflict() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        let pushed = ref_of(&repo, "refs/heads/main");
        // Landed with the push off: one merge commit on main, a push owed.
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, _) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert!(push_owed(&s, "proj".into()));
        let landed = ref_of(&repo, "refs/heads/main");
        // Meanwhile the remote became a squash of what had been pushed.
        let other = other_clone(&bare);
        let squash = squash_remote(&other, "Squashed history");
        assert_eq!(bare_ref(&bare, "refs/heads/main"), squash);
        assert_eq!(
            run(&other, &["git", "rev-parse", &format!("{squash}^{{tree}}")]),
            run(&repo, &["git", "rev-parse", &format!("{pushed}^{{tree}}")])
        );
        // What an earlier pass's merge attempt would have left for this pair.
        let data = pulse_store::gojson::to_string(&serde_json::json!({
            "upstream": squash, "base": landed, "state": "conflict", "note": "remembered conflict", "at": TEST_NOW
        }))
        .expect("memo");
        exec1(
            &s,
            "INSERT INTO meta (key, value) VALUES ('push-sync:proj', ?)",
            &[&data],
        );

        set_setting(&s, "landing.push", "main");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(
            lines[0].starts_with(&format!(
                "landing synced proj: origin/main is a squash of main's {}; 1 commit copied onto it",
                crate::recipe::short(&pushed, 12)
            )),
            "lines {lines:?}"
        );
        assert!(
            lines[1].starts_with("landing pushed proj: main -> origin/main (1 commit, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key = 'push-sync:proj'",
                &[]
            ),
            0,
            "the memo of the old conflict is gone"
        );
        assert_eq!(
            run(
                &repo,
                &["git", "rev-list", "--count", "--first-parent", "main"]
            )
            .trim(),
            "2"
        );
        assert_eq!(
            run(&repo, &["git", "log", "-1", "--format=%s", "main"]).trim(),
            "work on pulse/worker/1"
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        assert!(!push_owed(&s, "proj".into()));
    }

    /// A loopback http server that answers every request 401, the way a
    /// forge does before credentials; git then wants a username.
    fn http_401_server() -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut st, _)) => {
                        let _ = st.set_nonblocking(false);
                        let _ = st.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut buf = [0u8; 8192];
                        let _ = st.read(&mut buf);
                        let _ = st.write_all(
                            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"pulse\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        (port, stop)
    }

    /// Sends `http://example.invalid/x/y.git` to the 401 server (as git's http
    /// proxy) and makes `git@example.invalid:x/y.git` reach `bare` through a
    /// fake ssh.
    fn https_push_fixture(
        s: &pulse_store::Store,
    ) -> (
        crate::util::Row,
        String,
        String,
        String,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let (proj, repo, bare) = push_fixture(s);
        let (port, stop) = http_401_server();
        run(
            &repo,
            &[
                "git",
                "remote",
                "set-url",
                "--push",
                "origin",
                "http://example.invalid/x/y.git",
            ],
        );
        run(
            &repo,
            &[
                "git",
                "config",
                "http.proxy",
                &format!("http://127.0.0.1:{port}"),
            ],
        );
        let home = tempfile::tempdir().expect("ssh home").keep();
        std::fs::create_dir_all(home.join("x")).expect("x");
        std::os::unix::fs::symlink(&bare, home.join("x").join("y.git")).expect("link bare");
        let ssh = home.join("ssh");
        std::fs::write(
            &ssh,
            format!(
                "#!/bin/sh\ncd '{}' || exit 255\nfor a in \"$@\"; do last=\"$a\"; done\nexec sh -c \"$last\"\n",
                home.display()
            ),
        )
        .expect("write ssh");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755))
                .expect("chmod ssh");
        }
        (proj, repo, bare, ssh.display().to_string(), stop)
    }

    #[test]
    fn landing_moves_an_https_push_url_to_ssh_when_the_key_answers() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare, ssh, stop) = https_push_fixture(&s);
        let _env_1 = pulse_testutil::setenv("GIT_SSH_COMMAND", &ssh);
        set_setting(&s, "landing.push", "main");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(lines.len(), 3, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert_eq!(
            lines[1],
            "landing push url proj: origin pushes over ssh now (git@example.invalid:x/y.git); http://example.invalid/x/y.git took a password the daemon has none of"
        );
        assert!(
            lines[2].starts_with("landing pushed proj: main -> origin/main (1 commit, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes: {cur:?}");
        assert_eq!(
            run(&repo, &["git", "remote", "get-url", "--push", "origin"]).trim(),
            "git@example.invalid:x/y.git"
        );
        assert_eq!(
            run(&repo, &["git", "remote", "get-url", "origin"]).trim(),
            bare,
            "the fetch url is the owner's and stays"
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.push-url' AND json_extract(detail, '$.to') = 'git@example.invalid:x/y.git'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn landing_https_push_url_without_a_key_names_the_fix() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (proj, repo, bare, _ssh, stop) = https_push_fixture(&s);
        let _env_1 = pulse_testutil::setenv("GIT_SSH_COMMAND", "false");
        set_setting(&s, "landing.push", "main");
        let before = bare_ref(&bare, "refs/heads/main");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        let note = &cur["proj"];
        assert!(
            note.starts_with("1 commit landed but not pushed; push failed: "),
            "note {note:?}"
        );
        assert!(
            note.contains("could not read Username for 'http://example.invalid'"),
            "note {note:?}"
        );
        assert!(
            note.contains(
                "; http://example.invalid/x/y.git takes a password the daemon has none of, and git@example.invalid:x/y.git does not answer to the daemon's key ("
            ),
            "note {note:?}"
        );
        assert!(
            note.contains(&format!(
                "fix: git -C {repo} remote set-url --push origin git@example.invalid:x/y.git with a key the daemon has"
            )),
            "note {note:?}"
        );
        assert_eq!(
            run(&repo, &["git", "remote", "get-url", "--push", "origin"]).trim(),
            "http://example.invalid/x/y.git",
            "a url that does not answer over ssh stays"
        );
        assert_eq!(bare_ref(&bare, "refs/heads/main"), before);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.push-url'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn ssh_form_names_github_style_urls_only() {
        for (url, want) in [
            (
                "https://github.com/lamplitlabs/eszett.git",
                Some("git@github.com:lamplitlabs/eszett.git"),
            ),
            (
                "https://github.com/lamplitlabs/eszett",
                Some("git@github.com:lamplitlabs/eszett.git"),
            ),
            (
                "https://me:tok@github.com/o/r.git",
                Some("git@github.com:o/r.git"),
            ),
            ("http://forge.local/o/r/", Some("git@forge.local:o/r.git")),
            ("https://host:8443/o/r.git", None),
            ("https://github.com/o/r/extra.git", None),
            ("https://github.com/o", None),
            ("git@github.com:o/r.git", None),
            ("/srv/git/r.git", None),
        ] {
            assert_eq!(ssh_form(url).as_deref(), want, "{url}");
        }
        assert_eq!(
            shown_url("https://me:tok@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            shown_url("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            shown_url("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        assert!(no_http_credential(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        assert!(no_http_credential(
            "fatal: unable to get password from user"
        ));
        assert!(!no_http_credential(
            "! [rejected] main -> main (fetch first)"
        ));
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

        // The owner's edits are back in the checkout, not buried in `git stash list`.
        let stash_list = run(&dir, &["git", "stash", "list"]);
        if stash_list.trim() != "" {
            panic!("auto-stash left behind after a green landing: {stash_list:?}");
        }
        let status = run(
            &dir,
            &["git", "status", "--porcelain", "--untracked-files=no"],
        );
        if status.trim_end() != " M README.md" {
            panic!("owner's README.md edit not restored after landing: {status:?}");
        }
        let readme = std::fs::read_to_string(std::path::Path::new(&dir).join("README.md")).unwrap();
        if !readme.contains("owner's work") {
            panic!("README.md lost the owner's edit: {readme:?}");
        }
        if !std::path::Path::new(&dir).join("feature.txt").exists() {
            panic!("landed commit missing from the checkout");
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
        exec1(
            &s,
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('landing.base_red_retry_minutes', '3', ?, 'test')
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            &[&TEST_NOW],
        );

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
            || !note.contains("No ")
            || !note.contains("waits until 2026-09-06T10:03:00Z")
            || !note.contains("retry in 3m")
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
        let later = crate::util::plus_minutes(TEST_NOW, 4);
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
            None,
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
