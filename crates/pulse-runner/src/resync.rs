//! Re-applying the default branch's own commits onto an upstream that
//! conflicts with them.
//!
//! The landing path merges `<remote>/<branch>` into the default branch when
//! the forge moved under it (`landing_hold::sync_upstream`). A merge that
//! conflicted used to hold the push "until either side moves", and nothing
//! moved: the forge kept taking the owner's merges, the checkout stayed
//! behind them, workers kept cloning the stale checkout, and the project's
//! red pipeline on the forge was nobody's (leben-in-deutschland, 2026-10-30:
//! six landed commits held a day behind twenty-four upstream ones). The
//! upstream is the owner's published truth, so the conflict is resolved the
//! way a pull with a hand resolution would be. Each of the default branch's
//! own commits is re-applied onto the upstream tip in a throwaway worktree:
//! one the upstream already holds in another form becomes empty and is
//! dropped; one that only clashes on files the upstream removed loses those
//! files and keeps the rest; one that conflicts for real is set aside with
//! its patch and a job that re-applies what it meant. The tree that results
//! becomes one merge commit of both sides - the re-applied copies are only
//! the means to compute it - and that commit lands through
//! `land::land_branch` like any sync (the project's check, the
//! fast-forward of the checkout), so the push that follows is a fast-forward
//! and the checkout holds the forge's head: a red pipeline there is now
//! something a worker can start from.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most commits re-applied; a default branch further ahead of its
/// upstream than this keeps the hold and its by-hand line.
pub(crate) const RESYNC_MAX_COMMITS: usize = 200;
/// The jobs that re-apply a set-aside commit: `resync:<slug>:<short sha>`.
pub(crate) const JOB_SOURCE: &str = "resync";
const JOB_TIER: i32 = 1;
const JOB_PRIORITY: i32 = 60;

/// One commit of the default branch the upstream lacked, and how it fared.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Picked {
    pub sha: String,
    pub subject: String,
    /// `carried` (re-applied), `dropped` (the upstream already holds it) or
    /// `conflicted` (set aside for a job).
    pub fate: String,
    /// The files a carried commit lost because the upstream removed them;
    /// the files a conflicted commit clashed on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The patch of a conflicted commit, kept for the job that re-applies it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub patch: String,
    /// The job planned for a conflicted commit once the sync landed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job: String,
}

impl Picked {
    pub(crate) fn short(&self) -> String {
        crate::recipe::short(&self.sha, 12)
    }

    fn shown(&self) -> String {
        let subject = crate::util::trunc(&self.subject, 80);
        if subject.is_empty() {
            self.short()
        } else {
            format!("{} ({subject})", self.short())
        }
    }
}

/// What `resync_onto_upstream` built: the merge commit, the branch that
/// points at it for the landing, and each commit's fate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Resync {
    pub tip: String,
    pub branch: String,
    pub picked: Vec<Picked>,
}

impl Resync {
    pub(crate) fn with(&self, fate: &str) -> Vec<&Picked> {
        self.picked.iter().filter(|p| p.fate == fate).collect()
    }

    /// The counts, as a landing line says them: "3 carried, 2 already
    /// upstream, 1 set aside".
    pub(crate) fn tally(&self) -> String {
        let mut parts = Vec::new();
        for (fate, said) in [
            ("carried", "carried"),
            ("dropped", "already upstream"),
            ("conflicted", "set aside"),
        ] {
            let n = self.with(fate).len();
            if n > 0 {
                parts.push(format!("{n} {said}"));
            }
        }
        parts.join(", ")
    }

    /// The detail of the `land.synced` event.
    pub(crate) fn detail(&self) -> serde_json::Value {
        serde_json::json!({
            "tip": self.tip,
            "commits": self.picked.len(),
            "carried": self.with("carried").len(),
            "dropped": self.with("dropped").len(),
            "conflicted": self.with("conflicted").len(),
            "picked": self.picked,
        })
    }
}

/// The branch the resync's merge commit lands from.
pub(crate) fn resync_branch(t: &crate::landing_hold::PushTarget) -> String {
    format!("pulse/resync/{}/{}", t.remote, t.remote_branch)
}

struct WorktreeCleanup {
    root: String,
    worktree: String,
}

impl Drop for WorktreeCleanup {
    fn drop(&mut self) {
        crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["worktree", "remove", "--force", &self.worktree],
        );
        if Path::new(&self.worktree).is_dir() {
            let _ = std::fs::remove_dir_all(&self.worktree);
        }
        crate::util::git_timeout(&self.root, Duration::from_secs(30), &["worktree", "prune"]);
    }
}

