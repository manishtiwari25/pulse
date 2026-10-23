use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

pub const CARD_CLASS_PRODUCT: &str = "product";
pub const CARD_CLASS_OPERATING_LAYER: &str = "operating-layer";

static CARD_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:^|[\s("'`])((?:\.?/|/)?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.[A-Za-z0-9_.-]+)"#,
    )
    .unwrap()
});

pub(crate) fn card_fields(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.split('\n') {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim_matches(['*', ' ', '\t']).to_ascii_lowercase();
        let value = value.trim().trim_matches('`').to_string();
        match out.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut e) => {
                e.insert(String::new());
            }
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(value);
            }
        }
    }
    out
}

pub(crate) fn first_card_text(items: &[String]) -> String {
    items
        .iter()
        .find_map(|s| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
        .unwrap_or_default()
}

pub fn card_class(summary: &str, body: &str) -> String {
    let fields = card_fields(body);
    let declared = first_card_text(&[
        fields.get("file class").cloned().unwrap_or_default(),
        fields.get("class").cloned().unwrap_or_default(),
    ])
    .to_ascii_lowercase();
    let file_paths = card_paths(&first_card_text(&[
        fields.get("file").cloned().unwrap_or_default(),
        fields.get("path").cloned().unwrap_or_default(),
    ]));
    let mut paths = file_paths.clone();
    if paths.is_empty() {
        paths = sole_mentioned_path(&format!("{summary}\n{body}"));
    }
    for p in &paths {
        if is_operating_layer_path(p) {
            return CARD_CLASS_OPERATING_LAYER.into();
        }
    }
    if declared.contains(CARD_CLASS_OPERATING_LAYER) {
        return CARD_CLASS_OPERATING_LAYER.into();
    }
    let product_path = file_paths
        .iter()
        .any(|p| !p.trim().is_empty() && !Path::new(p.trim()).is_absolute());
    if check_only_card(&fields, summary) {
        return CARD_CLASS_OPERATING_LAYER.into();
    }
    if declared.contains(CARD_CLASS_PRODUCT) || product_path {
        return CARD_CLASS_PRODUCT.into();
    }
    CARD_CLASS_OPERATING_LAYER.into()
}

fn card_paths(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|part| {
            let p = part.trim().trim_matches(['`', '\'', '"']);
            (!p.is_empty()).then(|| p.to_string())
        })
        .collect()
}

fn sole_mentioned_path(text: &str) -> Vec<String> {
    let mut seen = BTreeMap::new();
    for m in CARD_PATH_RE.captures_iter(text) {
        let raw = m[1].trim_matches([
            '`', '\'', '"', '.', ',', ';', ':', '(', ')', '[', ']', '{', '}',
        ]);
        if raw.is_empty() || raw.contains('=') {
            continue;
        }
        let n = normalize_card_path(raw);
        if n.is_empty() {
            continue;
        }
        seen.insert(n, raw.to_string());
    }
    if seen.len() != 1 {
        return Vec::new();
    }
    seen.into_values().collect()
}

fn normalize_card_path(p: &str) -> String {
    let mut p = p.trim().trim_matches(['`', '\'', '"']).to_string();
    if let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    let p = clean_relative_path(&p)
        .replace('\\', "/")
        .to_ascii_lowercase();
    if p == "." || p == ".." || p.starts_with("../") {
        String::new()
    } else {
        p
    }
}

fn clean_relative_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let absolute = p.starts_with('/');
    let normalized = p.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

static OPERATING_LAYER_DIRS: &[&str] = &[
    ".github/",
    ".githooks/",
    ".claude/",
    ".copilot/",
    ".cursor/",
    ".codex/",
    ".agents/",
    "docs/memory/",
    "docs/plans/",
    "docs/decisions/",
    "docs/evolution/",
    "docs/context/",
    "docs/scripts/",
    "docs/prompts/",
    "docs/skills/",
];

pub fn is_operating_layer_path(path: &str) -> bool {
    let p = normalize_card_path(path);
    if p.is_empty() {
        return false;
    }
    let base = p.rsplit('/').next().unwrap_or(&p);
    if base.ends_with("_test.go") || base.contains(".test.") || base.contains(".spec.") {
        return true;
    }
    if matches!(
        base,
        "makefile"
            | "go.sum"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "gemfile.lock"
            | "poetry.lock"
            | "cargo.lock"
            | ".editorconfig"
            | ".pre-commit-config.yaml"
            | ".gitattributes"
            | "agents.md"
            | "claude.md"
            | "copilot-instructions.md"
    ) {
        return true;
    }
    if p == "tools/test.sh" || OPERATING_LAYER_DIRS.iter().any(|d| p.starts_with(d)) {
        return true;
    }
    for prefix in [
        ".golangci",
        ".eslintrc",
        "eslint.config.",
        ".prettierrc",
        "prettier.config.",
        "vitest.config.",
        "jest.config.",
        "playwright.config.",
        "pytest.ini",
        "tox.ini",
        ".mocharc",
        "tsconfig",
        "check-",
        "check_",
        "check.",
        "test.",
        "test-",
        "test_",
        "run-tests",
        "run_tests",
        "lint.",
    ] {
        if base.starts_with(prefix) {
            return true;
        }
    }
    for seg in p.split('/') {
        if matches!(
            seg,
            "__tests__"
                | "__mocks__"
                | "tests"
                | "test"
                | "testutil"
                | "testdata"
                | "spec"
                | "fixtures"
                | "e2e"
                | "ci"
                | ".git"
        ) || seg.ends_with(".tests")
            || seg.ends_with("_tests")
            || seg.ends_with("-tests")
            || seg.ends_with(".test")
        {
            return true;
        }
    }
    false
}

