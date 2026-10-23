//! Custom agents: Markdown files with front matter that describe a helper
//! the model can delegate to through the `subagent` tool. The layouts are
//! the ones Claude Code (`.claude/agents/<name>.md`) and Copilot
//! (`.github/agents/<name>.agent.md`) use, so a repository's existing
//! agents work as they are; `.pulse/agents` and the owner's
//! `~/.pulse/agents` come first.
//!
//! A file's `description` tells the model when to pick the agent, its
//! body becomes the helper's standing instructions, and `tools` (or
//! `disallowedTools`) narrows the tools the helper gets. Tool names come
//! in every harness's spelling (`Read`, `view`, `shell`, `WebFetch`,
//! `Task`); unknown names are kept for MCP tools.

use crate::instructions::front_list;
use crate::skills::{front_value, split_front_matter};
use std::fs;
use std::path::{Path, PathBuf};

/// Where a clone's agents live, relative to its root, in search order.
pub const AGENT_DIRS: &[&str] = &[".pulse/agents", ".claude/agents", ".github/agents"];

/// Bytes of an agent's instructions the helper gets before they are cut.
pub const AGENT_BODY_LIMIT: usize = 16 * 1024;

/// Agents named in the `subagent` tool description; the rest are counted.
pub const AGENT_LIST_LIMIT: usize = 40;

const DESCRIPTION_LIMIT: usize = 240;

/// One custom agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// The tools the helper may use, as PULSE names or globs
    /// (`mcp__github__*`); None gives it every tool of the parent.
    pub tools: Option<Vec<String>>,
    /// Tools the helper may not use, in PULSE names or globs.
    pub denied: Vec<String>,
    /// The standing instructions: the file's body.
    pub body: String,
    /// The file, relative to the clone root or `~/.pulse`.
    pub source: String,
}

/// Reads the agents of the clone at `cwd` (every [`AGENT_DIRS`] entry)
/// and the owner's own under `<home>/agents`. The first agent of a name
/// wins, so a repository's agent shadows the owner's.
pub fn load_agents(cwd: &str, home: &str) -> Vec<AgentDef> {
    let root = if cwd.is_empty() {
        Path::new(".")
    } else {
        Path::new(cwd)
    };
    let mut roots: Vec<(PathBuf, String)> = AGENT_DIRS
        .iter()
        .map(|d| (root.join(d), (*d).to_string()))
        .collect();
    if !home.is_empty() {
        roots.push((
            Path::new(home).join("agents"),
            "~/.pulse/agents".to_string(),
        ));
    }
    let mut out: Vec<AgentDef> = Vec::new();
    for (dir, label) in roots {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        for file in files {
            let Some(agent) = read_agent(&file, &label) else {
                continue;
            };
            if out.iter().any(|a| a.name.eq_ignore_ascii_case(&agent.name)) {
                continue;
            }
            out.push(agent);
        }
    }
    out
}

fn read_agent(file: &Path, label: &str) -> Option<AgentDef> {
    let raw = fs::read_to_string(file).ok()?;
    let (front, body) = split_front_matter(&raw);
    let stem = file.file_stem()?.to_string_lossy().to_string();
    let stem = stem.strip_suffix(".agent").unwrap_or(&stem).to_string();
    let mut name = front_value(front, "name").unwrap_or_default();
    if name.is_empty() {
        name = stem;
    }
    let name: String = name
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    let mut description = front_value(front, "description").unwrap_or_default();
    if description.is_empty() {
        description = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("")
            .to_string();
    }
    if description.chars().count() > DESCRIPTION_LIMIT {
        description = format!(
            "{}...",
            description
                .chars()
                .take(DESCRIPTION_LIMIT - 3)
                .collect::<String>()
                .trim_end()
        );
    }
    let tools = tool_list(
        front,
        &["tools", "allowed-tools", "allowedTools", "allowed_tools"],
    );
    let tools = match tools {
        Some(list) if list.iter().any(|t| t == "*") => None,
        other => other,
    };
    let denied = tool_list(
        front,
        &[
            "disallowedTools",
            "disallowed-tools",
            "disallowed_tools",
            "deny",
        ],
    )
    .unwrap_or_default();
    let mut body = body.trim().to_string();
    if body.len() > AGENT_BODY_LIMIT {
        let cut = body.floor_char_boundary(AGENT_BODY_LIMIT);
        body = format!(
            "{}\n[... cut at {AGENT_BODY_LIMIT} bytes]",
            body[..cut].trim_end()
        );
    }
    let source = format!("{label}/{}", file.file_name()?.to_string_lossy());
    Some(AgentDef {
        name,
        description,
        tools,
        denied,
        body,
        source,
    })
}