fn tree_of(repo: &str, rev: &str) -> String {
    crate::util::git_timeout(
        repo,
        Duration::from_secs(10),
        &["rev-parse", &format!("{rev}^{{tree}}")],
    )
    .stdout
    .trim()
    .to_string()
}

/// True when the unmerged `path` has no entry on our side: the upstream
/// removed the file and the re-applied commit changed it ("deleted by us").
fn ours_deleted(worktree: &str, path: &str) -> bool {
    let stages = crate::util::git_timeout(
        worktree,
        Duration::from_secs(10),
        &["ls-files", "-u", "--", path],
    );
    if !stages.ok() || stages.stdout.trim().is_empty() {
        return false;
    }
    !stages.stdout.lines().any(|l| {
        l.split('\t')
            .next()
            .and_then(|meta| meta.split_whitespace().nth(2))
            == Some("2")
    })
}

/// Writes the patch of one set-aside commit under the project's patch folder.
fn save_commit_patch(root: &str, home: &str, slug: &str, sha: &str) -> String {
    let folder = Path::new(home).join("patches").join(slug);
    let _ = std::fs::create_dir_all(&folder);
    let out = crate::util::git_timeout(
        root,
        Duration::from_secs(60),
        &["format-patch", "-1", "--stdout", sha],
    );
    let path = folder.join(format!("resync-{}.patch", crate::recipe::short(sha, 12)));
    let _ = std::fs::write(&path, if out.ok() { out.stdout } else { String::new() });
    path.display().to_string()
}

