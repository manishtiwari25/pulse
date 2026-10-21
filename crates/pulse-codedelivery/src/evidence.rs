use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const EVIDENCE_VERSION: i32 = 1;
pub const EVIDENCE_RECORDED: &str = "recorded";
pub const EVIDENCE_GIT: &str = "git";
pub const EVIDENCE_UNAVAILABLE: &str = "unavailable";

pub const DEFAULT_CODE_PATH_LIMIT: usize = 50;
pub const DEFAULT_FILE_LIMIT: usize = 1000;

pub const REASON_NO_TRUSTWORTHY_EVIDENCE: &str = "No trustworthy changed-file evidence";
pub const REASON_MISSING_TRANSITION: &str = "Missing landing transition evidence";
pub const REASON_GIT_UNAVAILABLE: &str = "Git changed-file evidence is unavailable";
pub const REASON_EVIDENCE_MALFORMED: &str = "Changed-file evidence is malformed";
pub const REASON_EVIDENCE_TOO_LARGE: &str = "Changed-file evidence exceeded the bounded scan";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    Code,
    Docs,
    NonCode,
}

pub const FILE_CODE: FileClass = FileClass::Code;
pub const FILE_DOCS: FileClass = FileClass::Docs;
pub const FILE_NON_CODE: FileClass = FileClass::NonCode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub status: String,
    pub path: String,
    #[serde(rename = "old_path", skip_serializing_if = "String::is_empty", default)]
    pub old_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub version: i32,
    pub evidence: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub before: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub after: String,
    pub files_changed: i32,
    pub code_files: i32,
    pub docs_files: i32,
    pub non_code_files: i32,
    pub code_paths: Vec<String>,
    pub paths_truncated: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub changes_truncated: bool,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source: String,
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub generation: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSnapshot {
    pub name: String,
    pub runner: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub session: String,
    #[serde(rename = "run_id", skip_serializing_if = "String::is_empty", default)]
    pub run_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub executor: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSnapshot {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub job: Option<JobSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker: Option<WorkerSnapshot>,
}

pub fn unavailable_evidence(before: &str, after: &str, reason: &str) -> Evidence {
    Evidence {
        version: EVIDENCE_VERSION,
        evidence: EVIDENCE_UNAVAILABLE.to_string(),
        before: before.to_string(),
        after: after.to_string(),
        code_paths: Vec::new(),
        reason: public_reason(reason),
        ..Evidence::default()
    }
}

pub fn public_reason(reason: &str) -> String {
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    match reason.as_str() {
        "" => REASON_NO_TRUSTWORTHY_EVIDENCE.to_string(),
        REASON_NO_TRUSTWORTHY_EVIDENCE
        | REASON_MISSING_TRANSITION
        | REASON_GIT_UNAVAILABLE
        | REASON_EVIDENCE_MALFORMED
        | REASON_EVIDENCE_TOO_LARGE => reason,
        _ => {
            let lower = reason.to_ascii_lowercase();
            if lower.contains("exceeded") {
                REASON_EVIDENCE_TOO_LARGE.to_string()
            } else if lower.contains("malformed") || lower.contains("name-status") {
                REASON_EVIDENCE_MALFORMED.to_string()
            } else if lower.contains("git") {
                REASON_GIT_UNAVAILABLE.to_string()
            } else if lower.contains("missing") {
                REASON_MISSING_TRANSITION.to_string()
            } else {
                REASON_NO_TRUSTWORTHY_EVIDENCE.to_string()
            }
        }
    }
}

pub fn summarize_name_status(
    raw: &[u8],
    before: &str,
    after: &str,
    path_limit: usize,
    file_limit: usize,
    binary_paths: Option<&BTreeMap<String, bool>>,
) -> Result<Evidence> {
    let path_limit = if path_limit == 0 {
        DEFAULT_CODE_PATH_LIMIT
    } else {
        path_limit
    };
    let file_limit = if file_limit == 0 {
        DEFAULT_FILE_LIMIT
    } else {
        file_limit
    };
    let (changes, truncated) = parse_name_status(raw, file_limit)?;
    let empty = BTreeMap::new();
    let binaries = binary_paths.unwrap_or(&empty);
    let mut e = Evidence {
        version: EVIDENCE_VERSION,
        evidence: EVIDENCE_RECORDED.to_string(),
        before: before.to_string(),
        after: after.to_string(),
        code_paths: Vec::new(),
        changes_truncated: truncated,
        ..Evidence::default()
    };
    let mut seen_code_paths = BTreeSet::new();
    for change in changes {
        let (class, code_path) = classify_change(&change, binaries);
        e.files_changed += 1;
        match class {
            FileClass::Code => {
                e.code_files += 1;
                if seen_code_paths.insert(code_path.clone()) {
                    if e.code_paths.len() < path_limit {
                        e.code_paths.push(code_path);
                    } else {
                        e.paths_truncated = true;
                    }
                }
            }
            FileClass::Docs => e.docs_files += 1,
            FileClass::NonCode => e.non_code_files += 1,
        }
    }
    if truncated && e.code_paths.len() >= path_limit {
        e.paths_truncated = true;
    }
    e.code_paths.sort();
    Ok(e)
}

pub fn parse_name_status(raw: &[u8], file_limit: usize) -> Result<(Vec<Change>, bool)> {
    let text = String::from_utf8_lossy(raw);
    let mut text = text.as_ref();
    let mut changes = Vec::new();
    let mut truncated = false;
    while !text.is_empty() {
        let (field, rest) = cut_nul(text);
        text = rest;
        let status = field.trim().to_string();
        if status.is_empty() {
            continue;
        }
        if text.is_empty() {
            return Err(anyhow!(
                "git name-status output ended after status {status:?}"
            ));
        }
        let code = status.as_bytes()[0];
        if code == b'R' || code == b'C' {
            let (raw_old, rest) = cut_nul(text);
            text = rest;
            let old_path = clean_repo_path(raw_old)
                .ok_or_else(|| anyhow!("git reported an unsafe repository path"))?;
            if text.is_empty() {
                return Err(anyhow!("git name-status output ended during {status:?}"));
            }
            let (raw_path, rest) = cut_nul(text);
            text = rest;
            let path = clean_repo_path(raw_path)
                .ok_or_else(|| anyhow!("git reported an unsafe repository path"))?;
            if changes.len() < file_limit {
                changes.push(Change {
                    status,
                    path,
                    old_path,
                });
            } else {
                truncated = true;
            }
            continue;
        }
        let (raw_path, rest) = cut_nul(text);
        text = rest;
        let path = clean_repo_path(raw_path)
            .ok_or_else(|| anyhow!("git reported an unsafe repository path"))?;
        if changes.len() < file_limit {
            changes.push(Change {
                status,
                path,
                old_path: String::new(),
            });
        } else {
            truncated = true;
        }
    }
    Ok((changes, truncated))
}

pub fn classify_path(path: &str) -> FileClass {
    let Some(clean) = clean_repo_path(path) else {
        return FileClass::NonCode;
    };
    let lower = clean.to_ascii_lowercase();
    if generated_or_vendor(&lower) || lower.contains(".min.") {
        return FileClass::NonCode;
    }
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = extension(base);
    if generated_file(base) || config_file(base) {
        return FileClass::NonCode;
    }
    match ext {
        ".md" | ".mdx" | ".rst" | ".adoc" | ".txt" => FileClass::Docs,
        ".go" | ".py" | ".js" | ".jsx" | ".mjs" | ".cjs" | ".ts" | ".tsx" | ".css" | ".scss"
        | ".sass" | ".less" | ".java" | ".kt" | ".kts" | ".rs" | ".c" | ".h" | ".cc" | ".cpp"
        | ".cxx" | ".hpp" | ".hh" | ".m" | ".mm" | ".swift" | ".rb" | ".php" | ".cs" | ".fs"
        | ".fsx" | ".scala" | ".sh" | ".bash" | ".zsh" | ".fish" | ".ps1" | ".vue" | ".svelte"
        | ".html" | ".htm" => FileClass::Code,
        _ => FileClass::NonCode,
    }
}

pub fn decode_evidence(value: Option<&Value>) -> (Evidence, bool) {
    let Some(value) = value else {
        return (Evidence::default(), false);
    };
    let Ok(e) = serde_json::from_value::<Evidence>(value.clone()) else {
        return (Evidence::default(), false);
    };
    if e.version != EVIDENCE_VERSION
        || (e.evidence != EVIDENCE_RECORDED
            && e.evidence != EVIDENCE_GIT
            && e.evidence != EVIDENCE_UNAVAILABLE)
    {
        return (Evidence::default(), false);
    }
    (e, true)
}

pub fn decode_work_snapshot(value: Option<&Value>) -> WorkSnapshot {
    let Some(value) = value else {
        return WorkSnapshot::default();
    };
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn classify_change(change: &Change, binary_paths: &BTreeMap<String, bool>) -> (FileClass, String) {
    if binary_paths.get(&change.path).copied().unwrap_or(false)
        || (!change.old_path.is_empty()
            && binary_paths.get(&change.old_path).copied().unwrap_or(false))
    {
        return (FileClass::NonCode, String::new());
    }
    let class = classify_path(&change.path);
    let code_path = change.path.clone();
    if class != FileClass::Code && !change.old_path.is_empty() {
        let old_class = classify_path(&change.old_path);
        if old_class == FileClass::Code {
            return (FileClass::Code, change.old_path.clone());
        }
        if class == FileClass::NonCode && old_class == FileClass::Docs {
            return (FileClass::Docs, String::new());
        }
    }
    (class, code_path)
}

pub fn parse_numstat_binary_paths(
    raw: &[u8],
    file_limit: usize,
) -> Result<(BTreeMap<String, bool>, bool)> {
    let text = String::from_utf8_lossy(raw);
    let mut out = BTreeMap::new();
    let mut truncated = false;
    let mut seen = 0usize;
    for record in text.split('\0') {
        if record.is_empty() {
            continue;
        }
        let fields = record.splitn(3, '\t').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(anyhow!("git numstat output is malformed"));
        }
        if fields[0] != "-" || fields[1] != "-" {
            continue;
        }
        seen += 1;
        if seen > file_limit {
            truncated = true;
            continue;
        }
        let path = clean_repo_path(fields[2])
            .ok_or_else(|| anyhow!("git reported an unsafe repository path"))?;
        out.insert(path, true);
    }
    Ok((out, truncated))
}

pub(crate) fn safe_code_paths_for_report(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| classify_path(path) == FileClass::Code)
        .cloned()
        .collect()
}

