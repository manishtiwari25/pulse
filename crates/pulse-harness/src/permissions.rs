//! Permission rules: what the agent may do without asking, and what it may
//! not do at all.
//!
//! A rule is `allow` or `deny` plus a pattern in the `kind(filter)` grammar
//! Claude Code and Copilot CLI users know:
//!
//! - `bash(git *)` — shell commands matching the glob (the whole command
//!   line; `*` matches anything, `?` one character);
//! - `edit(src/*)`, `write(*.md)`, `read(.env*)` — native file tools by the
//!   path they touch, relative to the working directory;
//! - `fetch(*.rust-lang.org)` — the fetch tool by host;
//! - `mcp__github__*` — tool names may themselves be globs;
//! - `edit`, `write`, `subagent` — a bare kind covers every call of it.
//!
//! Deny beats allow beats the default: a denied call is refused outright
//! (also under `--yes`), an allowed call runs without approval, and
//! anything else does what it would do anyway - ask when it changes
//! things, run when it only looks.

use std::fmt;

/// Where a rule came from, for `/permissions` and `/env`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `--allow-tool` / `--deny-tool`.
    Flag,
    /// `harness.allow_tools` / `harness.deny_tools`.
    Setting,
    /// Added during the session: `a` at an approval, `/permissions allow`.
    Session,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Flag => "flag",
            Self::Setting => "setting",
            Self::Session => "session",
        })
    }
}

/// One rule: allow or deny calls of `kind` whose subject matches `filter`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub allow: bool,
    /// The tool name, itself a glob (`mcp__github__*`).
    pub kind: String,
    /// The glob the call's subject must match; None covers every call.
    pub filter: Option<String>,
    pub source: Source,
}

impl Rule {
    /// Parses `kind` or `kind(filter)`. The filter may contain parentheses
    /// of its own; it runs to the last `)`.
    pub fn parse(allow: bool, spec: &str, source: Source) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty rule".into());
        }
        let (kind, filter) = match spec.find('(') {
            Some(open) => {
                if !spec.ends_with(')') {
                    return Err(format!("{spec:?}: missing the closing parenthesis"));
                }
                let kind = &spec[..open];
                let filter = &spec[open + 1..spec.len() - 1];
                (kind.trim(), Some(filter.trim()))
            }
            None => (spec, None),
        };
        if kind.is_empty() {
            return Err(format!(
                "{spec:?}: the tool name is missing before the parenthesis"
            ));
        }
        if kind
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '*' | '?')))
        {
            return Err(format!(
                "{spec:?}: a tool name has letters, digits, _ and -, or * and ? wildcards"
            ));
        }
        let filter = match filter {
            Some("") | Some("*") | None => None,
            Some(f) => Some(f.to_string()),
        };
        Ok(Self {
            allow,
            kind: kind.to_string(),
            filter,
            source,
        })
    }

    /// The rule's pattern as written: `kind` or `kind(filter)`.
    pub fn pattern(&self) -> String {
        match &self.filter {
            Some(f) => format!("{}({})", self.kind, f),
            None => self.kind.clone(),
        }
    }

    /// Whether the rule speaks for this call.
    pub fn matches(&self, kind: &str, subject: &str) -> bool {
        if !glob(&self.kind, kind) {
            return false;
        }
        match &self.filter {
            None => true,
            Some(f) => {
                let subject = subject.trim();
                glob(f, subject) || glob(f, subject.trim_start_matches("./"))
            }
        }
    }

    /// Whether the rule covers every call of the tool (no filter).
    pub fn covers(&self, kind: &str) -> bool {
        self.filter.is_none() && glob(&self.kind, kind)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} ({})",
            if self.allow { "allow" } else { "deny" },
            self.pattern(),
            self.source
        )
    }
}

/// The rules in force, in the order they were given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    rules: Vec<Rule>,
}

impl Rules {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a list of specs (one rule each) into rules.
    pub fn parse_all(allow: bool, specs: &[String], source: Source) -> Result<Vec<Rule>, String> {
        specs
            .iter()
            .flat_map(|s| split_specs(s))
            .map(|s| Rule::parse(allow, &s, source))
            .collect()
    }

    /// Adds a rule; one already there (whatever its source) stays as it is.
    pub fn push(&mut self, rule: Rule) {
        let same =
            |r: &Rule| r.allow == rule.allow && r.kind == rule.kind && r.filter == rule.filter;
        if !self.rules.iter().any(same) {
            self.rules.push(rule);
        }
    }

    pub fn extend(&mut self, rules: impl IntoIterator<Item = Rule>) {
        for r in rules {
            self.push(r);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter()
    }

    /// Drops the rules added during the session.
    pub fn clear_session(&mut self) -> usize {
        let before = self.rules.len();
        self.rules.retain(|r| r.source != Source::Session);
        before - self.rules.len()
    }

    /// The deny rule that refuses this call, if any. Deny beats allow.
    pub fn denies(&self, kind: &str, subject: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|r| !r.allow && r.matches(kind, subject))
    }