/// Re-applies the default branch's own commits (`up_sha..def_sha`, merges
/// left out) onto `up_sha` in a throwaway worktree and makes one merge commit
/// of the result with `def_sha` and `up_sha` as parents, on
/// `pulse/resync/<remote>/<branch>`. Returns `Ok(None)` when the branch has
/// no commit of its own to re-apply; `Err` names what stopped it, and leaves
/// nothing behind either way.
pub(crate) fn resync_onto_upstream(
    home: &str,
    t: &crate::landing_hold::PushTarget,
    up_sha: &str,
    def_sha: &str,
    actor: &str,
) -> std::result::Result<Option<Resync>, String> {
    let git = |timeout: u64, args: &[&str]| {
        crate::util::git_timeout(&t.root, Duration::from_secs(timeout), args)
    };
    if up_sha.is_empty() || def_sha.is_empty() {
        return Ok(None);
    }
    let listed = git(
        60,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            "--no-merges",
            &format!("{up_sha}..{def_sha}"),
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
    if shas.is_empty() {
        return Ok(None);
    }
    if shas.len() > RESYNC_MAX_COMMITS {
        return Err(format!(
            "{} commits to re-apply, more than {RESYNC_MAX_COMMITS}",
            shas.len()
        ));
    }
    let worktree = Path::new(home)
        .join("work")
        .join(format!("resync-{}-{}", t.slug, std::process::id()))
        .display()
        .to_string();
    git(60, &["worktree", "remove", "--force", &worktree]);
    git(30, &["worktree", "prune"]);
    if Path::new(&worktree).is_dir() {
        let _ = std::fs::remove_dir_all(&worktree);
    }
    let added = git(120, &["worktree", "add", "--detach", &worktree, up_sha]);
    if !added.ok() {
        return Err(format!(
            "git worktree add failed: {}",
            crate::util::trunc(added.stderr.trim(), 200)
        ));
    }
    let _cleanup = WorktreeCleanup {
        root: t.root.clone(),
        worktree: worktree.clone(),
    };
    let wt = |timeout: u64, args: &[&str]| {
        crate::util::git_timeout(&worktree, Duration::from_secs(timeout), args)
    };
    let identity = [
        "-c",
        "user.name=pulse",
        "-c",
        "user.email=pulse@pulse.local",
        "-c",
        "commit.gpgsign=false",
    ];
    let mut picked = Vec::new();
    for sha in &shas {
        let mut p = Picked {
            sha: sha.clone(),
            subject: git(10, &["log", "-1", "--format=%s", sha])
                .stdout
                .trim()
                .to_string(),
            ..Default::default()
        };
        let head_before = wt(10, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        let tree_before = tree_of(&worktree, "HEAD");
        let mut args = identity.to_vec();
        // A commit the upstream already holds becomes empty: kept as an empty
        // copy (the tree is all that matters) instead of stopping the pick.
        args.extend([
            "cherry-pick",
            "--keep-redundant-commits",
            "--allow-empty-message",
            sha,
        ]);
        let pick = wt(600, &args);
        if pick.ok() {
            p.fate = if tree_of(&worktree, "HEAD") == tree_before {
                "dropped"
            } else {
                "carried"
            }
            .into();
            picked.push(p);
            continue;
        }
        let unmerged = wt(30, &["diff", "--name-only", "--diff-filter=U", "-z"]);
        let paths = unmerged
            .stdout
            .split('\0')
            .filter(|x| !x.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let back = |fate: &str, files: Vec<String>| {
            wt(60, &["cherry-pick", "--abort"]);
            wt(30, &["cherry-pick", "--quit"]);
            wt(120, &["reset", "-q", "--hard", &head_before]);
            Picked {
                fate: fate.into(),
                files,
                ..p.clone()
            }
        };
        if paths.is_empty() {
            // Stopped without a conflict: nothing of it was left to apply.
            picked.push(back("dropped", Vec::new()));
            continue;
        }
        // Clashes only on files the upstream removed: the removal stands and
        // the rest of the commit is kept.
        if paths.iter().all(|path| ours_deleted(&worktree, path)) {
            let mut rm = vec!["rm", "-q", "--"];
            rm.extend(paths.iter().map(String::as_str));
            if wt(60, &rm).ok() {
                let nothing_left = wt(30, &["diff", "--cached", "--quiet", "HEAD"]).ok();
                let done = if nothing_left {
                    wt(60, &["cherry-pick", "--skip"]).ok()
                } else {
                    let mut cont = identity.to_vec();
                    cont.extend(["-c", "core.editor=true", "cherry-pick", "--continue"]);
                    wt(300, &cont).ok()
                };
                if done {
                    p.fate = if nothing_left { "dropped" } else { "carried" }.into();
                    p.files = paths;
                    picked.push(p);
                    continue;
                }
            }
        }
        let mut set_aside = back("conflicted", paths);
        set_aside.patch = save_commit_patch(&t.root, home, &t.slug, sha);
        picked.push(set_aside);
    }
    let tip_tree = tree_of(&worktree, "HEAD");
    if tip_tree.is_empty() {
        return Err("the re-applied tree could not be read".into());
    }
    let mut resync = Resync {
        tip: String::new(),
        branch: resync_branch(t),
        picked,
    };
    let message = commit_message(t, up_sha, &resync, actor);
    let mut args = identity.to_vec();
    args.extend([
        "commit-tree",
        &tip_tree,
        "-p",
        def_sha,
        "-p",
        up_sha,
        "-m",
        &message,
    ]);
    let made = git(30, &args);
    if !made.ok() {
        return Err(format!(
            "the merge commit could not be made: {}",
            crate::util::trunc(made.stderr.trim(), 200)
        ));
    }
    resync.tip = made.stdout.trim().to_string();
    let pointed = git(10, &["branch", "-f", &resync.branch, &resync.tip]);
    if !pointed.ok() {
        return Err(format!(
            "could not make {}: {}",
            resync.branch,
            crate::util::trunc(pointed.stderr.trim(), 200)
        ));
    }
    Ok(Some(resync))
}

/// The merge commit's message: what a reader of `git log` needs to know
/// about where each of the branch's own commits went.
fn commit_message(
    t: &crate::landing_hold::PushTarget,
    up_sha: &str,
    r: &Resync,
    actor: &str,
) -> String {
    let own = r.picked.len() as i32;
    let mut m = format!(
        "Sync {}/{} into {}: {}'s own {own} {} re-applied ({})\n\nA plain merge of {}/{} ({}) conflicted, so {}'s own commits were re-applied onto it one by one and the result merged as a single commit, the way a pull with a hand resolution would leave it.\n",
        t.remote,
        t.remote_branch,
        t.def,
        t.def,
        crate::util::plural(own, "commit"),
        r.tally(),
        t.remote,
        t.remote_branch,
        crate::recipe::short(up_sha, 12),
        t.def,
    );
    let carried = r.with("carried");
    if !carried.is_empty() {
        m.push_str("\nCarried over:\n");
        for p in carried {
            m.push_str(&format!("- {}", p.shown()));
            if !p.files.is_empty() {
                m.push_str(&format!(
                    " (dropped: {}, which {}/{} removed)",
                    p.files.join(", "),
                    t.remote,
                    t.remote_branch
                ));
            }
            m.push('\n');
        }
    }
    let dropped = r.with("dropped");
    if !dropped.is_empty() {
        m.push_str(&format!(
            "\nAlready in {}/{}, dropped:\n",
            t.remote, t.remote_branch
        ));
        for p in dropped {
            m.push_str(&format!("- {}\n", p.shown()));
        }
    }
    let conflicted = r.with("conflicted");
    if !conflicted.is_empty() {
        m.push_str("\nSet aside, re-applied by a job:\n");
        for p in conflicted {
            m.push_str(&format!(
                "- {} (conflicts: {}; patch at {})\n",
                p.shown(),
                p.files.join(", "),
                p.patch
            ));
        }
    }
    m.push_str(&format!("\nPulse-Landed-By: {actor}\n"));
    m
}

/// After the merge commit landed: one job per set-aside commit, each with
/// the commit's patch and what it meant, and a finding for the owner when
/// anything was set aside. Returns the picks with their job ids filled in.
pub(crate) fn after_landing(
    s: &pulse_store::Store,
    t: &crate::landing_hold::PushTarget,
    r: &Resync,
    actor: &str,
    now: &str,
) -> Resync {
    let mut r = r.clone();
    let slug = t.slug.clone();
    for p in r.picked.iter_mut().filter(|p| p.fate == "conflicted") {
        let short = p.short();
        let title = crate::util::trunc(
            &format!("Re-apply {short} onto the synced {}: {}", t.def, p.subject),
            160,
        );
        let brief = reapply_brief(t, p);
        match crate::jobs::add_job(
            s,
            slug.clone(),
            "task".into(),
            title,
            brief,
            JOB_TIER,
            JOB_PRIORITY,
            format!("{JOB_SOURCE}:{slug}:{short}"),
            actor.to_string(),
            true,
        ) {
            Ok((job, _)) => p.job = job.str("id"),
            Err(e) => eprintln!(
                "resync {slug}: the job re-applying {short} could not be planned: {}",
                crate::run::err_message(&e)
            ),
        }
    }
    let conflicted = r.with("conflicted");
    if !conflicted.is_empty() {
        let listed = conflicted
            .iter()
            .map(|p| {
                format!(
                    "- {} clashed on {}; patch at {}{}",
                    p.shown(),
                    p.files.join(", "),
                    p.patch,
                    if p.job.is_empty() {
                        String::new()
                    } else {
                        format!("; job {} re-applies it", p.job)
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let summary = crate::util::trunc(
            &format!(
                "sync of {}/{} into {}'s {}: {} of {}'s own {} {} set aside for a job ({})",
                t.remote,
                t.remote_branch,
                slug,
                t.def,
                conflicted.len(),
                t.def,
                r.picked.len(),
                crate::util::plural(r.picked.len() as i32, "commit"),
                conflicted
                    .iter()
                    .map(|p| p.short())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            236,
        );
        let body = format!(
            "{}/{} had moved under {}'s {} and a plain merge conflicted, so {} re-applied {}'s own commits onto {}/{} one by one and landed the result as one merge commit ({}): {}.\n\nSet aside:\n{listed}\n\nEach set-aside commit stays reachable on {} through the merge's first parent; the job starts from the synced {} and re-applies what the commit meant, or finishes without a change when {}/{} already holds it another way.\n",
            t.remote,
            t.remote_branch,
            slug,
            t.def,
            actor,
            t.def,
            t.remote,
            t.remote_branch,
            crate::recipe::short(&r.tip, 12),
            r.tally(),
            t.def,
            t.def,
            t.remote,
            t.remote_branch
        );
        if let Err(e) = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: slug.clone(),
                area: "landing".into(),
                thread: "sync".into(),
                summary,
                body,
                for_owner: true,
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: now.to_string(),
                ..Default::default()
            },
        ) {
            eprintln!(
                "resync {slug}: the finding could not be posted: {}",
                crate::run::err_message(&e)
            );
        }
    }
    r
}

/// The brief of the job that re-applies one set-aside commit.
fn reapply_brief(t: &crate::landing_hold::PushTarget, p: &Picked) -> String {
    let short = p.short();
    format!(
        "{}'s commit {short} ({}) was landed by pulse but never reached {}/{}: the forge had moved on, a plain merge conflicted, and when pulse re-applied {}'s own commits onto {}/{} this one clashed on {}. The sync landed without it; {} now holds {}/{} and the commit is still reachable through the merge's first parent (`git show {short}`). Its patch is at {}.\n\nRe-apply what the commit meant on the current {}: read the patch and `git show {short}`, then make the same change against the files as they are now. Where {}/{} already holds the change in another form (a newer dependency bump, a file it replaced), say so and finish without a change. Run the project's check before you finish.\n",
        t.def,
        crate::util::trunc(&p.subject, 120),
        t.remote,
        t.remote_branch,
        t.def,
        t.remote,
        t.remote_branch,
        p.files.join(", "),
        t.def,
        t.remote,
        t.remote_branch,
        p.patch,
        t.def,
        t.remote,
        t.remote_branch
    )
}
