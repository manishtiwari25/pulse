//! Git environment scrubbing for child git commands.

/// Go's runtime.GOOS for this build: PULSE stores and compares Go's names
/// ("darwin", not "macos"), so every port of `runtime.GOOS` must use this.
pub const GOOS: &str = if cfg!(target_os = "macos") {
    "darwin"
} else if cfg!(target_os = "linux") {
    "linux"
} else if cfg!(target_os = "windows") {
    "windows"
} else if cfg!(target_os = "freebsd") {
    "freebsd"
} else if cfg!(target_os = "openbsd") {
    "openbsd"
} else if cfg!(target_os = "netbsd") {
    "netbsd"
} else if cfg!(target_os = "dragonfly") {
    "dragonfly"
} else if cfg!(target_os = "android") {
    "android"
} else if cfg!(target_os = "ios") {
    "ios"
} else if cfg!(target_os = "illumos") {
    "illumos"
} else if cfg!(target_os = "solaris") {
    "solaris"
} else if cfg!(target_os = "aix") {
    "aix"
} else {
    std::env::consts::OS
};

/// Go's runtime.GOARCH for this build ("amd64", "arm64", ...).
pub const GOARCH: &str = if cfg!(target_arch = "x86_64") {
    "amd64"
} else if cfg!(target_arch = "aarch64") {
    "arm64"
} else if cfg!(target_arch = "x86") {
    "386"
} else if cfg!(target_arch = "arm") {
    "arm"
} else if cfg!(target_arch = "powerpc64") {
    "ppc64"
} else if cfg!(target_arch = "s390x") {
    "s390x"
} else if cfg!(target_arch = "riscv64") {
    "riscv64"
} else if cfg!(target_arch = "mips64") {
    "mips64"
} else if cfg!(target_arch = "loongarch64") {
    "loong64"
} else {
    std::env::consts::ARCH
};

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
