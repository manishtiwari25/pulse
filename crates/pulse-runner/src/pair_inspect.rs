#![allow(dead_code)]

use base64::Engine;
use std::collections::BTreeMap;
use std::sync::LazyLock;
#[cfg(test)]
use std::sync::RwLock;

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

#[cfg(test)]
#[allow(clippy::type_complexity)]
static PAIR_LEGACY_HANDS_TEST: LazyLock<RwLock<Option<Vec<(String, String)>>>> =
    LazyLock::new(|| RwLock::new(None));

fn pair_legacy_hands() -> Vec<(String, String)> {
    #[cfg(test)]
    if let Some(v) = PAIR_LEGACY_HANDS_TEST
        .read()
        .expect("legacy hands poisoned")
        .clone()
    {
        return v;
    }
    PAIR_LEGACY_HANDS
        .iter()
        .map(|(revision, hash)| ((*revision).to_string(), (*hash).to_string()))
        .collect()
}

pub(crate) fn pair_legacy_hands_index(hash: String) -> i32 {
    pair_legacy_hands()
        .iter()
        .position(|(_, h)| *h == hash)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

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

pub(crate) fn pair_hands_build_version(info: &pulse_cli::BuildInfo) -> String {
    if info.main.path != PAIR_HANDS_MODULE {
        return "unknown".into();
    }
    let mut version = pulse_cli::build_version(info);
    for setting in &info.settings {
        if setting.key != "-ldflags" {
            continue;
        }
        let args = crate::util::split_shell(&setting.value);
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
            {
                version = if v.is_empty() {
                    pulse_cli::build_version(info)
                } else {
                    v.to_string()
                };
            }
            i += 1;
        }
    }
    if !PAIR_HANDS_VERSION_RE.is_match(&version) {
        "unknown".into()
    } else {
        version
    }
}

fn pair_buildinfo_from_grep_bytes(data: &[u8]) -> Option<pulse_cli::BuildInfo> {
    let mut text = String::new();
    for raw in data.split(|b| *b == b'\n') {
        let mut line = raw;
        if let Some(i) = line.iter().position(|b| *b == 0) {
            line = &line[..i];
        }
        if line.starts_with(format!("mod\t{PAIR_HANDS_MODULE}\t").as_bytes())
            || line.starts_with(b"build\t-ldflags=")
            || line.starts_with(b"build\tvcs.revision=")
            || line.starts_with(b"build\tvcs.modified=")
        {
            text.push_str(&String::from_utf8_lossy(line));
            text.push('\n');
        }
    }
    pulse_cli::parse_build_info(&text)
}

pub(crate) fn pair_hands_version(data: Vec<u8>) -> String {
    if let Some((rev, _)) = pair_legacy_hands()
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
    pair_buildinfo_from_grep_bytes(&data)
        .map(|info| pair_hands_build_version(&info))
        .unwrap_or_else(|| "unknown".into())
}

pub(crate) fn pair_installed_hands_version(insp: BTreeMap<String, String>) -> String {
    if let Some(version) = insp.get("hands_version")
        && PAIR_HANDS_VERSION_RE.is_match(version)
    {
        return version.clone();
    }
    let encoded = insp.get("hands_buildinfo").cloned().unwrap_or_default();
    if encoded.len() > 10924 {
        return "unknown".into();
    }
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return "unknown".into();
    };
    pulse_cli::parse_build_info(&String::from_utf8_lossy(&raw))
        .map(|info| pair_hands_build_version(&info))
        .unwrap_or_else(|| "unknown".into())
}

pub(crate) fn pair_hands_sha7(hash: String) -> String {
    if PAIR_HANDS_HASH_RE.is_match(&hash) {
        hash[..7].into()
    } else {
        "unknown".into()
    }
}

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
        installed_version = format!("legacy-{}", pair_legacy_hands()[i as usize].0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    fn base64_std(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    fn build_info(module_version: &str, flags: &str, revision: &str) -> pulse_cli::BuildInfo {
        pulse_cli::BuildInfo {
            path: String::new(),
            main: pulse_cli::Module {
                path: PAIR_HANDS_MODULE.to_string(),
                version: module_version.to_string(),
                sum: String::new(),
            },
            deps: Vec::new(),
            settings: vec![
                pulse_cli::BuildSetting {
                    key: "vcs.revision".to_string(),
                    value: revision.to_string(),
                },
                pulse_cli::BuildSetting {
                    key: "vcs.modified".to_string(),
                    value: "true".to_string(),
                },
                pulse_cli::BuildSetting {
                    key: "-ldflags".to_string(),
                    value: flags.to_string(),
                },
            ],
        }
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
                    base64_std(&text)
                )])),
                want,
                "{name} installed"
            );
        }
        for encoded in [
            "not base64".to_string(),
            base64_std("mod\tnot-pulse\tv9.0.0\n"),
            base64_std("build\tbroken\n"),
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
        let _deadline = crate::test_helpers_daemon::test_deadline(Duration::from_secs(60));
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
        let _deadline = crate::test_helpers_daemon::test_deadline(Duration::from_secs(60));
        let dir = tempfile::tempdir().expect("rust fixture dir");
        let path = dir.path().join("pulse-hands");
        let src = dir.path().join("main.rs");
        let build_info = format!(
            "\npath\t{PAIR_HANDS_MODULE}/cmd/pulse\nmod\t{PAIR_HANDS_MODULE}\t(devel)\t\nbuild\t-ldflags=\"-X {PAIR_HANDS_MODULE}/internal/cli.version=v8.7.6\"\nbuild\tvcs.revision={}\nbuild\tvcs.modified=false\n",
            "ab".repeat(20)
        );
        std::fs::write(
            &src,
            format!(
                "#[used]\nstatic BUILD_INFO: &str = {build_info:?};\nfn main() {{ println!(\"{{}}\", BUILD_INFO.len()); }}\n"
            ),
        )
        .expect("write main.rs");
        let out = Command::new("rustc")
            .args([
                "-O",
                "-o",
                &path.display().to_string(),
                &src.display().to_string(),
            ])
            .output()
            .expect("spawn rustc");
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