fn generated_file(base: &str) -> bool {
    if base == "next-env.d.ts" || base.starts_with("zz_generated.") {
        return true;
    }
    let stem = base.strip_suffix(extension(base)).unwrap_or(base);
    base.ends_with(".pb.go")
        || base.ends_with(".gen.go")
        || base.ends_with("_generated.go")
        || base.ends_with(".generated.go")
        || stem.ends_with(".generated")
        || stem.ends_with(".gen")
        || stem.ends_with(".pb")
        || stem.ends_with("_pb")
}

fn config_file(base: &str) -> bool {
    if base.contains(".config.") {
        return true;
    }
    for prefix in [
        ".eslintrc.",
        ".babelrc.",
        ".prettierrc.",
        ".stylelintrc.",
        ".lintstagedrc.",
        ".commitlintrc.",
        ".mocharc.",
    ] {
        if base.starts_with(prefix) {
            return true;
        }
    }
    matches!(base, "dockerfile" | "containerfile")
}

fn generated_or_vendor(path: &str) -> bool {
    if path == "web/hub/out" || path.starts_with("web/hub/out/") {
        return true;
    }
    path.split('/').any(|part| {
        matches!(
            part,
            "node_modules" | "vendor" | ".next" | "coverage" | "generated" | ".generated"
        )
    })
}

