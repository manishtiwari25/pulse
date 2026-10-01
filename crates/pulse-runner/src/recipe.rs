use crate::util::{Querier, Row};
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const RECIPE_PATH: &str = "docs/context/environment.md";
pub(crate) const RECIPE_MAX_TOOLS: usize = 12;
pub(crate) const RECIPE_UNFIT_SERVICES: &str = "services";
pub(crate) const RECIPE_UNFIT_HANDS: &str = "hands outdated";
pub(crate) const RECIPE_UNFIT_CHECKOUT: &str = "checkout unreadable";
pub(crate) const RECIPE_UNFIT_MISE: &str = "mise";
pub(crate) const MISE_VERSION: &str = "v2026.9.1";
pub(crate) const SETUP_MINUTES: i32 = 10;
pub(crate) const UNFIT_HOURS: i32 = 24;

static RECIPE_KEYS: &[&str] = &[
    "recipe",
    "tools",
    "setup",
    "env",
    "services",
    "suggested_check",
];
static RECIPE_TOOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_.:-]*@[A-Za-z0-9][A-Za-z0-9_.+-]*$").unwrap());
static RECIPE_ENV_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z_][A-Z0-9_]*$").unwrap());
static RECIPE_SECRET_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"TOKEN|SECRET|PASSWORD|PASSWD|_KEY$|_KEY_|CREDENTIAL|PRIVATE").unwrap()
});
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recipe {
    pub version: String,
    pub tools: Vec<String>,
    pub setup: Vec<String>,
    pub has_setup: bool,
    pub env: BTreeMap<String, String>,
    pub services: Vec<String>,
    pub suggested_check: String,
    pub has_check: bool,
}
impl Recipe {
    /// Go: func (r *Recipe) envKeys() []string
    pub(crate) fn env_keys(&self) -> Vec<String> {
        self.env.keys().cloned().collect()
    }
}

/// Go: func recipeScalar(raw string, problems *[]string, where string) string
pub(crate) fn recipe_scalar(raw: &str, problems: &mut Vec<String>, where_: &str) -> String {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return String::new();
    }
    let bytes = text.as_bytes();
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        let mut end = None;
        let mut i = 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && quote == b'"' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                end = Some(i);
                break;
            }
            i += 1;
        }
        let Some(end) = end else {
            problems.push(format!("{where_}: unterminated quote"));
            return String::new();
        };
        let rest = text[end + 1..].trim();
        if !rest.is_empty() && !rest.starts_with('#') {
            problems.push(format!("{where_}: text after the closing quote"));
        }
        let mut value = text[1..end].to_string();
        if quote == b'"' {
            value = value.replace(r#"\""#, "\"").replace(r#"\\"#, r#"\"#);
        }
        return value;
    }
    if let Some(i) = text.find(" #") {
        text = text[..i].trim_end_matches([' ', '\t']).to_string();
    }
    if "&*!|>%@`".contains(text.chars().next().unwrap_or_default()) {
        problems.push(format!(
            "{where_}: YAML feature not allowed in a recipe ({})",
            text.chars().next().unwrap_or_default()
        ));
        return String::new();
    }
    text
}
/// Go: func secretShaped(value string) bool
pub(crate) fn secret_shaped(value: &str) -> bool {
    pulse_store::secret_shaped(value)
}
/// Go: func isBlank(s string) bool
pub(crate) fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}
/// Go: func indented(s string) bool
pub(crate) fn indented(s: &str) -> bool {
    s.starts_with(' ') || s.starts_with('\t')
}

#[derive(Clone)]
struct BlockLine {
    num: usize,
    text: String,
}

/// Go: func parseRecipe(text string) (*Recipe, []string)
pub(crate) fn parse_recipe(text: &str) -> (Option<Recipe>, Vec<String>) {
    let mut problems = Vec::new();
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() || lines[0].trim() != "---" {
        return (
            Some(Recipe::default()),
            vec!["no frontmatter: the file must start with a `---` line".to_string()],
        );
    }
    let end = (1..lines.len()).find(|&i| lines[i].trim() == "---");
    let Some(end) = end else {
        return (
            Some(Recipe::default()),
            vec!["frontmatter never closes (a second `---` line is missing)".to_string()],
        );
    };
    let mut r = Recipe {
        env: BTreeMap::new(),
        ..Recipe::default()
    };
    let mut seen: BTreeMap<String, bool> = BTreeMap::new();
    let mut i = 1;
    while i < end {
        let line = &lines[i];
        i += 1;
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if indented(line) {
            problems.push(format!("line {i}: unexpected indentation"));
            continue;
        }
        let Some((raw_key, raw_rest)) = line.split_once(':') else {
            problems.push(format!("line {i}: expected `key: value`"));
            continue;
        };
        let key = raw_key.trim();
        if !RECIPE_KEYS.contains(&key) {
            problems.push(format!(
                "line {i}: unknown key `{key}` (allowed: {})",
                RECIPE_KEYS.join(", ")
            ));
            continue;
        }
        if seen.contains_key(key) {
            problems.push(format!("line {i}: `{key}` given twice"));
            continue;
        }
        seen.insert(key.to_string(), true);
        let mut rest = raw_rest.trim().to_string();
        if rest.starts_with('#') {
            rest.clear();
        } else if !rest.starts_with('"')
            && !rest.starts_with('\'')
            && let Some(idx) = rest.find(" #")
        {
            rest = rest[..idx].trim_end_matches([' ', '\t']).to_string();
        }
        let mut block = Vec::new();
        while i < end && (is_blank(&lines[i]) || indented(&lines[i])) {
            if !is_blank(&lines[i]) && !lines[i].trim().starts_with('#') {
                block.push(BlockLine {
                    num: i + 1,
                    text: lines[i].clone(),
                });
            }
            i += 1;
        }
        let where_ = format!("`{key}`");
        match key {
            "recipe" => {
                if !block.is_empty() {
                    problems.push(format!("{where_}: must be `recipe: 1` on one line"));
                }
                let value = recipe_scalar(&rest, &mut problems, &where_);
                if value != "1" {
                    problems.push(format!(
                        "{where_}: only version 1 is known (got {})",
                        pulse_board::py_repr(&value)
                    ));
                }
                r.version = value;
            }
            "suggested_check" => {
                if !block.is_empty() {
                    problems.push(format!("{where_}: one line only"));
                }
                r.suggested_check = recipe_scalar(&rest, &mut problems, &where_);
                r.has_check = true;
            }
            "env" => {
                if !rest.is_empty() && rest != "{}" {
                    problems.push(format!(
                        "{where_}: write `env:` with `NAME: value` lines under it, or `env: {{}}`"
                    ));
                }
                for item in block {
                    let body = item.text.trim();
                    if body.starts_with("- ") {
                        problems.push(format!(
                            "line {}: env takes `NAME: value` lines, not a list",
                            item.num
                        ));
                        continue;
                    }
                    let Some((name_raw, value)) = body.split_once(':') else {
                        problems.push(format!(
                            "line {}: env name must match {}",
                            item.num,
                            RECIPE_ENV_NAME_RE.as_str()
                        ));
                        continue;
                    };
                    let name = name_raw.trim();
                    if !RECIPE_ENV_NAME_RE.is_match(name) {
                        problems.push(format!(
                            "line {}: env name must match {}",
                            item.num,
                            RECIPE_ENV_NAME_RE.as_str()
                        ));
                        continue;
                    }
                    if r.env.contains_key(name) {
                        problems.push(format!("line {}: env `{name}` given twice", item.num));
                        continue;
                    }
                    r.env.insert(
                        name.to_string(),
                        recipe_scalar(value, &mut problems, &format!("env `{name}`")),
                    );
                }
            }
            _ => {
                let mut items = Vec::new();
                if !rest.is_empty() && rest != "[]" {
                    problems.push(format!(
                        "{where_}: write `{key}:` with `- item` lines under it, or `{key}: []`"
                    ));
                } else if rest == "[]" && !block.is_empty() {
                    problems.push(format!("{where_}: `[]` and items cannot both be given"));
                }
                for item in block {
                    let body = item.text.trim();
                    if !body.starts_with("- ") {
                        problems.push(format!("line {}: {key} takes `- item` lines", item.num));
                        continue;
                    }
                    let v = recipe_scalar(&body[2..], &mut problems, &format!("line {}", item.num));
                    if !v.is_empty() {
                        items.push(v);
                    }
                }
                match key {
                    "tools" => r.tools = items,
                    "setup" => {
                        r.setup = items;
                        r.has_setup = true;
                    }
                    "services" => r.services = items,
                    _ => {}
                }
            }
        }
    }
    if !seen.contains_key("recipe") {
        problems.push("`recipe: 1` is required".to_string());
    }
    if r.tools.len() > RECIPE_MAX_TOOLS {
        problems.push(format!("`tools`: at most {RECIPE_MAX_TOOLS} pins"));
    }
    for pin in &r.tools {
        if !RECIPE_TOOL_RE.is_match(pin) {
            problems.push(format!(
                "`tools`: `{pin}` is not an exact `name@version` pin"
            ));
        }
    }
    for name in r.env_keys() {
        let value = r.env.get(&name).cloned().unwrap_or_default();
        if RECIPE_SECRET_NAME_RE.is_match(&name) {
            problems.push(format!(
                "env `{name}`: the name says secret; secrets never live in the recipe"
            ));
        } else if value.contains(['\n', '\r']) {
            problems.push(format!("env `{name}`: one line only"));
        } else if secret_shaped(&value) {
            problems.push(format!("env `{name}`: the value is shaped like a credential; secrets never live in the recipe"));
        }
    }
    for line in &r.setup {
        if line.contains('\n') {
            problems.push("`setup`: one shell line per item".to_string());
        }
    }
    (Some(r), problems)
}

