//! Landed work delivered as a pull request: the default branch pushed to the
//! PULSE-owned `pulse/landed` branch on the remote and one pull request kept
//! open from it into the upstream branch. The daemon takes this path when
//! `landing.push` is `pr`, or once when the owner asked for it from a task's
//! sheet or with `pulse land --pr` (`land.pr-asked`). ADR-037 pushed the
//! default branch itself and set pull requests aside; the owner asked for
//! them on 2026-10-30, when a landing sat behind a diverged upstream for
//! hours with nothing on the forge to show for it.
//!
//! The push is a fast-forward of `pulse/landed` and nothing else: a branch
//! someone else moved is never overwritten, the daemon holds and says so.
//! The pull request is opened with `gh`, the forge CLI the owner already
//! logs in to on this machine; its auth never reaches git.

use std::time::Duration;

pub(crate) use pulse_tasks::PR_HEAD;

/// `landing.push=pr`: every landing is delivered as a pull request.
pub(crate) fn pr_mode(s: &pulse_store::Store) -> bool {
    s.setting("landing.push", "off") == "pr"
}

/// `OWNER/REPO` (or `HOST/OWNER/REPO` off github.com) for `gh --repo`, from
/// a GitHub-style ssh or http(s) url; None for anything else.
pub(crate) fn repo_slug(url: &str) -> Option<String> {
    let (host, path) = if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.rsplit('@').next().unwrap_or(rest);
        rest.split_once('/')?
    } else if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let (host, path) = rest.split_once('/')?;
        (host.rsplit('@').next().unwrap_or(host), path)
    } else {
        let (userhost, path) = url.split_once(':')?;
        (userhost.rsplit('@').next().unwrap_or(userhost), path)
    };
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    if host.is_empty() || host.contains('/') {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segs = path.split('/');
    let owner = segs.next().unwrap_or_default();
    let repo = segs.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || segs.next().is_some() {
        return None;
    }
    Some(if host == "github.com" {
        format!("{owner}/{repo}")
    } else {
        format!("{host}/{owner}/{repo}")
    })
}

/// Runs `gh` in the project checkout (so it finds the repository from the
/// remotes when `repo` is empty) with the daemon's own environment: `gh`
/// keeps its login in its own config, nothing of it reaches git. Returns
/// stdout, or the failure put into words.
fn gh(root: &str, repo: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut words = vec!["gh".to_string()];
    words.extend(args.iter().map(|a| crate::util::shell_quote(a)));
    if !repo.is_empty() {
        words.push("--repo".into());
        words.push(crate::util::shell_quote(repo));
    }
    let what = format!("gh {} {}", args[0], args[1]);
    let (code, out, err) = crate::land::sh_with_timeout(root.to_string(), words.join(" "), timeout);
    match code.as_deref() {
        Some(0) => Ok(out),
        Some(_) => Err(gh_failure(&what, &format!("{err} {out}"))),
        None => Err(format!(
            "{what} did not finish within {}s",
            timeout.as_secs()
        )),
    }
}

