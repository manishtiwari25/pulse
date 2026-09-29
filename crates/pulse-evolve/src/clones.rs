//! Port of internal/evolve/clones.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_ANSWER_KEYS: &[&str] = &[
    ".pulse",
    "docs/evolution/evaluators.md",
    "docs/evolution/archive.md",
    "docs/board/evolution",
    "docs/board/governance",
    "docs/memory/mistakes.md",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViolationError {
    pub reason: String,
}

impl fmt::Display for ViolationError {
    /// Go: func (e *ViolationError) Error() string { return "evolution constraint violation: " + e.Reason }
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "evolution constraint violation: {}", self.reason)
    }
}

impl Error for ViolationError {}

/// Go: func projectAnswerKeys(s *store.Store, project string) ([]string, error) {
pub(crate) fn project_answer_keys(s: &pulse_store::Store, project: &str) -> Result<Vec<String>> {
    let fixtures = crate::fixtures::list_fixtures(s, project, false)?;
    let mut seen: BTreeSet<String> = DEFAULT_ANSWER_KEYS.iter().map(|s| s.to_string()).collect();
    for f in fixtures {
        for key in f.answer_keys {
            let k = crate::fixtures::relative_path(&key).map_err(|_| ViolationError {
                reason: "an answer-key path escapes the clone".to_string(),
            })?;
            seen.insert(k);
        }
    }
    Ok(seen.into_iter().collect())
}