static CHECK_TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)(?:\bgo (?:test|vet|build)\b|\bgofmt\b|\bgolangci\b|\bgit_\w+\b|\bgit\w*env\w*\b|\bos\.environ\b|\bnpm (?:run|test|t)\b|\bpnpm\b|\bnpx\b|\byarn (?:run|test)\b|\bcorepack\b|\bmake (?:test|check|lint)\b|\bdotnet test\b|\bvitest\b|\bjest\b|\bmocha\b|\bpytest\b|\bunittest\b|\bcargo test\b|\btsc\b|\btypecheck\b|\btype-check\b|\blint(?:er|ing)?\b|\beslint\b|\bprettier\b|\bcheck(?:s|-[a-z]+)?\b|\bci\b|\bpre-commit\b|\bhook\b|\btests?\b|\btest[A-Z]\w*|\b[a-z][\w./-]*_test\.go\b|\.(?:test|spec)\.[a-z]+\b|\bfixtures?\b|\btestdata\b|\bassert(?:s|ed|ion|ions|ing)?\b|\bstays? (?:green|ok|passing)\b|\bpasses green\b|\bto green\b|\bgoes green\b|->\s*(?:ok|pass)\b|\bfail(?:s|ing|ed)? ?(?:->|to) ?(?:ok|green|pass)|\bfails? (?:today|now|if|when|on|only|immediately|loudly|fast)\b|\bpass(?:es)? (?:today|now|after|again|consistently|green)\b|\bwould (?:fail|pass)\b|\bregressions?\b|\bcoverage\b|\bline count\b|\d+ lines?\b|\bgrep -|\bheadroom\b|\bflak(?:y|e|iness)\b)"
).unwrap()
});
static REFACTOR_TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:refactor\w*|extract\w*|dedup\w*|duplicat\w*|helper|rename\w*|inline\w*|consolidat\w*|unif(?:y|ies)|shrink\w*|dead code|unused)\b").unwrap()
});
static CLAUSE_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:,\s+|;\s*|\s+(?:and|while|so that|so|then|under|via|per|measured by|verified by|checked by)\s+)").unwrap()
});
static TRAILING_MEASURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+with\s+[^,;]*?\b(?:green|ok|passing|unchanged|intact|still pass\w*)\)?\s*$")
        .unwrap()
});
static PRECONDITION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:with|once|after|when|if|given|now that)\b").unwrap());
static ASIDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:not |rather than |instead of |e\.g\.|i\.e\.|see |cf\. |as |like )")
        .unwrap()
});

fn check_only_card(fields: &BTreeMap<String, String>, summary: &str) -> bool {
    if !check_only_text(&first_card_text(&[
        fields.get("hypothesis").cloned().unwrap_or_default(),
        summary.to_string(),
    ])) {
        return false;
    }
    let variable = fields.get("variable").map_or("", String::as_str).trim();
    variable.is_empty() || CHECK_TOKENS.is_match(variable) || REFACTOR_TOKENS.is_match(variable)
}

fn check_only_text(text: &str) -> bool {
    let text = TRAILING_MEASURE_RE.replace_all(text.trim(), "");
    if text.is_empty() {
        return false;
    }
    let mut clauses: Vec<&str> = CLAUSE_SPLIT_RE.split(&text).collect();
    if clauses.len() > 1 && PRECONDITION_RE.is_match(clauses[0].trim()) {
        clauses.remove(0);
    }
    let mut seen = false;
    for clause in clauses {
        let clause = clause.trim();
        if clause.is_empty() || ASIDE_RE.is_match(clause) {
            continue;
        }
        if CHECK_TOKENS.is_match(clause) {
            seen = true;
            continue;
        }
        if clause.split_whitespace().count() <= 2 {
            continue;
        }
        return false;
    }
    seen
}

pub(crate) fn normalized_summary(summary: &str) -> String {
    let s = summary.to_ascii_lowercase();
    let s = s.replace(['`', '\'', '"', '“', '”', '‘', '’'], "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn normalized_field(value: &str) -> String {
    value
        .trim_matches(['`', '\'', '"', ' '])
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
