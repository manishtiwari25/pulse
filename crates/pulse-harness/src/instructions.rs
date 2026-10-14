//! Instruction files in the layouts Claude Code, Copilot CLI and the
//! agents.md convention share, so a repository's existing guidance works
//! as it is. The owner's `~/.pulse/AGENTS.md` and the clone's root files
//! go into the system prompt; a rule that names paths (`applyTo:` in
//! `.github/instructions/*.instructions.md`, `paths:` in
//! `.claude/rules/*.md`) and an `AGENTS.md` or `CLAUDE.md` deeper in the
//! tree reach the model when it first touches a file they speak of.

use crate::skills::split_front_matter;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Repository instruction files, in the order they are read from the
/// clone root. Identical contents (CLAUDE.md as a copy of AGENTS.md) are
/// read once.
pub const INSTRUCTION_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    ".github/copilot-instructions.md",
];

/// The owner's standing instructions, under the pulse home; read before
/// the clone's own so a project can refine them.
pub const OWNER_INSTRUCTIONS: &str = "AGENTS.md";

/// Where a clone keeps rules - instruction files that may name the paths
/// they apply to - and what their files are called.
pub const RULE_DIRS: &[(&str, &str)] = &[
    (".pulse/rules", ".md"),
    (".claude/rules", ".md"),
    (".github/instructions", ".instructions.md"),
];

/// The instruction files a directory below the root may hold for what
/// lies under it, in the order they are tried.
pub const NESTED_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Bytes of one instruction file the prompt carries before it is cut.
pub const INSTRUCTION_FILE_LIMIT: usize = 16 * 1024;
/// Bytes of instructions the prompt carries in total.
pub const INSTRUCTIONS_LIMIT: usize = 32 * 1024;

/// A rule that applies to some paths only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scoped {
    /// The file, relative to the clone root.
    pub source: String,
    /// The path globs it names.
    pub globs: Vec<String>,
    /// Its text without the front matter, cut at [`INSTRUCTION_FILE_LIMIT`].
    pub body: String,
}

/// What [`load_instructions`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instructions {
    /// The sections that go into the system prompt.
    pub text: String,
    /// The files `text` came from, as the owner sees them.
    pub files: Vec<String>,
    /// The rules kept for the paths they name.
    pub scoped: Vec<Scoped>,
    /// Every text already in `text`, so a copy met later is not repeated.
    seen: Vec<String>,
}

impl Instructions {
    /// The path rules of this clone: the scoped rules plus whatever
    /// `AGENTS.md`/`CLAUDE.md` turn up below the root as files are touched.
    pub fn path_rules(&self, cwd: &str) -> PathRules {
        PathRules {
            root: root_of(cwd),
            scoped: self.scoped.clone(),
            given: BTreeSet::new(),
            dirs: BTreeSet::new(),
            seen: self.seen.clone(),
        }
    }
}

fn root_of(cwd: &str) -> PathBuf {
    if cwd.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(cwd)
    }
}

/// Reads the owner's `<home>/AGENTS.md`, the clone's [`INSTRUCTION_FILES`]
/// and the rules under [`RULE_DIRS`]: each text once (a copy of another is
/// skipped), cut at [`INSTRUCTION_FILE_LIMIT`] with a marker,
/// [`INSTRUCTIONS_LIMIT`] in all. A rule that names paths is kept aside
/// for them instead of going into the prompt.
pub fn load_instructions(cwd: &str, home: &str) -> Instructions {
    let root = root_of(cwd);
    let mut out = Instructions::default();
    if !home.is_empty() {
        let path = Path::new(home).join(OWNER_INSTRUCTIONS);
        add_file(&mut out, &path, "~/.pulse/AGENTS.md");
    }
    for name in INSTRUCTION_FILES {
        add_file(&mut out, &root.join(name), name);
    }
    for (dir, suffix) in RULE_DIRS {
        let mut files = Vec::new();
        collect_rule_files(&root.join(dir), suffix, &mut files);
        files.sort();
        for path in files {
            let name = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let (front, body) = split_front_matter(&raw);
            let globs = scope_of(front);
            if globs.is_empty() {
                add_text(&mut out, &name, body);
                continue;
            }
            let body = body.trim();
            if body.is_empty() || out.seen.iter().any(|s| s == body) {
                continue;
            }
            out.seen.push(body.to_string());
            out.scoped.push(Scoped {
                source: name.clone(),
                globs,
                body: cut(body, &name),
            });
        }
    }
    out
}

