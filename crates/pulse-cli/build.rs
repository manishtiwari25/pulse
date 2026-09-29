use std::fs;
use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Some(r) = fs::read_to_string("../../.git/HEAD")
        .ok()
        .and_then(|head| head.strip_prefix("ref: ").map(|r| r.trim().to_string()))
    {
        println!("cargo:rerun-if-changed=../../.git/{r}");
    }
    if !Path::new("../../.git").exists() {
        println!("cargo:rustc-env=PULSE_BUILD_VERSION=dev");
        println!("cargo:rustc-env=PULSE_REVISION=");
        return;
    }
    let rev = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    let short = rev.chars().take(12).collect::<String>();
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let version = if short.len() == 12 {
        format!("dev-{short}{}", if dirty { "-dirty" } else { "" })
    } else {
        "dev".to_string()
    };
    println!("cargo:rustc-env=PULSE_BUILD_VERSION={version}");
    println!("cargo:rustc-env=PULSE_REVISION={rev}");
}
