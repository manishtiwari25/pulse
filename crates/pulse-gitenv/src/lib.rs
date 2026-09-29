//! Git environment scrubbing for child git commands.

/// Hook-exported Git variables that redirect nested git commands at the hook repository.
pub const HOOK_VARS: &[&str] = &[
    "GIT_INDEX_FILE=",
    "GIT_PREFIX=",
    "GIT_AUTHOR_NAME=",
    "GIT_AUTHOR_EMAIL=",
    "GIT_AUTHOR_DATE=",
    "GIT_COMMITTER_NAME=",
    "GIT_COMMITTER_EMAIL=",
    "GIT_COMMITTER_DATE=",
    "GIT_DIR=",
    "GIT_WORK_TREE=",
    "GIT_EDITOR=",
    "GIT_REFLOG_ACTION=",
];

/// Returns the current environment without inherited `GIT_*` entries plus hermetic git settings.
pub fn env() -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in std::env::vars() {
        if !k.starts_with("GIT_") {
            out.push(format!("{k}={v}"));
        }
    }
    out.push("GIT_CONFIG_NOSYSTEM=1".to_string());
    out.push(format!(
        "GIT_CONFIG_GLOBAL={}",
        if cfg!(windows) { "NUL" } else { "/dev/null" }
    ));
    out.push("GIT_TERMINAL_PROMPT=0".to_string());
    out.push("GIT_OPTIONAL_LOCKS=0".to_string());
    out
}
/// Removes hook-exported Git variables from an environment vector, preserving other Git settings.
pub fn strip_hook(env: Vec<String>) -> Vec<String> {
    env.into_iter().filter(|kv| !hook_var(kv)).collect()
}

/// Reports whether `kv` is one of the hook-exported Git variable assignments.
pub fn hook_var(kv: &str) -> bool {
    HOOK_VARS.iter().any(|prefix| kv.starts_with(prefix))
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
        assert!(
            !got.iter()
                .any(|kv| kv.starts_with("GIT_DIR=") || kv.starts_with("GIT_INDEX_FILE="))
        );
        assert!(got.iter().any(|kv| kv == "PULSE_GITENV_KEEP=1"));
        assert!(got.iter().any(|kv| kv == "GIT_TERMINAL_PROMPT=0"));
        assert!(got.iter().any(|kv| kv == "GIT_OPTIONAL_LOCKS=0"));
    }
}
