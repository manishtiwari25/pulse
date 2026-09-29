//! Stamps the binary with the same version Go's toolchain would derive from
//! the checkout (Go >= 1.24 sets the main module version from VCS):
//! the exact semver tag at HEAD, else the pseudo-version
//! `vX.Y.Z-0.yyyymmddhhmmss-<12hex>` / `v0.0.0-yyyymmddhhmmss-<12hex>`,
//! each with `+dirty` when the tree has uncommitted changes.

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

fn semver(tag: &str) -> Option<(u64, u64, u64)> {
    let rest = tag.strip_prefix('v')?;
    let mut it = rest.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Unix seconds -> yyyymmddhhmmss in UTC, like Go's pseudo-version timestamp.
fn stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}{mo:02}{d:02}{h:02}{m:02}{s:02}")
}

/// Go: strconv.Quote, applied by BuildInfo.String() to a setting value that quoteValue()
/// says needs it (" \t\r\n\"`"). Versions and -ldflags values are printable ASCII, so the
/// short escapes are the whole alphabet strconv.Quote would use for them.
fn strconv_quote(value: &str) -> String {
    if !value.contains([' ', '\t', '\r', '\n', '"', '`']) {
        return value.to_string();
    }
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn rust_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The text `debug.ReadBuildInfo` would return for a Go build of cmd/pulse: `pulse version`,
/// `Revision()` and the pair hands inspection all read this block, so a Rust build carries the
/// same lines (leading '\n' so a grep for `^mod\t...` anchors inside the binary's rodata).
fn write_build_info(version: &str, rev: &str, dirty: Option<bool>) {
    let module_version = if rev.is_empty() { "(devel)" } else { version };
    let mut text = format!(
        "\npath\tgithub.com/manishtiwari25/pulse/cmd/pulse\nmod\tgithub.com/manishtiwari25/pulse\t{module_version}\t\n"
    );
    if let Ok(v) = std::env::var("PULSE_VERSION")
        && !v.is_empty()
    {
        text.push_str("build\t-ldflags=");
        text.push_str(&strconv_quote(&format!(
            "-X github.com/manishtiwari25/pulse/internal/cli.version={v}"
        )));
        text.push('\n');
    }
    if let Some(dirty) = dirty {
        text.push_str(&format!("build\tvcs.revision={rev}\n"));
        text.push_str(&format!("build\tvcs.modified={dirty}\n"));
    }
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(
        Path::new(&out).join("build_info_const.rs"),
        format!(
            "pub const BUILD_INFO_TEXT: &str = {};\n",
            rust_string(&text)
        ),
    )
    .expect("write build_info_const.rs");
}

fn main() {
    println!("cargo:rerun-if-env-changed=PULSE_VERSION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    if let Some(r) = fs::read_to_string("../../.git/HEAD")
        .ok()
        .and_then(|head| head.strip_prefix("ref: ").map(|r| r.trim().to_string()))
    {
        println!("cargo:rerun-if-changed=../../.git/{r}");
    }
    if !Path::new("../../.git").exists() {
        write_build_info("dev", "", None);
        return;
    }
    let rev = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    let short = rev.chars().take(12).collect::<String>();
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let suffix = if dirty { "+dirty" } else { "" };
    let mut version = "dev".to_string();
    if short.len() == 12 {
        let exact = git(&["tag", "--points-at", "HEAD"])
            .unwrap_or_default()
            .lines()
            .filter(|t| semver(t).is_some())
            .max_by_key(|t| semver(t))
            .map(str::to_string);
        let secs = git(&["log", "-1", "--format=%ct", "HEAD"])
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        version = match exact {
            Some(tag) => format!("{tag}{suffix}"),
            None => {
                let base = git(&[
                    "describe",
                    "--tags",
                    "--abbrev=0",
                    "--match",
                    "v[0-9]*.[0-9]*.[0-9]*",
                ])
                .and_then(|t| semver(&t));
                match base {
                    Some((ma, mi, pa)) => {
                        format!("v{ma}.{mi}.{}-0.{}-{short}{suffix}", pa + 1, stamp(secs))
                    }
                    None => format!("v0.0.0-{}-{short}{suffix}", stamp(secs)),
                }
            }
        };
    }
    write_build_info(&version, &rev, Some(dirty));
}