    /// The deny rule that refuses a call with these subjects (a read of
    /// several files has one per file); a call without subjects is
    /// checked with an empty one.
    pub fn denies_any(&self, kind: &str, subjects: &[String]) -> Option<&Rule> {
        if subjects.is_empty() {
            return self.denies(kind, "");
        }
        subjects.iter().find_map(|s| self.denies(kind, s))
    }

    /// Whether an allow rule lets this call run without approval (and no
    /// deny rule refuses it).
    pub fn allows(&self, kind: &str, subject: &str) -> bool {
        self.denies(kind, subject).is_none()
            && self
                .rules
                .iter()
                .any(|r| r.allow && r.matches(kind, subject))
    }

    /// Whether every subject of the call is allowed.
    pub fn allows_all(&self, kind: &str, subjects: &[String]) -> bool {
        if subjects.is_empty() {
            return self.allows(kind, "");
        }
        subjects.iter().all(|s| self.allows(kind, s))
    }

    /// Whether a deny rule covers every call of the tool, so the tool is
    /// not offered at all.
    pub fn withdraws(&self, kind: &str) -> bool {
        self.rules.iter().any(|r| !r.allow && r.covers(kind))
    }

    /// One line per rule for `/permissions` and `/env`.
    pub fn describe(&self) -> Vec<String> {
        self.rules.iter().map(ToString::to_string).collect()
    }

    /// Counts of allow and deny rules.
    pub fn counts(&self) -> (usize, usize) {
        let allow = self.rules.iter().filter(|r| r.allow).count();
        (allow, self.rules.len() - allow)
    }
}

/// Splits a flag or setting value into rule specs: one per line or comma,
/// commas inside parentheses belonging to the filter.
pub fn split_specs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' | '\n' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The pattern `a` (always) at an approval adds: the command's first word
/// (two for `git`, `cargo`, `npm` and other command families) with `*`,
/// the whole tool for edits and writes, the host for fetch, the tool name
/// for MCP and the rest.
pub fn always_pattern(kind: &str, subject: &str) -> String {
    const FAMILIES: &[&str] = &[
        "git",
        "cargo",
        "npm",
        "npx",
        "pnpm",
        "yarn",
        "bun",
        "go",
        "gh",
        "docker",
        "kubectl",
        "make",
        "pip",
        "pip3",
        "python",
        "python3",
        "uv",
        "poetry",
        "dotnet",
        "mvn",
        "gradle",
        "terraform",
        "aws",
        "az",
        "gcloud",
        "brew",
        "apt",
        "apt-get",
        "systemctl",
        "pulse",
    ];
    match kind {
        "bash" => {
            let words: Vec<&str> = subject.split_whitespace().collect();
            let Some(first) = words.first() else {
                return "bash".to_string();
            };
            let take = if FAMILIES.contains(first) && words.len() > 1 && !words[1].starts_with('-')
            {
                2
            } else {
                1
            };
            let prefix: String = words[..take].join(" ");
            if words.len() > take {
                format!("bash({} *)", escape_glob(&prefix))
            } else {
                format!("bash({})", escape_glob(&prefix))
            }
        }
        "fetch" => {
            if subject.is_empty() {
                "fetch".to_string()
            } else {
                format!("fetch({})", escape_glob(subject))
            }
        }
        _ => kind.to_string(),
    }
}

/// Replaces glob metacharacters in literal text so a pattern built from a
/// command matches only that text.
fn escape_glob(text: &str) -> String {
    text.replace('*', "?").replace(['(', ')'], "?")
}

