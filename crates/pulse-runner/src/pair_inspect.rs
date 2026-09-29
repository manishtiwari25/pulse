#![allow(dead_code)]

use base64::Engine;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub(crate) const PAIR_HANDS_MODULE: &str = "github.com/manishtiwari25/pulse";
pub(crate) static PAIR_HANDS_VERSION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9.+_-]{0,79}$").unwrap());
pub(crate) static PAIR_HANDS_MARKER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^# pulse-hands ([A-Za-z0-9][A-Za-z0-9.+_-]{0,79})[ \t]*\r?$").unwrap()
});
pub(crate) static PAIR_HANDS_HASH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-f0-9]{64}$").unwrap());
pub(crate) static PAIR_LEGACY_HANDS: &[(&str, &str)] = &[
    (
        "ccb0790",
        "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5118",
    ),
    (
        "53a1580",
        "4121f2dcb5a343f68b8bc2bce7a8e44c8f4eb1c9248c8b46aa1000142c382061",
    ),
    (
        "5eb76b5",
        "b7c859d6a0b25a34757d8b4b4ab4c18f14fb1292cc1e343098a94ec1bdeeed51",
    ),
    (
        "90a5b53",
        "3a0d0b405e9129a12aaebcba56057bc61fcf22655fdc3a073a0ecac7afa03572",
    ),
    (
        "71f4ea1",
        "02d0d9ca42e6721383ad6142b9795ed6284d8373ace3c8448a5396cd68edafba",
    ),
    (
        "d483be9",
        "1840323a68d3ee8e9122f446eaf3c021da308bc3939d041c13d2c33370cca07d",
    ),
    (
        "0627cb1",
        "980ee7cf744715ff4312813bf2d678edbbfb93730ea3b4a4b7d67a0c1f0b0977",
    ),
    (
        "479c333",
        "a1ae2f51c1ffa48af5e8b9107a776944f544c6daa234340b77f9be64b48f9b75",
    ),
    (
        "d80046e",
        "4f0005f607808ec307a9920b9198dd75728a5f16b0d429e9d26b84291378928c",
    ),
    (
        "b31f5a7",
        "cbffffff45c36ff8b660a302de85762a40a554444106cf7aa452e6f0b93af2d7",
    ),
    (
        "e181712",
        "82243fd782b61d9392deb399eefed7eea877f101c13a2ab838e949ef01e84c1b",
    ),
];