/// The path globs a rule's front matter names: Copilot's `applyTo`
/// (comma-separated) or Claude Code's `paths` (a list); none, or one
/// that covers everything, means the rule always applies.
fn scope_of(front: &str) -> Vec<String> {
    let mut globs = front_list(front, "applyTo");
    if globs.is_empty() {
        globs = front_list(front, "paths");
    }
    if globs
        .iter()
        .any(|g| matches!(g.as_str(), "**" | "*" | "**/*"))
    {
        return Vec::new();
    }
    globs
}

/// A front-matter value as a list: `key: a, b`, `key: [a, b]`, `key: "a"`
/// or `key:` followed by `- a` lines.
pub fn front_list(front: &str, key: &str) -> Vec<String> {
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        if v.is_empty() {
            let mut items = Vec::new();
            while let Some(next) = lines.peek() {
                let t = next.trim();
                if (next.starts_with(' ') || next.starts_with('\t') || t.starts_with("- "))
                    && !t.is_empty()
                {
                    if let Some(item) = t.strip_prefix("- ") {
                        items.push(unquote(item));
                    }
                    lines.next();
                } else {
                    break;
                }
            }
            return items.into_iter().filter(|s| !s.is_empty()).collect();
        }
        let v = unquote(v);
        let inner = v
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(&v);
        return inner
            .split(',')
            .map(|s| unquote(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    Vec::new()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
        .to_string()
}

fn collect_rule_files(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rule_files(&path, suffix, out);
        } else if path.to_string_lossy().ends_with(suffix) {
            out.push(path);
        }
    }
}

fn add_file(out: &mut Instructions, path: &Path, name: &str) {
    if let Ok(raw) = fs::read_to_string(path) {
        add_text(out, name, &raw);
    }
}

fn add_text(out: &mut Instructions, name: &str, raw: &str) {
    let text = raw.trim();
    if text.is_empty() || out.seen.iter().any(|s| s == text) {
        return;
    }
    out.seen.push(text.to_string());
    let section = format!("--- {name} ---\n{}\n", cut(text, name));
    if out.text.len() + section.len() > INSTRUCTIONS_LIMIT {
        out.text.push_str(&format!(
            "--- {name} ---\n[omitted: the instructions already fill {} bytes; read the file]\n",
            out.text.len()
        ));
        out.files.push(name.to_string());
        return;
    }
    out.text.push_str(&section);
    out.files.push(name.to_string());
}

fn cut(text: &str, name: &str) -> String {
    if text.len() <= INSTRUCTION_FILE_LIMIT {
        return text.to_string();
    }
    let at = text.floor_char_boundary(INSTRUCTION_FILE_LIMIT);
    format!(
        "{}\n[... {name} continues for {} more bytes; read the file for the rest ...]",
        &text[..at],
        text.len() - at
    )
}

/// An instruction file handed to the model for a path it touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub source: String,
    /// Why: `applies to **/*.ts` or `for everything under src/api/`.
    pub reason: String,
    pub body: String,
}

/// The rules that wait for a path: handed out once each, as the model
/// first reads or changes a file they apply to.
#[derive(Clone, Debug)]
pub struct PathRules {
    root: PathBuf,
    scoped: Vec<Scoped>,
    given: BTreeSet<String>,
    dirs: BTreeSet<String>,
    seen: Vec<String>,
}

