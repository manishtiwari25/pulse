//! Custom agents: Markdown files with front matter that describe a helper
//! the model can delegate to through the `subagent` tool, a run can be
//! started as (`pulse harness run --agent NAME`), and a job worker runs
//! under when the card's kind and tags match. The layouts are the ones
//! Claude Code (`.claude/agents/<name>.md`) and Copilot
//! (`.github/agents/<name>.agent.md`) use, so a repository's existing
//! agents work as they are.
//!
//! One name is one spec. The layers, in trust order: `agent` records of
//! the store, the owner's `~/.pulse/agents`, the clone's `.pulse/agents`,
//! `.claude/agents`, `.github/agents`. The highest layer owning a name
//! wins; a lower twin may only narrow it (fewer tools, more denies, a
//! smaller step cap, read-only), and what else it says is dropped with a
//! warning. A file's `description` tells the model when to pick the
//! agent, its body becomes the standing instructions, `tools` (or
//! `disallowedTools`) narrows the tools, and the nested `pulse:` block
//! says which job cards the agent takes. Tool names come in every
//! harness's spelling (`Read`, `view`, `shell`, `WebFetch`, `Task`,
//! `github/*`, `mcp_github_search`); unknown names are kept as patterns,
//! which can never grant a tool the run does not offer.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::permissions::{Rule, Source, split_specs};
use crate::skills::split_front_matter;

/// Where a clone's agents live, relative to its root, in trust order.
pub const AGENT_DIRS: &[&str] = &[".pulse/agents", ".claude/agents", ".github/agents"];

/// Characters of an agent's instructions the run gets before they are cut.
pub const AGENT_BODY_LIMIT: usize = 30_000;

/// Characters of a description; a longer one is a lint error and is cut.
pub const DESCRIPTION_LIMIT: usize = 1024;

/// Characters of a description the `subagent` tool's listing shows.
const LISTING_DESCRIPTION_LIMIT: usize = 240;

/// Agents named in the `subagent` tool description; the rest are counted.
pub const AGENT_LIST_LIMIT: usize = 40;

/// Longest agent name: `^[a-z0-9][a-z0-9-]{0,63}$`.
pub const NAME_LIMIT: usize = 64;

/// Minutes a `pulse.schedule.every` may not go under (Tier 0/1).
pub const MIN_EVERY_MINUTES: i64 = pulse_tasks::leads::MIN_EVERY_MINUTES;

/// Where a spec came from, most trusted first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentLayer {
    /// An `agent` record of the store.
    Store,
    /// `~/.pulse/agents`.
    Home,
    /// The clone's `.pulse/agents`.
    #[default]
    Repo,
    /// The clone's `.claude/agents`.
    Claude,
    /// The clone's `.github/agents`.
    Github,
}

impl AgentLayer {
    /// The layers read from files, in trust order.
    pub const FILE_LAYERS: [AgentLayer; 4] = [Self::Home, Self::Repo, Self::Claude, Self::Github];

    pub fn label(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Home => "~/.pulse/agents",
            Self::Repo => ".pulse/agents",
            Self::Claude => ".claude/agents",
            Self::Github => ".github/agents",
        }
    }

    /// Whether the owner put the spec there (the store or the owner's own
    /// files), as opposed to whoever last committed to the repository.
    pub fn owner_trusted(self) -> bool {
        matches!(self, Self::Store | Self::Home)
    }

    /// Whether the spec is a file of the clone, so a remote hand reading
    /// the clone sees it too.
    pub fn in_clone(self) -> bool {
        matches!(self, Self::Repo | Self::Claude | Self::Github)
    }

    /// The directory of a file layer under `cwd` or the PULSE home.
    fn dir(self, cwd: &Path, home: &str) -> Option<PathBuf> {
        match self {
            Self::Store => None,
            Self::Home => (!home.is_empty()).then(|| Path::new(home).join("agents")),
            Self::Repo => Some(cwd.join(AGENT_DIRS[0])),
            Self::Claude => Some(cwd.join(AGENT_DIRS[1])),
            Self::Github => Some(cwd.join(AGENT_DIRS[2])),
        }
    }

    fn what(self) -> &'static str {
        match self {
            Self::Store => "record",
            Self::Home => "owner file",
            Self::Repo | Self::Claude | Self::Github => "repository file",
        }
    }
}

/// One custom agent: a spec read from a file or a record, with what the
/// reading found (warnings, lint errors, a rejection).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// The tools the agent may use, as PULSE names or globs
    /// (`mcp__github__*`); None gives it every tool of the run.
    pub tools: Option<Vec<String>>,
    /// Tools the agent may not use, in PULSE names or globs.
    pub denied: Vec<String>,
    /// The standing instructions: the file's body.
    pub body: String,
    /// The file, relative to the clone root or `~/.pulse`, or
    /// `store:<record id>`.
    pub source: String,
    /// The model the agent asks for; applied only when `models.allowed`
    /// lets it through.
    pub model: Option<String>,
    pub skills: Vec<String>,
    /// MCP server names the file lists; lint checks they exist.
    pub mcp_servers: Vec<String>,
    /// A cap on model calls; it only ever lowers the run's own.
    pub max_steps: Option<u32>,
    pub max_subagents: Option<u32>,
    /// Permission rules as `allow:PATTERN` / `deny:PATTERN`, PATTERN in
    /// the harness's `tool` or `tool(filter)` form; deny wins.
    pub permissions: Vec<String>,
    pub read_only: bool,
    /// Job kinds the agent takes (`pulse.kinds`).
    pub kinds: Vec<String>,
    /// Card tags (or `Class:` words) the agent asks for; empty takes all.
    pub select_tags: Vec<String>,
    /// Globs a card's `File:` path must match; empty takes all.
    pub select_files: Vec<String>,
    /// `product`, `operating-layer` or `both`.
    pub project_scope: String,
    /// The brief text a later phase schedules the agent with.
    pub brief: String,
    pub quorum: Option<i64>,
    /// `pulse.schedule.every` as written (`6h`, `1d`).
    pub schedule_every: String,
    /// The lead role the agent seats (`pulse.role`).
    pub role: String,
    pub layer: AgentLayer,
    /// What the reading noticed; the spec loads anyway.
    pub warnings: Vec<String>,
    /// Lint errors; the spec still loads.
    pub errors: Vec<String>,
    /// Why the spec does not load, when it must not.
    pub rejected: Option<String>,
    /// What each foreign tool name became, one `Given -> pulse` per name.
    pub renamed: Vec<String>,
}

impl AgentDef {
    /// Whether the spec loads (it was not rejected).
    pub fn loads(&self) -> bool {
        self.rejected.is_none()
    }

    /// The spec's permission rules, parsed; a bad one was already
    /// reported at load and is skipped here.
    pub fn permission_rules(&self) -> Vec<Rule> {
        self.permissions
            .iter()
            .filter_map(|p| {
                let (allow, spec) = p.split_once(':')?;
                Rule::parse(allow == "allow", spec, Source::Agent).ok()
            })
            .collect()
    }

    fn warn(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.warnings.contains(&text) {
            self.warnings.push(text);
        }
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.errors.contains(&text) {
            self.errors.push(text);
        }
    }

    fn reject(&mut self, why: impl Into<String>) {
        let why = why.into();
        match &mut self.rejected {
            None => self.rejected = Some(why),
            Some(have) if !have.contains(&why) => {
                have.push_str("; ");
                have.push_str(&why);
            }
            Some(_) => {}
        }
    }
}

/// What the reader does with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyUse {
    /// Read into the spec.
    Honoured,
    /// Not used by PULSE; a warning names it.
    Ignored,
    /// Must not come from a repository file; the file does not load.
    Guarded,
    /// Passed in silence (a record's own key).
    Passed,
    /// Widens the daemon's reach (Tier 0/1): a lint error.
    Forbidden,
}

/// One key of the front matter: the schema, the reader and the linter
/// all read this table, so they cannot drift.
#[derive(Clone, Copy, Debug)]
pub struct KeySpec {
    pub key: &'static str,
    /// Other spellings the reader takes.
    pub aliases: &'static [&'static str],
    /// JSON Schema types, `|`-separated (`string|array`).
    pub kind: &'static str,
    pub about: &'static str,
    pub handling: KeyUse,
}

const fn key(
    key: &'static str,
    aliases: &'static [&'static str],
    kind: &'static str,
    about: &'static str,
    handling: KeyUse,
) -> KeySpec {
    KeySpec {
        key,
        aliases,
        kind,
        about,
        handling,
    }
}