/// Go: func recipeRecord(s *store.Store, slug string) (*store.Record, *store.Revision)
pub(crate) fn recipe_record(
    s: &pulse_store::Store,
    slug: &str,
) -> (Option<pulse_store::Record>, Option<pulse_store::Revision>) {
    let recs = s
        .list_records(pulse_store::RecordFilter {
            project: slug.to_string(),
            kind: "context".into(),
            slug: "environment".into(),
            ..Default::default()
        })
        .unwrap_or_default();
    if recs.is_empty() {
        return (None, None);
    }
    match s.get_record(&recs[0].id, 0) {
        Ok((rec, rev)) => (Some(rec), rev),
        Err(_) => (None, None),
    }
}
/// Go: func recipeRecordID(s *store.Store, slug string) string
#[allow(dead_code)]
pub(crate) fn recipe_record_id(s: &pulse_store::Store, slug: &str) -> String {
    if let (Some(rec), _) = recipe_record(s, slug) {
        return rec.id;
    }
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return "CTX-environment".to_string(),
    };
    let brain = crate::board::brain_project(&conn);
    if !brain.0.is_empty() && brain.str("slug") == slug {
        return "CTX-environment".to_string();
    }
    if brain.0.is_empty() && s.get_record("CTX-environment", 0).is_err() {
        return "CTX-environment".to_string();
    }
    format!("CTX-environment-{slug}")
}
/// Go: func recipeText(s *store.Store, slug string) (string, bool)
pub(crate) fn recipe_text(s: &pulse_store::Store, slug: &str) -> (String, bool) {
    let (_, rev) = recipe_record(s, slug);
    let Some(rev) = rev else {
        return (String::new(), false);
    };
    if rev.body.trim_start_matches('\u{feff}').starts_with("---") {
        return (rev.body, true);
    }
    (
        render_recipe_frontmatter(&rev.frontmatter) + &rev.body,
        true,
    )
}
/// Go: func renderRecipeFrontmatter(fm map[string]any) string
pub(crate) fn render_recipe_frontmatter(fm: &BTreeMap<String, Value>) -> String {
    if fm.is_empty() {
        return "---\n---\n".to_string();
    }
    let mut keys: Vec<String> = RECIPE_KEYS
        .iter()
        .filter(|k| fm.contains_key(**k))
        .map(|s| s.to_string())
        .collect();
    for k in fm.keys() {
        if !RECIPE_KEYS.contains(&k.as_str())
            && !["path", "title", "status", "date"].contains(&k.as_str())
        {
            keys.push(k.clone());
        }
    }
    let mut b = String::from("---\n");
    for k in keys {
        let v = &fm[&k];
        match v {
            Value::Array(a) => {
                if a.is_empty() {
                    let _ = writeln!(b, "{k}: []");
                } else {
                    let _ = writeln!(b, "{k}:");
                    for item in a {
                        let _ = writeln!(b, "  - {}", recipe_scalar_text(item));
                    }
                }
            }
            Value::Object(m) => {
                if m.is_empty() {
                    let _ = writeln!(b, "{k}: {{}}");
                } else {
                    let _ = writeln!(b, "{k}:");
                    let mut names: Vec<_> = m.keys().cloned().collect();
                    names.sort();
                    for name in names {
                        let _ = writeln!(b, "  {name}: {}", recipe_scalar_text(&m[&name]));
                    }
                }
            }
            _ => {
                let _ = writeln!(b, "{k}: {}", recipe_scalar_text(v));
            }
        }
    }
    b.push_str("---\n");
    b
}
/// Go: func recipeScalarText(v any) string
pub(crate) fn recipe_scalar_text(v: &Value) -> String {
    match v {
        Value::Null => "''".to_string(),
        Value::String(s) => {
            if s == "[]" || s == "{}" {
                s.clone()
            } else {
                format!("'{}'", s.replace('\'', "''"))
            }
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                n.to_string()
            }
        }
        Value::Bool(b) => {
            if *b {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        other => format!("'{}'", other.to_string().replace('\'', "''")),
    }
}
/// Go: func indexOf(list []string, s string) int
#[allow(dead_code)]
pub(crate) fn index_of(list: &[String], s: &str) -> i32 {
    list.iter().position(|x| x == s).unwrap_or(list.len()) as i32
}
/// Go: func readRecipe(s *store.Store, slug, root, rev string) (*Recipe, []string, bool)
pub(crate) fn read_recipe(
    s: &pulse_store::Store,
    slug: &str,
    root: &str,
    rev: &str,
) -> (Option<Recipe>, Vec<String>, bool) {
    if !slug.is_empty() {
        let (text, ok) = recipe_text(s, slug);
        if ok {
            let (r, p) = parse_recipe(&text);
            return (r, p, true);
        }
    }
    let spec = format!("{rev}:{RECIPE_PATH}");
    let shown = crate::util::git_timeout(root, Duration::from_secs(30), &["show", &spec]);
    if !shown.ok() {
        return (None, Vec::new(), false);
    }
    let (r, p) = parse_recipe(&shown.stdout);
    (r, p, true)
}
/// Go: func recipeSummary(r *Recipe) string
pub(crate) fn recipe_summary(r: &Recipe) -> String {
    let mut parts = Vec::new();
    if !r.tools.is_empty() {
        parts.push(format!("tools {}", r.tools.join(", ")));
    }
    if r.has_setup {
        parts.push(format!("{} setup line(s)", r.setup.len()));
    }
    if !r.env.is_empty() {
        parts.push(format!("env {}", r.env_keys().join(", ")));
    }
    if !r.services.is_empty() {
        parts.push(format!("services {}", r.services.join(", ")));
    }
    if parts.is_empty() {
        "recipe with nothing to install".to_string()
    } else {
        parts.join("; ")
    }
}
/// Go: func recipeEnvExports(r *Recipe) string
pub(crate) fn recipe_env_exports(r: &Recipe) -> String {
    if r.env.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = r
        .env_keys()
        .into_iter()
        .map(|k| format!("{k}={}", crate::util::shell_quote(&r.env[&k])))
        .collect();
    format!("export {}; ", parts.join(" "))
}
/// Go: func recipeBlock(r *Recipe, remote bool) string
pub(crate) fn recipe_block(r: &Recipe, remote: bool) -> String {
    if !r.services.is_empty() {
        return RECIPE_UNFIT_SERVICES.to_string();
    }
    if remote && (!r.tools.is_empty() || !r.env.is_empty()) {
        return RECIPE_UNFIT_HANDS.to_string();
    }
    String::new()
}
/// Go: func miseEnv(clone string) [][2]string
pub(crate) fn mise_env(clone: &str) -> Vec<[String; 2]> {
    vec![
        ["MISE_IGNORED_CONFIG_PATHS".into(), clone.into()],
        ["MISE_YES".into(), "1".into()],
    ]
}
/// Go: func miseExports(clone string) string
pub(crate) fn mise_exports(clone: &str) -> String {
    mise_env(clone)
        .into_iter()
        .map(|kv| format!("{}={}", kv[0], crate::util::shell_quote(&kv[1])))
        .collect::<Vec<_>>()
        .join(" ")
}
/// Go: func miseExecArgv(pins []string) []string
pub(crate) fn mise_exec_argv(pins: &[String]) -> Vec<String> {
    if pins.is_empty() {
        Vec::new()
    } else {
        [
            vec!["mise".into(), "exec".into()],
            pins.to_vec(),
            vec!["--".into()],
        ]
        .concat()
    }
}
/// Go: func miseWrapShell(pins []string, cmd, clone string) string
pub(crate) fn mise_wrap_shell(pins: &[String], cmd: &str, clone: &str) -> String {
    if pins.is_empty() || cmd.is_empty() {
        return cmd.to_string();
    }
    let mut argv = mise_exec_argv(pins);
    argv.extend(["sh".to_string(), "-c".to_string(), cmd.to_string()]);
    format!(
        "export {}; {}",
        mise_exports(clone),
        crate::util::shell_join(&argv)
    )
}
/// Go: func miseInstallCommand(pins []string, clone string) string
pub(crate) fn mise_install_command(pins: &[String], clone: &str) -> String {
    let mut argv = vec!["mise".to_string(), "install".to_string()];
    argv.extend_from_slice(pins);
    format!(
        "export {}; {}",
        mise_exports(clone),
        crate::util::shell_join(&argv)
    )
}
/// Go: func miseFailedPin(pins []string, output string) string
pub(crate) fn mise_failed_pin(pins: &[String], output: &str) -> String {
    for pin in pins {
        let name = pin.split_once('@').map(|(n, _)| n).unwrap_or(pin);
        let re = Regex::new(&format!(
            r#"(?m)(^|[\s`']){}([\s@`'":]|$)"#,
            regex::escape(name)
        ))
        .unwrap();
        if re.is_match(output) {
            return pin.clone();
        }
    }
    pins.first().cloned().unwrap_or_default()
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SetupRule {
    pub(crate) group: String,
    pub(crate) lockfile: String,
    pub(crate) command: String,
    pub(crate) tool: String,
}
static SETUP_RULES: LazyLock<Vec<SetupRule>> = LazyLock::new(|| {
    vec![
        SetupRule {
            group: "python".into(),
            lockfile: "uv.lock".into(),
            command: "uv sync --frozen --all-extras".into(),
            tool: "uv".into(),
        },
        SetupRule {
            group: "python".into(),
            lockfile: "poetry.lock".into(),
            command: "poetry install --no-interaction".into(),
            tool: "poetry".into(),
        },
        SetupRule {
            group: "python".into(),
            lockfile: "Pipfile.lock".into(),
            command: "pipenv sync --dev".into(),
            tool: "pipenv".into(),
        },
        SetupRule {
            group: "node".into(),
            lockfile: "pnpm-lock.yaml".into(),
            command: "corepack pnpm install --frozen-lockfile --prefer-offline".into(),
            tool: "corepack".into(),
        },
        SetupRule {
            group: "node".into(),
            lockfile: "yarn.lock".into(),
            command: "corepack yarn install --frozen-lockfile".into(),
            tool: "corepack".into(),
        },
        SetupRule {
            group: "node".into(),
            lockfile: "bun.lock".into(),
            command: "bun install --frozen-lockfile".into(),
            tool: "bun".into(),
        },
        SetupRule {
            group: "node".into(),
            lockfile: "bun.lockb".into(),
            command: "bun install --frozen-lockfile".into(),
            tool: "bun".into(),
        },
        SetupRule {
            group: "node".into(),
            lockfile: "package-lock.json".into(),
            command: "npm ci --no-audit --no-fund".into(),
            tool: "npm".into(),
        },
        SetupRule {
            group: "go".into(),
            lockfile: "go.sum".into(),
            command: "go mod download".into(),
            tool: "go".into(),
        },
        SetupRule {
            group: "rust".into(),
            lockfile: "Cargo.lock".into(),
            command: "cargo fetch".into(),
            tool: "cargo".into(),
        },
        SetupRule {
            group: "ruby".into(),
            lockfile: "Gemfile.lock".into(),
            command: "bundle install".into(),
            tool: "bundle".into(),
        },
        SetupRule {
            group: "php".into(),
            lockfile: "composer.lock".into(),
            command: "composer install --no-interaction".into(),
            tool: "composer".into(),
        },
    ]
});
static SETUP_ENV: &[(&str, &str)] = &[
    ("CI", "1"),
    ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
    ("UV_NO_PROGRESS", "1"),
    ("npm_config_update_notifier", "false"),
    ("npm_config_fund", "false"),
];
/// Go: func setupExports() string
pub(crate) fn setup_exports() -> String {
    SETUP_ENV
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}
/// Go: func setupCommand(root, rev string) (string, []string)
pub(crate) fn setup_command(root: &str, rev: &str) -> (String, Vec<String>) {
    let listed = crate::util::git_timeout(
        root,
        Duration::from_secs(30),
        &["ls-tree", "--name-only", rev],
    );
    let mut names = BTreeMap::new();
    if listed.ok() {
        for n in listed.stdout.split('\n') {
            names.insert(n.to_string(), true);
        }
    }
    let mut seen = BTreeMap::new();
    let mut commands = Vec::new();
    let mut files = Vec::new();
    for rule in SETUP_RULES.iter() {
        if names.contains_key(&rule.lockfile) && !seen.contains_key(&rule.group) {
            seen.insert(rule.group.clone(), true);
            commands.push(rule.command.clone());
            files.push(rule.lockfile.clone());
        }
    }
    if commands.is_empty() {
        return (String::new(), Vec::new());
    }
    (
        format!("export {}; {}", setup_exports(), commands.join(" && ")),
        files,
    )
}
/// Go: func notFoundRe(tool string) (*regexp.Regexp, *regexp.Regexp)
pub(crate) fn not_found_re(tool: &str) -> (Regex, Regex) {
    (
        Regex::new(&format!(
            r"(?m)(^|[\s:'`]){}: (command )?not found",
            regex::escape(tool)
        ))
        .unwrap(),
        Regex::new(&format!(
            r"{}: No such file or directory",
            regex::escape(tool)
        ))
        .unwrap(),
    )
}
fn broken_tool_res(tool: &str) -> Vec<Regex> {
    match tool {
        "bundle" => vec![
            Regex::new(r"cannot load such file -- [^\n]*bundler[^\n]*\(LoadError\)").unwrap(),
            Regex::new(r"Could not find 'bundler' \(").unwrap(),
            Regex::new(r"Gem::GemNotFoundException").unwrap(),
        ],
        _ => Vec::new(),
    }
}
/// Go: func missingTool(setupCmd string, rc int, output string) string
pub(crate) fn missing_tool(setup_cmd: &str, rc: i32, output: &str) -> String {
    let mut tools = Vec::new();
    for rule in SETUP_RULES.iter() {
        if setup_cmd.contains(&rule.command) {
            tools.push(rule.tool.clone());
        }
    }
    for tool in &tools {
        let (a, b) = not_found_re(tool);
        if a.is_match(output) || b.is_match(output) {
            return tool.clone();
        }
        for re in broken_tool_res(tool) {
            if re.is_match(output) {
                return tool.clone();
            }
        }
    }
    if rc == 127 && !tools.is_empty() {
        return tools[0].clone();
    }
    String::new()
}
/// Go: func setupUnfitTool(files []string) string
pub(crate) fn setup_unfit_tool(files: &[String]) -> String {
    format!("setup:{}", files.join(","))
}
/// Go: func setupUnfitFiles(tool string) string
pub(crate) fn setup_unfit_files(tool: &str) -> String {
    tool.strip_prefix("setup:").unwrap_or(tool).to_string()
}
/// Go: func setupUnfitCommands(tool string) string
pub(crate) fn setup_unfit_commands(tool: &str) -> String {
    let files: Vec<String> = setup_unfit_files(tool)
        .split(',')
        .map(str::to_string)
        .collect();
    let mut commands = Vec::new();
    for file in files {
        for rule in SETUP_RULES.iter() {
            if file == rule.lockfile {
                commands.push(rule.command.clone());
                break;
            }
        }
    }
    if commands.is_empty() {
        return "the setup command".to_string();
    }
    commands
        .into_iter()
        .map(|c| format!("`{c}`"))
        .collect::<Vec<_>>()
        .join(" && ")
}
/// Go: func setupFailureSummary(executor string, rc int, detail string) string
pub(crate) fn setup_failure_summary(executor: &str, rc: i32, detail: &str) -> String {
    format!("setup failed on {executor} (exit {rc}): {detail}")
}
// A full disk, like a dead network, says nothing about whether the machine can
// set the project up; marking the executor unfit for a day would hide the
// project's queue behind "no queued job" long after the space came back.
static SETUP_TRANSIENT_MARKS: &[&str] = &[
    "failed to fetch",
    "could not resolve",
    "could not connect",
    "connection timed out",
    "tls handshake",
    "network",
    "certificate",
    "proxy",
    "no space left on device",
    "enospc",
    "disk quota exceeded",
    "not enough space",
    "insufficient space",
    "disk full",
];
static TRANSIENT_MARKS: &[&str] = &[
    "the runner stopped while",
    "runner error",
    "daemon error",
    "unreachable",
    "worker rejected at launch",
    "timed out",
    "could not land the branch",
    "conflicts with",
    "the checkout changed",
    "moved while",
    "lease generation",
    "no response was returned",
    "provider_error",
    "interrupted",
    "rate limit",
    "too many requests",
    "429",
    "temporarily unavailable",
    "connection reset",
    "connection refused",
    "context canceled",
    "context deadline exceeded",
    "i/o timeout",
    "no such host",
    "network is unreachable",
    "resource temporarily",
    "database is locked",
    "unexpected eof",
];
fn contains_any_str(text: &str, marks: &[&str]) -> bool {
    marks.iter().any(|m| text.contains(m))
}
static ID_LIKE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(job|run|post|task|msg|req|sg|q)-[a-z0-9]+(-[a-z0-9]+)*").unwrap()
});
static PATH_LIKE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\w.-]*(/[^\s;,:'"`()]+)+"#).unwrap());
static DIGITS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static BRANCH_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bpulse/[^\s;,]+").unwrap());
fn failure_signature_transient(reason: &str) -> String {
    let mut head = reason
        .trim()
        .to_lowercase()
        .trim_matches(|c| c == ';' || c == ' ')
        .to_string();
    if let Some(i) = head.find(';') {
        head.truncate(i);
    }
    head = BRANCH_REF_RE.replace_all(&head, "<branch>").to_string();
    head = ID_LIKE_RE.replace_all(&head, "<id>").to_string();
    head = PATH_LIKE_RE.replace_all(&head, "<path>").to_string();
    head = DIGITS_RE.replace_all(&head, "#").to_string();
    head = SPACES_RE.replace_all(&head, " ").trim().to_string();
    let chars: Vec<char> = head.chars().collect();
    if chars.len() > 72 {
        head = chars[..72].iter().collect::<String>().trim().to_string();
    }
    if head.is_empty() {
        head = "unknown".to_string();
    }
    format!("transient:{head}")
}
/// Go: func repeatedSetupFailure(q querier, now, project, executor, summary string) bool
pub(crate) fn repeated_setup_failure(
    q: &impl Querier,
    now: &str,
    project: &str,
    executor: &str,
    summary: &str,
) -> bool {
    let lower = summary.to_lowercase();
    if contains_any_str(&lower, TRANSIENT_MARKS) || contains_any_str(&lower, SETUP_TRANSIENT_MARKS)
    {
        return false;
    }
    let since = crate::util::format_iso(
        crate::util::must_iso(now) - chrono::Duration::hours(UNFIT_HOURS as i64),
    );
    let last = crate::util::must_one(
        q,
        "SELECT status, summary FROM runs WHERE project = ? AND executor = ? AND finished_at >= ? ORDER BY finished_at DESC LIMIT 1",
        &[&project, &executor, &since],
    );
    if last.0.is_empty() || (last.str("status") != "error" && last.str("status") != "failed") {
        return false;
    }
    let prev = last.str("summary");
    prev.starts_with("setup failed on ")
        && failure_signature_transient(&prev) == failure_signature_transient(summary)
}
/// Go: func checkToolMissing(verifyCmd string, rc int, output string) string
pub(crate) fn check_tool_missing(verify_cmd: &str, rc: i32, output: &str) -> String {
    let tokens: Vec<String> = crate::util::split_shell(verify_cmd)
        .into_iter()
        .filter(|t| {
            !t.is_empty()
                && !t.contains('=')
                && t != "env"
                && t != "exec"
                && t != "nice"
                && t != "time"
        })
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    let tool = Path::new(&tokens[0])
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| tokens[0].clone());
    let (a, b) = not_found_re(&tool);
    if a.is_match(output) || b.is_match(output) {
        return tool;
    }
    if rc == 127 && output.trim().is_empty() {
        return tool;
    }
    String::new()
}
/// Go: func projectSetup(root, rev string, r *Recipe, clone string) (string, []string, string)
pub(crate) fn project_setup(
    root: &str,
    rev: &str,
    r: &Recipe,
    clone: &str,
) -> (String, Vec<String>, String) {
    let mut cmd;
    let mut shown = String::new();
    let files;
    if !r.has_setup {
        let (c, f) = setup_command(root, rev);
        cmd = c;
        files = f;
        if !cmd.is_empty() {
            shown = cmd
                .split_once("; ")
                .map(|(_, a)| a.to_string())
                .unwrap_or_else(|| cmd.clone());
        }
        if !cmd.is_empty() && !r.env.is_empty() {
            cmd = recipe_env_exports(r) + &cmd;
        }
    } else {
        files = vec![RECIPE_PATH.to_string()];
        let lines: Vec<String> = r
            .setup
            .iter()
            .filter(|l| !l.trim().is_empty())
            .cloned()
            .collect();
        if lines.is_empty() {
            return (String::new(), files, String::new());
        }
        shown = lines.join(" && ");
        cmd = format!(
            "export {}; {}{}",
            setup_exports(),
            recipe_env_exports(r),
            shown
        );
    }
    if !cmd.is_empty() && !r.tools.is_empty() && !clone.is_empty() {
        cmd = mise_wrap_shell(&r.tools, &cmd, clone);
    }
    (cmd, files, shown)
}
/// Go: func unfitPhrase(tool string) string
pub(crate) fn unfit_phrase(tool: &str) -> String {
    if let Some(x) = tool.strip_prefix("environment:") {
        return format!("has environment issue {x}");
    }
    match tool {
        RECIPE_UNFIT_SERVICES => "needs services".to_string(),
        RECIPE_UNFIT_HANDS => RECIPE_UNFIT_HANDS.to_string(),
        RECIPE_UNFIT_CHECKOUT => "cannot read the checkout".to_string(),
        _ if tool.starts_with("setup:") => format!("cannot set up {}", setup_unfit_files(tool)),
        _ => format!("lacks {}", tool.strip_prefix("tool:").unwrap_or(tool)),
    }
}
static TOOL_HINTS: LazyLock<BTreeMap<&'static str, &'static str>> = LazyLock::new(|| {
    BTreeMap::from([
        (
            "uv",
            "curl -LsSf https://astral.sh/uv/install.sh | sh   (installs to ~/.local/bin, which the hands put on PATH)",
        ),
        (
            "poetry",
            "curl -sSL https://install.python-poetry.org | python3 -",
        ),
        ("pipenv", "python3 -m pip install --user pipenv"),
        (
            "corepack",
            "corepack ships with Node 22; if missing: npm install -g corepack",
        ),
        (
            "npm",
            "install Node 22 (pulse pair prints the nodesource steps)",
        ),
        ("bun", "curl -fsSL https://bun.sh/install | bash"),
        ("go", "sudo apt-get install -y golang-go"),
        ("cargo", "curl https://sh.rustup.rs -sSf | sh -s -- -y"),
        (
            "bundle",
            "gem install bundler; on Homebrew macOS, `brew reinstall ruby` can repair a stale bundle shim; then `pulse executor probe <executor>`",
        ),
        ("composer", "sudo apt-get install -y composer"),
        ("make", "sudo apt-get install -y make"),
        (
            RECIPE_UNFIT_MISE,
            "curl https://mise.run | sh   (installs ~/.local/bin/mise; `pulse pair` puts a pinned, checksummed release on remote hands)",
        ),
        (
            RECIPE_UNFIT_SERVICES,
            "the recipe lists `services:`; provide them another way and empty the list - the runner starts no services",
        ),
        (
            RECIPE_UNFIT_HANDS,
            "the recipe pins tools or env, which these hands cannot take yet; refresh them with `pulse pair <admin@host> --name <executor>` once the hub's hands support it",
        ),
    ])
});
/// Go: func toolHint(tool string) string
pub(crate) fn tool_hint(tool: &str) -> String {
    if let Some(x) = tool.strip_prefix("environment:") {
        return format!(
            "fix the environment issue (environment:{x}) and unblock the job, or wait 1h for the block to age out"
        );
    }
    if let Some(x) = tool.strip_prefix("tool:") {
        return format!(
            "as the executor user: `mise install {x}`, then `pulse executor probe <executor>`; a pin mise cannot build here is the recipe's to change"
        );
    }
    if tool.starts_with("setup:") {
        return format!(
            "the setup command {} fails the same way on every run; fix the machine (see the run log), then `pulse executor probe <executor>`",
            setup_unfit_commands(tool)
        );
    }
    if let Some(h) = TOOL_HINTS.get(tool) {
        return (*h).to_string();
    }
    if tool == RECIPE_UNFIT_CHECKOUT {
        return "the project's path did not answer: a macOS permission dialog for pulse, an unmounted volume, or a moved checkout (pulse projects)".to_string();
    }
    format!("install {tool} on the machine")
}
/// Go: func recipeAtHead(s *store.Store, project row) (*Recipe, []string, bool)
pub(crate) fn recipe_at_head(
    s: &pulse_store::Store,
    project: Row,
) -> (Option<Recipe>, Vec<String>, bool) {
    let root = project.str("path");
    let (text, ok) = recipe_text(s, &project.str("slug"));
    if ok {
        let (r, p) = parse_recipe(&text);
        return (r, p, true);
    }
    if !crate::util::file_exists(&Path::new(&root).join(".git").display().to_string()) {
        return (None, Vec::new(), false);
    }
    let mut def = project.str("default_branch");
    if def.is_empty() {
        def = "main".to_string();
    }
    read_recipe(s, "", &root, &format!("refs/heads/{def}"))
}
/// Go: func RecipeBlocks(s *store.Store, executor string, projects []string) map[string]string
pub fn recipe_blocks(
    s: &pulse_store::Store,
    executor: &str,
    projects: &[String],
) -> BTreeMap<String, String> {
    recipe_blocks_inner(s, executor, projects)
}
/// Go: func recipeBlocks(s *store.Store, executor string, projects []string) map[string]string
pub(crate) fn recipe_blocks_inner(
    s: &pulse_store::Store,
    executor: &str,
    projects: &[String],
) -> BTreeMap<String, String> {
    let remote = executor != crate::executor::LOCAL_EXECUTOR;
    let mut out = BTreeMap::new();
    let held = crate::checkout::held_checkouts();
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return out,
    };
    for p in crate::util::must_rows(&conn, "SELECT * FROM projects WHERE status = 'active'", &[]) {
        if !projects.is_empty() && !projects.iter().any(|x| x == &p.str("slug")) {
            continue;
        }
        if held.contains_key(&p.str("slug")) {
            out.insert(p.str("slug"), RECIPE_UNFIT_CHECKOUT.to_string());
            continue;
        }
        let (r, problems, found) = recipe_at_head(s, p.clone());
        if !found || !problems.is_empty() {
            continue;
        }
        if let Some(r) = r {
            let why = recipe_block(&r, remote);
            if !why.is_empty() {
                out.insert(p.str("slug"), why);
            }
        }
    }
    out
}
/// Go: func unfitMarks(q querier) map[string]map[string]map[string]string
pub(crate) fn unfit_marks(
    q: &impl Querier,
) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>> {
    let mut out = BTreeMap::new();
    let r = crate::util::must_one(q, "SELECT value FROM meta WHERE key = 'unfit'", &[]);
    if r.0.is_empty() {
        return out;
    }
    let raw = crate::util::json_map(&r.str("value"));
    for (ex, v) in raw {
        let Some(m) = v.as_object() else {
            continue;
        };
        let mut projects = BTreeMap::new();
        for (proj, mv) in m {
            let Some(mm) = mv.as_object() else {
                continue;
            };
            projects.insert(
                proj.clone(),
                BTreeMap::from([
                    (
                        "tool".to_string(),
                        mm.get("tool")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                mm.get("tool").map(Value::to_string).unwrap_or_default()
                            }),
                    ),
                    (
                        "at".to_string(),
                        mm.get("at")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                mm.get("at").map(Value::to_string).unwrap_or_default()
                            }),
                    ),
                ]),
            );
        }
        out.insert(ex, projects);
    }
    out
}
/// Go: func saveUnfitMarks(q querier, marks map[string]map[string]map[string]string) error
pub(crate) fn save_unfit_marks(
    q: &impl Querier,
    marks: &BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
) -> Result<()> {
    q.conn().execute("INSERT INTO meta (key, value) VALUES ('unfit', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [&crate::util::py_json(marks)])?;
    Ok(())
}
/// Go: func markUnfit(q querier, now, executor, project, tool string)
pub(crate) fn mark_unfit(q: &impl Querier, now: &str, executor: &str, project: &str, tool: &str) {
    let mut marks = unfit_marks(q);
    marks.entry(executor.to_string()).or_default().insert(
        project.to_string(),
        BTreeMap::from([
            ("tool".to_string(), tool.to_string()),
            ("at".to_string(), now.to_string()),
        ]),
    );
    let _ = save_unfit_marks(q, &marks);
    crate::util::event_at(
        q,
        now,
        "runner",
        "executor.unfit",
        executor,
        json!({"project": project, "tool": tool}),
    );
}
/// Go: func unfitProjects(q querier, executor, now string) map[string]string
pub(crate) fn unfit_projects(
    q: &impl Querier,
    executor: &str,
    now: &str,
) -> BTreeMap<String, String> {
    let mut marks = unfit_marks(q);
    let mut current = BTreeMap::new();
    if let Some(exmarks) = marks.get(executor) {
        for (project, mark) in exmarks {
            let at = mark.get("at").cloned().unwrap_or_default();
            let fresh = chrono::NaiveDateTime::parse_from_str(&at, crate::util::ISO_LAYOUT)
                .map(|n| {
                    crate::util::must_iso(now).signed_duration_since(n.and_utc())
                        < chrono::Duration::hours(UNFIT_HOURS as i64)
                })
                .unwrap_or(false);
            if fresh {
                current.insert(
                    project.clone(),
                    mark.get("tool")
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| "?".to_string()),
                );
            }
        }
    }
    if current.len() != marks.get(executor).map(|m| m.len()).unwrap_or(0) {
        if current.is_empty() {
            marks.remove(executor);
        } else if let Some(old) = marks.get(executor).cloned() {
            let kept = current
                .keys()
                .filter_map(|p| old.get(p).map(|m| (p.clone(), m.clone())))
                .collect();
            marks.insert(executor.to_string(), kept);
        }
        let _ = save_unfit_marks(q, &marks);
    }
    current
}
/// Go: func clearUnfit(q querier, now, executor string)
pub(crate) fn clear_unfit(q: &impl Querier, now: &str, executor: &str) {
    let mut marks = unfit_marks(q);
    if marks.remove(executor).is_some() {
        let _ = save_unfit_marks(q, &marks);
        crate::util::event_at(
            q,
            now,
            "runner",
            "executor.fit",
            executor,
            Value::String(String::new()),
        );
    }
}
/// Go: func toolsWanted(s *store.Store, brain row, executor, tool, project, detail, now, actor string) row
#[allow(clippy::too_many_arguments)]
pub(crate) fn tools_wanted(
    s: &pulse_store::Store,
    brain: Row,
    executor: &str,
    tool: &str,
    project: &str,
    detail: &str,
    now: &str,
    actor: &str,
) -> Row {
    if brain.0.is_empty() {
        return Row::new();
    }
    let thread = "tools-wanted";
    let needle = format!("{executor} {}", unfit_phrase(tool));
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return Row::new(),
    };
    let like_date = format!("{}%", &now[..10]);
    let like_summary = format!("{needle}%");
    if !crate::util::must_one(&conn, "SELECT 1 FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = ? AND date LIKE ? AND summary LIKE ?", &[&brain.str("slug"), &thread, &like_date, &like_summary]).0.is_empty() { return Row::new(); }
    let hint = tool_hint(tool);
    let (summary, why) = if tool.starts_with("setup:") {
        let files = setup_unfit_files(tool);
        (
            crate::util::trunc(
                &format!(
                    "{needle} for {project}: setup keeps failing the same way on {executor}; jobs run elsewhere until the machine is fixed"
                ),
                230,
            ),
            format!(
                "Setting up a clone of {project} on executor `{executor}` from {files} failed with the same signature on repeated runs (WU-021). The runner stops spending more attempts on that executor until a probe clears the mark."
            ),
        )
    } else {
        match tool {
            RECIPE_UNFIT_SERVICES => (
                crate::util::trunc(
                    &format!(
                        "{needle} for {project}: its recipe lists running services no machine starts; jobs wait until an owner clears the list"
                    ),
                    230,
                ),
                format!(
                    "The environment recipe of {project} (`{RECIPE_PATH}`, ADR-026) lists `services:`. The runner starts no databases, brokers, or other services, so the project is off every machine until the list is empty."
                ),
            ),
            RECIPE_UNFIT_HANDS => (
                crate::util::trunc(
                    &format!(
                        "{needle} for {project}: its recipe pins tools or env the remote hands cannot take yet; jobs run on the hub meanwhile"
                    ),
                    230,
                ),
                format!(
                    "The environment recipe of {project} (`{RECIPE_PATH}`, ADR-026) pins tools or sets env, and the hands on `{executor}` have no verb for that yet; the project runs where they can be honoured until the hands are refreshed."
                ),
            ),
            _ => {
                let adr = if tool.starts_with("tool:") || tool == RECIPE_UNFIT_MISE {
                    ", ADR-026"
                } else {
                    ""
                };
                let usable = if tool == "bundle" {
                    "not installed or not usable"
                } else {
                    "not installed"
                };
                (
                    crate::util::trunc(
                        &format!(
                            "{needle} for {project}: its jobs run elsewhere until the tool is installed and `pulse executor probe {executor}` passes"
                        ),
                        230,
                    ),
                    format!(
                        "Setting up a clone of {project} on executor `{executor}` failed because `{}` is {usable} there (WU-021{adr}). The runner installs what the project pins before a worker starts; without the tool it cannot.",
                        tool.strip_prefix("tool:").unwrap_or(tool)
                    ),
                )
            }
        }
    };
    let mut body = format!(
        "{why} So {project} jobs go to other slots for {UNFIT_HOURS} hours and again after that until a probe passes.\n\nOn {executor}, as the executor user unless the line says sudo:\n\n    {hint}\n\nthen on the hub: `pulse executor probe {executor}`. The human owns the machine; this post is a measurement, not a request to act.\n"
    );
    if !detail.is_empty() {
        body.push_str(&format!("\nLast setup error tail:\n\n    {detail}\n"));
    }
    let post = crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: brain.str("slug"),
            area: "runner".into(),
            thread: thread.into(),
            summary,
            body,
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_default();
    if post.0.is_empty() {
        return Row::new();
    }
    crate::util::event_at(
        &conn,
        now,
        actor,
        "capacity.tools-wanted",
        &brain.str("slug"),
        json!({"executor": executor, "tool": tool, "project": project, "post": post.str("id")}),
    );
    post
}
/// Go: func recipeFinding(s *store.Store, project row, why []string, base, now, actor string) row
pub(crate) fn recipe_finding(
    s: &pulse_store::Store,
    project: Row,
    why: &[String],
    base: &str,
    now: &str,
    actor: &str,
) -> Row {
    let thread = "environment";
    let needle = "invalid environment recipe";
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return Row::new(),
    };
    let like_date = format!("{}%", &now[..10]);
    let like_summary = format!("{needle}%");
    if !crate::util::must_one(&conn, "SELECT 1 FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = ? AND date LIKE ? AND summary LIKE ?", &[&project.str("slug"), &thread, &like_date, &like_summary]).0.is_empty() { return Row::new(); }
    let mut joined = why.join("; ");
    if joined.len() > 120 {
        joined.truncate(120);
    }
    let summary = crate::util::trunc(
        &format!(
            "{needle} at {}: jobs error before a worker starts until it is fixed ({joined})",
            short(base, 12)
        ),
        230,
    );
    let bullets: Vec<String> = why.iter().map(|p| format!("- {p}")).collect();
    let body = format!(
        "`{RECIPE_PATH}` at the base commit {} does not parse as a recipe (ADR-026), so the runner cannot know what a clone needs and starts no worker for {}:\n\n{}\n\nThe file is the project's; whoever fixes it commits it alone, and the branch waits for a human landing.\n",
        short(base, 12),
        project.str("slug"),
        bullets.join("\n")
    );
    let post = crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: project.str("slug"),
            area: "runner".into(),
            thread: thread.into(),
            summary,
            body,
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_default();
    if post.0.is_empty() {
        return Row::new();
    }
    let problems: Vec<String> = why.iter().take(6).cloned().collect();
    crate::util::event_at(
        &conn,
        now,
        actor,
        "env.invalid",
        &project.str("slug"),
        json!({"base": base, "problems": problems, "post": post.str("id")}),
    );
    post
}
/// Go: func short(s string, n int) string
pub(crate) fn short(s: &str, n: usize) -> String {
    if s.len() > n {
        String::from_utf8_lossy(&s.as_bytes()[..n]).to_string()
    } else {
        s.to_string()
    }
}
/// Go: func recipePulseCLI(r *Recipe) string
pub(crate) fn recipe_pulse_cli(r: &Recipe) -> String {
    r.env
        .get("PULSE_CLI")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_classifies_broken_bundle_shim() {
        let _serial = crate::test_helpers::serial();
        let blog_error = "/lib/ruby/4.0.0/rubygems.rb:305:in 'Kernel#load': cannot load such file -- /opt/homebrew/lib/ruby/gems/4.0.0/gems/bundler-2.4.10/exe/bundle (LoadError)\nfrom /opt/homebrew/Cellar/ruby/4.0.6/lib/ruby/4.0.0/rubygems.rb:305:in 'Gem.activate_and_load_bin_path'\nfrom /opt/homebrew/bin/bundle:25:in '<main>'";
        let cmd = "export CI=1; bundle install";
        assert_eq!(missing_tool(cmd, 1, blog_error), "bundle");
        for output in [
            "Could not find 'bundler' (2.4.10) required by your /repo/Gemfile.lock.",
            "Gem::GemNotFoundException: can't find gem bundler (= 2.4.10) with executable bundle",
        ] {
            assert_eq!(missing_tool(cmd, 1, output), "bundle", "output {output:?}");
        }
    }

    #[test]
    fn missing_tool_does_not_classify_bundle_project_build_failures() {
        let _serial = crate::test_helpers::serial();
        let output = "Installing nokogiri 1.16.0 with native extensions\nGem::Ext::BuildError: ERROR: Failed to build gem native extension.\ncurrent directory: /repo/vendor/bundle/ruby/3.3.0/gems/nokogiri/ext/nokogiri\nmake: *** [compile] Error 2";
        assert_eq!(missing_tool("export CI=1; bundle install", 1, output), "");
    }

    #[test]
    fn setup_repeat_guard_matches_project_executor_and_signature() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            create: true,
            ..Default::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        let summary = setup_failure_summary("local", 1, "same squeezed error");
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &summary
        ));
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-prev', 'job-prev', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-06T19:00:00Z", &"2026-09-06T20:00:00Z", &summary],
        );
        assert!(repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &setup_failure_summary("local", 1, "same squeezed error")
        ));
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &setup_failure_summary("local", 1, "different squeezed error")
        ));
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "remote",
            &setup_failure_summary("remote", 1, "same squeezed error")
        ));
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "other",
            "local",
            &setup_failure_summary("local", 1, "same squeezed error")
        ));
    }

    #[test]
    fn setup_repeat_guard_ignores_old_failures() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            create: true,
            ..Default::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        let summary = setup_failure_summary("local", 1, "same squeezed error");
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-old', 'job-old', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-05T15:00:00Z", &"2026-09-05T20:00:00Z", &summary],
        );
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &summary
        ));
    }

    #[test]
    fn setup_repeat_guard_needs_consecutive_failures_and_skips_network() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            create: true,
            ..Default::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        let summary = setup_failure_summary("local", 1, "same squeezed error");
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-prev', 'job-prev', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-06T18:00:00Z", &"2026-09-06T19:00:00Z", &summary],
        );
        assert!(repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &summary
        ));
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-ok', 'job-ok', 'w', 'blog', ?, ?, 'done', 'no commits; nothing to land', 'local')",
            &[&"2026-09-06T19:30:00Z", &"2026-09-06T20:00:00Z"],
        );
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &summary
        ));
        let fetch = setup_failure_summary(
            "local",
            1,
            "g>=1.25` ├─▶ Failed to fetch: `https://pypi.org/simple/x/`",
        );
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-net', 'job-net', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-06T20:30:00Z", &"2026-09-06T20:45:00Z", &fetch],
        );
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &fetch
        ));
    }

    #[test]
    fn setup_repeat_guard_treats_a_full_disk_as_transient() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            create: true,
            ..Default::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        let full = setup_failure_summary(
            "local",
            1,
            "npm error code ENOSPC npm error syscall write npm error errno -28 npm error nospc ENOSPC: no space left on device, write",
        );
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-full', 'job-full', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-06T20:30:00Z", &"2026-09-06T20:45:00Z", &full],
        );
        assert!(
            !repeated_setup_failure(&conn, "2026-09-06T21:00:00Z", "blog", "local", &full),
            "two setup failures caused by a full disk must not mark the executor unfit"
        );
        let quota = setup_failure_summary("local", 1, "cargo: Disk quota exceeded (os error 69)");
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor) VALUES ('run-quota', 'job-quota', 'w', 'blog', ?, ?, 'error', ?, 'local')",
            &[&"2026-09-06T20:50:00Z", &"2026-09-06T20:55:00Z", &quota],
        );
        assert!(!repeated_setup_failure(
            &conn,
            "2026-09-06T21:00:00Z",
            "blog",
            "local",
            &quota
        ));
    }

    #[test]
    fn pre_commit_recipe_env_export_shape() {
        let _serial = crate::test_helpers::serial();
        let recipe = Recipe {
            env: BTreeMap::from([
                ("CI".to_string(), "1".to_string()),
                ("FOO".to_string(), "bar baz".to_string()),
            ]),
            ..Default::default()
        };
        assert_eq!(recipe_env_exports(&recipe), "export CI=1 FOO='bar baz'; ");
    }

    #[test]
    fn pre_commit_mise_wrap_shape() {
        let _serial = crate::test_helpers::serial();
        let pins = vec!["go@1.22".to_string(), "node@20".to_string()];
        let got = mise_wrap_shell(&pins, "go test ./...", "/tmp/clone");
        assert!(got.contains("export MISE_IGNORED_CONFIG_PATHS=/tmp/clone MISE_YES=1;"));
        assert!(got.contains("mise exec go@1.22 node@20 -- sh -c 'go test ./...'"));
    }

    #[test]
    fn pulse_on_path() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(recipe_pulse_cli(&Recipe::default()), "");
        let (recipe, problems) = parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("PULSE_CLI", "/opt/pulse/bin/pulse ")]),
            "",
        ));
        assert!(problems.is_empty(), "unexpected problems: {problems:?}");
        let recipe = recipe.unwrap();
        assert_eq!(recipe_pulse_cli(&recipe), "/opt/pulse/bin/pulse");
        assert!(
            regex::Regex::new(r"^(PULSE_[A-Z_]+|GIT_TERMINAL_PROMPT)=.*$")
                .unwrap()
                .is_match(&format!("PULSE_CLI={}", recipe_pulse_cli(&recipe))),
            "hands env filter rejects PULSE_CLI; the recipe value would never reach the host"
        );
        let (plain, _) = parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("OTHER", "x")]),
            "",
        ));
        assert_eq!(recipe_pulse_cli(&plain.unwrap()), "");
    }

    #[test]
    fn remote_pulse_cli() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let prev = *crate::run::STAGED_PULSE_PATH.read().unwrap();
        *crate::run::STAGED_PULSE_PATH.write().unwrap() = None;
        assert_eq!(
            crate::run::remote_pulse_cli(None),
            crate::pair::PAIR_PULSE_PATH
        );
        let (recipe, problems) = parse_recipe(&crate::test_helpers::recipe_with(
            None,
            None,
            None,
            Some(&[("PULSE_CLI", "/opt/pulse/bin/pulse")]),
            "",
        ));
        assert!(problems.is_empty(), "recipe problems: {problems:?}");
        assert_eq!(
            crate::run::remote_pulse_cli(recipe.as_ref()),
            "/opt/pulse/bin/pulse"
        );
        *crate::run::STAGED_PULSE_PATH.write().unwrap() = prev;
    }

    #[test]
    fn run_job_recipe_setup_runs_once_before_worker_and_verify() {
        let _serial = crate::test_helpers::serial();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let _guard = pulse_testutil::env_lock();
        let s = crate::test_helpers::new_store();
        let (proj, _) = crate::test_helpers::new_repo(&s);
        crate::test_helpers::join(&s, "aurora");

        let marker_path = std::path::Path::new(&s.home).join("setup-order");
        let marker = crate::util::shell_quote(&marker_path.display().to_string());
        let recipe = crate::test_helpers::recipe_with(
            None,
            Some(&[&format!("printf 'setup\\n' >> {marker}")]),
            None,
            None,
            "",
        );
        let (id, _, _) = crate::env::env_set(
            &s,
            proj.clone(),
            recipe,
            crate::util::HUMAN_AGENT.into(),
            "store".into(),
            "setup ordering regression".into(),
        )
        .unwrap();
        assert_eq!(id, "CTX-environment");
        let verify_cmd = format!(
            "test \"$(cat {marker})\" = \"$(printf 'setup\\nworker')\" && printf 'verify\\n' >> {marker}"
        );
        crate::util::exec1(
            &s.conn().unwrap(),
            "UPDATE projects SET verify_cmd = ? WHERE slug = ?",
            &[&verify_cmd, &proj.str("slug")],
        );

        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "setup ordering".into(),
            "add a worker marker".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            crate::test_helpers::TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!job.is_empty(), "lease returned no job");
        let worker = format!(
            "test \"$(cat {marker})\" = setup &&\nprintf 'worker\\n' >> {marker} &&\nprintf 'worker\\n' > worker.txt &&\ngit add worker.txt &&\ngit commit -q -m 'add worker marker' &&\necho 'SUMMARY: setup ran once before worker'"
        );
        let r = crate::run::run_job(
            &s,
            job,
            "aurora",
            crate::test_helpers::TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done");
        assert_eq!(r.int("commits"), 1);
        assert_eq!(r.str("verify"), "verify exit 0");
        let got = std::fs::read_to_string(marker_path).unwrap();
        assert_eq!(got, "setup\nworker\nverify\n");
    }

    #[test]
    fn setup_unfit_mark_makes_only_local_executor_wait_until_probe_clears() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (proj, _) = crate::test_helpers::new_bare_repo(&s);
        mark_unfit(
            &s.conn().unwrap(),
            crate::test_helpers::TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            &proj.str("slug"),
            "setup:Gemfile.lock",
        );
        let (job, exrow, why) = crate::run::take_job(
            &s,
            "aurora",
            &proj.str("slug"),
            "",
            false,
            &crate::util::plus_hours(crate::test_helpers::TEST_NOW, 1.0),
            crate::executor::LOCAL_EXECUTOR,
            0,
        )
        .unwrap();
        assert!(job.0.is_empty(), "job = {job:?}");
        assert!(!exrow.0.is_empty(), "executor = {exrow:?}");
        for want in [
            "cannot set up Gemfile.lock",
            "setup command `bundle install` fails the same way",
            "pulse executor probe <executor>",
        ] {
            assert!(why.contains(want), "wait reason lacks {want:?}: {why}");
        }
        // The command the wait reason names works for this machine too. A worker's probe
        // does not clear the mark; a human's passing probe does; a machine under the free
        // space floor fails the probe and keeps the mark.
        let later = crate::util::plus_hours(crate::test_helpers::TEST_NOW, 2.0);
        let (ok, note) =
            crate::executor::probe_executor(&s, crate::executor::LOCAL_EXECUTOR, "aurora", &later)
                .unwrap();
        assert!(
            ok && note.starts_with("ok: git="),
            "worker probe: {ok} {note}"
        );
        let got = unfit_projects(&s.conn().unwrap(), crate::executor::LOCAL_EXECUTOR, &later);
        assert_eq!(
            got.len(),
            1,
            "a worker's probe cleared the unfit mark: {got:?}"
        );
        s.set_setting(
            crate::util::HUMAN_AGENT,
            "daemon.min_free_mb",
            &i32::MAX.to_string(),
        )
        .unwrap();
        let (ok, note) = crate::executor::probe_executor(
            &s,
            crate::executor::LOCAL_EXECUTOR,
            crate::util::HUMAN_AGENT,
            &later,
        )
        .unwrap();
        assert!(
            !ok && note.contains("free on"),
            "full disk probe: {ok} {note}"
        );
        let got = unfit_projects(&s.conn().unwrap(), crate::executor::LOCAL_EXECUTOR, &later);
        assert_eq!(
            got.len(),
            1,
            "a failed probe cleared the unfit mark: {got:?}"
        );
        s.set_setting(crate::util::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let (ok, note) = crate::executor::probe_executor(
            &s,
            crate::executor::LOCAL_EXECUTOR,
            crate::util::HUMAN_AGENT,
            &later,
        )
        .unwrap();
        assert!(
            ok && note.contains(&format!("unfit_cleared={}", proj.str("slug"))),
            "human probe: {ok} {note}"
        );
        let got = unfit_projects(&s.conn().unwrap(), crate::executor::LOCAL_EXECUTOR, &later);
        assert!(got.is_empty(), "probe clear left unfit marks: {got:?}");
    }

    #[test]
    fn idle_reason_names_the_queue_an_unfit_mark_holds() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (proj, _) = crate::test_helpers::new_bare_repo(&s);
        let conn = s.conn().unwrap();
        let other = tempfile::tempdir().unwrap();
        crate::util::exec1(
            &conn,
            "INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) SELECT 'other', 'Other', ?, '', 0, '', 'main', 'active', added_at, added_by, last_seen FROM projects WHERE slug = ?",
            &[&other.path().display().to_string(), &proj.str("slug")],
        );
        for n in 0..2 {
            crate::jobs::add_job(
                &s,
                proj.str("slug"),
                "task".into(),
                format!("Held job {n}"),
                format!("Wait behind the mark ({n})."),
                0,
                0,
                "test".into(),
                crate::util::HUMAN_AGENT.into(),
                false,
            )
            .expect("add job");
        }
        let now = crate::util::plus_hours(crate::test_helpers::TEST_NOW, 1.0);
        let local = crate::executor::LOCAL_EXECUTOR;
        mark_unfit(
            &conn,
            crate::test_helpers::TEST_NOW,
            local,
            &proj.str("slug"),
            "setup:Gemfile.lock",
        );
        let (job, _, why) =
            crate::run::take_job(&s, "aurora", "", "", false, &now, local, 0).unwrap();
        assert!(job.0.is_empty(), "job = {job:?}");
        for want in [
            "no queued job; 2 queued wait: 2 for proj (local cannot set up Gemfile.lock)",
            "pulse executor probe <executor>",
        ] {
            assert!(why.contains(want), "idle reason lacks {want:?}: {why}");
        }
        // A request for one particular project keeps its plain reason.
        let (_, _, why) =
            crate::run::take_job(&s, "aurora", "other", "", false, &now, local, 0).unwrap();
        assert_eq!(why, "no queued job for other");
    }

    #[test]
    fn tools_wanted_for_bundle_and_repeated_setup_include_hints_and_error_tail() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (brain, _) = crate::test_helpers::new_bare_repo(&s);
        crate::util::exec1(
            &s.conn().unwrap(),
            "UPDATE projects SET brain = 1 WHERE slug = ?",
            &[&brain.str("slug")],
        );
        let brain = crate::board::brain_project(&s.conn().unwrap());
        let bundle_tail = "cannot load such file -- /opt/homebrew/lib/ruby/gems/4.0.0/gems/bundler-2.4.10/exe/bundle (LoadError)";
        let post = tools_wanted(
            &s,
            brain.clone(),
            "local",
            "bundle",
            "blog",
            bundle_tail,
            crate::test_helpers::TEST_NOW,
            "runner",
        );
        assert!(!post.0.is_empty(), "bundle tools_wanted did not post");
        assert!(post.str("summary").contains("local lacks bundle for blog"));
        let body = post.str("body");
        for want in [
            "brew reinstall ruby",
            "gem install bundler",
            "pulse executor probe local",
            "Last setup error tail",
            bundle_tail,
        ] {
            assert!(body.contains(want), "bundle body lacks {want:?}:\n{body}");
        }
        let again = tools_wanted(
            &s,
            brain.clone(),
            "local",
            "bundle",
            "blog",
            bundle_tail,
            crate::test_helpers::TEST_NOW,
            "runner",
        );
        assert!(
            again.0.is_empty(),
            "bundle tools_wanted dedupe posted twice in one day"
        );

        let setup_tail = "bundle install crashed before dependency resolution";
        let post = tools_wanted(
            &s,
            brain,
            "local",
            "setup:Gemfile.lock",
            "blog",
            setup_tail,
            &crate::util::plus_hours(crate::test_helpers::TEST_NOW, 1.0),
            "runner",
        );
        assert!(!post.0.is_empty(), "setup tools_wanted did not post");
        assert!(
            post.str("summary")
                .contains("local cannot set up Gemfile.lock for blog")
        );
        let body = post.str("body");
        for want in [
            "setup command `bundle install` fails the same way",
            "fix the machine",
            "pulse executor probe local",
            "Last setup error tail",
            setup_tail,
        ] {
            assert!(body.contains(want), "setup body lacks {want:?}:\n{body}");
        }
    }

    #[test]
    fn pre_commit_hook_script_runs_verify_with_recipe_env() {
        let _serial = crate::test_helpers::serial();
        let recipe = Recipe {
            env: BTreeMap::from([
                ("CI".to_string(), "1".to_string()),
                ("FOO".to_string(), "bar baz".to_string()),
            ]),
            ..Default::default()
        };
        let script =
            crate::keys::pre_commit_hook_script("make test", Some(&recipe), &[], "/tmp/clone");
        assert!(
            script.contains("export CI=1 FOO='bar baz'"),
            "recipe env not exported correctly:\n{script}"
        );
        assert!(
            script.contains("make test"),
            "verify command missing:\n{script}"
        );
        assert!(
            script.starts_with("#!/bin/sh\n"),
            "not a shell script:\n{script}"
        );
        let unset_at = script.find(crate::keys::PRE_COMMIT_HOOK_UNSET);
        let verify_at = script.find("make test");
        assert!(
            unset_at.is_some() && verify_at.is_some() && unset_at.unwrap() < verify_at.unwrap(),
            "hook env unset missing or after verify:\n{script}"
        );
        for v in [
            "GIT_INDEX_FILE",
            "GIT_PREFIX",
            "GIT_AUTHOR_NAME",
            "GIT_COMMITTER_DATE",
        ] {
            assert!(
                crate::keys::PRE_COMMIT_HOOK_UNSET.contains(v),
                "hook unset does not drop {v}"
            );
        }
    }

    #[test]
    fn pre_commit_hook_script_wraps_mise_pins() {
        let _serial = crate::test_helpers::serial();
        let recipe = Recipe {
            env: BTreeMap::from([("CI".to_string(), "1".to_string())]),
            ..Default::default()
        };
        let pins = vec!["go@1.22".to_string(), "node@20".to_string()];
        let script = crate::keys::pre_commit_hook_script(
            "go test ./...",
            Some(&recipe),
            &pins,
            "/tmp/clone",
        );
        assert!(
            script.contains("export CI=1"),
            "recipe env missing:\n{script}"
        );
        assert!(
            script.contains("mise exec go@1.22 node@20 -- sh -c 'go test ./...'"),
            "mise wrap missing or wrong:\n{script}"
        );
    }

    #[test]
    fn write_pre_commit_hook_creates_executable_hook() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(clone.join(".git/hooks")).unwrap();
        crate::keys::write_pre_commit_hook(&clone.display().to_string(), "#!/bin/sh\nexit 0\n")
            .unwrap();
        let hook = clone.join(".git/hooks/pre-commit");
        let st = std::fs::metadata(&hook).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                st.permissions().mode() & 0o111,
                0,
                "hook is not executable: {:o}",
                st.permissions().mode()
            );
        }
        let body = std::fs::read_to_string(hook).unwrap();
        assert_eq!(body, "#!/bin/sh\nexit 0\n");
    }
}