impl PathRules {
    pub fn is_empty(&self) -> bool {
        self.scoped.is_empty()
    }

    /// The scoped rules as `/env` lists them: `source (globs)`.
    pub fn describe(&self) -> Vec<String> {
        self.scoped
            .iter()
            .map(|s| format!("{} ({})", s.source, s.globs.join(", ")))
            .collect()
    }

    /// The instructions for `rel` (a file path relative to the root) not
    /// given before: an `AGENTS.md`/`CLAUDE.md` of a directory above it,
    /// outermost first, then the scoped rules whose globs match.
    pub fn attach(&mut self, rel: &str) -> Vec<Attached> {
        let rel = rel.trim_start_matches("./").replace('\\', "/");
        let mut out = Vec::new();
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        for depth in 1..parts.len() {
            let dir = parts[..depth].join("/");
            if !self.dirs.insert(dir.clone()) {
                continue;
            }
            for name in NESTED_FILES {
                let path = self.root.join(&dir).join(name);
                let Ok(raw) = fs::read_to_string(&path) else {
                    continue;
                };
                let text = raw.trim();
                if text.is_empty() {
                    continue;
                }
                if !self.seen.iter().any(|s| s == text) {
                    self.seen.push(text.to_string());
                    let source = format!("{dir}/{name}");
                    out.push(Attached {
                        body: cut(text, &source),
                        source,
                        reason: format!("for everything under {dir}/"),
                    });
                }
                break;
            }
        }
        for s in &self.scoped {
            if self.given.contains(&s.source) || !s.globs.iter().any(|g| path_matches(g, &rel)) {
                continue;
            }
            self.given.insert(s.source.clone());
            out.push(Attached {
                source: s.source.clone(),
                reason: format!("applies to {}", s.globs.join(", ")),
                body: s.body.clone(),
            });
        }
        out
    }
}

/// Whether `path` (relative, `/`-separated) is one `pattern` speaks of,
/// the way `.gitignore` and `applyTo` globs are read: `*` and `?` stay
/// within one path segment, `**` spans any number of them, `{a,b}` is
/// either, a pattern without a `/` matches at any depth, and a plain
/// directory name covers everything under it.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let path = path.trim_start_matches("./").trim_start_matches('/');
    expand_braces(pattern.trim())
        .iter()
        .any(|p| one_pattern_matches(p, path))
}

fn one_pattern_matches(pattern: &str, path: &str) -> bool {
    let mut p = pattern.trim_start_matches("./").trim_start_matches('/');
    if p.is_empty() {
        return false;
    }
    let dir_only = p.ends_with('/');
    p = p.trim_end_matches('/');
    let owned;
    if !p.contains('/') && !dir_only {
        owned = format!("**/{p}");
        p = &owned;
    }
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if !dir_only && segments_match(&segs, &parts) {
        return true;
    }
    // A directory the pattern names covers what lies under it.
    let names_a_directory = dir_only || !pattern.contains(['*', '?']);
    names_a_directory && (1..parts.len()).any(|k| segments_match(&segs, &parts[..k]))
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((seg, rest)) => match path.split_first() {
            Some((part, more)) => segment_glob(seg, part) && segments_match(rest, more),
            None => false,
        },
    }
}