fn clean_repo_path(path: &str) -> Option<String> {
    if path.contains('\0') || path.trim().is_empty() || path.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn cut_nul(text: &str) -> (&str, &str) {
    match text.find('\0') {
        Some(i) => (&text[..i], &text[i + 1..]),
        None => (text, ""),
    }
}

fn extension(base: &str) -> &str {
    match base.rfind('.') {
        Some(0) | None => "",
        Some(i) => &base[i..],
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_name_status_classifies_code_docs_generated_renames_and_deletions() {
        let raw = [
            "A",
            "internal/api/server.go",
            "M",
            "README.md",
            "M",
            "web/hub/src/card.css",
            "M",
            "src/main.mjs",
            "M",
            "src/main.cjs",
            "R100",
            "internal/old.go",
            "internal/new.go",
            "R100",
            "internal/code.go",
            "docs/code.md",
            "R100",
            "docs/guide.md",
            "internal/guide.go",
            "D",
            "cmd/deleted/main.go",
            "M",
            "web/hub/out/index.js",
            "M",
            "package.json",
            "M",
            "next.config.ts",
            "M",
            "eslint.config.js",
            "M",
            "api/service.pb.go",
            "M",
            "scripts/run.sh",
            "M",
            "web/hub/src/minified.min.js",
            "M",
            "assets/logo.png",
            "",
        ]
        .join("\0");
        let binaries = BTreeMap::from([("assets/logo.png".to_string(), true)]);
        let evidence =
            summarize_name_status(raw.as_bytes(), "before", "after", 20, 100, Some(&binaries))
                .unwrap();
        assert_eq!(evidence.files_changed, 17);
        assert_eq!(evidence.code_files, 9);
        assert_eq!(evidence.docs_files, 1);
        assert_eq!(evidence.non_code_files, 7);
        assert_eq!(
            evidence.code_paths.join(","),
            "cmd/deleted/main.go,internal/api/server.go,internal/code.go,internal/guide.go,internal/new.go,scripts/run.sh,src/main.cjs,src/main.mjs,web/hub/src/card.css"
        );
        assert!(!evidence.paths_truncated);
        assert!(!evidence.changes_truncated);
        assert_eq!(evidence.evidence, EVIDENCE_RECORDED);
        assert_eq!(evidence.version, EVIDENCE_VERSION);
    }

    #[test]
    fn summarize_name_status_bounds_preview_and_file_count() {
        let raw = ["M", "a.go", "M", "b.go", "M", "c.go", ""].join("\0");
        let evidence =
            summarize_name_status(raw.as_bytes(), "before", "after", 2, 2, None).unwrap();
        assert_eq!(evidence.files_changed, 2);
        assert_eq!(evidence.code_files, 2);
        assert_eq!(evidence.code_paths.join(","), "a.go,b.go");
        assert!(evidence.changes_truncated);
        assert!(evidence.paths_truncated);
    }

    #[test]
    fn parse_name_status_rejects_unsafe_or_malformed_paths() {
        for raw in [
            vec!["M"],
            vec!["M", "../outside.go"],
            vec!["R100", "old.go"],
            vec!["R100", "old.go", "/tmp/new.go"],
        ] {
            assert!(
                parse_name_status(format!("{}\0", raw.join("\0")).as_bytes(), 100).is_err(),
                "{}",
                raw.join(",")
            );
        }
    }

    #[test]
    fn parse_numstat_binary_paths() {
        let raw = [
            "1\t0\tinternal/api.go",
            "-\t-\tassets/logo.png",
            "-\t-\tassets/other.png",
            "",
        ]
        .join("\0");
        let (binaries, truncated) = super::parse_numstat_binary_paths(raw.as_bytes(), 1).unwrap();
        assert!(truncated);
        assert!(binaries["assets/logo.png"]);
        assert!(!binaries.contains_key("assets/other.png"));
    }

    #[test]
    fn code_delivery_config_and_generated_conventions() {
        for path in [
            ".eslintrc.cjs",
            ".babelrc.js",
            ".prettierrc.mjs",
            ".stylelintrc.ts",
            ".lintstagedrc.js",
            ".commitlintrc.cjs",
            ".mocharc.cjs",
            "next-env.d.ts",
            "src/schema.generated.ts",
            "src/service_pb.js",
            "internal/zz_generated.deepcopy.go",
        ] {
            assert_eq!(classify_path(path), FileClass::NonCode, "{path}");
        }
        for path in [
            "src/main.mjs",
            "src/main.cjs",
            "src/types.d.ts",
            "src/editor.tsx",
        ] {
            assert_eq!(classify_path(path), FileClass::Code, "{path}");
        }
    }
}