/// Shell-style glob: `*` matches any run of characters (including spaces
/// and slashes), `?` one character. Case-sensitive; matches the whole
/// text.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(allow: bool, spec: &str) -> Rule {
        Rule::parse(allow, spec, Source::Flag).unwrap()
    }

    #[test]
    fn glob_matches_whole_text_with_star_and_question_mark() {
        assert!(glob("git *", "git push origin main"));
        assert!(!glob("git *", "git"));
        assert!(glob("git*", "git"));
        assert!(glob("*.md", "docs/README.md"));
        assert!(!glob("*.md", "README.mdx"));
        assert!(glob("mcp__github__*", "mcp__github__list_issues"));
        assert!(glob("?at", "cat"));
        assert!(!glob("?at", "at"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }

    #[test]
    fn rules_parse_kind_and_filter() {
        let r = rule(true, "bash(git commit *)");
        assert_eq!(r.kind, "bash");
        assert_eq!(r.filter.as_deref(), Some("git commit *"));
        assert_eq!(r.pattern(), "bash(git commit *)");
        assert_eq!(rule(false, "edit").filter, None);
        assert_eq!(rule(false, "edit(*)").filter, None, "a * filter covers all");
        assert_eq!(
            rule(true, "bash(echo (x))").filter.as_deref(),
            Some("echo (x)")
        );
        assert!(Rule::parse(true, "bash(git", Source::Flag).is_err());
        assert!(Rule::parse(true, "(x)", Source::Flag).is_err());
        assert!(Rule::parse(true, "ba sh", Source::Flag).is_err());
        assert!(Rule::parse(true, "", Source::Flag).is_err());
        assert_eq!(
            rule(false, "write(*.lock)").to_string(),
            "deny write(*.lock) (flag)"
        );
    }

    #[test]
    fn deny_beats_allow_beats_default() {
        let mut rules = Rules::new();
        rules.push(rule(true, "bash(git *)"));
        rules.push(rule(false, "bash(git push *)"));
        rules.push(rule(false, "write"));
        rules.push(rule(true, "mcp__github__*"));
        assert!(rules.allows("bash", "git status"));
        assert!(!rules.allows("bash", "git push origin main"));
        assert_eq!(
            rules
                .denies("bash", "git push origin main")
                .map(|r| r.pattern()),
            Some("bash(git push *)".to_string())
        );
        assert!(!rules.allows("bash", "cargo test"), "no rule: the default");
        assert!(rules.denies("bash", "cargo test").is_none());
        assert!(rules.withdraws("write"));
        assert!(!rules.withdraws("edit"));
        assert!(rules.allows("mcp__github__list_issues", "{}"));
        assert!(rules.denies("write", "notes.md").is_some());
        assert_eq!(rules.counts(), (2, 2));
        let reads = ["src/a.rs".to_string(), "git push origin".to_string()];
        assert!(rules.denies_any("bash", &reads).is_some());
        assert!(rules.denies_any("read", &reads).is_none());
        assert!(rules.allows_all("bash", &["git a".to_string(), "git b".to_string()]));
        assert!(!rules.allows_all("bash", &["git a".to_string(), "ls".to_string()]));
        assert!(rules.allows_all("mcp__github__x", &[]));
        assert!(!rules.allows_all("bash", &[]));
    }

    #[test]
    fn path_filters_ignore_a_leading_dot_slash() {
        let mut rules = Rules::new();
        rules.push(rule(false, "read(.env*)"));
        rules.push(rule(true, "edit(src/*)"));
        assert!(rules.denies("read", ".env.local").is_some());
        assert!(rules.denies("read", "./.env").is_some());
        assert!(rules.denies("read", "src/.envelope").is_none());
        assert!(rules.allows("edit", "./src/main.rs"));
        assert!(!rules.allows("edit", "README.md"));
    }

    #[test]
    fn session_rules_clear_and_duplicates_collapse() {
        let mut rules = Rules::new();
        rules.push(rule(true, "edit"));
        rules.push(Rule::parse(true, "bash(git commit *)", Source::Session).unwrap());
        rules.push(Rule::parse(true, "bash(git commit *)", Source::Session).unwrap());
        assert_eq!(rules.len(), 2);
        assert_eq!(rules.clear_session(), 1);
        assert_eq!(rules.describe(), vec!["allow edit (flag)".to_string()]);
    }

    #[test]
    fn specs_split_on_commas_and_lines_outside_parentheses() {
        assert_eq!(
            split_specs("bash(git *), edit\nwrite(a,b)"),
            vec!["bash(git *)", "edit", "write(a,b)"]
        );
        assert!(split_specs(" , \n").is_empty());
        let parsed = Rules::parse_all(
            true,
            &["bash(git *),edit".to_string(), "fetch(docs.rs)".to_string()],
            Source::Setting,
        )
        .unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[2].source, Source::Setting);
        assert!(Rules::parse_all(true, &["bash(".to_string()], Source::Setting).is_err());
    }

    #[test]
    fn always_patterns_follow_the_command_family() {
        assert_eq!(
            always_pattern("bash", "git commit -m x"),
            "bash(git commit *)"
        );
        assert_eq!(always_pattern("bash", "git -C . status"), "bash(git *)");
        assert_eq!(
            always_pattern("bash", "cargo test --all"),
            "bash(cargo test *)"
        );
        assert_eq!(always_pattern("bash", "rm -rf target"), "bash(rm *)");
        assert_eq!(always_pattern("bash", "make"), "bash(make)");
        assert_eq!(always_pattern("bash", "echo a*b (c)"), "bash(echo *)");
        assert_eq!(always_pattern("bash", "ls*"), "bash(ls?)");
        assert_eq!(always_pattern("bash", ""), "bash");
        assert_eq!(always_pattern("edit", "src/x.rs"), "edit");
        assert_eq!(always_pattern("fetch", "docs.rs"), "fetch(docs.rs)");
        assert_eq!(
            always_pattern("mcp__github__create_issue", "{}"),
            "mcp__github__create_issue"
        );
        let p = always_pattern("bash", "git commit -m x");
        assert!(rule(true, &p).matches("bash", "git commit --amend"));
        assert!(!rule(true, &p).matches("bash", "git push"));
    }
}