fn segment_glob(pattern: &str, text: &str) -> bool {
    fn inner(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => inner(rest, t) || (!t.is_empty() && inner(p, &t[1..])),
            Some(('?', rest)) => !t.is_empty() && inner(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && inner(rest, &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    inner(&p, &t)
}

/// `a/{b,c}/*.{ts,tsx}` as every combination; a pattern without braces
/// is itself.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        return vec![pattern.to_string()];
    };
    let close = open + close_rel;
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    pattern[open + 1..close]
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{head}{}{tail}", alt.trim())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn path_globs_follow_gitignore_and_apply_to_rules() {
        for (pattern, path, want) in [
            ("**/*.ts", "src/a.ts", true),
            ("**/*.ts", "a.ts", true),
            ("**/*.ts", "src/a.tsx", false),
            ("*.ts", "src/deep/a.ts", true),
            ("src/*.ts", "src/a.ts", true),
            ("src/*.ts", "src/deep/a.ts", false),
            ("src/**/*.ts", "src/deep/er/a.ts", true),
            ("src/**/*.ts", "src/a.ts", true),
            ("src/**", "src/a.ts", true),
            ("src/**", "lib/a.ts", false),
            ("src", "src/a.ts", true),
            ("src/", "src/a.ts", true),
            ("src/", "src", false),
            ("src/api", "src/api/x/y.rs", true),
            ("src/api", "src/apiary/y.rs", false),
            ("**/*.{ts,tsx}", "web/app.tsx", true),
            ("docs/*.md", "docs/readme.md", true),
            ("docs/*.md", "README.md", false),
            ("?.rs", "src/a.rs", true),
            ("?.rs", "src/ab.rs", false),
            ("./src/*.rs", "./src/a.rs", true),
            ("", "a", false),
        ] {
            assert_eq!(path_matches(pattern, path), want, "{pattern} vs {path}");
        }
        assert_eq!(
            expand_braces("a/{b,c}/*.{ts,tsx}"),
            ["a/b/*.ts", "a/b/*.tsx", "a/c/*.ts", "a/c/*.tsx"]
        );
    }

    #[test]
    fn front_lists_come_in_every_spelling() {
        assert_eq!(
            front_list("applyTo: \"**/*.ts, **/*.tsx\"\n", "applyTo"),
            ["**/*.ts", "**/*.tsx"]
        );
        assert_eq!(front_list("applyTo: '**'\n", "applyTo"), ["**"]);
        assert_eq!(
            front_list("paths: [\"src/**\", 'lib/**']\n", "paths"),
            ["src/**", "lib/**"]
        );
        assert_eq!(
            front_list(
                "name: x\npaths:\n  - \"src/**/*.rs\"\n  - tests/**\ndescription: y\n",
                "paths"
            ),
            ["src/**/*.rs", "tests/**"]
        );
        assert!(front_list("paths:\nname: x\n", "paths").is_empty());
        assert!(front_list("other: a\n", "paths").is_empty());
    }

    #[test]
    fn owner_root_and_always_rules_go_into_the_prompt_and_scoped_rules_wait() {
        let home = tempfile::tempdir().unwrap();
        let clone = tempfile::tempdir().unwrap();
        write(home.path(), "AGENTS.md", "Owner: be brief.\n");
        write(clone.path(), "AGENTS.md", "# Project\nRun cargo test.\n");
        write(clone.path(), "CLAUDE.md", "# Project\nRun cargo test.\n");
        write(
            clone.path(),
            ".claude/rules/style.md",
            "Four-space indents everywhere.\n",
        );
        write(
            clone.path(),
            ".claude/rules/api/handlers.md",
            "---\npaths:\n  - \"src/api/**\"\n---\nEvery handler returns Result.\n",
        );
        write(
            clone.path(),
            ".github/instructions/ts.instructions.md",
            "---\napplyTo: \"**/*.ts,**/*.tsx\"\n---\nUse strict TypeScript.\n",
        );
        write(
            clone.path(),
            ".github/instructions/all.instructions.md",
            "---\napplyTo: \"**\"\n---\nAlways: no secrets in logs.\n",
        );
        write(
            clone.path(),
            ".github/instructions/notes.md",
            "not an instructions file\n",
        );
        let got = load_instructions(
            clone.path().to_str().unwrap(),
            home.path().to_str().unwrap(),
        );
        assert_eq!(
            got.files,
            [
                "~/.pulse/AGENTS.md",
                "AGENTS.md",
                ".claude/rules/style.md",
                ".github/instructions/all.instructions.md",
            ]
        );
        assert_eq!(
            got.text,
            "--- ~/.pulse/AGENTS.md ---\nOwner: be brief.\n--- AGENTS.md ---\n# Project\nRun cargo test.\n--- .claude/rules/style.md ---\nFour-space indents everywhere.\n--- .github/instructions/all.instructions.md ---\nAlways: no secrets in logs.\n"
        );
        assert_eq!(
            got.scoped
                .iter()
                .map(|s| (s.source.as_str(), s.globs.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    ".claude/rules/api/handlers.md",
                    vec!["src/api/**".to_string()]
                ),
                (
                    ".github/instructions/ts.instructions.md",
                    vec!["**/*.ts".to_string(), "**/*.tsx".to_string()]
                ),
            ]
        );

        // Rules are handed out once, for the first matching path.
        let mut rules = got.path_rules(clone.path().to_str().unwrap());
        assert_eq!(
            rules.describe(),
            [
                ".claude/rules/api/handlers.md (src/api/**)",
                ".github/instructions/ts.instructions.md (**/*.ts, **/*.tsx)"
            ]
        );
        assert!(rules.attach("README.md").is_empty());
        let first = rules.attach("src/api/users.ts");
        assert_eq!(
            first
                .iter()
                .map(|a| (a.source.as_str(), a.reason.as_str(), a.body.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    ".claude/rules/api/handlers.md",
                    "applies to src/api/**",
                    "Every handler returns Result."
                ),
                (
                    ".github/instructions/ts.instructions.md",
                    "applies to **/*.ts, **/*.tsx",
                    "Use strict TypeScript."
                ),
            ]
        );
        assert!(rules.attach("src/api/other.ts").is_empty());
        assert!(
            rules.attach("web/app.tsx").is_empty(),
            "the ts rule was given already"
        );

        // Without a home there is no owner file; without any file, nothing.
        let got = load_instructions(clone.path().to_str().unwrap(), "");
        assert_eq!(got.files[0], "AGENTS.md");
        let empty = tempfile::tempdir().unwrap();
        let got = load_instructions(
            empty.path().to_str().unwrap(),
            home.path().to_str().unwrap(),
        );
        assert_eq!(got.files, ["~/.pulse/AGENTS.md"]);
        let got = load_instructions(empty.path().to_str().unwrap(), "");
        assert_eq!(got, Instructions::default());
    }

    #[test]
    fn nested_agents_files_come_with_the_first_file_under_them() {
        let clone = tempfile::tempdir().unwrap();
        write(clone.path(), "AGENTS.md", "root rules\n");
        write(clone.path(), "crates/AGENTS.md", "root rules\n");
        write(clone.path(), "crates/web/CLAUDE.md", "web: run npm test\n");
        write(clone.path(), "crates/web/AGENTS.md", "   \n");
        write(clone.path(), "crates/core/AGENTS.md", "core: no unsafe\n");
        let got = load_instructions(clone.path().to_str().unwrap(), "");
        assert_eq!(got.files, ["AGENTS.md"]);
        let mut rules = got.path_rules(clone.path().to_str().unwrap());
        assert!(rules.is_empty());
        assert!(
            rules.attach("Cargo.toml").is_empty(),
            "nothing above a root file"
        );
        let got = rules.attach("crates/web/src/app.ts");
        assert_eq!(
            got.iter()
                .map(|a| (a.source.as_str(), a.reason.as_str(), a.body.as_str()))
                .collect::<Vec<_>>(),
            [(
                "crates/web/CLAUDE.md",
                "for everything under crates/web/",
                "web: run npm test"
            )],
            "a copy of the root file and an empty file are passed over"
        );
        assert!(rules.attach("crates/web/src/other.ts").is_empty());
        let got = rules.attach("./crates/core/lib.rs");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "crates/core/AGENTS.md");
        assert!(rules.attach("crates/core/lib.rs").is_empty());
    }
}