/// The pull request `gh` reports open from `head` into `base`, as
/// (number, url).
fn open_pull_request(
    root: &str,
    repo: &str,
    head: &str,
    base: &str,
) -> Result<Option<(i64, String)>, String> {
    let out = gh(
        root,
        repo,
        &[
            "pr",
            "list",
            "--head",
            head,
            "--base",
            base,
            "--state",
            "open",
            "--json",
            "number,url",
            "--limit",
            "1",
        ],
        Duration::from_secs(60),
    )?;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(out.trim()).map_err(|e| format!("gh pr list answered oddly: {e}"))?;
    Ok(list.first().map(|v| {
        (
            v.get("number").and_then(|n| n.as_i64()).unwrap_or(0),
            v.get("url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
        )
    }))
}

fn gh_failure(what: &str, why: &str) -> String {
    let why = why.trim();
    if why.contains("No such file or directory")
        || (why.contains("not found") && why.contains("gh"))
    {
        return "gh (the GitHub CLI) is not installed where the daemon can run it; install it and run gh auth login as the user the daemon runs as".to_string();
    }
    if why.contains("gh auth login") || why.contains("not logged in") {
        return format!(
            "{what} failed: gh is not logged in for the user the daemon runs as; run gh auth login there"
        );
    }
    let squeezed: String = why.split_whitespace().collect::<Vec<_>>().join(" ");
    let squeezed = if squeezed.chars().count() > 200 {
        squeezed.chars().take(199).collect::<String>() + "~"
    } else {
        squeezed
    };
    format!("{what} failed: {squeezed}")
}

/// The title and body of a new pull request: the landings the upstream has
/// not seen - every `land.landed` of the project since its last push or
/// pull request - with their jobs, commits and branches.
fn pull_request_text(
    s: &pulse_store::Store,
    slug: &str,
    def: &str,
    base: &str,
    commits: i32,
) -> (String, String) {
    let mut landings: Vec<(String, String, String)> = Vec::new();
    if let Ok(conn) = s.conn() {
        let since: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM events WHERE action IN ('land.pushed', 'land.pr') AND subject = ?",
                [slug],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if let Ok(mut stmt) = conn.prepare(
            "SELECT subject, detail FROM events WHERE action = 'land.landed' \
             AND json_extract(detail, '$.project') = ? AND seq > ? ORDER BY seq",
        ) && let Ok(rows) = stmt.query_map(rusqlite::params![slug, since], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for (branch, detail) in rows.flatten() {
                let v: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
                let title = v
                    .pointer("/work/job/title")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                let sha = v
                    .pointer("/code_delivery/after")
                    .and_then(|a| a.as_str())
                    .unwrap_or_default()
                    .to_string();
                landings.push((title, branch, sha));
            }
        }
    }
    let title = match landings.as_slice() {
        [(title, branch, _)] => {
            let t = if title.is_empty() {
                branch.rsplit('/').next().unwrap_or(branch).to_string()
            } else {
                title.clone()
            };
            crate::util::trunc(&t, 120)
        }
        [] => format!(
            "pulse: {commits} {} landed in {slug}",
            crate::util::plural(commits, "commit")
        ),
        many => format!("pulse: {} changes landed in {slug}", many.len()),
    };
    let mut body = format!(
        "Work PULSE landed on `{def}` of {slug} after the project's own check passed, pushed here as `{PR_HEAD}` for review and merge into `{base}`.\n"
    );
    if !landings.is_empty() {
        body.push('\n');
        for (title, branch, sha) in &landings {
            let short: String = sha.chars().take(12).collect();
            body.push_str(&format!(
                "- {}{} ({branch})\n",
                if title.is_empty() {
                    "landed"
                } else {
                    title.as_str()
                },
                if short.is_empty() {
                    String::new()
                } else {
                    format!(" `{short}`")
                }
            ));
        }
    }
    body.push_str(
        "\nPULSE pushes later landings to the same branch until this is merged. Merge with a merge commit so the next pull request shows only what is new.\n",
    );
    (title, body)
}

/// Pushes the default branch to `pulse/landed` on the remote and makes sure
/// a pull request is open from it. `n` commits are ahead of the upstream;
/// `conflict` is the sync hold when merging the upstream into the default
/// branch conflicted (the pull request then shows the conflict for the
/// owner to resolve on the forge). Returns the log lines and the hold note,
/// empty when the pull request stands.
pub(crate) fn deliver(
    s: &pulse_store::Store,
    t: &crate::landing_hold::PushTarget,
    actor: &str,
    now: &str,
    n: i32,
    conflict: Option<String>,
) -> (Vec<String>, String) {
    let commits = crate::util::plural(n, "commit");
    let head_ref = format!("refs/heads/{PR_HEAD}");
    let tracking = format!("refs/remotes/{}/{PR_HEAD}", t.remote);
    let def_ref = format!("refs/heads/{}", t.def);
    let git = |timeout: u64, args: &[&str]| {
        crate::util::git_timeout_env(&t.root, Duration::from_secs(timeout), &t.env, args)
    };
    let rev = |r: &str| {
        git(10, &["rev-parse", "--verify", "-q", r])
            .stdout
            .trim()
            .to_string()
    };
    let mut lines = Vec::new();
    let fetched = git(
        120,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            &t.url,
            &format!("+{head_ref}:{tracking}"),
        ],
    );
    let remote_head = if fetched.ok() {
        rev(&tracking)
    } else if fetched.stderr.contains("couldn't find remote ref") {
        git(10, &["update-ref", "-d", &tracking]);
        String::new()
    } else {
        let why = if let Some(e) = &fetched.err {
            e.clone()
        } else {
            crate::run::squeeze(&format!("{} {}", fetched.stderr, fetched.stdout), 200)
        };
        return (
            lines,
            format!(
                "{n} {commits} landed but not delivered; fetch of {PR_HEAD} from {} failed: {}",
                crate::landing_hold::shown_url(&t.url),
                pulse_cred::redact(&why, &[])
            ),
        );
    };
    let to = rev(&def_ref);
    if !remote_head.is_empty()
        && remote_head != to
        && !git(30, &["merge-base", "--is-ancestor", &remote_head, &to]).ok()
    {
        return (
            lines,
            format!(
                "{n} {commits} landed but not delivered; {}/{PR_HEAD} has commits {} does not (someone pushed to the pull request, or {} was rewritten); merge or close that pull request and delete the branch, pulse pushes it again then",
                t.remote, t.def, t.def
            ),
        );
    }
    let pushed_now = remote_head != to;
    if pushed_now {
        let pushed = git(180, &["push", &t.remote, &format!("{def_ref}:{head_ref}")]);
        if !pushed.ok() {
            let why = if let Some(e) = pushed.err {
                e
            } else {
                crate::run::squeeze(&format!("{} {}", pushed.stderr, pushed.stdout), 300)
            };
            return (
                lines,
                format!(
                    "{n} {commits} landed but not pushed; push of {PR_HEAD} failed: {}",
                    pulse_cred::redact(&why, &[])
                ),
            );
        }
    }
    // A GitHub-style url names the repository outright (the push url may
    // differ from the fetch url gh would read); any other url is left to gh,
    // which reads the checkout's remotes itself.
    let repo = repo_slug(&t.url).unwrap_or_default();
    let existing = match open_pull_request(&t.root, &repo, PR_HEAD, &t.remote_branch) {
        Ok(v) => v,
        Err(why) => {
            return (
                lines,
                format!(
                    "{n} {commits} pushed to {}/{PR_HEAD} but no pull request yet: {why}",
                    t.remote
                ),
            );
        }
    };
    let (number, url, opened) = match existing {
        Some((number, url)) if !pushed_now => {
            // An ask for what the pull request already carries.
            let why = format!(
                "{}/{PR_HEAD} already has {}; pull request #{number} is open",
                t.remote, t.def
            );
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    now,
                    actor,
                    "land.pr-answered",
                    &t.slug,
                    serde_json::json!({"project": t.slug, "why": why, "number": number, "url": url}),
                );
            }
            lines.push(format!(
                "landing pull request {}: nothing new to deliver, {why}{}",
                t.slug,
                if url.is_empty() {
                    String::new()
                } else {
                    format!(" {url}")
                }
            ));
            return (lines, String::new());
        }
        Some((number, url)) => (number, url, false),
        None => {
            let (title, body) = pull_request_text(s, &t.slug, &t.def, &t.remote_branch, n);
            match gh(
                &t.root,
                &repo,
                &[
                    "pr",
                    "create",
                    "--head",
                    PR_HEAD,
                    "--base",
                    &t.remote_branch,
                    "--title",
                    &title,
                    "--body",
                    &body,
                ],
                Duration::from_secs(90),
            ) {
                Ok(out) => {
                    let url = out
                        .lines()
                        .rev()
                        .map(str::trim)
                        .find(|l| l.starts_with("http"))
                        .unwrap_or_default()
                        .to_string();
                    let number = url
                        .rsplit('/')
                        .next()
                        .and_then(|n| n.parse::<i64>().ok())
                        .unwrap_or(0);
                    (number, url, true)
                }
                Err(why) => {
                    return (
                        lines,
                        format!(
                            "{n} {commits} pushed to {}/{PR_HEAD} but no pull request yet: {why}",
                            t.remote
                        ),
                    );
                }
            }
        }
    };
    let from = rev(&t.upstream);
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.pr",
            &t.slug,
            serde_json::json!({
                "project": t.slug, "remote": t.remote, "branch": t.def, "head": PR_HEAD,
                "base": t.remote_branch, "number": number, "url": url, "from": from, "to": to,
                "commits": n, "opened": opened, "conflicts": conflict.is_some(),
            }),
        );
    }
    let mut line = format!(
        "landing pull request {}: {} -> {}/{PR_HEAD}, #{number} {} ({n} {commits}, {}..{})",
        t.slug,
        t.def,
        t.remote,
        if opened { "opened" } else { "updated" },
        crate::recipe::short(&from, 12),
        crate::recipe::short(&to, 12)
    );
    if !url.is_empty() {
        line.push_str(&format!(" {url}"));
    }
    if let Some(c) = conflict {
        line.push_str(&format!(
            "; it conflicts with {}/{}: {c}",
            t.remote, t.remote_branch
        ));
    }
    lines.push(line);
    (lines, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::*;
    use std::collections::BTreeMap;

    /// A `gh` on PATH that answers like GitHub would: `pr list` finds no
    /// pull request until `pr create` ran once (then #7), every call is
    /// written to `log`. `fail` makes every call exit 1 with an HTTP 403.
    fn fake_gh(fail: bool) -> (tempfile::TempDir, pulse_testutil::SetenvGuard, String) {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("gh.log").display().to_string();
        let marker = bin.path().join("created").display().to_string();
        let body = if fail {
            format!(
                "#!/bin/sh\necho \"$@\" >> '{log}'\necho 'HTTP 403: Resource not accessible by integration' >&2\nexit 1\n"
            )
        } else {
            format!(
                "#!/bin/sh\necho \"$@\" >> '{log}'\ncase \"$1 $2\" in\n\"pr list\") if [ -e '{marker}' ]; then echo '[{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\"}}]'; else echo '[]'; fi ;;\n\"pr create\") touch '{marker}'; echo 'https://github.com/o/r/pull/7' ;;\n*) echo \"unexpected gh $*\" >&2; exit 2 ;;\nesac\n"
            )
        };
        let path = bin.path().join("gh");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let guard = pulse_testutil::setenv(
            "PATH",
            format!(
                "{}:{}",
                bin.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        (bin, guard, log)
    }

    fn gh_calls(log: &str) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("pr "))
            .map(|l| l.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn tend(
        s: &pulse_store::Store,
        proj: &crate::util::Row,
    ) -> (Vec<String>, BTreeMap<String, String>) {
        crate::landing_hold::tend_landings(
            s,
            vec![proj.clone()],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        )
    }

    #[test]
    fn landing_push_pr_opens_one_pull_request_and_keeps_it_current() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (_bin, _path, log) = fake_gh(false);
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "pr");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let main_before = bare_ref(&bare, "refs/heads/main");

        let (lines, cur) = tend(&s, &proj);
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(lines[0].starts_with("landing landed"), "lines {lines:?}");
        assert!(
            lines[1].starts_with(
                "landing pull request proj: main -> origin/pulse/landed, #7 opened (1 commit, "
            ) && lines[1].ends_with(" https://github.com/o/r/pull/7"),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "hold notes after a pull request: {cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            main_before,
            "main itself is never pushed in pr mode"
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/pulse/landed"),
            ref_of(&repo, "refs/heads/main")
        );
        assert_eq!(gh_calls(&log), ["pr list", "pr create"]);
        let detail: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT detail FROM events WHERE action = 'land.pr' AND subject = 'proj'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(v["opened"], true, "{detail}");
        assert_eq!(v["number"], 7, "{detail}");
        assert_eq!(v["base"], "main", "{detail}");
        assert_eq!(v["head"], PR_HEAD, "{detail}");
        assert!(!crate::landing_hold::push_owed(&s, "proj".into()));
        let (lines, cur) = tend(&s, &proj);
        assert!(
            lines.is_empty() && cur.is_empty(),
            "quiet tick said {lines:?} {cur:?}"
        );

        push_branch(&s, &proj, &repo, "pulse/worker/2");
        let (lines, cur) = tend(&s, &proj);
        assert_eq!(lines.len(), 2, "lines {lines:?}");
        assert!(
            lines[1].contains("#7 updated (2 commits, "),
            "lines {lines:?}"
        );
        assert!(cur.is_empty(), "{cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/pulse/landed"),
            ref_of(&repo, "refs/heads/main")
        );
        assert_eq!(
            gh_calls(&log),
            ["pr list", "pr create", "pr list"],
            "one pull request, updated"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pr'",
                &[]
            ),
            2
        );
    }

    #[test]
    fn an_ask_delivers_a_pull_request_with_push_off_and_answers_when_nothing_is_ahead() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (_bin, _path, log) = fake_gh(false);
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (lines, cur) = tend(&s, &proj);
        assert_eq!(
            lines.len(),
            1,
            "push off lands and stops: {lines:?} {cur:?}"
        );
        assert!(crate::landing_hold::push_owed(&s, "proj".into()));
        assert!(gh_calls(&log).is_empty());

        pulse_tasks::ask_pull_request(&s, "proj", "TASK-001", "human").unwrap();
        assert!(pulse_tasks::pull_request_asked(&s, "proj"));
        let (lines, cur) = tend(&s, &proj);
        assert_eq!(lines.len(), 1, "lines {lines:?}");
        assert!(lines[0].contains("#7 opened"), "lines {lines:?}");
        assert!(cur.is_empty(), "{cur:?}");
        assert_eq!(
            bare_ref(&bare, "refs/heads/pulse/landed"),
            ref_of(&repo, "refs/heads/main")
        );
        assert!(
            !pulse_tasks::pull_request_asked(&s, "proj"),
            "the pull request answers the ask"
        );
        assert!(!crate::landing_hold::push_owed(&s, "proj".into()));

        // Asked again while the pull request already carries main: answered,
        // not "updated".
        pulse_tasks::ask_pull_request(&s, "proj", "TASK-001", "human").unwrap();
        let (lines, cur) = tend(&s, &proj);
        assert_eq!(
            lines,
            vec!["landing pull request proj: nothing new to deliver, origin/pulse/landed already has main; pull request #7 is open https://github.com/o/r/pull/7".to_string()],
            "{cur:?}"
        );
        assert!(
            !pulse_tasks::pull_request_asked(&s, "proj"),
            "the answer closes the ask"
        );
        assert_eq!(gh_calls(&log), ["pr list", "pr create", "pr list"]);

        // Asked once the upstream has everything: answered without gh.
        run(&repo, &["git", "push", "-q", "origin", "main"]);
        run(&repo, &["git", "fetch", "-q", "origin"]);
        pulse_tasks::ask_pull_request(&s, "proj", "TASK-001", "human").unwrap();
        let (lines, cur) = tend(&s, &proj);
        assert_eq!(
            lines,
            vec![
                "landing pull request proj: nothing to deliver, main has nothing origin/main lacks"
                    .to_string()
            ],
            "{cur:?}"
        );
        assert!(!pulse_tasks::pull_request_asked(&s, "proj"));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pr-answered'",
                &[]
            ),
            2
        );
        assert_eq!(gh_calls(&log).len(), 3);
    }

    #[test]
    fn a_pull_request_branch_someone_else_moved_holds_instead_of_being_overwritten() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (_bin, _path, _log) = fake_gh(false);
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "pr");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (_, cur) = tend(&s, &proj);
        assert!(cur.is_empty(), "{cur:?}");

        // A reviewer pushes a fix-up to the pull request's branch.
        let other = tempfile::tempdir().unwrap();
        let other = other.path().display().to_string();
        run(".", &["git", "clone", "-q", "-b", PR_HEAD, &bare, &other]);
        std::fs::write(format!("{other}/REVIEW.txt"), "fixed in review\n").unwrap();
        run(&other, &["git", "add", "REVIEW.txt"]);
        run(
            &other,
            &[
                "git",
                "-c",
                "user.name=r",
                "-c",
                "user.email=r@x",
                "commit",
                "-q",
                "-m",
                "review fix",
            ],
        );
        run(
            &other,
            &["git", "push", "-q", "origin", &format!("HEAD:{PR_HEAD}")],
        );
        let moved = bare_ref(&bare, &format!("refs/heads/{PR_HEAD}"));

        push_branch(&s, &proj, &repo, "pulse/worker/2");
        let (lines, cur) = tend(&s, &proj);
        assert!(lines[0].starts_with("landing landed"), "{lines:?}");
        let note = cur.get("proj").cloned().unwrap_or_default();
        assert!(
            note.contains(
                "landed but not delivered; origin/pulse/landed has commits main does not"
            ),
            "{note}"
        );
        assert_eq!(
            bare_ref(&bare, &format!("refs/heads/{PR_HEAD}")),
            moved,
            "never overwritten"
        );
        assert!(
            crate::landing_hold::push_owed(&s, "proj".into()),
            "still owed"
        );
    }

    #[test]
    fn a_gh_failure_leaves_the_push_and_says_why() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (_bin, _path, _log) = fake_gh(true);
        let s = new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "pr");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let (_, cur) = tend(&s, &proj);
        let note = cur.get("proj").cloned().unwrap_or_default();
        assert!(
            note.contains("1 commit pushed to origin/pulse/landed but no pull request yet: gh pr list failed: HTTP 403"),
            "{note}"
        );
        assert_eq!(
            bare_ref(&bare, &format!("refs/heads/{PR_HEAD}")),
            ref_of(&repo, "refs/heads/main"),
            "the push stands, the pull request is retried"
        );
        assert!(crate::landing_hold::push_owed(&s, "proj".into()));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.pr'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn repo_slug_reads_github_style_urls() {
        for (url, want) in [
            ("git@github.com:owner/repo.git", Some("owner/repo")),
            ("git@github.com:owner/repo", Some("owner/repo")),
            ("https://github.com/owner/repo.git", Some("owner/repo")),
            ("https://me@github.com/owner/repo/", Some("owner/repo")),
            ("ssh://git@github.com/owner/repo.git", Some("owner/repo")),
            (
                "ssh://git@ghe.corp:2222/owner/repo.git",
                Some("ghe.corp/owner/repo"),
            ),
            ("https://ghe.corp/owner/repo", Some("ghe.corp/owner/repo")),
            ("/tmp/origin.git", None),
            ("https://github.com/owner", None),
            ("https://github.com/owner/repo/deeper", None),
        ] {
            assert_eq!(repo_slug(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn gh_failure_names_a_missing_gh() {
        assert!(
            gh_failure("gh pr list", "No such file or directory (os error 2)")
                .contains("gh auth login")
        );
        assert_eq!(
            gh_failure(
                "gh pr create",
                "  HTTP 403: Resource not accessible  \n by integration "
            ),
            "gh pr create failed: HTTP 403: Resource not accessible by integration"
        );
    }
}
