//! Git environment scrubbing for hook-launched PULSE processes.

/// Environment variable names Git injects while running hooks. A nested `git`
/// command must not inherit them or it will operate on the hook repository
/// rather than the target checkout.
pub const HOOK_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_PREFIX",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_QUARANTINE_PATH",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
];

/// Returns the current process environment with Git hook variables removed and
/// `GIT_TERMINAL_PROMPT=0` appended.
pub fn env() -> Vec<String> {
    let mut out = strip_hook(std::env::vars().map(|(k, v)| format!("{k}={v}")).collect());
    out.push("GIT_TERMINAL_PROMPT=0".to_string());
    out
}

/// Removes Git hook variables from an environment vector.
pub fn strip_hook(env: Vec<String>) -> Vec<String> {
    env.into_iter().filter(|kv| !hook_var(kv)).collect()
}

/// Reports whether `kv` is a Git hook environment assignment.
pub fn hook_var(kv: &str) -> bool {
    let name = kv.split_once('=').map_or(kv, |(k, _)| k);
    HOOK_VARS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_testutil::env_lock;

    #[test]
    fn env_drops_inherited_git_vars() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("GIT_DIR", "/nonexistent/.git");
            std::env::set_var("GIT_INDEX_FILE", "/nonexistent/index");
            std::env::set_var("PULSE_GITENV_KEEP", "1");
        }
        let got = env();
        assert!(!got.iter().any(|kv| hook_var(kv)));
        assert!(got.iter().any(|kv| kv == "PULSE_GITENV_KEEP=1"));
        assert!(got.iter().any(|kv| kv == "GIT_TERMINAL_PROMPT=0"));
    }
}
