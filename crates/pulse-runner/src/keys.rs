#![allow(unused_variables, dead_code, unused_imports)]
use crate::util::Row;
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
pub(crate) static PROVENANCE_TRAILERS: &[&str] = &["Pulse-Agent", "Pulse-Job", "Pulse-Executor"];
pub(crate) static OWNER_PATHS: &[&str] = pulse_tally::OWNER_PATHS;
/// Go: func agentEmail(agent string) string
pub(crate) fn agent_email(agent: &str) -> String {
    format!("{agent}@pulse.local")
}
/// Go: func makeRunKey(path, agent, runID string) (string, error)
pub(crate) fn make_run_key(path: &str, agent: &str, run_id: &str) -> Result<String> {
    let pub_path = format!("{path}.pub");
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(&pub_path);
    std::fs::create_dir_all(Path::new(path).parent().unwrap_or_else(|| Path::new("."))).map_err(
        |e| {
            crate::util::fail(
                1,
                format!("cannot make the directory for the run's signing key: {e}"),
            )
        },
    )?;
    let out = Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            &format!("{agent} {run_id}"),
            "-f",
            path,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            if which::which("ssh-keygen").is_err() {
                return Err(crate::util::fail(1, "ssh-keygen is not installed; the runner needs it to give each run its signing key (ADR-022)").into());
            }
            return Err(crate::util::fail(
                1,
                format!(
                    "cannot make the run's signing key: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
            )
            .into());
        }
        Err(e) => {
            if which::which("ssh-keygen").is_err() {
                return Err(crate::util::fail(1, "ssh-keygen is not installed; the runner needs it to give each run its signing key (ADR-022)").into());
            }
            return Err(
                crate::util::fail(1, format!("cannot make the run's signing key: {e}")).into(),
            );
        }
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    if !crate::util::file_exists(path) {
        return Err(crate::util::fail(1, format!("the run's signing key was not written at {path}; commits would fail at commit time (ADR-022)")).into());
    }
    let fields = crate::util::read_file(&pub_path)
        .split_whitespace()
        .take(2)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Ok(fields.join(" "))
}
/// Go: func removeRunKey(path string)
pub(crate) fn remove_run_key(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{path}.pub"));
}
/// Go: func provenanceHook(agent, jobID, executor string) string
pub(crate) fn provenance_hook(agent: &str, job_id: &str, executor: &str) -> String {
    let values = [agent, job_id, executor];
    let parts = PROVENANCE_TRAILERS
        .iter()
        .zip(values)
        .map(|(k, v)| {
            format!(
                "--trailer {}",
                crate::util::shell_quote(&format!("{k}: {v}"))
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "#!/bin/sh\n# pulse: provenance trailers for every commit of this run (ADR-022). Local to this clone, never committed.\nexec git interpret-trailers --in-place --if-exists doNothing {parts} \"$1\"\n"
    )
}
pub(crate) const PRE_COMMIT_HOOK_UNSET: &str = "unset GIT_INDEX_FILE GIT_PREFIX GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_AUTHOR_DATE GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_COMMITTER_DATE";
/// Go: func preCommitHookScript(verifyCmd string, recipe *Recipe, pins []string, clone string) string
pub(crate) fn pre_commit_hook_script(
    verify_cmd: &str,
    recipe: Option<&crate::recipe::Recipe>,
    pins: &[String],
    clone: &str,
    pulse_home: &str,
    project: &str,
    agent: &str,
) -> String {
    let mut b = String::new();
    b.push_str("#!/bin/sh\n");
    b.push_str("# pulse: verify-before-commit gate (auto-installed by runner)\n");
    b.push_str(PRE_COMMIT_HOOK_UNSET);
    b.push('\n');
    if let Some(r) = recipe {
        let exports = crate::recipe::recipe_env_exports(r);
        if !exports.is_empty() {
            b.push_str(exports.trim_end_matches("; "));
            b.push('\n');
        }
    }
    // The verify runs in a subshell: the lock wrapper ends in `exit`, which
    // would otherwise leave the hook before the memo below is written.
    b.push_str("(\n");
    if pins.is_empty() {
        b.push_str(verify_cmd);
    } else {
        b.push_str(&crate::recipe::mise_wrap_shell(pins, verify_cmd, clone));
    }
    b.push_str("\n)\nrc=$?\nif [ \"$rc\" -ne 0 ]; then exit \"$rc\"; fi\n");
    append_verify_memo_hook(&mut b, verify_cmd, clone, pulse_home, project, agent);
    b
}

fn append_verify_memo_hook(
    b: &mut String,
    verify_cmd: &str,
    clone: &str,
    pulse_home: &str,
    project: &str,
    agent: &str,
) {
    let hash = crate::verify_memo::verify_sha256(verify_cmd);
    let project_q = crate::util::shell_quote(project);
    let home_q = crate::util::shell_quote(pulse_home);
    let agent_json = serde_json::to_string(agent).unwrap_or_else(|_| "\"\"".into());
    let clone_json = serde_json::to_string(clone).unwrap_or_else(|_| "\"\"".into());
    // The memo names the index tree, so it is written only when the working
    // tree the verify ran on is that tree: no unstaged changes.
    let _ = writeln!(
        b,
        "tree=$(git diff --quiet 2>/dev/null && git write-tree 2>/dev/null || true)
if [ -n \"$tree\" ]; then
  memo_home=${{PULSE_HOME:-{home_q}}}
  memo_dir=\"$memo_home/verified/\"{project_q}
  at=$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || true)
  if [ -n \"$at\" ] && mkdir -p \"$memo_dir\" 2>/dev/null; then
    {{ printf '%s' '{{\"verify_sha256\":\"{hash}\",\"at\":\"'; printf '%s' \"$at\"; printf '%s\\n' '\",\"by\":\"hook\",\"agent\":{agent_json},\"clone\":{clone_json}}}'; }} > \"$memo_dir/$tree\" 2>/dev/null || true
  fi
fi"
    );
}
/// Go: func configureCloneIdentity(clone, agent, keyPath, hookText string) error
pub(crate) fn configure_clone_identity(
    clone: &str,
    agent: &str,
    key_path: &str,
    hook_text: &str,
) -> Result<()> {
    if !crate::util::file_exists(key_path) {
        return Err(crate::util::fail(1, format!("the run's signing key {key_path} does not exist; refusing to configure user.signingkey (ADR-022)")).into());
    }
    let hooks = Path::new(clone).join(".git").join("hooks");
    let mut identity = String::new();
    for (key, value) in [
        ("user.name", agent.to_string()),
        ("user.email", agent_email(agent)),
        ("gpg.format", "ssh".to_string()),
        ("gpg.ssh.program", "ssh-keygen".to_string()),
        ("user.signingkey", key_path.to_string()),
        ("commit.gpgsign", "true".to_string()),
        ("tag.gpgsign", "false".to_string()),
        ("core.hooksPath", hooks.display().to_string()),
    ] {
        let leaf = key.rsplit_once('.').map(|(_, x)| x).unwrap_or(key);
        let _ = writeln!(
            identity,
            "{}\t{} = {}",
            git_config_section(key),
            leaf,
            git_config_quote(&value)
        );
    }
    let identity_path = Path::new(clone).join(".git").join("pulse-identity");
    std::fs::write(&identity_path, identity)?;
    crate::util::git_check(
        clone,
        &[
            "config",
            "include.path",
            &identity_path.display().to_string(),
        ],
    )?;
    std::fs::create_dir_all(&hooks)?;
    let hook = hooks.join("prepare-commit-msg");
    std::fs::write(&hook, hook_text)?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}
/// Go: func writePreCommitHook(clone, script string) error
pub(crate) fn write_pre_commit_hook(clone: &str, script: &str) -> Result<()> {
    let hook = Path::new(clone)
        .join(".git")
        .join("hooks")
        .join("pre-commit");
    std::fs::write(&hook, script)?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}
/// Go: func gitConfigSection(key string) string
pub(crate) fn git_config_section(key: &str) -> String {
    let (section, rest) = key.split_once('.').unwrap_or((key, ""));
    if let Some((sub, _)) = rest.rsplit_once('.') {
        format!("[{} {}]\n", section, git_config_quote(sub))
    } else {
        format!("[{section}]\n")
    }
}
/// Go: func gitConfigQuote(v string) string
pub(crate) fn git_config_quote(v: &str) -> String {
    format!(
        "\"{}\"",
        v.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}
/// Go: func SignersLine(agent, pubkey string) string
pub fn signers_line(agent: &str, pubkey: &str) -> String {
    format!("{} namespaces=\"git\" {}\n", agent_email(agent), pubkey)
}
/// Go: func SignersText(s *store.Store) string
pub fn signers_text(s: &pulse_store::Store) -> String {
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    crate::util::must_rows(
        &conn,
        "SELECT agent, pubkey FROM runs WHERE pubkey != '' ORDER BY started_at, id",
        &[],
    )
    .into_iter()
    .map(|r| signers_line(&r.str("agent"), &r.str("pubkey")))
    .collect()
}
/// Go: func landingProblems(root, base, ref, agent, jobID, pubkey, signersPath string) []string
pub(crate) fn landing_problems(
    root: &str,
    base: &str,
    ref_: &str,
    agent: &str,
    job_id: &str,
    pubkey: &str,
    signers_path: &str,
) -> Vec<String> {
    if let Err(e) = std::fs::write(signers_path, signers_line(agent, pubkey)) {
        return vec![format!("could not write the allowed-signers file: {e}")];
    }
    let _ = std::fs::set_permissions(signers_path, std::fs::Permissions::from_mode(0o600));
    let format = "%H%x1f%G?%x1f%an%x1f%ae%x1f%(trailers:key=Pulse-Agent,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Job,valueonly,separator=%x1d)%x1e";
    let allowed = format!("gpg.ssh.allowedSignersFile={signers_path}");
    let out = crate::util::git_timeout(
        root,
        Duration::from_secs(120),
        &[
            "-c",
            "gpg.ssh.program=ssh-keygen",
            "-c",
            &allowed,
            "log",
            &format!("--format={format}"),
            &format!("{base}..{ref_}"),
        ],
    );
    let _ = std::fs::remove_file(signers_path);
    if !out.ok() {
        return vec![format!(
            "could not read the fetched commits: {}",
            out.stderr.trim()
        )];
    }
    let mut problems = Vec::new();
    for record in out
        .stdout
        .split('\x1e')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut f = record
            .split('\x1f')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>();
        f.resize(6, String::new());
        let short_sha = crate::recipe::short(&f[0], 12);
        if f[1] != "G" {
            let w = match f[1].as_str() {
                "N" => "is not signed".to_string(),
                "U" => "is signed by a key that is not this run's".to_string(),
                "B" => "has a bad signature".to_string(),
                "E" => "has a signature that cannot be checked".to_string(),
                x => format!("has signature status {x}"),
            };
            problems.push(format!("commit {short_sha} {w}"));
        }
        if f[2] != agent || f[3] != agent_email(agent) {
            problems.push(format!(
                "commit {short_sha} is authored by {} <{}>, not {agent}",
                f[2], f[3]
            ));
        }
        let agents = split_trailer(&f[4]);
        let jobs = split_trailer(&f[5]);
        if agents.is_empty() {
            problems.push(format!("commit {short_sha} lacks its Pulse-Agent trailer"));
        } else if agents.len() != 1 || agents[0] != agent {
            problems.push(format!(
                "commit {short_sha} names {} in its Pulse-Agent trailer, not {agent}",
                agents.join(", ")
            ));
        }
        if jobs.is_empty() {
            problems.push(format!("commit {short_sha} lacks its Pulse-Job trailer"));
        } else if jobs.len() != 1 || jobs[0] != job_id {
            problems.push(format!(
                "commit {short_sha} names job {} in its Pulse-Job trailer, not {job_id}",
                jobs.join(", ")
            ));
        }
    }
    problems
}
/// Go: func splitTrailer(s string) []string
pub(crate) fn split_trailer(s: &str) -> Vec<String> {
    s.split('\x1d')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
/// Go: func reprobeDue(s *store.Store, interval int, now string) [][2]string
pub(crate) fn reprobe_due(s: &pulse_store::Store, interval: i32, now: &str) -> Vec<[String; 2]> {
    let _ = (s, interval, now);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_key_must_exist_before_identity() {
        let _serial = crate::test_helpers::serial();
        let clone = tempfile::tempdir().unwrap();
        crate::test_helpers::run(clone.path(), &["git", "init", "-q"]);
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing.key")
            .display()
            .to_string();
        let err = configure_clone_identity(
            &clone.path().display().to_string(),
            "aurora",
            &missing,
            &provenance_hook("aurora", "JOB-1", crate::executor::LOCAL_EXECUTOR),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("signing key"), "{err}");
        let out = std::process::Command::new(pulse_gitenv::git_program())
            .args(["config", "--local", "--get", "user.signingkey"])
            .current_dir(clone.path())
            .env_clear()
            .envs(crate::util::git_env(&[]).into_iter().filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            }))
            .output()
            .unwrap();
        assert!(!out.status.success(), "user.signingkey set unexpectedly");
    }

    #[test]
    fn signing_key_creates_work_dir() {
        let _serial = crate::test_helpers::serial();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let key = tempfile::tempdir()
            .unwrap()
            .path()
            .join("work/job.key")
            .display()
            .to_string();
        make_run_key(&key, "aurora", "run-1").unwrap();
        assert!(crate::util::file_exists(&key));
    }

    #[test]
    fn configure_clone_identity_include_file() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap().path().join("with space \"q\"");
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::run(&dir, &["git", "init", "-q"]);
        let key = dir.join("key \"x\" y").display().to_string();
        std::fs::write(&key, b"k").unwrap();
        configure_clone_identity(&dir.display().to_string(), "aurora", &key, "#!/bin/sh\n")
            .unwrap();
        for (k, want) in [
            ("user.name", "aurora".to_string()),
            ("user.email", "aurora@pulse.local".to_string()),
            ("gpg.format", "ssh".to_string()),
            ("gpg.ssh.program", "ssh-keygen".to_string()),
            ("user.signingkey", key.clone()),
            ("commit.gpgsign", "true".to_string()),
            ("tag.gpgsign", "false".to_string()),
            (
                "core.hooksPath",
                dir.join(".git/hooks").display().to_string(),
            ),
        ] {
            let got = crate::util::git(&dir.display().to_string(), &["config", "--get", k]);
            assert_eq!(got.stdout, format!("{want}\n"), "{k}: {}", got.stderr);
        }
    }

    #[test]
    fn pre_commit_hook_writes_verified_tree_memo() {
        let script = pre_commit_hook_script(
            "cargo test -p pulse-runner",
            None,
            &[],
            "/clone",
            "/home/me/.pulse",
            "proj",
            "aurora",
        );
        let hash = crate::verify_memo::verify_sha256("cargo test -p pulse-runner");
        assert!(script.contains("git write-tree"), "{script}");
        assert!(script.contains("/verified/\"proj"), "{script}");
        assert!(script.contains(&hash), "{script}");
        assert!(script.contains("\"by\":\"hook\""), "{script}");
        assert!(script.contains("\"agent\":\"aurora\""), "{script}");
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The lock wrapper around a verify command ends in `exit`; the hook must
    /// still write the memo for the committed tree, and only when the working
    /// tree it verified is that tree.
    #[test]
    fn pre_commit_hook_memo_survives_an_exiting_verify_and_skips_dirty_trees() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        git_in(&clone, &["init", "-q", "-b", "main"]);
        git_in(&clone, &["config", "user.email", "t@example.com"]);
        git_in(&clone, &["config", "user.name", "t"]);
        git_in(&clone, &["config", "commit.gpgsign", "false"]);
        let verify = "( true )\npulse_rc=$?\nexit $pulse_rc";
        let script = pre_commit_hook_script(
            verify,
            None,
            &[],
            &clone.display().to_string(),
            &home.display().to_string(),
            "proj",
            "aurora",
        );
        write_pre_commit_hook(&clone.display().to_string(), &script).unwrap();

        std::fs::write(clone.join("a.txt"), "one\n").unwrap();
        git_in(&clone, &["add", "a.txt"]);
        git_in(&clone, &["commit", "-q", "-m", "one"]);
        let tree = git_in(&clone, &["rev-parse", "HEAD^{tree}"]);
        let memo = crate::verify_memo::lookup(
            &home.display().to_string(),
            "proj",
            &tree,
            verify,
            24,
            chrono::Utc::now(),
        )
        .expect("the hook wrote a memo for the committed tree");
        assert_eq!(memo.by, "hook");
        assert_eq!(memo.agent, "aurora");

        // A second commit with another file left modified but unstaged: the
        // verify saw a tree that is not the one committed, so no memo.
        std::fs::write(clone.join("b.txt"), "two\n").unwrap();
        git_in(&clone, &["add", "b.txt"]);
        std::fs::write(clone.join("a.txt"), "changed\n").unwrap();
        git_in(&clone, &["commit", "-q", "-m", "two"]);
        let dirty_tree = git_in(&clone, &["rev-parse", "HEAD^{tree}"]);
        assert!(
            crate::verify_memo::lookup(
                &home.display().to_string(),
                "proj",
                &dirty_tree,
                verify,
                24,
                chrono::Utc::now(),
            )
            .is_none(),
            "no memo for a tree verified with unstaged changes"
        );
    }
}