/// The top-level keys of an agent file.
pub const FRONT_KEYS: &[KeySpec] = &[
    key(
        "name",
        &[],
        "string",
        "The agent's name; the file name wins when they differ.",
        KeyUse::Honoured,
    ),
    key(
        "description",
        &[],
        "string",
        "When the model should pick this agent (up to 1024 characters).",
        KeyUse::Honoured,
    ),
    key(
        "tools",
        &["allowed-tools", "allowedTools", "allowed_tools"],
        "string|array",
        "The tools the agent may use, in any harness's spelling; a comma list or an array. Absent: every tool of the run.",
        KeyUse::Honoured,
    ),
    key(
        "disallowedTools",
        &["disallowed-tools", "disallowed_tools", "deny"],
        "string|array",
        "Tools the agent may not use.",
        KeyUse::Honoured,
    ),
    key(
        "model",
        &[],
        "string",
        "The model to run as; applied when the `models.allowed` setting lets it through, else the run's routing stays.",
        KeyUse::Honoured,
    ),
    key(
        "skills",
        &["allowed-skills", "allowedSkills"],
        "string|array",
        "Skills the agent loads.",
        KeyUse::Honoured,
    ),
    key(
        "mcpServers",
        &["mcp-servers", "mcp_servers"],
        "string|array",
        "MCP server names the agent uses; lint checks each names an `mcp_servers` record.",
        KeyUse::Honoured,
    ),
    key(
        "permissionMode",
        &["permission-mode", "permission_mode"],
        "string",
        "`plan` makes the agent read-only; `bypassPermissions` rejects the file.",
        KeyUse::Honoured,
    ),
    key(
        "permissions",
        &["permission"],
        "object|array|string",
        "Rules: `{allow: [...], deny: [...]}`, `{tool: allow|deny}`, `{tool: {\"filter\": allow}}` or a list of `allow:tool(filter)` / `deny:tool`. A repository file may only deny; `bypass` rejects the file.",
        KeyUse::Honoured,
    ),
    key(
        "maxTurns",
        &[
            "max-turns",
            "max_turns",
            "maxSteps",
            "max-steps",
            "max_steps",
            "steps",
        ],
        "integer",
        "A cap on model calls; it only lowers the run's own.",
        KeyUse::Honoured,
    ),
    key(
        "readonly",
        &["read-only", "read_only", "readOnly"],
        "boolean",
        "Run with the read-only tool set.",
        KeyUse::Honoured,
    ),
    key(
        "hooks",
        &[],
        "object",
        "Not run by PULSE; a repository file that sets it does not load.",
        KeyUse::Guarded,
    ),
    key(
        "pulse",
        &[],
        "object",
        "What PULSE reads beyond the harness keys: see the `pulse` object.",
        KeyUse::Honoured,
    ),
    key("title", &[], "string", "A record's title.", KeyUse::Passed),
    key(
        "handoffs",
        &[],
        "array",
        "Copilot hand-offs; not used.",
        KeyUse::Ignored,
    ),
    key(
        "argument-hint",
        &["argumentHint", "argument_hint"],
        "string",
        "Claude's slash-command hint; not used.",
        KeyUse::Ignored,
    ),
    key(
        "user-invocable",
        &["userInvocable", "user_invocable"],
        "boolean",
        "Not used.",
        KeyUse::Ignored,
    ),
    key(
        "disable-model-invocation",
        &["disableModelInvocation", "disable_model_invocation"],
        "boolean",
        "Not used.",
        KeyUse::Ignored,
    ),
    key("color", &[], "string", "Not used.", KeyUse::Ignored),
    key(
        "target",
        &[],
        "string",
        "Copilot's target (vscode, github-copilot); not used.",
        KeyUse::Ignored,
    ),
    key("temperature", &[], "number", "Not used.", KeyUse::Ignored),
    key("effort", &[], "string", "Not used.", KeyUse::Ignored),
    key(
        "memory",
        &[],
        "string",
        "Claude's memory scope; not used.",
        KeyUse::Ignored,
    ),
    key(
        "isolation",
        &[],
        "string",
        "Claude's worktree isolation; not used (a run has its own clone).",
        KeyUse::Ignored,
    ),
    key(
        "background",
        &["is_background", "isBackground"],
        "boolean",
        "Not used.",
        KeyUse::Ignored,
    ),
    key("hidden", &[], "boolean", "Not used.", KeyUse::Ignored),
    key(
        "mode",
        &[],
        "string",
        "OpenCode's mode; not used.",
        KeyUse::Ignored,
    ),
    key(
        "infer",
        &[],
        "boolean",
        "Copilot's infer switch; not used.",
        KeyUse::Ignored,
    ),
];

/// The keys of the nested `pulse` object.
pub const PULSE_KEYS: &[KeySpec] = &[
    key(
        "role",
        &[],
        "string",
        "The lead role this agent seats.",
        KeyUse::Honoured,
    ),
    key(
        "kinds",
        &[],
        "array|string",
        "Job kinds whose cards this agent takes (`task`, `review`, ...).",
        KeyUse::Honoured,
    ),
    key(
        "select",
        &[],
        "object|array",
        "`{tags: [...], files: [...]}`: a card is taken when its `Tags:` (else `Class:`) words meet `tags` and a `File:` path matches a `files` glob; a plain list means tags.",
        KeyUse::Honoured,
    ),
    key(
        "max_steps",
        &["maxSteps", "max-steps"],
        "integer",
        "A cap on model calls; it only lowers the run's own.",
        KeyUse::Honoured,
    ),
    key(
        "max_subagents",
        &["maxSubagents", "max-subagents"],
        "integer",
        "How many helpers a run as this agent may start.",
        KeyUse::Honoured,
    ),
    key(
        "quorum",
        &[],
        "integer",
        "Approvals the agent's proposals need; a lower layer may raise it, never lower it.",
        KeyUse::Honoured,
    ),
    key(
        "project_scope",
        &["projectScope", "project-scope"],
        "string",
        "`product`, `operating-layer` or `both`.",
        KeyUse::Honoured,
    ),
    key(
        "brief",
        &[],
        "string",
        "The brief a scheduled run of this agent gets.",
        KeyUse::Honoured,
    ),
    key(
        "schedule",
        &[],
        "object|string",
        "`{every: 6h}` (or the text `6h`): how often the agent runs; 60 minutes is the floor.",
        KeyUse::Honoured,
    ),
    key(
        "every",
        &[],
        "string",
        "Short for `schedule.every`.",
        KeyUse::Honoured,
    ),
    key("title", &[], "string", "A role's title.", KeyUse::Passed),
    key(
        "away_seatable",
        &["awaySeatable"],
        "boolean",
        "A role record key; read from the role record only.",
        KeyUse::Passed,
    ),
    key(
        "gaps",
        &[],
        "array",
        "A role record key; read from the role record only.",
        KeyUse::Passed,
    ),
    key(
        "model_class",
        &["modelClass"],
        "string",
        "A role record key; read from the role record only.",
        KeyUse::Passed,
    ),
    key(
        "cron",
        &[],
        "string",
        "Reserved for a later phase.",
        KeyUse::Passed,
    ),
    key(
        "depth",
        &[],
        "integer",
        "Reserved for a later phase.",
        KeyUse::Passed,
    ),
    key(
        "executor",
        &["executors"],
        "string",
        "An agent file may not choose an executor (Tier 0/1).",
        KeyUse::Forbidden,
    ),
    key(
        "self_update",
        &["self-update", "selfUpdate"],
        "object",
        "An agent file may not touch self-update (Tier 0/1).",
        KeyUse::Forbidden,
    ),
    key(
        "secrets",
        &["credentials"],
        "object",
        "An agent file may not carry secrets.",
        KeyUse::Forbidden,
    ),
    key(
        "hooks",
        &[],
        "object",
        "Not run by PULSE.",
        KeyUse::Forbidden,
    ),
];