/// The first of `keys` present in the front matter, as PULSE tool names
/// (deduplicated, in order); None when no key is there.
fn tool_list(front: &str, keys: &[&str]) -> Option<Vec<String>> {
    let key = keys.iter().find(|k| front_has(front, k))?;
    let mut out: Vec<String> = Vec::new();
    for item in front_list(front, key) {
        for name in tool_names(&item) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    Some(out)
}

fn front_has(front: &str, key: &str) -> bool {
    front.lines().any(|l| {
        l.split_once(':')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
    })
}

/// The PULSE tool names a tool name of any harness stands for. Claude's
/// `Bash(git *)` style keeps its filter out: the agent's rules speak for
/// tools whole.
pub fn tool_names(given: &str) -> Vec<String> {
    let name = given.trim();
    let name = name.split_once('(').map_or(name, |(n, _)| n).trim();
    if name.is_empty() {
        return Vec::new();
    }
    let lower = name.to_ascii_lowercase();
    let one = |s: &str| vec![s.to_string()];
    match lower.as_str() {
        "*" | "all" => one("*"),
        "read" | "read_file" | "readfile" | "view" | "cat" | "notebookread" => one("read"),
        "edit" | "multiedit" | "str_replace" | "str_replace_editor" | "edit_file"
        | "notebookedit" | "apply_patch" => one("edit"),
        "write" | "create" | "write_file" | "create_file" | "writefile" => one("write"),
        "bash" | "shell" | "execute" | "run" | "terminal" | "powershell" | "run_in_terminal"
        | "execute_command" => one("bash"),
        "grep" | "rg" | "search_text" | "grep_search" => one("grep"),
        "glob" | "find" | "ls" | "list" | "file_search" | "list_dir" => one("glob"),
        "search" | "codebase" => vec!["grep".to_string(), "glob".to_string()],
        "webfetch" | "fetch" | "web_fetch" | "fetch_webpage" => one("fetch"),
        "websearch" | "web_search" => one("web_search"),
        "web" => vec!["fetch".to_string(), "web_search".to_string()],
        "task" | "subagent" | "agent" | "delegate" | "runsubagent" => one("subagent"),
        "skill" | "skills" => one("skill"),
        "todowrite" | "todo" | "todos" | "plan" | "manage_todo_list" => one("plan"),
        "askuserquestion" | "ask_user" | "question" | "ask" => one("question"),
        "memory" | "remember" => one("memory"),
        "process" | "bashoutput" | "killshell" | "killbash" | "background" => one("process"),
        _ => one(name),
    }
}

/// The tools of `offered` a helper of `agent` loses: everything outside
/// its `tools` list and everything `disallowedTools` names, as glob
/// patterns a deny rule takes.
pub fn withheld_tools(agent: &AgentDef, offered: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in offered {
        let kept = agent
            .tools
            .as_ref()
            .is_none_or(|list| list.iter().any(|p| glob(p, name)));
        let denied = agent.denied.iter().any(|p| glob(p, name));
        if (!kept || denied) && !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// Whether `name` matches the tool glob `pattern` (`*` any run, `?` one
/// character; case-insensitive).
fn glob(pattern: &str, name: &str) -> bool {
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
                a.name.clone()
            } else {
                format!("{}: {}", a.name, a.description)
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn agents_come_from_every_layout_in_order_and_the_first_name_wins() {
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
        assert_eq!(names, ["Test-Runner", "reviewer", "docs", "owner"]);
        let reviewer = &agents[1];
        assert_eq!(reviewer.source, ".claude/agents/reviewer.md");
        assert_eq!(reviewer.description, "Reviews a diff for bugs.");
        assert_eq!(
            reviewer.tools.as_deref(),
            Some(
                &[
                    "read".to_string(),
                    "grep".into(),
                    "glob".into(),
                    "bash".into()
                ][..]
            )
        );
        assert_eq!(reviewer.body, "You review code. Report findings only.");
        let docs = &agents[2];
        assert_eq!(docs.source, ".github/agents/docs.agent.md");
        assert_eq!(
            docs.tools.as_deref(),
            Some(
                &[
                    "read".to_string(),
                    "edit".into(),
                    "grep".into(),
                    "glob".into()
                ][..]
            )
        );
        let tester = &agents[0];
        assert_eq!(tester.tools, None);
        assert_eq!(tester.denied, ["edit", "write"]);
        let owner = &agents[3];
        assert_eq!(owner.source, "~/.pulse/agents/owner.md");
        assert_eq!(
            owner.description,
            "Owner's helper with the first line as description."
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
        let offered: Vec<String> = [
            "bash",
            "read",
            "grep",
            "glob",
            "edit",
            "write",
            "plan",
            "mcp__github__issues",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let agent = AgentDef {
            name: "r".into(),
            description: String::new(),
            tools: Some(vec!["read".into(), "grep".into(), "mcp__github__*".into()]),
            denied: vec!["grep".into()],
            body: String::new(),
            source: String::new(),
        };
        assert_eq!(
            withheld_tools(&agent, &offered),
            ["bash", "grep", "glob", "edit", "write", "plan"]
        );
        let open = AgentDef {
            tools: None,
            denied: Vec::new(),
            ..agent
        };
        assert!(withheld_tools(&open, &offered).is_empty());
        let text = describe_for_tool(&[open]);
        assert!(text.contains("Custom agents") && text.contains("r"));
        assert_eq!(describe_for_tool(&[]), "");
    }
}