/// Go: func pairLegacyHandsIndex(hash string) int
pub(crate) fn pair_legacy_hands_index(hash: String) -> i32 {
    PAIR_LEGACY_HANDS
        .iter()
        .position(|(_, h)| *h == hash)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// Go: func pairInspectHandsScript(path, hash string) string
pub(crate) fn pair_inspect_hands_script(path: String, hash: String) -> String {
    format!(
        r#"HANDS_FILE={path_q}
HANDS_HASH={hash_q}
if [ ! -f "$HANDS_FILE" ] && [ -f "$HANDS_FILE.sh" ]; then HANDS_FILE="$HANDS_FILE.sh"; fi
if [ -f "$HANDS_FILE" ]; then
  if [ ! -r "$HANDS_FILE" ]; then
    printf 'cannot read hands file: %s\n' "$HANDS_FILE" >&2
    exit 1
  fi
  hands_sha256=unknown
  if command -v sha256sum >/dev/null 2>&1; then
    hands_sha256="$(sha256sum "$HANDS_FILE")" || exit 1
    hands_sha256="${{hands_sha256%% *}}"
  fi
  printf 'hands_sha256=%s\n' "$hands_sha256"
  printf 'hands_version=%s\n' "$(head -c 4096 "$HANDS_FILE" | LC_ALL=C sed -n 's/^# pulse-hands \([A-Za-z0-9][A-Za-z0-9.+_-]*\)[[:space:]]*$/\1/p' | head -n 1)"
  printf 'hands_buildinfo=%s\n' "$(LC_ALL=C grep -aE '^(mod[[:space:]]github.com/manishtiwari25/pulse[[:space:]]|build[[:space:]](-ldflags=|vcs\.revision=|vcs\.modified=))' "$HANDS_FILE" | head -c 8192 | base64 | tr -d '\n')"
  if [ -n "$HANDS_HASH" ] && [ "$hands_sha256" = "$HANDS_HASH" ]; then
    echo hands=present
  else
    echo hands=differs
  fi
else
  echo hands=absent
fi
"#,
        path_q = crate::util::shell_quote(&path),
        hash_q = crate::util::shell_quote(&hash)
    )
}

/// Go: func pairHandsBuildVersion(info *debug.BuildInfo) string
pub(crate) fn pair_hands_build_version(info: &serde_json::Value) -> String {
    let path = info
        .pointer("/Main/Path")
        .or_else(|| info.pointer("/main/path"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if path != PAIR_HANDS_MODULE {
        return "unknown".into();
    }
    let mut version = info
        .pointer("/Main/Version")
        .or_else(|| info.pointer("/main/version"))
        .and_then(|v| v.as_str())
        .unwrap_or("(devel)")
        .to_string();
    let mut revision = String::new();
    let mut modified = false;
    if let Some(settings) = info
        .get("Settings")
        .or_else(|| info.get("settings"))
        .and_then(|v| v.as_array())
    {
        for s in settings {
            let key = s
                .get("Key")
                .or_else(|| s.get("key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let value = s
                .get("Value")
                .or_else(|| s.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match key {
                "vcs.revision" => revision = value.to_string(),
                "vcs.modified" => modified = value == "true",
                "-ldflags" => {
                    let args = crate::util::split_shell(value);
                    let mut i = 0;
                    while i < args.len() {
                        let mut arg = args[i].clone();
                        if arg == "-X" && i + 1 < args.len() {
                            i += 1;
                            arg = args[i].clone();
                        } else if let Some(rest) = arg.strip_prefix("-X=") {
                            arg = rest.to_string();
                        } else {
                            i += 1;
                            continue;
                        }
                        if let Some(v) =
                            arg.strip_prefix(&format!("{PAIR_HANDS_MODULE}/internal/cli.version="))
                        {
                            version = if v.is_empty() {
                                version.clone()
                            } else {
                                v.to_string()
                            };
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
        }
    }
    if version == "(devel)" {
        let short = revision.chars().take(12).collect::<String>();
        version = if short.is_empty() {
            "dev".into()
        } else {
            format!("dev-{short}")
        };
        if modified {
            version.push_str("-dirty");
        }
    }
    if !PAIR_HANDS_VERSION_RE.is_match(&version) {
        "unknown".into()
    } else {
        version
    }
}

/// Go: func pairHandsVersion(data []byte) string
pub(crate) fn pair_hands_version(data: Vec<u8>) -> String {
    if let Some((rev, _)) = PAIR_LEGACY_HANDS
        .iter()
        .find(|(_, h)| *h == crate::pair::sha256_bytes(data.clone()))
    {
        return format!("legacy-{rev}");
    }
    let n = data.len().min(4096);
    let head = String::from_utf8_lossy(&data[..n]);
    if let Some(c) = PAIR_HANDS_MARKER_RE.captures(&head) {
        return c[1].to_string();
    }
    parse_buildinfo_text(&String::from_utf8_lossy(&data)).unwrap_or_else(|| "unknown".into())
}

/// Go: func pairInstalledHandsVersion(insp map[string]string) string
pub(crate) fn pair_installed_hands_version(insp: BTreeMap<String, String>) -> String {
    if let Some(version) = insp.get("hands_version")
        && PAIR_HANDS_VERSION_RE.is_match(version)
    {
        return version.clone();
    }
    let encoded = insp.get("hands_buildinfo").cloned().unwrap_or_default();
    if encoded.len() > 12_000 {
        return "unknown".into();
    }
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return "unknown".into();
    };
    parse_buildinfo_text(&String::from_utf8_lossy(&raw)).unwrap_or_else(|| "unknown".into())
}

/// Go: func pairHandsSHA7(hash string) string
pub(crate) fn pair_hands_sha7(hash: String) -> String {
    if PAIR_HANDS_HASH_RE.is_match(&hash) {
        hash[..7].into()
    } else {
        "unknown".into()
    }
}

/// Go: func pairHandsSummary(insp map[string]string, template []byte) string
pub(crate) fn pair_hands_summary(insp: BTreeMap<String, String>, template: Vec<u8>) -> String {
    if insp.get("hands").map(String::as_str) == Some("absent") {
        return "absent".into();
    }
    let installed_hash = insp.get("hands_sha256").cloned().unwrap_or_default();
    let mut template_hash = String::new();
    if !template.is_empty() {
        template_hash = crate::pair::sha256_bytes(template.clone());
        if installed_hash == template_hash {
            return "present".into();
        }
    }
    let mut status = "differs".to_string();
    let mut installed_version = pair_installed_hands_version(insp);
    let template_version = pair_hands_version(template);
    let i = pair_legacy_hands_index(installed_hash.clone());
    if i >= 0 {
        installed_version = format!("legacy-{}", PAIR_LEGACY_HANDS[i as usize].0);
        if template_version != "unknown" && i > pair_legacy_hands_index(template_hash.clone()) {
            status = "older".into();
        }
    }
    format!(
        "{} (installed {} {}, template {} {})",
        status,
        installed_version,
        pair_hands_sha7(installed_hash),
        template_version,
        pair_hands_sha7(template_hash)
    )
}

fn parse_buildinfo_text(text: &str) -> Option<String> {
    let mut module = String::new();
    let mut version = "(devel)".to_string();
    let mut revision = String::new();
    let mut modified = false;
    let mut ldflags = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("mod\t") {
            let parts = rest.split('\t').collect::<Vec<_>>();
            module = parts.first().copied().unwrap_or("").to_string();
            version = parts.get(1).copied().unwrap_or("(devel)").to_string();
        } else if let Some(rest) = line.strip_prefix("build\t") {
            if let Some(v) = rest.strip_prefix("-ldflags=") {
                if v.starts_with('"') && !v.ends_with('"') {
                    return Some("unknown".into());
                }
                ldflags = v.trim_matches('"').to_string();
            } else if let Some(v) = rest.strip_prefix("vcs.revision=") {
                revision = v.to_string();
            } else if let Some(v) = rest.strip_prefix("vcs.modified=") {
                modified = v == "true";
            }
        }
    }
    if module != PAIR_HANDS_MODULE {
        return None;
    }
    if !ldflags.is_empty() {
        let args = crate::util::split_shell(&ldflags);
        let mut i = 0;
        while i < args.len() {
            let mut arg = args[i].clone();
            if arg == "-X" && i + 1 < args.len() {
                i += 1;
                arg = args[i].clone();
            } else if let Some(rest) = arg.strip_prefix("-X=") {
                arg = rest.to_string();
            } else {
                i += 1;
                continue;
            }
            if let Some(v) = arg.strip_prefix(&format!("{PAIR_HANDS_MODULE}/internal/cli.version="))
                && !v.is_empty()
            {
                version = v.to_string();
            }
            i += 1;
        }
    }
    if version == "(devel)" {
        let short = revision.chars().take(12).collect::<String>();
        version = if short.is_empty() {
            "dev".into()
        } else {
            format!("dev-{short}")
        };
        if modified {
            version.push_str("-dirty");
        }
    }
    if PAIR_HANDS_VERSION_RE.is_match(&version) {
        Some(version)
    } else {
        Some("unknown".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::Path;
    use std::process::Command;

    fn b64(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    fn build_info(module_version: &str, flags: &str, revision: &str) -> serde_json::Value {
        serde_json::json!({
            "Main": {"Path": PAIR_HANDS_MODULE, "Version": module_version},
            "Settings": [
                {"Key": "vcs.revision", "Value": revision},
                {"Key": "vcs.modified", "Value": "true"},
                {"Key": "-ldflags", "Value": flags}
            ]
        })
    }

    fn write_exe(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod executable");
        }
    }

    #[test]
    fn pair_hands_build_version() {
        let _serial = crate::test_helpers::serial();
        let revision = "abcdef01".repeat(5);
        for (name, module_version, flags, want) in [
            ("release", "v2.3.4", "", "v2.3.4"),
            ("development", "(devel)", "", "dev-abcdef01abcd-dirty"),
            (
                "linked",
                "(devel)",
                "-s -w -X github.com/manishtiwari25/pulse/internal/cli.version=v2.4.0",
                "v2.4.0",
            ),
            (
                "linked-equals",
                "(devel)",
                "-X=github.com/manishtiwari25/pulse/internal/cli.version=v2.4.1",
                "v2.4.1",
            ),
            (
                "linked-quoted",
                "(devel)",
                "-X 'github.com/manishtiwari25/pulse/internal/cli.version=v2.4.2'",
                "v2.4.2",
            ),
            (
                "linked-precedence",
                "v2.3.4",
                "-X github.com/manishtiwari25/pulse/internal/cli.version=v2.4.3",
                "v2.4.3",
            ),
            (
                "linked-empty",
                "v2.3.4",
                "-X github.com/manishtiwari25/pulse/internal/cli.version=",
                "v2.3.4",
            ),
            (
                "unrelated-override",
                "v2.3.4",
                "-X another/package.version=v9.0.0",
                "v2.3.4",
            ),
            (
                "unsafe-version",
                "(devel)",
                "-X 'github.com/manishtiwari25/pulse/internal/cli.version=v2.4.3\nhands=present'",
                "unknown",
            ),
        ] {
            let info = build_info(module_version, flags, &revision);
            assert_eq!(super::pair_hands_build_version(&info), want, "{name}");
            let text = format!(
                "mod\t{PAIR_HANDS_MODULE}\t{module_version}\t\nbuild\t-ldflags=\"{flags}\"\nbuild\tvcs.revision={revision}\nbuild\tvcs.modified=true\n"
            );
            assert_eq!(
                super::pair_installed_hands_version(BTreeMap::from([(
                    "hands_buildinfo".into(),
                    b64(&text)
                )])),
                want,
                "{name} installed"
            );
        }
        for encoded in [
            "not base64".to_string(),
            b64("mod\tnot-pulse\tv9.0.0\n"),
            b64("build\tbroken\n"),
            "A".repeat(12000),
        ] {
            assert_eq!(
                super::pair_installed_hands_version(BTreeMap::from([(
                    "hands_buildinfo".into(),
                    encoded
                )])),
                "unknown"
            );
        }
        for (text, want) in [
            ("# pulse-hands bad/version\n".to_string(), "unknown"),
            ("#!/bin/sh\n# pulse-hands v1\n".to_string(), "v1"),
            ("#!/bin/sh\r\n# pulse-hands v1\r\n".to_string(), "v1"),
            (format!("# pulse-hands {}\n", "v".repeat(81)), "unknown"),
        ] {
            assert_eq!(super::pair_hands_version(text.into_bytes()), want);
        }
    }

    #[test]
    fn pair_inspect_hands_script() {
        let _serial = crate::test_helpers::serial();
        let shared_bin = tempfile::tempdir().expect("shared bin");
        for name in ["head", "sed", "grep", "base64", "tr"] {
            let tool = which::which(name).unwrap_or_else(|_| panic!("{name} missing"));
            #[cfg(unix)]
            std::os::unix::fs::symlink(tool, shared_bin.path().join(name)).expect("symlink tool");
        }
        let hasher_bin = shared_bin.path().join("hasher");
        let fail_bin = shared_bin.path().join("fail");
        std::fs::create_dir(&hasher_bin).expect("hasher dir");
        std::fs::create_dir(&fail_bin).expect("fail dir");
        write_exe(
            &hasher_bin.join("sha256sum"),
            "#!/bin/sh\n[ \"$1\" = \"$PAIR_TEST_PATH\" ] || exit 2\nprintf '%s  %s\\n' \"$PAIR_TEST_HASH\" \"$1\"\n",
        );
        write_exe(
            &fail_bin.join("sha256sum"),
            "#!/bin/sh\necho 'hash read failed' >&2\nexit 1\n",
        );
        let build_info = format!(
            "mod\t{PAIR_HANDS_MODULE}\t(devel)\t\nbuild\t-ldflags=\"-X {PAIR_HANDS_MODULE}/internal/cli.version=v3.2.1\"\nbuild\tvcs.revision={}\nbuild\tvcs.modified=false\n",
            "ab".repeat(20)
        );
        for tc in [
            (
                "script",
                "#!/bin/sh\n# pulse-hands v1.2.3\n",
                "v1.2.3",
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "crlf",
                "#!/bin/sh\r\n# pulse-hands v1.2.3\r\n",
                "v1.2.3",
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "legacy-path",
                "#!/bin/sh\n# pulse-hands v1.2.3\n",
                "v1.2.3",
                true,
                false,
                false,
                false,
                false,
            ),
            (
                "matching",
                "#!/bin/sh\n# pulse-hands v1.2.3\n",
                "v1.2.3",
                false,
                true,
                false,
                false,
                false,
            ),
            (
                "go-build-info",
                &format!("\u{7f}ELF\u{0}\n{build_info}\u{0}"),
                "v3.2.1",
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "unversioned",
                "#!/bin/sh\n",
                "unknown",
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "missing-sha256sum",
                "#!/bin/sh\n# pulse-hands v1.2.3\n",
                "v1.2.3",
                false,
                false,
                true,
                false,
                false,
            ),
            (
                "hash-error",
                "#!/bin/sh\n",
                "unknown",
                false,
                false,
                false,
                true,
                false,
            ),
            ("absent", "", "unknown", false, false, false, false, true),
        ] {
            let (name, content, want_version, legacy, same, no_hash, hash_fails, absent) = tc;
            let dir = tempfile::tempdir().expect("case dir");
            let path = dir.path().join("hands ' file");
            let installed_path = if legacy {
                path.with_file_name("hands ' file.sh")
            } else {
                path.clone()
            };
            let marker = dir.path().join("executed");
            let content = format!(
                "{content}: > {}\n",
                crate::util::shell_quote(&marker.display().to_string())
            );
            if !absent {
                write_exe(&installed_path, &content);
                if !legacy {
                    write_exe(
                        &path.with_file_name("hands ' file.sh"),
                        "# pulse-hands wrong\n",
                    );
                }
            }
            let mut hash = crate::pair::sha256_bytes(content.clone().into_bytes());
            let path_sep = if cfg!(windows) { ";" } else { ":" };
            let search_path = if no_hash {
                shared_bin.path().display().to_string()
            } else if hash_fails {
                format!(
                    "{}{}{}",
                    fail_bin.display(),
                    path_sep,
                    shared_bin.path().display()
                )
            } else {
                format!(
                    "{}{}{}",
                    hasher_bin.display(),
                    path_sep,
                    shared_bin.path().display()
                )
            };
            let (want_state, expected_hash) = if same {
                ("present", hash.clone())
            } else {
                ("differs", String::new())
            };
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-s")
                .env("PATH", search_path)
                .env("PAIR_TEST_PATH", installed_path.display().to_string())
                .env("PAIR_TEST_HASH", &hash)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().expect("spawn shell");
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(
                    format!(
                        "set -u\n{}",
                        super::pair_inspect_hands_script(path.display().to_string(), expected_hash)
                    )
                    .as_bytes(),
                )
                .expect("write stdin");
            let out = child.wait_with_output().expect("wait shell");
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if hash_fails {
                assert!(
                    !out.status.success() && combined.contains("hash read failed"),
                    "hash error not surfaced: {combined}"
                );
                continue;
            }
            assert!(out.status.success(), "inspect {name}: {combined}");
            let (insp, _) = crate::pair::pair_parse_inspection(
                String::from_utf8_lossy(&out.stdout).to_string(),
            );
            if absent {
                assert_eq!(insp, BTreeMap::from([("hands".into(), "absent".into())]));
                continue;
            }
            if no_hash {
                hash = "unknown".into();
            }
            assert_eq!(
                insp.get("hands").map(String::as_str),
                Some(want_state),
                "{name}: {insp:?}"
            );
            assert_eq!(
                insp.get("hands_sha256").map(String::as_str),
                Some(hash.as_str()),
                "{name}: {insp:?}"
            );
            assert_eq!(
                super::pair_installed_hands_version(insp),
                want_version,
                "{name}"
            );
            assert!(
                std::fs::metadata(&marker).is_err(),
                "installed artifact was executed"
            );
        }
    }

    #[test]
    fn pair_hands_legacy_order() {
        let _serial = crate::test_helpers::serial();
        let newer = b"# pulse-hands new\n".to_vec();
        let older = b"# pulse-hands old\n".to_vec();
        for (installed, template, want) in [
            (
                b"#!/bin/sh\n# pulse-hands v2.0.0\nexit 0\n".to_vec(),
                b"#!/bin/sh\n# pulse-hands v2.0.0\nexit 0\n".to_vec(),
                "present",
            ),
            (older.clone(), newer.clone(), "differs (installed unknown "),
            (newer.clone(), older.clone(), "differs (installed unknown "),
            (older, Vec::new(), "differs (installed unknown "),
        ] {
            let insp = BTreeMap::from([
                ("hands".into(), "differs".into()),
                ("hands_sha256".into(), crate::pair::sha256_bytes(installed)),
            ]);
            assert!(
                super::pair_hands_summary(insp, template).starts_with(want),
                "want prefix {want}"
            );
        }
    }

    #[test]
    fn pair_hands_go_binary_metadata() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().expect("go fixture dir");
        std::fs::create_dir_all(dir.path().join("internal/cli")).expect("cli dir");
        let version_source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("internal/cli/version.go"),
        )
        .expect("read version.go");
        std::fs::write(
            dir.path().join("go.mod"),
            format!("module {PAIR_HANDS_MODULE}\n\ngo 1.26\n"),
        )
        .expect("write go.mod");
        std::fs::write(dir.path().join("internal/cli/version.go"), version_source)
            .expect("write version.go");
        std::fs::write(
            dir.path().join("main.go"),
            format!(
                "package main\nimport (\n\"fmt\"\n\"{PAIR_HANDS_MODULE}/internal/cli\"\n)\nfunc main() {{ fmt.Println(cli.Version()) }}\n"
            ),
        )
        .expect("write main.go");
        let path = dir.path().join("pulse-hands");
        let out = Command::new("go")
            .args([
                "build",
                "-o",
                &path.display().to_string(),
                "-ldflags",
                &format!("-X {PAIR_HANDS_MODULE}/internal/cli.version=v8.7.6"),
                ".",
            ])
            .current_dir(dir.path())
            .env("GOOS", "linux")
            .env("GOARCH", "amd64")
            .env("CGO_ENABLED", "0")
            .env("GOWORK", "off")
            .env("GOPROXY", "off")
            .env("GOSUMDB", "off")
            .env("GOTOOLCHAIN", "local")
            .env("GOFLAGS", "")
            .output()
            .expect("spawn go build");
        assert!(
            out.status.success(),
            "build fixture: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let data = std::fs::read(&path).expect("read binary");
        assert_eq!(super::pair_hands_version(data), "v8.7.6");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-s")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("spawn shell");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(
                format!(
                    "set -u\n{}",
                    super::pair_inspect_hands_script(path.display().to_string(), String::new())
                )
                .as_bytes(),
            )
            .expect("write stdin");
        let out = child.wait_with_output().expect("wait shell");
        assert!(
            out.status.success(),
            "inspect Go binary: {}\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let (insp, _) =
            crate::pair::pair_parse_inspection(String::from_utf8_lossy(&out.stdout).to_string());
        assert_eq!(super::pair_installed_hands_version(insp), "v8.7.6");
    }
}
