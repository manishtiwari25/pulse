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
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("pulse");
        std::fs::write(&binary, b"not a mach-o").unwrap();
        let binary = binary.display().to_string();

        *SIGNING_SYSTEM.write().unwrap() = "linux".into();
        assert!(!signed(&binary));
        sign(&binary, true).unwrap();
        let mut out = Vec::new();
        ensure_signed(&mut out, &binary);
        assert!(out.is_empty());
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "not a mach-o");

        #[cfg(not(target_os = "macos"))]
        {
            *SIGNING_SYSTEM.write().unwrap() = String::new();
            return;
        }

        #[cfg(target_os = "macos")]
        {
            if which::which("codesign").is_err() {
                *SIGNING_SYSTEM.write().unwrap() = String::new();
                return;
            }
            *SIGNING_SYSTEM.write().unwrap() = "darwin".into();
            let missing = dir.path().join("missing").display().to_string();
            assert!(sign(&missing, true).is_err());
            assert!(!std::path::Path::new(&(missing + ".signing")).exists());

            let build = dir.path().join("hello").display().to_string();
            let src = dir.path().join("hello.go").display().to_string();
            std::fs::write(&src, b"package main\nfunc main() {}\n").unwrap();
            let built = std::process::Command::new("go")
                .args(["build", "-o", &build, &src])
                .output();
            let Ok(built) = built else {
                *SIGNING_SYSTEM.write().unwrap() = String::new();
                return;
            };
            if !built.status.success() {
                *SIGNING_SYSTEM.write().unwrap() = String::new();
                return;
            }
            let (ok, applicable) = pulse_cli::code_signed(&build);
            assert!(
                !ok && applicable,
                "a fresh build is not pulse-signed: {ok} {applicable}"
            );
            sign(&build, false).unwrap();
            assert!(signed(&build));
            let req = std::process::Command::new("codesign")
                .args(["-d", "-r-", &build])
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&req.stdout),
                String::from_utf8_lossy(&req.stderr)
            )
            .replace('"', "");
            assert!(
                text.contains("designated => identifier pulse"),
                "requirement: {text}"
            );
            let verify = std::process::Command::new("codesign")
                .args(["--verify", "--strict", &build])
                .output()
                .unwrap();
            assert!(
                verify.status.success(),
                "the signed build does not verify: {}",
                String::from_utf8_lossy(&verify.stderr)
            );
            out.clear();
            ensure_signed(&mut out, &build);
            let said = String::from_utf8(out.clone()).unwrap();
            assert!(said.contains("is signed as pulse"), "{said}");

            let resign = std::process::Command::new("codesign")
                .args(["--force", "-s", "-", &build])
                .output()
                .unwrap();
            assert!(
                resign.status.success(),
                "re-sign ad-hoc: {}",
                String::from_utf8_lossy(&resign.stderr)
            );
            out.clear();
            ensure_signed(&mut out, &build);
            let said = String::from_utf8(out).unwrap();
            assert!(
                said.contains(&format!("signed {build} as pulse"))
                    && signed(&build)
                    && !std::path::Path::new(&(build + ".signing")).exists(),
                "running-binary sign: {said}"
            );
        }
        *SIGNING_SYSTEM.write().unwrap() = String::new();
    }
}
