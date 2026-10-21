//! Memory across runs and sessions: a Markdown file of bullets per project
//! and one for the user under `~/.pulse/memory/`, read into the system
//! prompt at the start and extended by the `memory` tool (or `/memory` in
//! a session) as the model learns things the repository's files do not say.
//!
//! The files are plain and the owner may edit them: every line that starts
//! with `- ` is one memory, anything else (a heading, a blank line, a
//! comment) is kept as it is.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pulse_provider::Tool;
use serde_json::{Value, json};

/// The tool's name as the model sees it.
pub const MEMORY_TOOL: &str = "memory";
/// The most one file holds; past it `remember` refuses until something is
/// forgotten.
pub const MEMORY_FILE_LIMIT: usize = 64 * 1024;
/// How much of each file the system prompt carries.
pub const MEMORY_PROMPT_LIMIT: usize = 16 * 1024;
/// The longest single memory: a fact, not an essay.
pub const MEMORY_TEXT_LIMIT: usize = 600;
/// The user's own file, shared by every project.
pub const USER_FILE: &str = "user.md";
/// The directory under the PULSE home.
pub const MEMORY_DIR: &str = "memory";

/// Whom a memory is for: this project, or the user in every project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Project,
    User,
}

impl Scope {
    pub fn parse(s: &str) -> Result<Scope, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "project" | "repo" | "repository" => Ok(Scope::Project),
            "user" | "global" | "me" => Ok(Scope::User),
            other => Err(format!("scope {other:?} is not project or user")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::User => "user",
        }
    }
}

/// The two memory files of one run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    dir: PathBuf,
    key: String,
}

/// What `remember` and `forget` did, for the observation and the step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub scope: Scope,
    pub path: PathBuf,
    pub count: usize,
}

