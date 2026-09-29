#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::Result;
use std::io::Write;
use std::process::Command;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
pub(crate) const SIGNING_IDENTIFIER: &str = pulse_cli::SIGNING_IDENTIFIER;
pub(crate) const SIGNING_REQUIREMENT_PREFIX: &str = "=designated => identifier ";
pub(crate) const SIGNING_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) static SIGNING_SYSTEM: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(String::new()));
/// Go: func signingOS() string
pub(crate) fn signing_os() -> String {
    let v = SIGNING_SYSTEM.read().unwrap().clone();
    if v.is_empty() {
        std::env::consts::OS.to_string()
    } else {
        v
    }
}
/// Go: func signed(path string) bool
pub(crate) fn signed(path: &str) -> bool {
    if signing_os() != "macos" && signing_os() != "darwin" {
        return false;
    }
    pulse_cli::code_signed(path).0
}
/// Go: func sign(path string, running bool) error
pub(crate) fn sign(path: &str, running: bool) -> Result<()> {
    if signing_os() != "macos" && signing_os() != "darwin" {
        return Ok(());
    }
    let target = if running {
        let tmp = format!("{path}.signing");
        copy_file(path, &tmp)?;
        tmp
    } else {
        path.to_string()
    };
    let requirement = format!("{SIGNING_REQUIREMENT_PREFIX}\"{SIGNING_IDENTIFIER}\"");
    let out = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            SIGNING_IDENTIFIER,
            "--requirements",
            &requirement,
            &target,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            if running {
                let _ = std::fs::remove_file(&target);
            }
            let text = String::from_utf8_lossy(if o.stderr.is_empty() {
                &o.stdout
            } else {
                &o.stderr
            })
            .trim()
            .to_string();
            return Err(anyhow::anyhow!("codesign: {text}"));
        }
        Err(e) => {
            if running {
                let _ = std::fs::remove_file(&target);
            }
            return Err(anyhow::anyhow!("codesign: {e}"));
        }
    }
    if running {
        std::fs::rename(&target, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&target);
        })?;
    }
    Ok(())
}
/// Go: func ensureSigned(out io.Writer, binary string)
pub(crate) fn ensure_signed(out: &mut dyn Write, binary: &str) {
    if signing_os() != "macos" && signing_os() != "darwin" {
        return;
    }
    if signed(binary) {
        let _ = writeln!(
            out,
            "macOS: {binary} is signed as pulse; folder answers outlive rebuilds."
        );
        return;
    }
    match sign(binary, true) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "macOS: signed {binary} as pulse; macOS asks about your folders one last time, and every later build keeps the answer."
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "macOS: could not sign {binary} ({e}); each rebuild asks about your folders again."
            );
        }
    }
}
/// Go: func signNewBuild(_ *store.Store, path string) error
pub(crate) fn sign_new_build(s: &pulse_store::Store, path: &str) -> Result<()> {
    let _ = s;
    sign(path, false)
}
/// Go: func copyFile(src, dst string) error
pub(crate) fn copy_file(src: &str, dst: &str) -> Result<()> {
    std::fs::copy(src, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_gives_every_build_the_same_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("pulse");
        std::fs::write(&binary, b"not a mach-o").unwrap();

        *SIGNING_SYSTEM.write().unwrap() = "linux".into();
        assert!(!signed(&binary.display().to_string()));
        sign(&binary.display().to_string(), true).unwrap();
        let mut out = Vec::new();
        ensure_signed(&mut out, &binary.display().to_string());
        assert!(out.is_empty());
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "not a mach-o");
        *SIGNING_SYSTEM.write().unwrap() = String::new();
    }
}
