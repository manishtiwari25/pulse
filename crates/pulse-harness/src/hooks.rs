//! Repository hooks: shell commands the clone's `.pulse/hooks.json` (and
//! the owner's `~/.pulse/hooks.json`) runs around the model's tool calls
//! and around the run. A `pre_tool` hook that exits 2 blocks the call and
//! its output goes to the model; a `post_tool` hook's output is appended to
//! the tool's result; `run_start` output joins the system prompt; `run_end`
//! output is noted. Hooks are the owner's code, not the model's: the deny
//! rules do not apply to them.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// The hook file of a clone, relative to its root.
pub const HOOK_FILE: &str = ".pulse/hooks.json";

/// The owner's hook file, relative to the pulse home.
pub const HOME_HOOK_FILE: &str = "hooks.json";

/// How long a hook may run when its entry does not say.
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest a hook entry may ask for.
pub const MAX_HOOK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Bytes of a tool's input or output the hook payload carries.
pub const HOOK_PAYLOAD_LIMIT: usize = 8 * 1024;

/// Characters of a hook's output the model gets.
pub const HOOK_OUTPUT_LIMIT: usize = 4000;

/// The exit code of a `pre_tool` hook that blocks the call.
pub const HOOK_BLOCK_EXIT: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    RunStart,
    RunEnd,
    PreTool,
    PostTool,
}

impl fmt::Display for HookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RunStart => "run_start",
            Self::RunEnd => "run_end",
            Self::PreTool => "pre_tool",
            Self::PostTool => "post_tool",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    pub event: HookEvent,
    /// Tool names the hook applies to; empty means every tool. A trailing
    /// `*` matches a prefix (`mcp__*`).
    pub tools: Vec<String>,
    pub command: String,
    pub timeout: Duration,
    /// The file the hook came from, for the transcript.
    pub source: String,
}