impl Memory {
    /// Memory under `home` for the project `key` (see [`project_key`]).
    pub fn new(home: &str, key: &str) -> Memory {
        Memory {
            dir: Path::new(home).join(MEMORY_DIR),
            key: key.to_string(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// The file of one scope.
    pub fn path(&self, scope: Scope) -> PathBuf {
        match scope {
            Scope::Project => self.dir.join(format!("{}.md", self.key)),
            Scope::User => self.dir.join(USER_FILE),
        }
    }

    /// The file as shown to people: `~/...` where it lies under HOME.
    pub fn shown_path(&self, scope: Scope) -> String {
        tilde(&self.path(scope))
    }

    /// The memories of one scope, oldest first, without the bullet.
    pub fn entries(&self, scope: Scope) -> Vec<String> {
        fs::read_to_string(self.path(scope))
            .map(|text| {
                text.lines()
                    .filter_map(entry_of)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Adds one memory to a scope's file (created with a heading when new).
    /// The same text twice is kept once.
    pub fn remember(&self, scope: Scope, text: &str) -> Result<Change, String> {
        let text = one_line(text);
        if text.is_empty() {
            return Err("memory: nothing to remember; give text".into());
        }
        if text.chars().count() > MEMORY_TEXT_LIMIT {
            return Err(format!(
                "memory: {} characters is too long for one memory; keep it under {MEMORY_TEXT_LIMIT} (a fact, not an essay)",
                text.chars().count()
            ));
        }
        let path = self.path(scope);
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let count = existing.lines().filter_map(entry_of).count();
        if existing
            .lines()
            .filter_map(entry_of)
            .any(|e| strip_date(e) == text)
        {
            return Ok(Change { scope, path, count });
        }
        let line = format!("- [{}] {text}\n", today());
        if existing.len() + line.len() > MEMORY_FILE_LIMIT {
            return Err(format!(
                "memory: {} is full ({} bytes); forget what no longer holds before adding more",
                tilde(&path),
                existing.len()
            ));
        }
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("memory: {}: {e}", self.dir.display()))?;
        let mut out = if existing.is_empty() {
            match scope {
                Scope::Project => format!("# Memory for {}\n\n", self.key),
                Scope::User => "# Memory for every project\n\n".to_string(),
            }
        } else if existing.ends_with('\n') {
            existing
        } else {
            format!("{existing}\n")
        };
        out.push_str(&line);
        write_atomically(&path, &out)?;
        Ok(Change {
            scope,
            path,
            count: count + 1,
        })
    }

    /// Removes the memory whose text contains `needle` (case-insensitive)
    /// from the given scope, or from either when none is given. Several
    /// matches are an error that lists them, so nothing goes by accident.
    pub fn forget(&self, scope: Option<Scope>, needle: &str) -> Result<Change, String> {
        let needle = one_line(needle);
        if needle.is_empty() {
            return Err("memory: nothing to forget; give part of the memory's text".into());
        }
        let scopes: Vec<Scope> = match scope {
            Some(s) => vec![s],
            None => vec![Scope::Project, Scope::User],
        };
        let lower = needle.to_lowercase();
        let mut hits: Vec<(Scope, String)> = Vec::new();
        for s in &scopes {
            for e in self.entries(*s) {
                if e.to_lowercase().contains(&lower) {
                    hits.push((*s, e));
                }
            }
        }
        match hits.len() {
            0 => Err(format!(
                "memory: nothing remembered contains {needle:?}{}",
                if scope.is_some() {
                    ""
                } else {
                    "; memory list shows what there is"
                }
            )),
            1 => {
                let (s, entry) = hits.remove(0);
                let path = self.path(s);
                let text = fs::read_to_string(&path).unwrap_or_default();
                let kept: Vec<&str> = text
                    .lines()
                    .filter(|l| entry_of(l) != Some(entry.as_str()))
                    .collect();
                let mut out = kept.join("\n");
                if !out.is_empty() {
                    out.push('\n');
                }
                write_atomically(&path, &out)?;
                Ok(Change {
                    scope: s,
                    path,
                    count: kept.iter().filter_map(|l| entry_of(l)).count(),
                })
            }
            n => Err(format!(
                "memory: {n} memories contain {needle:?}; give more of the one to forget:\n{}",
                hits.iter()
                    .map(|(s, e)| format!("  ({}) {e}", s.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
        }
    }

    /// Empties one scope's file.
    pub fn clear(&self, scope: Scope) -> Result<usize, String> {
        let n = self.entries(scope).len();
        let path = self.path(scope);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("memory: {}: {e}", tilde(&path)))?;
        }
        Ok(n)
    }

    /// Both files for people and the model: path, count and entries.
    pub fn list(&self) -> String {
        let mut out = Vec::new();
        for scope in [Scope::Project, Scope::User] {
            let entries = self.entries(scope);
            out.push(format!(
                "{} memory ({}): {}",
                scope.as_str(),
                self.shown_path(scope),
                match entries.len() {
                    0 => "nothing yet".to_string(),
                    1 => "1 memory".to_string(),
                    n => format!("{n} memories"),
                }
            ));
            out.extend(entries.into_iter().map(|e| format!("  - {e}")));
        }
        out.join("\n")
    }

    /// One line for `/env`.
    pub fn describe(&self) -> String {
        let p = self.entries(Scope::Project).len();
        let u = self.entries(Scope::User).len();
        format!(
            "memory: {p} for the project ({}), {u} for the user ({})",
            self.shown_path(Scope::Project),
            self.shown_path(Scope::User)
        )
    }

    /// What the system prompt carries: each file's entries, cut at
    /// [`MEMORY_PROMPT_LIMIT`] with a marker. Empty when nothing is
    /// remembered. The second value names the files that contributed.
    pub fn prompt_section(&self) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut files = Vec::new();
        for scope in [Scope::Project, Scope::User] {
            let entries = self.entries(scope);
            if entries.is_empty() {
                continue;
            }
            let shown = self.shown_path(scope);
            let mut body = String::new();
            let mut left = 0;
            for e in &entries {
                let line = format!("- {e}\n");
                if body.len() + line.len() > MEMORY_PROMPT_LIMIT {
                    left += 1;
                } else {
                    body.push_str(&line);
                }
            }
            if left > 0 {
                body.push_str(&format!(
                    "[... {left} more in {shown}; memory list shows them all ...]\n"
                ));
            }
            out.push_str(&format!(
                "--- {} memory ({shown}) ---\n{body}",
                scope.as_str()
            ));
            files.push(shown);
        }
        (out, files)
    }
}

/// The system-prompt paragraph that carries the memory.
pub fn memory_paragraph(memory: &str) -> String {
    let text = memory.trim();
    if text.is_empty() {
        return String::new();
    }
    format!(
        "What you remembered in earlier runs and sessions follows. It may be out of date, so check it against the files where it matters; the memory tool adds to it or corrects it.\n{text}\n"
    )
}

/// The `memory` tool.
pub fn memory_tool() -> Tool {
    Tool {
        name: MEMORY_TOOL.to_string(),
        description: format!(
            "Remember something for later runs and sessions in this project: a convention the files do not state, a gotcha, a decision and its reason, where things live, how to run a check - one short fact per call, under {MEMORY_TEXT_LIMIT} characters. It is read into the prompt of every later run here (scope project) or of every project (scope user). Do not store secrets, or what the repository's files already say. forget removes a memory that no longer holds (give enough of its text to match one); list shows everything remembered."
        ),
        parameters: json!({"type":"object","properties":{
            "action":{"type":"string","enum":["remember","forget","list"],"description":"remember adds text; forget removes the one memory containing text; list shows all."},
            "text":{"type":"string","description":"The fact to remember, or part of the memory to forget."},
            "scope":{"type":"string","enum":["project","user"],"description":"project (default): this repository; user: every project of this user."}
        },"required":["action"]}),
    }
}

/// A parsed `memory` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryAction {
    Remember { scope: Scope, text: String },
    Forget { scope: Option<Scope>, text: String },
    List,
}

impl MemoryAction {
    pub fn parse(arguments: &str) -> Result<MemoryAction, String> {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        if !args.is_object() {
            return Err("arguments must be a JSON object with an action".into());
        }
        let str_of = |key: &str| -> Result<String, String> {
            match args.get(key) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) => Ok(s.trim().to_string()),
                Some(other) => Err(format!(
                    "argument {key} must be a string, not {}",
                    match other {
                        Value::Bool(_) => "a boolean",
                        Value::Number(_) => "a number",
                        Value::Array(_) => "an array",
                        _ => "an object",
                    }
                )),
            }
        };
        let action = str_of("action")?.to_ascii_lowercase();
        let text = str_of("text")?;
        let scope_given = !str_of("scope")?.is_empty();
        let scope = Scope::parse(&str_of("scope")?)?;
        match action.as_str() {
            "remember" | "add" | "store" | "save" => Ok(MemoryAction::Remember { scope, text }),
            "forget" | "remove" | "delete" => Ok(MemoryAction::Forget {
                scope: scope_given.then_some(scope),
                text,
            }),
            "list" | "show" | "" => Ok(MemoryAction::List),
            other => Err(format!(
                "unknown action {other}; use remember, forget or list"
            )),
        }
    }

    /// The short form a step shows: `remember project: text`.
    pub fn spec(&self) -> String {
        match self {
            MemoryAction::Remember { scope, text } => {
                format!("remember {}: {}", scope.as_str(), one_line(text))
            }
            MemoryAction::Forget { scope, text } => format!(
                "forget{}: {}",
                scope
                    .map(|s| format!(" {}", s.as_str()))
                    .unwrap_or_default(),
                one_line(text)
            ),
            MemoryAction::List => "list".to_string(),
        }
    }

    /// What a permission rule matches: the scope of a change, `list` for a
    /// look.
    pub fn subject(&self) -> String {
        match self {
            MemoryAction::Remember { scope, .. } => scope.as_str().to_string(),
            MemoryAction::Forget { scope, .. } => {
                scope.map_or("project".to_string(), |s| s.as_str().to_string())
            }
            MemoryAction::List => "list".to_string(),
        }
    }
}

/// The key a project's memory file is named by: the fleet's project slug
/// when the run has one, else the repository's `origin` as a slug
/// (`github.com-owner-repo`), else the directory's path as one. A chat in
/// the owner's checkout and the jobs on the project's clones share a key
/// when the project is registered (the caller passes its slug).
pub fn project_key(cwd: &str, project: &str) -> String {
    let project = project.trim();
    if !project.is_empty() {
        return slug(project);
    }
    if let Some(remote) = origin_url(cwd) {
        return slug(&normalize_remote(&remote));
    }
    let abs = fs::canonicalize(cwd)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    slug(abs.trim_start_matches('/'))
}

fn origin_url(cwd: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", cwd, "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// `git@github.com:owner/repo.git`, `https://github.com/owner/repo.git`
/// and `ssh://git@github.com/owner/repo` all become `github.com/owner/repo`.
pub fn normalize_remote(url: &str) -> String {
    let mut s = url.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    if let Some(i) = s.find('@')
        && !s[..i].contains('/')
    {
        s = &s[i + 1..];
    }
    let s = s.replacen(':', "/", 1);
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    s.to_lowercase()
}

/// A file-name-safe form: lowercase, `[a-z0-9._-]`, runs of anything else
/// as one dash, at most 100 characters.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    let out: String = out.chars().take(100).collect();
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

fn entry_of(line: &str) -> Option<&str> {
    let t = line.trim_end();
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .map(str::trim)
        .filter(|e| !e.is_empty())
}

/// The text of an entry without the `[YYYY-MM-DD] ` the tool put first.
fn strip_date(entry: &str) -> &str {
    if entry.len() > 12 && entry.starts_with('[') && entry.as_bytes()[11] == b']' {
        entry[12..].trim_start()
    } else {
        entry
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn tilde(path: &Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && shown.starts_with(&home) => {
            format!("~{}", &shown[home.len()..])
        }
        _ => shown,
    }
}

fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("md.{}.tmp", std::process::id()));
    fs::write(&tmp, text).map_err(|e| format!("memory: {}: {e}", tilde(path)))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("memory: {}: {e}", tilde(path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> (tempfile::TempDir, Memory) {
        let dir = tempfile::tempdir().unwrap();
        let m = Memory::new(dir.path().to_str().unwrap(), "github.com-acme-app");
        (dir, m)
    }

    #[test]
    fn remember_appends_dated_bullets_once_each_and_forget_takes_one_out() {
        let (_dir, m) = memory();
        let c = m
            .remember(Scope::Project, "  tests need\n  cargo test -p app  ")
            .unwrap();
        assert_eq!(c.count, 1);
        assert_eq!(c.path, m.path(Scope::Project));
        let again = m
            .remember(Scope::Project, "tests need cargo test -p app")
            .unwrap();
        assert_eq!(again.count, 1, "the same text is kept once");
        m.remember(Scope::Project, "the API lives in crates/api")
            .unwrap();
        m.remember(Scope::User, "prefers the short reply").unwrap();
        let text = fs::read_to_string(m.path(Scope::Project)).unwrap();
        assert!(
            text.starts_with("# Memory for github.com-acme-app\n\n- ["),
            "{text}"
        );
        assert_eq!(text.matches("\n- [").count(), 2, "{text}");
        let entries = m.entries(Scope::Project);
        assert_eq!(entries.len(), 2);
        assert!(
            entries[0].ends_with("] tests need cargo test -p app"),
            "{entries:?}"
        );

        let err = m.forget(None, "the").unwrap_err();
        assert!(err.contains("2 memories contain \"the\""), "{err}");
        assert!(
            err.contains("(project) ") && err.contains("(user) "),
            "{err}"
        );
        let c = m.forget(None, "API lives").unwrap();
        assert_eq!((c.scope, c.count), (Scope::Project, 1));
        assert_eq!(m.entries(Scope::Project).len(), 1);
        let err = m.forget(Some(Scope::User), "API").unwrap_err();
        assert!(err.contains("nothing remembered contains \"API\""), "{err}");
        assert_eq!(m.clear(Scope::User).unwrap(), 1);
        assert!(m.entries(Scope::User).is_empty());
        assert!(!m.path(Scope::User).exists());
    }

    #[test]
    fn hand_edited_files_keep_their_other_lines() {
        let (_dir, m) = memory();
        fs::create_dir_all(m.path(Scope::Project).parent().unwrap()).unwrap();
        fs::write(
            m.path(Scope::Project),
            "# Notes\n\nSome prose.\n- hand written one\n* starred two\n\n## More\n- three",
        )
        .unwrap();
        assert_eq!(
            m.entries(Scope::Project),
            ["hand written one", "starred two", "three"]
        );
        m.remember(Scope::Project, "four").unwrap();
        m.forget(Some(Scope::Project), "two").unwrap();
        let text = fs::read_to_string(m.path(Scope::Project)).unwrap();
        assert!(
            text.starts_with("# Notes\n\nSome prose.\n- hand written one\n\n## More\n- three\n- ["),
            "{text}"
        );
        assert!(text.ends_with("] four\n"), "{text}");
    }

    #[test]
    fn limits_hold_for_one_memory_and_the_file() {
        let (_dir, m) = memory();
        let long = "x".repeat(MEMORY_TEXT_LIMIT + 1);
        let err = m.remember(Scope::Project, &long).unwrap_err();
        assert!(err.contains("too long for one memory"), "{err}");
        assert!(
            m.remember(Scope::Project, "")
                .unwrap_err()
                .contains("nothing to remember")
        );
        fs::create_dir_all(&m.dir).unwrap();
        fs::write(
            m.path(Scope::User),
            format!("# Memory\n{}", "- a\n".repeat(MEMORY_FILE_LIMIT / 4)),
        )
        .unwrap();
        let err = m.remember(Scope::User, "one more").unwrap_err();
        assert!(err.contains("is full"), "{err}");
    }

    #[test]
    fn the_prompt_section_carries_both_files_cut_with_a_marker() {
        let (_dir, m) = memory();
        assert_eq!(m.prompt_section(), (String::new(), Vec::new()));
        assert!(memory_paragraph("").is_empty());
        m.remember(Scope::User, "likes tabs").unwrap();
        for i in 0..200 {
            m.remember(Scope::Project, &format!("fact {i} {}", "y".repeat(100)))
                .unwrap();
        }
        let (text, files) = m.prompt_section();
        assert_eq!(files.len(), 2);
        assert!(text.starts_with("--- project memory ("), "{text}");
        assert!(text.contains("] fact 0 yyyy"), "{text}");
        assert!(text.contains("more in "), "{text}");
        assert!(text.contains("--- user memory ("), "{text}");
        assert!(text.contains("] likes tabs\n"), "{text}");
        let para = memory_paragraph(&text);
        assert!(
            para.starts_with("What you remembered in earlier runs"),
            "{para}"
        );
        let list = m.list();
        assert!(
            list.contains("project memory (") && list.contains("200 memories"),
            "{list}"
        );
        assert!(
            list.contains("user memory (") && list.contains("1 memory\n  - ["),
            "{list}"
        );
        let line = m.describe();
        assert!(line.starts_with("memory: 200 for the project ("), "{line}");
        assert!(line.contains(", 1 for the user ("), "{line}");
    }

    #[test]
    fn actions_parse_with_scopes_and_describe_themselves() {
        let a = MemoryAction::parse(r#"{"action":"remember","text":" use  rg ","scope":"user"}"#)
            .unwrap();
        assert_eq!(
            a,
            MemoryAction::Remember {
                scope: Scope::User,
                text: "use  rg".into()
            }
        );
        assert_eq!(a.spec(), "remember user: use rg");
        assert_eq!(a.subject(), "user");
        let f = MemoryAction::parse(r#"{"action":"forget","text":"rg"}"#).unwrap();
        assert_eq!(
            f,
            MemoryAction::Forget {
                scope: None,
                text: "rg".into()
            }
        );
        assert_eq!(f.spec(), "forget: rg");
        assert_eq!(f.subject(), "project");
        let f =
            MemoryAction::parse(r#"{"action":"delete","text":"rg","scope":"project"}"#).unwrap();
        assert_eq!(f.spec(), "forget project: rg");
        assert_eq!(
            MemoryAction::parse(r#"{"action":"list"}"#).unwrap(),
            MemoryAction::List
        );
        assert!(
            MemoryAction::parse(r#"{"action":"dance"}"#)
                .unwrap_err()
                .contains("unknown action dance")
        );
        assert!(
            MemoryAction::parse(r#"{"action":"remember","scope":"team"}"#)
                .unwrap_err()
                .contains("scope \"team\" is not project or user")
        );
        assert!(
            MemoryAction::parse(r#"{"action":"remember","text":7}"#)
                .unwrap_err()
                .contains("text must be a string, not a number")
        );
        assert!(
            MemoryAction::parse("[]")
                .unwrap_err()
                .contains("JSON object")
        );
    }

    #[test]
    fn keys_come_from_the_project_slug_the_remote_or_the_path() {
        assert_eq!(project_key("/nowhere", " My Project "), "my-project");
        assert_eq!(
            normalize_remote("git@github.com:Acme/App.git"),
            "github.com/acme/app"
        );
        assert_eq!(
            normalize_remote("https://user@github.com/acme/app.git/"),
            "github.com/acme/app"
        );
        assert_eq!(
            normalize_remote("ssh://git@github.com/acme/app"),
            "github.com/acme/app"
        );
        assert_eq!(slug("github.com/acme/app"), "github.com-acme-app");
        assert_eq!(slug("/Users/me/src/app"), "users-me-src-app");
        assert_eq!(slug("///"), "project");
        assert_eq!(slug("a".repeat(150).as_str()).len(), 100);
        let dir = tempfile::tempdir().unwrap();
        let plain = project_key(dir.path().to_str().unwrap(), "");
        assert!(
            plain.ends_with(&slug(dir.path().file_name().unwrap().to_str().unwrap())),
            "{plain}"
        );
        let ok = Command::new("git")
            .args(["-C", dir.path().to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success();
        if ok {
            Command::new("git")
                .args([
                    "-C",
                    dir.path().to_str().unwrap(),
                    "remote",
                    "add",
                    "origin",
                    "git@github.com:acme/app.git",
                ])
                .status()
                .unwrap();
            assert_eq!(
                project_key(dir.path().to_str().unwrap(), ""),
                "github.com-acme-app"
            );
        }
    }
}