/// Go: func fleetRoot(s *store.Store, create bool) (string, error) {
pub(crate) fn fleet_root(s: &pulse_store::Store, create: bool) -> Result<String> {
    let home = crate::private::canonical_path(&s.home)?;
    let root = Path::new(&home).join("fleet").display().to_string();
    let resolved = crate::private::canonical_path(&root);
    if resolved.as_deref().is_ok_and(|v| v != root) {
        return Err(crate::fixtures::input_error(
            "Cannot use fleet directory",
            "the managed fleet directory must not be a symlink",
            "pulse --home DIR fleet list",
        ));
    }
    if let Err(e) = resolved
        && Path::new(&root).exists()
    {
        return Err(e);
    }
    if let Err(e) = crate::private::outside_projects(s, &root) {
        let _ = e;
        return Err(crate::fixtures::input_error(
            "Cannot use fleet directory",
            "PULSE_HOME/fleet must be outside every registered project",
            "pulse --home DIR fleet prepare --project P --n 1",
        ));
    }
    let private = s.setting("evolve.heldout_dir", "");
    if !private.is_empty() {
        let dir = crate::private::canonical_path(&private);
        if match dir.as_deref() {
            Ok(d) => crate::private::within(d, &root) || crate::private::within(&root, d),
            Err(_) => true,
        } {
            return Err(crate::private::private_error(
                "private fixtures must not overlap the disposable fleet area",
            ));
        }
    }
    if create {
        fs::create_dir_all(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

/// Go: func resolveRevision(ctx context.Context, repo, ref string) (string, string, error) {
pub(crate) fn resolve_revision(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    ref_: &str,
) -> Result<(String, String)> {
    let root = crate::private::canonical_path(repo).map_err(|_| {
        crate::fixtures::input_error(
            "Cannot measure checkout",
            "the checkout path cannot be resolved",
            "pulse projects",
        )
    })?;
    let ref_name = if ref_.is_empty() { "HEAD" } else { ref_ };
    let commit = crate::process::git_output(
        cancel,
        &root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--end-of-options".into(),
            format!("{ref_name}^{{commit}}"),
        ],
    );
    match commit {
        Ok(commit)
            if commit.len() >= 40
                && !commit.contains(['\n', '\r', ' '])
                && !cancel.is_cancelled() =>
        {
            Ok((root, commit))
        }
        _ if cancel.is_cancelled() => Err(cancel.err().unwrap().into()),
        _ => Err(crate::fixtures::input_error(
            "Cannot resolve measurement revision",
            "the requested ref must name a local commit",
            "git rev-parse --verify HEAD",
        )),
    }
}

/// Go: func strippedClone(ctx context.Context, repo, commit, dest string, keys []string) error {
pub(crate) fn stripped_clone(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    commit: &str,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    if crate::private::within(dest, repo) {
        return Err(ViolationError {
            reason: "a clone destination is inside its source repository".to_string(),
        }
        .into());
    }
    if fs::symlink_metadata(dest).is_ok() {
        return Err(crate::fixtures::input_error(
            "Cannot prepare clone",
            "the clone path must not already exist",
            "pulse fleet list",
        ));
    }
    crate::process::git_output(
        cancel,
        "",
        &[
            "clone".into(),
            "--quiet".into(),
            "--no-hardlinks".into(),
            "--no-tags".into(),
            "--no-checkout".into(),
            "--".into(),
            repo.into(),
            dest.into(),
        ],
    )
    .map_err(|_| clone_failure(cancel, "clone"))?;
    crate::process::git_output(
        cancel,
        dest,
        &[
            "checkout".into(),
            "--quiet".into(),
            "--detach".into(),
            commit.into(),
        ],
    )
    .map_err(|_| clone_failure(cancel, "detach the clone at the requested revision"))?;
    crate::process::git_output(
        cancel,
        dest,
        &["remote".into(), "remove".into(), "origin".into()],
    )
    .map_err(|_| clone_failure(cancel, "remove the clone's remote"))?;
    let tags = crate::process::git_output(
        cancel,
        dest,
        &[
            "for-each-ref".into(),
            "--format=%(refname)".into(),
            "refs/tags/".into(),
        ],
    )
    .map_err(|_| clone_failure(cancel, "inspect clone tags"))?;
    for tag in tags.split_whitespace() {
        crate::process::git_output(
            cancel,
            dest,
            &["update-ref".into(), "-d".into(), tag.into()],
        )
        .map_err(|_| clone_failure(cancel, "remove clone tags"))?;
    }
    for key in keys {
        let key = crate::fixtures::relative_path(key).map_err(|_| ViolationError {
            reason: "an answer-key path escapes the clone".to_string(),
        })?;
        let path = Path::new(dest).join(PathBuf::from(
            key.replace('/', std::path::MAIN_SEPARATOR_STR),
        ));
        if path.exists() {
            fs::remove_dir_all(&path)
                .or_else(|_| fs::remove_file(&path))
                .map_err(|_| ViolationError {
                    reason: "cannot strip an answer-key path".to_string(),
                })?;
        }
    }
    hide_stripped(cancel, dest, keys)?;
    check_clone(cancel, dest, keys)
}

/// Go: func hideStripped(ctx context.Context, dest string, keys []string) error {
pub(crate) fn hide_stripped(
    cancel: &pulse_bus::CancellationToken,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "--literal-pathspecs".to_string(),
        "ls-files".to_string(),
        "-z".to_string(),
        "--cached".to_string(),
        "--".to_string(),
    ];
    args.extend(keys.iter().cloned());
    let listed = crate::process::git_raw(cancel, dest, &args)
        .map_err(|_| clone_failure(cancel, "list the stripped answer-key paths"))?;
    let mut paths: Vec<String> = listed
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect();
    while !paths.is_empty() {
        let n = paths.len().min(64);
        let mut args = vec![
            "update-index".to_string(),
            "--skip-worktree".to_string(),
            "--".to_string(),
        ];
        args.extend(paths.drain(..n));
        crate::process::git_output(cancel, dest, &args)
            .map_err(|_| clone_failure(cancel, "hide the stripped answer-key paths"))?;
    }
    Ok(())
}

/// Go: func cloneFailure(ctx context.Context, step string) error {
pub(crate) fn clone_failure(cancel: &pulse_bus::CancellationToken, step: &str) -> anyhow::Error {
    if let Some(err) = cancel.err() {
        return err.into();
    }
    crate::fixtures::input_error(
        "Cannot prepare isolated clone",
        &format!("git could not {step}"),
        "pulse fleet prepare --project P --n 1 --ref HEAD",
    )
}

/// Go: func checkClone(ctx context.Context, dest string, keys []string) error {
pub(crate) fn check_clone(
    cancel: &pulse_bus::CancellationToken,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    if let Some(err) = cancel.err() {
        return Err(err.into());
    }
    let info = fs::symlink_metadata(Path::new(dest).join(".git"));
    if match info.as_ref() {
        Ok(m) => !m.is_dir() || m.file_type().is_symlink(),
        Err(_) => true,
    } {
        return Err(ViolationError {
            reason: "clone Git metadata is not a local directory".to_string(),
        }
        .into());
    }
    let head = crate::process::git_output(
        cancel,
        dest,
        &["symbolic-ref".into(), "--quiet".into(), "HEAD".into()],
    );
    if let Some(err) = cancel.err() {
        return Err(err.into());
    }
    if head.is_ok_and(|h| !h.is_empty()) {
        return Err(ViolationError {
            reason: "clone is no longer detached".to_string(),
        }
        .into());
    }
    if crate::process::git_output(
        cancel,
        dest,
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
    )
    .is_err()
    {
        if let Some(err) = cancel.err() {
            return Err(err.into());
        }
        return Err(ViolationError {
            reason: "clone HEAD is unreadable".to_string(),
        }
        .into());
    }
    for args in [
        vec!["remote".to_string()],
        vec!["tag".to_string(), "--list".to_string()],
    ] {
        let out = crate::process::git_output(cancel, dest, &args);
        if let Some(err) = cancel.err() {
            return Err(err.into());
        }
        if match out.as_deref() {
            Ok(o) => !o.is_empty(),
            Err(_) => true,
        } {
            return Err(ViolationError {
                reason: "clone has a remote or tag, or cannot be inspected".to_string(),
            }
            .into());
        }
    }
    for key in keys {
        let p = Path::new(dest).join(key);
        if fs::symlink_metadata(p).is_ok() {
            return Err(ViolationError {
                reason: "a stripped answer-key path is present or cannot be inspected".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

/// Go: func removeManaged(s *store.Store, id string) error {
pub(crate) fn remove_managed(s: &pulse_store::Store, id: &str) -> Result<()> {
    if !crate::fleet::MANAGED_ID.is_match(id) {
        return Err(crate::fixtures::input_error(
            "Cannot clean fleet",
            "expected a fleet id, not a path",
            "pulse fleet list",
        ));
    }
    let base = fleet_root(s, false)?;
    let root = Path::new(&base);
    if !root.exists() {
        return Ok(());
    }
    let target = root.join(id);
    let info = match fs::symlink_metadata(&target) {
        Ok(i) => i,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if info.is_dir() && !info.file_type().is_symlink() {
        fn chmod_dirs(path: &Path) -> Result<()> {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let meta = fs::symlink_metadata(entry.path())?;
                if meta.is_dir() && !meta.file_type().is_symlink() {
                    chmod_dirs(&entry.path())?;
                }
            }
            Ok(())
        }
        chmod_dirs(&target).map_err(|e| anyhow!("cannot make managed clone removable: {e}"))?;
    }
    fs::remove_dir_all(&target).or_else(|_| fs::remove_file(&target))?;
    Ok(())
}