/// The table entry a key (in any spelling, any case) belongs to.
pub fn key_spec(table: &'static [KeySpec], name: &str) -> Option<&'static KeySpec> {
    table.iter().find(|k| {
        k.key.eq_ignore_ascii_case(name) || k.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    })
}

/// Reads one agent file's text into a spec. `stem` is the file name
/// without `.md` / `.agent.md` (a record's slug); `source` is what the
/// spec is shown as.
pub fn parse_agent(raw: &str, stem: &str, layer: AgentLayer, source: &str) -> AgentDef {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let (front, body) = split_front_matter(raw);
    let unclosed = front.is_empty() && raw.trim_start().starts_with("---");
    let map = front_map(front);
    let mut def = parse_spec(map, body, stem, layer, source);
    if unclosed {
        def.error("front matter opens with --- and never closes");
    }
    def
}

/// The front matter as a map: the mini-YAML reader of the role records,
/// after the `>` / `|` blocks are folded into one line each.
pub fn front_map(front: &str) -> Map<String, Value> {
    if front.trim().is_empty() {
        return Map::new();
    }
    let text = format!("---\n{}\n---\n", fold_block_scalars(front));
    pulse_tasks::leads::role_block(&text).0
}

/// Rewrites the `key: >` / `key: |` blocks of a front matter into one
/// `key: "..."` line each, so a reader of one line per key keeps their
/// text.
fn fold_block_scalars(front: &str) -> String {
    let lines: Vec<&str> = front.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        let key = line.split_once(':').map(|(k, _)| k.trim());
        let value = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
        let style = value.trim_end_matches(['-', '+']);
        let folds = key.is_some_and(|k| !k.is_empty() && !k.starts_with('#'))
            && (style == ">" || style == "|");
        if !folds {
            out.push(line.to_string());
            i += 1;
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut block_indent: Option<usize> = None;
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            if next.trim().is_empty() {
                parts.push(String::new());
                i += 1;
                continue;
            }
            let next_indent = next.len() - next.trim_start().len();
            if next_indent <= indent {
                break;
            }
            let cut = next_indent.min(*block_indent.get_or_insert(next_indent));
            parts.push(next[cut..].to_string());
            i += 1;
        }
        while parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
        let text = if style == "|" {
            parts.join("\n")
        } else {
            let mut text = String::new();
            for part in &parts {
                if part.is_empty() {
                    text.push('\n');
                } else {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push(' ');
                    }
                    text.push_str(part.trim());
                }
            }
            text
        };
        out.push(format!(
            "{}{}: {}",
            &line[..indent],
            key.unwrap_or(""),
            serde_json::to_string(&text).unwrap_or_default()
        ));
    }
    out.join("\n")
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A list key's items: an array's scalars, or a text split on its commas
/// (the ones outside parentheses).
fn list_of(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items.iter().filter_map(scalar_text).collect(),
        Value::Null => Vec::new(),
        other => scalar_text(other)
            .map(|s| split_specs(&s))
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `Name With Spaces` -> `name-with-spaces`; whether it fits
/// `^[a-z0-9][a-z0-9-]{0,63}$` is checked apart.
pub fn normalize_name(given: &str) -> String {
    given
        .trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Whether a name fits `^[a-z0-9][a-z0-9-]{0,63}$`.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= NAME_LIMIT
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The spec a parsed front matter and a body make.
fn parse_spec(
    map: Map<String, Value>,
    body: &str,
    stem: &str,
    layer: AgentLayer,
    source: &str,
) -> AgentDef {
    let mut def = AgentDef {
        layer,
        source: source.to_string(),
        ..AgentDef::default()
    };
    let stem = stem.strip_suffix(".agent").unwrap_or(stem);
    let name = normalize_name(stem);
    if name != stem {
        def.warn(format!("name `{stem}` read as `{name}`"));
    }
    if !valid_name(&name) {
        def.error(format!(
            "name `{name}` is not `^[a-z0-9][a-z0-9-]{{0,63}}$`"
        ));
    }
    def.name = name;
    let mut max_steps: Option<u32> = None;
    let mut lower_steps = |def: &mut AgentDef, v: &Value, key: &str| match as_int(v) {
        Some(n) if n > 0 => {
            let n = u32::try_from(n).unwrap_or(u32::MAX);
            max_steps = Some(max_steps.map_or(n, |have| have.min(n)));
        }
        _ => def.warn(format!("`{key}` wants a positive whole number — ignored")),
    };
    for (k, v) in &map {
        let Some(spec) = key_spec(FRONT_KEYS, k) else {
            def.warn(format!("unknown key `{k}` — ignored"));
            continue;
        };
        match spec.handling {
            KeyUse::Ignored => {
                def.warn(format!("`{k}` is not used by PULSE — ignored"));
                continue;
            }
            KeyUse::Passed => continue,
            KeyUse::Forbidden => {
                def.error(format!("`{k}`: {}", spec.about));
                continue;
            }
            KeyUse::Guarded => {
                if layer.owner_trusted() {
                    def.warn(format!("`{k}` is not run by PULSE — ignored"));
                } else {
                    def.reject(format!("a {} may not set `{k}`", layer.what()));
                }
                continue;
            }
            KeyUse::Honoured => {}
        }
        match spec.key {
            "name" => {
                let given = scalar_text(v).unwrap_or_default();
                if !given.is_empty() && normalize_name(&given) != def.name {
                    let what = if layer == AgentLayer::Store {
                        "record's slug"
                    } else {
                        "file name"
                    };
                    def.warn(format!(
                        "name `{given}` is not the {what} `{}` — the {what} wins",
                        def.name
                    ));
                }
            }
            "description" => def.description = scalar_text(v).unwrap_or_default(),
            "tools" => {
                let list = read_tools(&mut def, v);
                def.tools = if list.iter().any(|t| t == "*") {
                    None
                } else {
                    Some(list)
                };
            }
            "disallowedTools" => def.denied = read_tools(&mut def, v),
            "model" => {
                let m = scalar_text(v).unwrap_or_default();
                let word = m.to_ascii_lowercase();
                def.model = (!m.is_empty() && word != "inherit" && word != "default").then_some(m);
            }
            "skills" => def.skills = list_of(v),
            "mcpServers" => def.mcp_servers = list_of(v),
            "permissionMode" => {
                let mode = scalar_text(v).unwrap_or_default();
                match mode.to_ascii_lowercase().as_str() {
                    "plan" => def.read_only = true,
                    "bypasspermissions" | "bypass" => {
                        def.reject(format!("`{k}: {mode}` asks to bypass the permission gate"));
                    }
                    "" | "default" | "acceptedits" | "dontask" | "ask" | "auto" => {
                        if !mode.is_empty() {
                            def.warn(format!("`{k}: {mode}` has no PULSE equivalent — ignored"));
                        }
                    }
                    _ => def.warn(format!("`{k}: {mode}` is not a mode PULSE knows — ignored")),
                }
            }
            "permissions" => read_permissions(&mut def, v, k),
            "maxTurns" => lower_steps(&mut def, v, k),
            "readonly" => match as_bool(v) {
                Some(b) => def.read_only |= b,
                None => def.warn(format!("`{k}` wants true or false — ignored")),
            },
            "pulse" => match v {
                Value::Object(block) => read_pulse_block(&mut def, block, &mut lower_steps),
                _ => def.warn("`pulse` wants a block of keys — ignored"),
            },
            _ => {}
        }
    }
    def.max_steps = max_steps;
    if def.description.is_empty() {
        def.description = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("")
            .to_string();
    }
    let described = def.description.chars().count();
    if described > DESCRIPTION_LIMIT {
        def.error(format!(
            "description is {described} characters, over the {DESCRIPTION_LIMIT} limit — cut"
        ));
        def.description = format!(
            "{}...",
            def.description
                .chars()
                .take(DESCRIPTION_LIMIT - 3)
                .collect::<String>()
                .trim_end()
        );
    }
    let body = body.trim();
    let length = body.chars().count();
    if length > AGENT_BODY_LIMIT {
        def.error(format!(
            "body is {length} characters, over the {AGENT_BODY_LIMIT} limit — cut"
        ));
        let cut = body
            .char_indices()
            .nth(AGENT_BODY_LIMIT)
            .map_or(body.len(), |(i, _)| i);
        def.body = format!(
            "{}\n[... cut at {AGENT_BODY_LIMIT} characters]",
            body[..cut].trim_end()
        );
    } else {
        def.body = body.to_string();
    }
    def
}

/// A `tools` or `disallowedTools` value as PULSE names, deduplicated, in
/// order; each foreign name is noted in `renamed`, an unknown one warned.
fn read_tools(def: &mut AgentDef, v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list_of(v) {
        let names = tool_names(&item);
        if names.is_empty() {
            continue;
        }
        let bare = item
            .split_once('(')
            .map_or(item.as_str(), |(n, _)| n)
            .trim();
        if names.len() != 1 || names[0] != bare {
            def.renamed.push(format!("{item} -> {}", names.join(", ")));
        }
        for name in names {
            if !known_tool(&name) {
                def.warn(format!(
                    "tool `{item}` is not a PULSE tool name; kept as `{name}`, which grants nothing the run does not offer"
                ));
            }
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Whether a name is one the harness can offer: a native tool, one of the
/// run's own (`subagent`, `skill`, `fetch`, ...), or an MCP pattern.
pub fn known_tool(name: &str) -> bool {
    const KNOWN: &[&str] = &[
        "*",
        "bash",
        "read",
        "grep",
        "glob",
        "edit",
        "write",
        "plan",
        "subagent",
        "skill",
        "fetch",
        "web_search",
        "question",
        "memory",
        "process",
        "mcp_search",
    ];
    KNOWN.contains(&name) || name.starts_with("mcp__")
}

/// `Bash(git *)` as the harness rule patterns it stands for: the PULSE
/// names with the filter kept (`bash(git *)`).
fn rule_patterns(given: &str) -> Vec<String> {
    let given = given.trim();
    let (name, filter) = match given.split_once('(') {
        Some((n, rest)) => (n.trim(), rest.strip_suffix(')').unwrap_or(rest).trim()),
        None => (given, ""),
    };
    tool_names(name)
        .into_iter()
        .map(|n| {
            if filter.is_empty() || filter == "*" {
                n
            } else {
                format!("{n}({filter})")
            }
        })
        .collect()
}

/// Adds one allow or deny rule for a pattern in any harness's spelling.
/// A repository file may only deny: its allow rules are dropped, and one
/// that allows everything rejects the file.
fn add_rule(def: &mut AgentDef, allow: bool, given: &str, key: &str) {
    for pattern in rule_patterns(given) {
        if allow && !def.layer.owner_trusted() {
            if pattern == "*" {
                def.reject(format!(
                    "`{key}` allows every tool from a {}",
                    def.layer.what()
                ));
            } else {
                def.warn(format!(
                    "`{key}` allow rule `{pattern}` from a {} — dropped (a repository file may only narrow)",
                    def.layer.what()
                ));
            }
            continue;
        }
        if let Err(e) = Rule::parse(allow, &pattern, Source::Agent) {
            def.error(format!("`{key}` rule `{given}`: {e}"));
            continue;
        }
        let entry = format!("{}:{pattern}", if allow { "allow" } else { "deny" });
        if !def.permissions.contains(&entry) {
            def.permissions.push(entry);
        }
    }
}

/// `allow`, `deny`, `ask` or `bypass` for one pattern.
fn add_rule_word(def: &mut AgentDef, tool: &str, filter: &str, word: &str, key: &str) {
    let pattern = if filter.is_empty() {
        tool.to_string()
    } else {
        format!("{tool}({filter})")
    };
    match word.trim().to_ascii_lowercase().as_str() {
        "allow" | "true" => add_rule(def, true, &pattern, key),
        "deny" | "false" => add_rule(def, false, &pattern, key),
        "ask" => def.warn(format!(
            "`{key}`: `{pattern}: ask` — PULSE does not ask; the rule is dropped"
        )),
        "bypass" | "bypasspermissions" => def.reject(format!(
            "`{key}`: `{pattern}: {word}` asks to bypass the permission gate"
        )),
        other => def.warn(format!(
            "`{key}`: `{pattern}: {other}` is not allow or deny — ignored"
        )),
    }
}

/// The `permissions` key in its forms: Claude's `{allow: [...], deny:
/// [...]}`, OpenCode's `{tool: allow}` and `{tool: {"filter": allow}}`, a
/// list of `allow:PATTERN` / `deny:PATTERN`, or the word `bypass`.
fn read_permissions(def: &mut AgentDef, value: &Value, key: &str) {
    match value {
        Value::String(s) => {
            let word = s.trim().to_ascii_lowercase();
            if word == "bypass" || word == "bypasspermissions" {
                def.reject(format!("`{key}: {s}` asks to bypass the permission gate"));
            } else {
                def.warn(format!("`{key}: {s}` is not a form PULSE reads — ignored"));
            }
        }
        Value::Array(items) => {
            for item in items.iter().filter_map(scalar_text) {
                match item.split_once(':') {
                    Some((verb, pattern)) if verb.trim().eq_ignore_ascii_case("allow") => {
                        add_rule(def, true, pattern, key);
                    }
                    Some((verb, pattern)) if verb.trim().eq_ignore_ascii_case("deny") => {
                        add_rule(def, false, pattern, key);
                    }
                    _ => def.error(format!(
                        "`{key}` entry `{item}`: say allow:PATTERN or deny:PATTERN"
                    )),
                }
            }
        }
        Value::Object(map) => {
            let claude_form = !map.is_empty()
                && map
                    .keys()
                    .all(|k| matches!(k.as_str(), "allow" | "deny" | "ask"));
            if claude_form {
                for (verb, list) in map {
                    for item in list_of(list) {
                        add_rule_word(def, &item, "", verb, key);
                    }
                }
                return;
            }
            for (tool, v) in map {
                match v {
                    Value::String(_) | Value::Bool(_) => {
                        let word = scalar_text(v).unwrap_or_default();
                        add_rule_word(def, tool, "", &word, key);
                    }
                    Value::Object(inner) => {
                        for (filter, word) in inner {
                            match (filter.as_str(), word) {
                                ("allow" | "deny" | "ask", list) => {
                                    for item in list_of(list) {
                                        add_rule_word(def, tool, &item, filter, key);
                                    }
                                }
                                (filter, Value::String(w)) => {
                                    add_rule_word(def, tool, filter, w, key);
                                }
                                _ => def.warn(format!(
                                    "`{key}`: `{tool}.{filter}` is not a form PULSE reads — ignored"
                                )),
                            }
                        }
                    }
                    Value::Array(list) => {
                        for filter in list.iter().filter_map(scalar_text) {
                            add_rule_word(def, tool, &filter, "allow", key);
                        }
                    }
                    _ => def.warn(format!(
                        "`{key}`: `{tool}` is not a form PULSE reads — ignored"
                    )),
                }
            }
        }
        _ => def.warn(format!("`{key}` is not a form PULSE reads — ignored")),
    }
}

/// The nested `pulse` block: what PULSE reads beyond the harness keys.
fn read_pulse_block(
    def: &mut AgentDef,
    block: &Map<String, Value>,
    lower_steps: &mut impl FnMut(&mut AgentDef, &Value, &str),
) {
    for (k, v) in block {
        let Some(spec) = key_spec(PULSE_KEYS, k) else {
            def.warn(format!("unknown key `pulse.{k}` — ignored"));
            continue;
        };
        match spec.handling {
            KeyUse::Passed => continue,
            KeyUse::Forbidden => {
                def.error(format!("`pulse.{k}`: {}", spec.about));
                continue;
            }
            KeyUse::Ignored => {
                def.warn(format!("`pulse.{k}` is not used — ignored"));
                continue;
            }
            KeyUse::Guarded | KeyUse::Honoured => {}
        }
        match spec.key {
            "role" => def.role = scalar_text(v).unwrap_or_default(),
            "kinds" => {
                def.kinds = list_of(v)
                    .into_iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
            }
            "select" => match v {
                Value::Object(sel) => {
                    for (sk, sv) in sel {
                        match sk.as_str() {
                            "tags" | "tag" | "class" => def.select_tags = lowered(list_of(sv)),
                            "files" | "file" | "paths" | "path" => def.select_files = list_of(sv),
                            other => {
                                def.warn(format!("unknown key `pulse.select.{other}` — ignored"))
                            }
                        }
                    }
                }
                other => def.select_tags = lowered(list_of(other)),
            },
            "max_steps" => lower_steps(def, v, "pulse.max_steps"),
            "max_subagents" => match as_int(v) {
                Some(n) if n >= 0 => {
                    let n = u32::try_from(n).unwrap_or(u32::MAX);
                    def.max_subagents = Some(def.max_subagents.map_or(n, |have| have.min(n)));
                }
                _ => def.warn("`pulse.max_subagents` wants a whole number — ignored"),
            },
            "quorum" => match as_int(v) {
                Some(n) if n >= 1 => def.quorum = Some(n),
                _ => def.warn("`pulse.quorum` wants a whole number of at least 1 — ignored"),
            },
            "project_scope" => {
                let scope = scalar_text(v).unwrap_or_default().to_ascii_lowercase();
                match scope.as_str() {
                    "product" | "operating-layer" | "both" => def.project_scope = scope,
                    other => def.warn(format!(
                        "`pulse.project_scope: {other}` is not product, operating-layer or both — ignored"
                    )),
                }
            }
            "brief" => def.brief = scalar_text(v).unwrap_or_default(),
            "schedule" => match v {
                Value::Object(sched) => {
                    for (sk, sv) in sched {
                        match sk.as_str() {
                            "every" => set_every(def, &scalar_text(sv).unwrap_or_default()),
                            "cron" => {}
                            other => {
                                def.warn(format!("unknown key `pulse.schedule.{other}` — ignored"))
                            }
                        }
                    }
                }
                other => set_every(def, &scalar_text(other).unwrap_or_default()),
            },
            "every" => set_every(def, &scalar_text(v).unwrap_or_default()),
            _ => {}
        }
    }
}

fn lowered(items: Vec<String>) -> Vec<String> {
    items.into_iter().map(|s| s.to_ascii_lowercase()).collect()
}

/// `pulse.schedule.every`: a period the schedules read, 60 minutes or more.
fn set_every(def: &mut AgentDef, text: &str) {
    if text.is_empty() {
        return;
    }
    match pulse_tasks::schedules::parse_every(text) {
        Ok(minutes) if minutes < MIN_EVERY_MINUTES => def.error(format!(
            "`pulse.schedule.every: {text}` is under the {MIN_EVERY_MINUTES}-minute floor (Tier 0/1)"
        )),
        Ok(_) => {}
        Err(e) => def.error(format!("`pulse.schedule.every: {text}`: {e}")),
    }
    def.schedule_every = text.to_string();
}

/// The PULSE tool names a tool name of any harness stands for. Claude's
/// `Bash(git *)` style keeps its filter out: the agent's lists speak for
/// tools whole (the `permissions` key keeps filters). MCP tools come as
/// Claude's `mcp__server__tool` (kept), Copilot's `server/tool` and
/// `server/*`, or OpenCode's `mcp_server_tool` (the first underscore
/// after `mcp_` ends the server name).
pub fn tool_names(given: &str) -> Vec<String> {
    let name = given.trim();
    let name = name.split_once('(').map_or(name, |(n, _)| n).trim();
    if name.is_empty() {
        return Vec::new();
    }
    if name.starts_with("mcp__") {
        return vec![name.to_string()];
    }
    if let Some((server, tool)) = name.split_once('/')
        && !server.is_empty()
        && !server.contains(char::is_whitespace)
    {
        let tool = if tool.is_empty() { "*" } else { tool };
        return vec![format!("mcp__{server}__{tool}")];
    }
    if let Some(rest) = name.strip_prefix("mcp_")
        && !rest.is_empty()
        && !rest.starts_with('_')
    {
        return vec![match rest.split_once('_') {
            Some((server, tool)) if !tool.is_empty() => format!("mcp__{server}__{tool}"),
            Some((server, _)) => format!("mcp__{server}__*"),
            None if rest == "search" => "mcp_search".to_string(),
            None => format!("mcp__{rest}__*"),
        }];
    }
    let lower = name.to_ascii_lowercase();
    let one = |s: &str| vec![s.to_string()];
    match lower.as_str() {
        "*" | "all" => one("*"),
        "read" | "read_file" | "readfile" | "view" | "cat" | "notebookread" => one("read"),
        "edit" | "multiedit" | "str_replace" | "str_replace_editor" | "edit_file"
        | "notebookedit" | "apply_patch" | "patch" => one("edit"),
        "write" | "create" | "write_file" | "create_file" | "writefile" => one("write"),
        "bash" | "shell" | "execute" | "run" | "terminal" | "powershell" | "run_in_terminal"
        | "execute_command" | "run_command" | "shell_command" => one("bash"),
        "grep" | "rg" | "search_text" | "grep_search" => one("grep"),
        "glob" | "find" | "ls" | "list" | "file_search" | "list_dir" => one("glob"),
        "search" | "codebase" | "codebase_search" => vec!["grep".to_string(), "glob".to_string()],
        "webfetch" | "fetch" | "web_fetch" | "fetch_webpage" => one("fetch"),
        "websearch" | "web_search" => one("web_search"),
        "web" => vec!["fetch".to_string(), "web_search".to_string()],
        "task" | "subagent" | "agent" | "delegate" | "runsubagent" | "custom-agent"
        | "custom_agent" | "customagent" => one("subagent"),
        "skill" | "skills" => one("skill"),
        "todowrite" | "todo" | "todos" | "plan" | "manage_todo_list" => one("plan"),
        "askuserquestion" | "ask_user" | "question" | "ask" => one("question"),
        "memory" | "remember" => one("memory"),
        "process" | "bashoutput" | "killshell" | "killbash" | "background" => one("process"),
        _ => one(name),
    }
}

/// The system-prompt section a run gets when it runs as this agent
/// (`pulse harness run --agent NAME`): the body as standing instructions,
/// wrapped the way the `subagent` tool wraps a helper's.
pub fn standing_instructions(a: &AgentDef) -> String {
    if a.body.trim().is_empty() {
        return format!("You are the {} agent ({}).", a.name, a.source);
    }
    format!(
        "You are the {name} agent. Your standing instructions follow; the task comes after them and the rules above still hold.\n\n--- {source} ---\n{body}\n--- end of {name} ---",
        name = a.name,
        source = a.source,
        body = a.body.trim_end()
    )
}

/// The tools of `offered` an agent loses: everything outside its `tools`
/// list and everything `disallowedTools` names, as glob patterns a deny
/// rule takes.
pub fn withheld_tools(agent: &AgentDef, offered: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in offered {
        let kept = agent
            .tools
            .as_ref()
            .is_none_or(|list| list.iter().any(|p| tool_glob(p, name)));
        let denied = agent.denied.iter().any(|p| tool_glob(p, name));
        if (!kept || denied) && !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// Whether `name` matches the tool glob `pattern` (`*` any run, `?` one
/// character; case-insensitive).
pub fn tool_glob(pattern: &str, name: &str) -> bool {
    fn walk(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => walk(&p[1..], n) || (!n.is_empty() && walk(p, &n[1..])),
            (Some('?'), Some(_)) => walk(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => walk(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    walk(&p, &n)
}

/// Whether the `models.allowed` setting (`allowed`: a comma or newline
/// list, `*` globs; empty lets every model through) lets `model` run.
pub fn model_allowed(allowed: &str, model: &str) -> bool {
    let patterns: Vec<&str> = allowed
        .split([',', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    patterns.is_empty() || patterns.iter().any(|p| tool_glob(p, model.trim()))
}

/// The agents' part of the `subagent` tool description.
pub fn describe_for_tool(agents: &[AgentDef]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let mut listing: Vec<String> = agents
        .iter()
        .take(AGENT_LIST_LIMIT)
        .map(|a| {
            if a.description.is_empty() {
                return a.name.clone();
            }
            let mut description = a.description.clone();
            if description.chars().count() > LISTING_DESCRIPTION_LIMIT {
                description = format!(
                    "{}...",
                    description
                        .chars()
                        .take(LISTING_DESCRIPTION_LIMIT - 3)
                        .collect::<String>()
                        .trim_end()
                );
            }
            format!("{}: {description}", a.name)
        })
        .collect();
    if agents.len() > AGENT_LIST_LIMIT {
        listing.push(format!("... and {} more", agents.len() - AGENT_LIST_LIMIT));
    }
    format!(
        " Custom agents (pass one as agent; it brings its own instructions and tools): {}.",
        listing.join("; ")
    )
}

/// Reads the agents of the clone at `cwd` and the owner's own under
/// `<home>/agents`, without the store: the specs that load, one per
/// name, the most trusted layer owning each.
pub fn load_agents(cwd: &str, home: &str) -> Vec<AgentDef> {
    load_agent_specs(cwd, home, None, "")
        .into_iter()
        .filter(AgentDef::loads)
        .collect()
}

/// Reads every layer - the store's `agent` records of `project` (and the
/// commons), `~/.pulse/agents`, the clone's `.pulse/agents`,
/// `.claude/agents`, `.github/agents` - and merges them: one spec per
/// name, the most trusted layer owning it, lower twins narrowing it. A
/// rejected spec is returned too, with `rejected` set, so a caller can
/// name it; [`AgentDef::loads`] tells them apart.
pub fn load_agent_specs(
    cwd: &str,
    home: &str,
    store: Option<&pulse_store::Store>,
    project: &str,
) -> Vec<AgentDef> {
    merge_specs(read_agent_layers(cwd, home, store, project))
}

/// Every spec of every layer, unmerged, in trust order (then file order):
/// what `pulse agents lint` reads one by one.
pub fn read_agent_layers(
    cwd: &str,
    home: &str,
    store: Option<&pulse_store::Store>,
    project: &str,
) -> Vec<AgentDef> {
    let root = if cwd.is_empty() {
        Path::new(".")
    } else {
        Path::new(cwd)
    };
    let mut out: Vec<AgentDef> = Vec::new();
    if let Some(s) = store {
        out.extend(store_agents(s, project));
    }
    for layer in AgentLayer::FILE_LAYERS {
        let Some(dir) = layer.dir(root, home) else {
            continue;
        };
        for file in agent_files(&dir) {
            if let Some(def) = read_agent_file(&file, layer) {
                out.push(def);
            }
        }
    }
    out
}

/// The `*.md` files of a directory, sorted.
pub fn agent_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

/// One file of a layer as a spec; None when it cannot be read.
pub fn read_agent_file(file: &Path, layer: AgentLayer) -> Option<AgentDef> {
    let raw = fs::read_to_string(file).ok()?;
    let stem = file.file_stem()?.to_string_lossy().to_string();
    let source = format!("{}/{}", layer.label(), file.file_name()?.to_string_lossy());
    Some(parse_agent(&raw, &stem, layer, &source))
}

/// Record statuses that take an `agent` record out of the running.
const RETIRED_STATUSES: &[&str] = &[
    "retired",
    "deprecated",
    "superseded",
    "rejected",
    "archived",
    "ended",
    "withdrawn",
];

/// The `agent` records of the commons, then the project (a project
/// record of a name replaces the commons' one), as specs.
fn store_agents(s: &pulse_store::Store, project: &str) -> Vec<AgentDef> {
    let mut out: Vec<AgentDef> = Vec::new();
    let mut scopes = vec![pulse_store::COMMONS_SLUG.to_string()];
    if !project.is_empty() && project != pulse_store::COMMONS_SLUG {
        scopes.push(project.to_string());
    }
    for scope in scopes {
        let Ok(mut records) = s.list_records(pulse_store::RecordFilter {
            project: scope,
            kind: "agent".into(),
            ..Default::default()
        }) else {
            continue;
        };
        records.sort_by(|a, b| a.slug.cmp(&b.slug));
        for r in records {
            if RETIRED_STATUSES
                .iter()
                .any(|st| r.status.eq_ignore_ascii_case(st))
            {
                continue;
            }
            let Ok((_, Some(rev))) = s.get_record(&r.id, 0) else {
                continue;
            };
            let def = agent_from_record(&r, &rev);
            match out.iter_mut().find(|x| x.name == def.name) {
                Some(slot) => *slot = def,
                None => out.push(def),
            }
        }
    }
    out
}

/// One `agent` record as a spec: its body is the file whole (front matter
/// included, as the importer keeps it); the record's own front matter
/// fills in the spec keys the body does not set.
pub fn agent_from_record(r: &pulse_store::Record, rev: &pulse_store::Revision) -> AgentDef {
    let raw = rev.body.strip_prefix('\u{feff}').unwrap_or(&rev.body);
    let (front, body) = split_front_matter(raw);
    let mut map = front_map(front);
    for (k, v) in &rev.frontmatter {
        if key_spec(FRONT_KEYS, k).is_some() && !map.contains_key(k) {
            map.insert(k.clone(), v.clone());
        }
    }
    let source = format!("store:{}", r.id);
    parse_spec(map, body, &r.slug, AgentLayer::Store, &source)
}

/// One spec per name out of the layers' specs (in trust order): the first
/// spec of a name owns it, a later twin narrows it. A rejected spec takes
/// no part and is kept at the end, flagged.
pub fn merge_specs(all: Vec<AgentDef>) -> Vec<AgentDef> {
    let mut out: Vec<AgentDef> = Vec::new();
    let mut rejected: Vec<AgentDef> = Vec::new();
    for def in all {
        if !def.loads() {
            rejected.push(def);
            continue;
        }
        match out.iter_mut().find(|a| a.name == def.name) {
            Some(winner) => narrow(winner, &def),
            None => out.push(def),
        }
    }
    out.extend(rejected);
    out
}

fn min_opt(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Folds a lower-trust twin into the spec that owns the name: fewer
/// tools, more denies, a smaller cap, read-only, a higher quorum; what
/// else the twin says is dropped with a warning.
fn narrow(winner: &mut AgentDef, lower: &AgentDef) {
    let who = format!("{} `{}`", lower.layer.what(), lower.source);
    if let Some(lower_tools) = &lower.tools {
        match winner.tools.clone() {
            None => winner.tools = Some(lower_tools.clone()),
            Some(have) => {
                let covered = |t: &String, p: &String| tool_glob(p, t) || tool_glob(t, p);
                let kept: Vec<String> = have
                    .iter()
                    .filter(|t| lower_tools.iter().any(|p| covered(t, p)))
                    .cloned()
                    .collect();
                let widened: Vec<String> = lower_tools
                    .iter()
                    .filter(|p| !have.iter().any(|t| covered(t, p)))
                    .cloned()
                    .collect();
                if !widened.is_empty() {
                    winner.warn(format!(
                        "{who} tried to widen tools: {} — ignored",
                        widened.join(", ")
                    ));
                }
                winner.tools = Some(kept);
            }
        }
    }
    for d in &lower.denied {
        if !winner.denied.contains(d) {
            winner.denied.push(d.clone());
        }
    }
    let mut allowed: Vec<&str> = Vec::new();
    for p in &lower.permissions {
        if p.starts_with("deny:") {
            if !winner.permissions.contains(p) {
                winner.permissions.push(p.clone());
            }
        } else {
            allowed.push(p);
        }
    }
    if !allowed.is_empty() {
        winner.warn(format!(
            "{who} tried to add allow rules: {} — ignored",
            allowed.join(", ")
        ));
    }
    winner.max_steps = min_opt(winner.max_steps, lower.max_steps);
    winner.max_subagents = min_opt(winner.max_subagents, lower.max_subagents);
    winner.read_only |= lower.read_only;
    if let Some(q) = lower.quorum {
        match winner.quorum {
            Some(have) if q < have => winner.warn(format!(
                "{who} tried to lower quorum from {have} to {q} — ignored"
            )),
            _ => winner.quorum = Some(q),
        }
    }
    let mut widened: Vec<&str> = Vec::new();
    let differs = |mine: &str, theirs: &str| !theirs.is_empty() && mine != theirs;
    if differs(&winner.description, &lower.description) {
        widened.push("description");
    }
    if differs(&winner.body, &lower.body) {
        widened.push("body");
    }
    if lower.model.is_some() && lower.model != winner.model {
        widened.push("model");
    }
    let lists: [(&str, &Vec<String>, &Vec<String>); 5] = [
        ("skills", &winner.skills, &lower.skills),
        ("mcpServers", &winner.mcp_servers, &lower.mcp_servers),
        ("kinds", &winner.kinds, &lower.kinds),
        ("select.tags", &winner.select_tags, &lower.select_tags),
        ("select.files", &winner.select_files, &lower.select_files),
    ];
    for (what, mine, theirs) in lists {
        if !theirs.is_empty() && mine != theirs {
            widened.push(what);
        }
    }
    let texts: [(&str, &str, &str); 4] = [
        ("project_scope", &winner.project_scope, &lower.project_scope),
        ("brief", &winner.brief, &lower.brief),
        ("schedule", &winner.schedule_every, &lower.schedule_every),
        ("role", &winner.role, &lower.role),
    ];
    for (what, mine, theirs) in texts {
        if differs(mine, theirs) {
            widened.push(what);
        }
    }
    if !widened.is_empty() {
        winner.warn(format!(
            "{who} also sets {} — ignored (the {} spec owns the name)",
            widened.join(", "),
            winner.layer.label()
        ));
    }
}

/// The JSON Schema (draft 2020-12) of an agent file's front matter, built
/// from [`FRONT_KEYS`] and [`PULSE_KEYS`].
pub fn agent_schema() -> Value {
    fn types(kind: &str) -> Value {
        let list: Vec<&str> = kind.split('|').collect();
        if list.len() == 1 {
            json!(list[0])
        } else {
            json!(list)
        }
    }
    fn property(k: &KeySpec, nested: Option<Value>) -> Value {
        let mut prop = json!({ "type": types(k.kind), "description": k.about });
        if k.kind.contains("array") {
            prop["items"] = json!({ "type": "string" });
        }
        if !k.aliases.is_empty() {
            prop["x-aliases"] = json!(k.aliases);
        }
        if k.handling != KeyUse::Honoured {
            prop["x-pulse"] = json!(format!("{:?}", k.handling).to_ascii_lowercase());
        }
        if let Some(nested) = nested {
            prop["properties"] = nested;
        }
        prop
    }
    fn properties(table: &'static [KeySpec], nested_key: &str, nested: Value) -> Value {
        let mut out = Map::new();
        for k in table {
            let inner = (k.key == nested_key).then(|| nested.clone());
            let prop = property(k, inner);
            out.insert(k.key.to_string(), prop.clone());
            for alias in k.aliases {
                out.insert((*alias).to_string(), prop.clone());
            }
        }
        Value::Object(out)
    }
    let select = json!({
        "tags": { "type": "array", "items": { "type": "string" }, "description": "Words a card's Tags: line (else its Class: line) must share one of." },
        "files": { "type": "array", "items": { "type": "string" }, "description": "Globs one of the card's File: paths must match." }
    });
    let mut pulse = properties(PULSE_KEYS, "select", select);
    pulse["schedule"]["properties"] = json!({
        "every": { "type": "string", "description": "A period such as 6h or 1d; 60 minutes is the floor." },
        "cron": { "type": "string", "description": "Reserved for a later phase." }
    });
    pulse["project_scope"]["enum"] = json!(["product", "operating-layer", "both"]);
    let front = properties(FRONT_KEYS, "pulse", pulse);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/manishtiwari25/pulse/schemas/agent-front-matter.json",
        "title": "PULSE agent file front matter",
        "description": format!("The front matter of a `.pulse/agents/<name>.md` (or `.claude/agents`, `.github/agents`) file. The body after it is the agent's standing instructions, up to {AGENT_BODY_LIMIT} characters. Names fit ^[a-z0-9][a-z0-9-]{{0,63}}$."),
        "type": "object",
        "properties": front,
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn agents_come_from_every_layout_in_trust_order_and_the_file_name_wins() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".claude/agents/reviewer.md",
            "---\nname: reviewer\ndescription: Reviews a diff for bugs.\ntools: Read, Grep, Glob, Bash(git diff *)\nmodel: sonnet\n---\n\nYou review code. Report findings only.\n",
        );
        write(
            dir.path(),
            ".github/agents/docs.agent.md",
            "---\ndescription: Writes documentation.\ntools: ['read', 'edit', 'search']\n---\n# Docs\n\nKeep prose short.\n",
        );
        write(
            dir.path(),
            ".github/agents/reviewer.agent.md",
            "---\ndescription: Shadowed.\n---\nnever seen\n",
        );
        write(
            dir.path(),
            ".pulse/agents/tester.md",
            "---\nname: Test Runner\ndisallowedTools:\n  - Edit\n  - Write\n---\nRun the tests and report.\n",
        );
        write(
            home.path(),
            "agents/owner.md",
            "Owner's helper with the first line as description.\n\nMore.\n",
        );
        write(home.path(), "agents/notes.txt", "not an agent");
        let agents = load_agents(dir.path().to_str().unwrap(), home.path().to_str().unwrap());
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["owner", "tester", "reviewer", "docs"]);
        let owner = &agents[0];
        assert_eq!(owner.source, "~/.pulse/agents/owner.md");
        assert_eq!(owner.layer, AgentLayer::Home);
        assert_eq!(
            owner.description,
            "Owner's helper with the first line as description."
        );
        let tester = &agents[1];
        assert_eq!(tester.tools, None);
        assert_eq!(tester.denied, ["edit", "write"]);
        assert!(
            tester
                .warnings
                .iter()
                .any(|w| w.contains("name `Test Runner` is not the file name `tester`")),
            "{:?}",
            tester.warnings
        );
        let reviewer = &agents[2];
        assert_eq!(reviewer.source, ".claude/agents/reviewer.md");
        assert_eq!(reviewer.layer, AgentLayer::Claude);
        assert_eq!(reviewer.description, "Reviews a diff for bugs.");
        assert_eq!(reviewer.model.as_deref(), Some("sonnet"));
        assert_eq!(
            reviewer.tools.as_deref(),
            Some(&strs(&["read", "grep", "glob", "bash"])[..])
        );
        assert_eq!(reviewer.body, "You review code. Report findings only.");
        assert!(
            reviewer.warnings.iter().any(
                |w| w.contains(".github/agents/reviewer.agent.md") && w.contains("description")
            ),
            "{:?}",
            reviewer.warnings
        );
        let docs = &agents[3];
        assert_eq!(docs.source, ".github/agents/docs.agent.md");
        assert_eq!(
            docs.tools.as_deref(),
            Some(&strs(&["read", "edit", "grep", "glob"])[..])
        );
    }

    #[test]
    fn foreign_tool_lists_become_pulse_names() {
        let claude = parse_agent(
            "---\ntools: Bash, Read, Edit, mcp__github__*\n---\nbody\n",
            "a",
            AgentLayer::Claude,
            ".claude/agents/a.md",
        );
        assert_eq!(
            claude.tools.as_deref(),
            Some(&strs(&["bash", "read", "edit", "mcp__github__*"])[..])
        );
        assert!(claude.renamed.contains(&"Bash -> bash".to_string()));
        assert!(claude.warnings.is_empty(), "{:?}", claude.warnings);
        let copilot = parse_agent(
            "---\ntools: ['read', 'edit', 'github/*']\n---\nbody\n",
            "b.agent",
            AgentLayer::Github,
            ".github/agents/b.agent.md",
        );
        assert_eq!(copilot.name, "b");
        assert_eq!(
            copilot.tools.as_deref(),
            Some(&strs(&["read", "edit", "mcp__github__*"])[..])
        );
        assert!(
            copilot
                .renamed
                .contains(&"github/* -> mcp__github__*".to_string())
        );
        let opencode = parse_agent(
            "---\ntools:\n  - mcp_github_search\n  - mcp_search\n  - run_command\n---\nbody\n",
            "c",
            AgentLayer::Repo,
            ".pulse/agents/c.md",
        );
        assert_eq!(
            opencode.tools.as_deref(),
            Some(&strs(&["mcp__github__search", "mcp_search", "bash"])[..])
        );
        assert_eq!(tool_names("github/issues"), ["mcp__github__issues"]);
        assert_eq!(tool_names("mcp_github"), ["mcp__github__*"]);
        assert_eq!(tool_names("patch"), ["edit"]);
        assert_eq!(tool_names("codebase_search"), ["grep", "glob"]);
        assert_eq!(tool_names("custom-agent"), ["subagent"]);
        assert_eq!(tool_names("TodoWrite"), ["plan"]);
        let odd = parse_agent(
            "---\ntools: [read, frobnicate]\n---\nbody\n",
            "d",
            AgentLayer::Repo,
            ".pulse/agents/d.md",
        );
        assert_eq!(
            odd.tools.as_deref(),
            Some(&strs(&["read", "frobnicate"])[..])
        );
        assert!(
            odd.warnings
                .iter()
                .any(|w| w.contains("`frobnicate` is not a PULSE tool name")),
            "{:?}",
            odd.warnings
        );
    }

    #[test]
    fn tool_names_take_every_spelling_and_withheld_tools_follow_the_lists() {
        assert_eq!(tool_names("WebFetch"), ["fetch"]);
        assert_eq!(tool_names("shell"), ["bash"]);
        assert_eq!(tool_names("Bash(npm test)"), ["bash"]);
        assert_eq!(tool_names("Task"), ["subagent"]);
        assert_eq!(tool_names("web"), ["fetch", "web_search"]);
        assert_eq!(tool_names("mcp__github__*"), ["mcp__github__*"]);
        assert!(tool_names("  ").is_empty());
        let offered = strs(&[
            "bash",
            "read",
            "grep",
            "glob",
            "edit",
            "write",
            "plan",
            "mcp__github__issues",
        ]);
        let agent = AgentDef {
            name: "r".into(),
            tools: Some(strs(&["read", "grep", "mcp__github__*"])),
            denied: strs(&["grep"]),
            ..AgentDef::default()
        };
        assert_eq!(
            withheld_tools(&agent, &offered),
            ["bash", "grep", "glob", "edit", "write", "plan"]
        );
        let open = AgentDef {
            tools: None,
            denied: Vec::new(),
            description: "x".repeat(300),
            ..agent
        };
        assert!(withheld_tools(&open, &offered).is_empty());
        let text = describe_for_tool(std::slice::from_ref(&open));
        assert!(text.contains("Custom agents") && text.contains("r: xxx"));
        assert!(text.contains("...") && text.len() < 400);
        assert_eq!(describe_for_tool(&[]), "");
    }

    #[test]
    fn the_owner_file_wins_and_a_repo_twin_only_narrows() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "agents/x.md",
            "---\ndescription: Owner's x.\ntools: [read, grep, edit]\nmodel: kimi-k3\npulse:\n  kinds: [task]\n  quorum: 2\n---\nOwner body.\n",
        );
        write(
            dir.path(),
            ".pulse/agents/x.md",
            "---\ndescription: Repo's x.\ntools: [read, grep, write]\nmaxTurns: 5\nmodel: other\ndisallowedTools: [bash]\nreadonly: true\npulse:\n  quorum: 1\n  kinds: [review]\n---\nRepo body.\n",
        );
        let agents = load_agents(dir.path().to_str().unwrap(), home.path().to_str().unwrap());
        assert_eq!(agents.len(), 1);
        let x = &agents[0];
        assert_eq!(x.layer, AgentLayer::Home);
        assert_eq!(x.source, "~/.pulse/agents/x.md");
        assert_eq!(x.description, "Owner's x.");
        assert_eq!(x.body, "Owner body.");
        assert_eq!(x.model.as_deref(), Some("kimi-k3"));
        assert_eq!(x.tools.as_deref(), Some(&strs(&["read", "grep"])[..]));
        assert_eq!(x.denied, ["bash"]);
        assert_eq!(x.max_steps, Some(5));
        assert!(x.read_only);
        assert_eq!(x.quorum, Some(2));
        assert_eq!(x.kinds, ["task"]);
        assert!(
            x.warnings.iter().any(|w| w
                == "repository file `.pulse/agents/x.md` tried to widen tools: write — ignored"),
            "{:?}",
            x.warnings
        );
        assert!(
            x.warnings
                .iter()
                .any(|w| w.contains("tried to lower quorum from 2 to 1")),
            "{:?}",
            x.warnings
        );
        assert!(
            x.warnings
                .iter()
                .any(|w| w.contains("also sets description, body, model, kinds")),
            "{:?}",
            x.warnings
        );
    }

    #[test]
    fn bypass_and_repo_hooks_reject_the_file_in_every_layer_that_asks() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".pulse/agents/free.md",
            "---\ndescription: Wants it all.\npermissionMode: bypassPermissions\n---\nbody\n",
        );
        write(
            home.path(),
            "agents/free.md",
            "---\ndescription: Owner wants it all.\npermissionMode: bypassPermissions\n---\nbody\n",
        );
        write(
            dir.path(),
            ".pulse/agents/hooked.md",
            "---\nhooks:\n  PreToolUse: [{matcher: Bash, hooks: [{type: command, command: echo}]}]\n---\nbody\n",
        );
        write(
            home.path(),
            "agents/hooked-home.md",
            "---\nhooks:\n  PreToolUse: []\n---\nbody\n",
        );
        write(
            dir.path(),
            ".pulse/agents/star.md",
            "---\npermissions:\n  allow: \"*\"\n---\nbody\n",
        );
        let cwd = dir.path().to_str().unwrap();
        let home = home.path().to_str().unwrap();
        let loaded = load_agents(cwd, home);
        let names: Vec<&str> = loaded.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["hooked-home"]);
        assert!(
            loaded[0]
                .warnings
                .iter()
                .any(|w| w.contains("`hooks` is not run by PULSE"))
        );
        let all = load_agent_specs(cwd, home, None, "");
        let rejected: Vec<(&str, &str)> = all
            .iter()
            .filter(|a| !a.loads())
            .map(|a| (a.source.as_str(), a.rejected.as_deref().unwrap()))
            .collect();
        assert_eq!(rejected.len(), 4, "{rejected:?}");
        assert!(
            rejected
                .iter()
                .any(|(s, why)| *s == "~/.pulse/agents/free.md"
                    && why.contains("bypass the permission gate"))
        );
        assert!(
            rejected.iter().any(|(s, why)| *s == ".pulse/agents/free.md"
                && why.contains("bypass the permission gate"))
        );
        assert!(
            rejected
                .iter()
                .any(|(s, why)| *s == ".pulse/agents/hooked.md"
                    && why.contains("may not set `hooks`"))
        );
        assert!(
            rejected
                .iter()
                .any(|(s, why)| *s == ".pulse/agents/star.md" && why.contains("allows every tool"))
        );
    }

    #[test]
    fn permissions_come_in_every_form_and_a_repo_file_only_denies() {
        let claude = parse_agent(
            "---\npermissions:\n  allow:\n    - Bash(git *)\n    - Read\n  deny: [Edit, WebFetch]\n---\nbody\n",
            "p",
            AgentLayer::Home,
            "~/.pulse/agents/p.md",
        );
        assert_eq!(
            claude.permissions,
            ["allow:bash(git *)", "allow:read", "deny:edit", "deny:fetch"]
        );
        let rules = claude.permission_rules();
        assert_eq!(rules.len(), 4);
        assert!(rules[0].allow && rules[0].kind == "bash");
        assert_eq!(rules[0].filter.as_deref(), Some("git *"));
        assert_eq!(rules[0].source, Source::Agent);
        let repo = parse_agent(
            "---\npermissions:\n  allow: [Bash(git *)]\n  deny: [Edit]\n---\nbody\n",
            "p",
            AgentLayer::Repo,
            ".pulse/agents/p.md",
        );
        assert_eq!(repo.permissions, ["deny:edit"]);
        assert!(repo.loads());
        assert!(
            repo.warnings
                .iter()
                .any(|w| w.contains("allow rule `bash(git *)` from a repository file — dropped"))
        );
        let opencode = parse_agent(
            "---\npermission:\n  bash:\n    \"git *\": allow\n    \"rm *\": deny\n  edit: deny\n  write: ask\n---\nbody\n",
            "o",
            AgentLayer::Home,
            "~/.pulse/agents/o.md",
        );
        assert_eq!(
            opencode.permissions,
            ["allow:bash(git *)", "deny:bash(rm *)", "deny:edit"]
        );
        assert!(opencode.warnings.iter().any(|w| w.contains("`write: ask`")));
        let listed = parse_agent(
            "---\npermissions: [\"deny:edit\", \"allow:bash(git tag --list*)\", \"edit\"]\n---\nbody\n",
            "l",
            AgentLayer::Store,
            "store:AGENT-l",
        );
        assert_eq!(
            listed.permissions,
            ["deny:edit", "allow:bash(git tag --list*)"]
        );
        assert!(
            listed
                .errors
                .iter()
                .any(|e| e.contains("say allow:PATTERN"))
        );
        let bypass = parse_agent(
            "---\npermission: bypass\n---\nbody\n",
            "b",
            AgentLayer::Home,
            "~/.pulse/agents/b.md",
        );
        assert!(!bypass.loads());
    }

    #[test]
    fn the_pulse_block_is_read_and_the_tier_rules_are_errors() {
        let def = parse_agent(
            "---\ndescription: >\n  Refactors Rust\n  carefully.\nmodel: kimi-k3\nskills: [rust, tests]\nmcpServers: github, jira\nmaxTurns: 40\ncolor: red\nfoo: bar\npulse:\n  role: reviewer\n  kinds: [task, Review]\n  select:\n    tags: [refactor, Rust]\n    files: ['crates/*.rs']\n  max_steps: 12\n  max_subagents: 2\n  quorum: 3\n  project_scope: product\n  brief: Do the thing.\n  schedule:\n    every: 30m\n  executor: ssh\n  self_update: {on: true}\n  bogus: 1\n---\nBody text.\n",
            "rust-refactorer",
            AgentLayer::Repo,
            ".pulse/agents/rust-refactorer.md",
        );
        assert!(def.loads());
        assert_eq!(def.description, "Refactors Rust carefully.");
        assert_eq!(def.model.as_deref(), Some("kimi-k3"));
        assert_eq!(def.skills, ["rust", "tests"]);
        assert_eq!(def.mcp_servers, ["github", "jira"]);
        assert_eq!(def.role, "reviewer");
        assert_eq!(def.kinds, ["task", "review"]);
        assert_eq!(def.select_tags, ["refactor", "rust"]);
        assert_eq!(def.select_files, ["crates/*.rs"]);
        assert_eq!(def.max_steps, Some(12));
        assert_eq!(def.max_subagents, Some(2));
        assert_eq!(def.quorum, Some(3));
        assert_eq!(def.project_scope, "product");
        assert_eq!(def.brief, "Do the thing.");
        assert_eq!(def.schedule_every, "30m");
        assert_eq!(def.body, "Body text.");
        let errors = def.errors.join("\n");
        assert!(errors.contains("under the 60-minute floor"), "{errors}");
        assert!(errors.contains("`pulse.executor`"), "{errors}");
        assert!(errors.contains("`pulse.self_update`"), "{errors}");
        let warnings = def.warnings.join("\n");
        assert!(warnings.contains("`color` is not used"), "{warnings}");
        assert!(warnings.contains("unknown key `foo`"), "{warnings}");
        assert!(warnings.contains("unknown key `pulse.bogus`"), "{warnings}");
        let plan = parse_agent(
            "---\npermissionMode: plan\nsteps: 3\nmodel: inherit\n---\n",
            "planner",
            AgentLayer::Claude,
            ".claude/agents/planner.md",
        );
        assert!(plan.read_only);
        assert_eq!(plan.max_steps, Some(3));
        assert_eq!(plan.model, None);
        assert_eq!(plan.description, "");
    }

    #[test]
    fn the_limits_count_characters_and_the_name_is_checked() {
        let long_body = "é".repeat(AGENT_BODY_LIMIT + 1);
        let def = parse_agent(
            &format!("---\ndescription: {}\n---\n{long_body}\n", "d".repeat(1025)),
            "Bad_Name",
            AgentLayer::Repo,
            ".pulse/agents/Bad_Name.md",
        );
        assert!(def.loads());
        assert_eq!(def.name, "bad_name");
        let errors = def.errors.join("\n");
        assert!(errors.contains("name `bad_name` is not"), "{errors}");
        assert!(
            errors.contains(&format!("body is {} characters", AGENT_BODY_LIMIT + 1)),
            "{errors}"
        );
        assert!(
            errors.contains("description is 1025 characters"),
            "{errors}"
        );
        assert!(def.body.ends_with("[... cut at 30000 characters]"));
        assert_eq!(
            def.body.chars().filter(|c| *c == 'é').count(),
            AGENT_BODY_LIMIT
        );
        assert_eq!(def.description.chars().count(), DESCRIPTION_LIMIT);
        let fine = parse_agent("Just a body.", "fine-1", AgentLayer::Home, "x");
        assert!(fine.errors.is_empty() && fine.warnings.is_empty());
        assert_eq!(fine.description, "Just a body.");
        let unclosed = parse_agent("---\nname: x\nbody without end", "u", AgentLayer::Home, "x");
        assert!(unclosed.errors.iter().any(|e| e.contains("never closes")));
        assert!(valid_name("a") && valid_name("rust-refactorer-2"));
        assert!(!valid_name("-a") && !valid_name("") && !valid_name(&"a".repeat(65)));
    }

    #[test]
    fn block_scalars_fold_and_the_schema_lists_every_key() {
        let folded = fold_block_scalars(
            "description: |\n  line one\n  line two\n\n  line four\ntools: Read\nnote: >-\n  a\n  b\n",
        );
        assert_eq!(
            folded,
            "description: \"line one\\nline two\\n\\nline four\"\ntools: Read\nnote: \"a b\""
        );
        let schema = agent_schema();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(schema["$id"].as_str().unwrap().ends_with(".json"));
        let props = schema["properties"].as_object().unwrap();
        for k in FRONT_KEYS {
            assert!(props.contains_key(k.key), "{}", k.key);
            for alias in k.aliases {
                assert!(props.contains_key(*alias), "{alias}");
            }
        }
        let pulse = props["pulse"]["properties"].as_object().unwrap();
        for k in PULSE_KEYS {
            assert!(pulse.contains_key(k.key), "pulse.{}", k.key);
        }
        assert_eq!(pulse["select"]["properties"]["tags"]["type"], "array");
        assert_eq!(pulse["schedule"]["properties"]["every"]["type"], "string");
        assert_eq!(props["tools"]["type"], json!(["string", "array"]));
        assert_eq!(props["hooks"]["x-pulse"], "guarded");
        assert_eq!(pulse["executor"]["x-pulse"], "forbidden");
    }

    #[test]
    fn a_store_record_wins_over_the_owner_file_which_only_narrows() {
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("PULSE_JOB", "");
        }
        let store_home = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: store_home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        let put = |project: &str, slug: &str, status: &str, body: &str| {
            s.put_record(
                pulse_store::Record {
                    id: format!("AGENT-{slug}-{project}"),
                    project: project.into(),
                    kind: "agent".into(),
                    slug: slug.into(),
                    title: slug.into(),
                    status: status.into(),
                    origin: "store".into(),
                    ..Default::default()
                },
                [("model".to_string(), Value::String("record-model".into()))]
                    .into_iter()
                    .collect(),
                body,
                pulse_store::HUMAN_AGENT,
                "",
            )
            .unwrap();
        };
        put(
            "commons",
            "x",
            "live",
            "---\ndescription: Commons x.\ntools: [read, grep, edit, bash]\n---\nCommons body.\n",
        );
        put(
            "demo",
            "x",
            "live",
            "---\ndescription: Demo x.\ntools: [read, grep, edit]\npulse:\n  kinds: [task]\n---\nDemo body.\n",
        );
        put("demo", "gone", "retired", "Retired body.\n");
        put("demo", "plain", "live", "A record without front matter.\n");
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "agents/x.md",
            "---\ndescription: Owner's x.\ntools: [read, grep]\nmodel: kimi-k3\n---\nOwner body.\n",
        );
        write(dir.path(), ".pulse/agents/y.md", "Repo y.\n");
        let agents = load_agent_specs(
            dir.path().to_str().unwrap(),
            home.path().to_str().unwrap(),
            Some(&s),
            "demo",
        );
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["x", "plain", "y"]);
        let x = &agents[0];
        assert_eq!(x.layer, AgentLayer::Store);
        assert_eq!(x.source, "store:AGENT-x-demo");
        assert_eq!(x.description, "Demo x.");
        assert_eq!(x.body, "Demo body.");
        assert_eq!(x.kinds, ["task"]);
        assert_eq!(x.model.as_deref(), Some("record-model"));
        assert_eq!(x.tools.as_deref(), Some(&strs(&["read", "grep"])[..]));
        assert!(
            x.warnings
                .iter()
                .any(|w| w.contains("owner file `~/.pulse/agents/x.md`") && w.contains("model")),
            "{:?}",
            x.warnings
        );
        let plain = &agents[1];
        assert_eq!(plain.description, "A record without front matter.");
        assert_eq!(plain.model.as_deref(), Some("record-model"));
        let commons_only = load_agent_specs("", "", Some(&s), "other");
        assert_eq!(commons_only.len(), 1);
        assert_eq!(commons_only[0].source, "store:AGENT-x-commons");
    }

    #[test]
    fn model_allowed_reads_the_list() {
        assert!(model_allowed("", "anything"));
        assert!(model_allowed("kimi-k3, claude-*", "kimi-k3"));
        assert!(model_allowed("kimi-k3\nclaude-*", "claude-sonnet-4-5"));
        assert!(!model_allowed("kimi-k3, claude-*", "gpt-5"));
    }
}