impl Hook {
    pub fn matches(&self, tool: &str) -> bool {
        self.tools.is_empty()
            || self.tools.iter().any(|t| match t.strip_suffix('*') {
                Some(prefix) => tool.starts_with(prefix),
                None => t == tool,
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookSet {
    pub hooks: Vec<Hook>,
    /// The hook files read, for the start note.
    pub files: Vec<String>,
    /// Files that did not parse, with the reason; their hooks are skipped.
    pub errors: Vec<String>,
}

impl HookSet {
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn matching(&self, event: HookEvent, tool: &str) -> Vec<&Hook> {
        self.hooks
            .iter()
            .filter(|h| h.event == event && (tool.is_empty() || h.matches(tool)))
            .collect()
    }

    pub fn has(&self, event: HookEvent) -> bool {
        self.hooks.iter().any(|h| h.event == event)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HookFile {
    Wrapped {
        #[serde(default)]
        hooks: Vec<HookEntry>,
    },
    Bare(Vec<HookEntry>),
}

#[derive(Deserialize)]
struct HookEntry {
    event: HookEvent,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    command: String,
    /// Seconds; zero means [`DEFAULT_HOOK_TIMEOUT`].
    #[serde(default)]
    timeout: u64,
}

/// Reads the clone's [`HOOK_FILE`] and the owner's `<home>/hooks.json`;
/// the clone's hooks run first.
pub fn load_hooks(cwd: &str, home: &str) -> HookSet {
    let root = if cwd.is_empty() {
        Path::new(".")
    } else {
        Path::new(cwd)
    };
    let mut sources = vec![(root.join(HOOK_FILE), HOOK_FILE.to_string())];
    if !home.is_empty() {
        sources.push((
            Path::new(home).join(HOME_HOOK_FILE),
            format!("~/.pulse/{HOME_HOOK_FILE}"),
        ));
    }
    let mut set = HookSet::default();
    for (path, label) in sources {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        match parse_hooks(&raw, &label) {
            Ok(hooks) => {
                set.hooks.extend(hooks);
                set.files.push(label);
            }
            Err(e) => set.errors.push(format!("{label}: {e}")),
        }
    }
    set
}

/// Parses one hook file; `source` labels the hooks it yields.
pub fn parse_hooks(raw: &str, source: &str) -> Result<Vec<Hook>, String> {
    let file: HookFile = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let entries = match file {
        HookFile::Wrapped { hooks } | HookFile::Bare(hooks) => hooks,
    };
    let mut out = Vec::new();
    for (i, e) in entries.into_iter().enumerate() {
        if e.command.trim().is_empty() {
            return Err(format!("hook {} ({}) has no command", i + 1, e.event));
        }
        let timeout = if e.timeout == 0 {
            DEFAULT_HOOK_TIMEOUT
        } else {
            Duration::from_secs(e.timeout).min(MAX_HOOK_TIMEOUT)
        };
        out.push(Hook {
            event: e.event,
            tools: e
                .tools
                .into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            command: e.command.trim().to_string(),
            timeout,
            source: source.to_string(),
        });
    }
    Ok(out)
}

/// The payload a tool hook reads on stdin: the tool's parsed input and,
/// after the call, its output and exit code.
pub fn tool_payload(
    event: HookEvent,
    tool: &str,
    arguments: &str,
    result: Option<(&str, i32)>,
    cwd: &str,
) -> Value {
    let input = match serde_json::from_str::<Value>(arguments) {
        Ok(v) if arguments.len() <= HOOK_PAYLOAD_LIMIT => v,
        Ok(_) => json!({"truncated": true, "text": cut_bytes(arguments, HOOK_PAYLOAD_LIMIT)}),
        Err(_) => json!({"text": cut_bytes(arguments, HOOK_PAYLOAD_LIMIT)}),
    };
    let mut payload = json!({
        "event": event.to_string(),
        "tool": tool,
        "input": input,
        "cwd": cwd,
    });
    if let Some((output, exit_code)) = result {
        payload["output"] = Value::String(cut_bytes(output, HOOK_PAYLOAD_LIMIT));
        payload["exit_code"] = json!(exit_code);
    }
    payload
}

/// The payload of a run hook.
pub fn run_payload(event: HookEvent, cwd: &str, fields: Value) -> Value {
    let mut payload = json!({"event": event.to_string(), "cwd": cwd});
    if let (Some(dst), Some(src)) = (payload.as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    payload
}

/// The shell line that runs `hook` with `payload` on its stdin and the
/// event and tool in `PULSE_HOOK_EVENT` and `PULSE_TOOL`.
pub fn hook_command(hook: &Hook, tool: &str, payload: &Value) -> String {
    format!(
        "printf '%s' {} | PULSE_HOOK_EVENT={} PULSE_TOOL={} /bin/sh -c {}",
        shell_quote(&payload.to_string()),
        shell_quote(&hook.event.to_string()),
        shell_quote(tool),
        shell_quote(&hook.command)
    )
}

/// Single-quotes `s` for `/bin/sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `s` cut at `limit` bytes with a note of what was left out.
pub(crate) fn cut_bytes(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let at = s.floor_char_boundary(limit);
    format!("{}[... {} more bytes]", &s[..at], s.len() - at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_file_shapes_and_validates() {
        let wrapped = r#"{"hooks":[{"event":"pre_tool","tools":["bash"," edit "],"command":"check.sh","timeout":30},{"event":"run_start","command":"echo hi"}]}"#;
        let hooks = parse_hooks(wrapped, ".pulse/hooks.json").unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].event, HookEvent::PreTool);
        assert_eq!(hooks[0].tools, ["bash", "edit"]);
        assert_eq!(hooks[0].timeout, Duration::from_secs(30));
        assert_eq!(hooks[1].tools, Vec::<String>::new());
        assert_eq!(hooks[1].timeout, DEFAULT_HOOK_TIMEOUT);
        let bare = r#"[{"event":"post_tool","command":"cargo fmt","timeout":100000}]"#;
        let hooks = parse_hooks(bare, "x").unwrap();
        assert_eq!(hooks[0].timeout, MAX_HOOK_TIMEOUT);
        let err =
            parse_hooks(r#"{"hooks":[{"event":"pre_tool","command":" "}]}"#, "x").unwrap_err();
        assert_eq!(err, "hook 1 (pre_tool) has no command");
        assert!(parse_hooks(r#"{"hooks":[{"event":"sometimes","command":"x"}]}"#, "x").is_err());
        assert!(parse_hooks("not json", "x").is_err());
    }

    #[test]
    fn matching_by_tool_name_and_prefix() {
        let set = HookSet {
            hooks: parse_hooks(
                r#"[{"event":"pre_tool","tools":["bash","mcp__*"],"command":"a"},{"event":"pre_tool","command":"b"},{"event":"post_tool","tools":["edit"],"command":"c"}]"#,
                "x",
            )
            .unwrap(),
            ..HookSet::default()
        };
        let cmds = |event, tool| -> Vec<String> {
            set.matching(event, tool)
                .iter()
                .map(|h| h.command.clone())
                .collect()
        };
        assert_eq!(cmds(HookEvent::PreTool, "bash"), ["a", "b"]);
        assert_eq!(cmds(HookEvent::PreTool, "mcp__srv__tool"), ["a", "b"]);
        assert_eq!(cmds(HookEvent::PreTool, "read"), ["b"]);
        assert_eq!(cmds(HookEvent::PostTool, "edit"), ["c"]);
        assert!(cmds(HookEvent::PostTool, "bash").is_empty());
        assert!(set.has(HookEvent::PreTool));
        assert!(!set.has(HookEvent::RunEnd));
    }

    #[test]
    fn loads_clone_then_home_and_reports_bad_files() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        let home = tmp.path().join("home");
        fs::create_dir_all(clone.join(".pulse")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            clone.join(".pulse/hooks.json"),
            r#"{"hooks":[{"event":"run_end","command":"clone-hook"}]}"#,
        )
        .unwrap();
        fs::write(home.join("hooks.json"), "{broken").unwrap();
        let set = load_hooks(clone.to_str().unwrap(), home.to_str().unwrap());
        assert_eq!(set.hooks.len(), 1);
        assert_eq!(set.hooks[0].source, ".pulse/hooks.json");
        assert_eq!(set.files, [".pulse/hooks.json"]);
        assert_eq!(set.errors.len(), 1);
        assert!(
            set.errors[0].starts_with("~/.pulse/hooks.json: "),
            "{:?}",
            set.errors
        );
        fs::write(
            home.join("hooks.json"),
            r#"[{"event":"run_end","command":"home-hook"}]"#,
        )
        .unwrap();
        let set = load_hooks(clone.to_str().unwrap(), home.to_str().unwrap());
        let cmds: Vec<&str> = set.hooks.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(cmds, ["clone-hook", "home-hook"]);
        assert!(set.errors.is_empty());
        // No files at all: an empty set, no errors.
        let none = load_hooks(tmp.path().join("nowhere").to_str().unwrap(), "");
        assert!(none.is_empty() && none.errors.is_empty());
    }

    #[test]
    fn payload_and_command_quote_everything() {
        let payload = tool_payload(
            HookEvent::PreTool,
            "bash",
            r#"{"command":"echo 'hi'"}"#,
            None,
            "/work",
        );
        assert_eq!(payload["event"], "pre_tool");
        assert_eq!(payload["input"]["command"], "echo 'hi'");
        assert!(payload.get("output").is_none());
        let after = tool_payload(
            HookEvent::PostTool,
            "edit",
            "not json",
            Some(("done\n", 0)),
            "/work",
        );
        assert_eq!(after["input"]["text"], "not json");
        assert_eq!(after["output"], "done\n");
        assert_eq!(after["exit_code"], 0);
        let big = format!(r#"{{"command":"{}"}}"#, "x".repeat(HOOK_PAYLOAD_LIMIT));
        let capped = tool_payload(HookEvent::PreTool, "bash", &big, None, "/work");
        assert_eq!(capped["input"]["truncated"], true);
        let hook = Hook {
            event: HookEvent::PreTool,
            tools: Vec::new(),
            command: "jq -r .input.command | grep -q \"rm -rf\" && exit 2; echo it's fine".into(),
            timeout: DEFAULT_HOOK_TIMEOUT,
            source: "x".into(),
        };
        let line = hook_command(&hook, "bash", &payload);
        assert!(line.starts_with("printf '%s' '{"), "{line}");
        assert!(line.contains("| PULSE_HOOK_EVENT='pre_tool' PULSE_TOOL='bash' /bin/sh -c '"));
        assert!(line.ends_with(r#"echo it'\''s fine'"#), "{line}");
        assert_eq!(shell_quote("a'b"), r#"'a'\''b'"#);
        let run = run_payload(
            HookEvent::RunEnd,
            "/work",
            json!({"exit_status": "submitted"}),
        );
        assert_eq!(run["event"], "run_end");
        assert_eq!(run["exit_status"], "submitted");
    }

    #[test]
    fn hook_command_runs_with_payload_on_stdin() {
        let hook = Hook {
            event: HookEvent::PreTool,
            tools: Vec::new(),
            command: "cat; printf ' %s %s' \"$PULSE_HOOK_EVENT\" \"$PULSE_TOOL\"".into(),
            timeout: DEFAULT_HOOK_TIMEOUT,
            source: "x".into(),
        };
        let payload = json!({"event": "pre_tool", "tool": "bash", "input": {"command": "it's"}});
        let line = hook_command(&hook, "bash", &payload);
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&line)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{payload} pre_tool bash")
        );
    }
}
