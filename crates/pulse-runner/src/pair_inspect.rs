#![allow(dead_code)]

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

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
    if let Some(version) = insp.get("hands_version") {
        if PAIR_HANDS_VERSION_RE.is_match(version) {
            return version.clone();
        }
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
            {
                if !v.is_empty() {
                    version = v.to_string();
                }
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
