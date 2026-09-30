use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

pub const HARNESSES: &[&str] = &["copilot", "claude"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Install {
    pub harness: String,
    pub path: String,
    pub content: String,
    pub undo: String,
    pub changed: bool,
}

const CONTEXT_CMD: &str = " hooks context --harness ";
const STOP_CMD: &str = " hooks stop --harness ";
const BEAT_CMD: &str = " hooks beat --harness ";

fn pulse_bin() -> String {
    if path_has_pulse() {
        "pulse".to_string()
    } else {
        std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "pulse".to_string())
    }
}

fn path_has_pulse() -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|p| {
        let c = p.join("pulse");
        c.is_file()
    })
}

pub fn config_path(harness: &str, dir: &str) -> Result<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    match harness {
        "copilot" => {
            if !dir.is_empty() {
                return Ok(Path::new(dir)
                    .join(".github/hooks/pulse.json")
                    .display()
                    .to_string());
            }
            let base = std::env::var("COPILOT_HOME")
                .unwrap_or_else(|_| Path::new(&home).join(".copilot").display().to_string());
            Ok(Path::new(&base)
                .join("hooks/pulse.json")
                .display()
                .to_string())
        }
        "claude" => {
            if !dir.is_empty() {
                return Ok(Path::new(dir)
                    .join(".claude/settings.json")
                    .display()
                    .to_string());
            }
            Ok(Path::new(&home)
                .join(".claude/settings.json")
                .display()
                .to_string())
        }
        _ => Err(anyhow!(
            "unknown harness {:?}; choose {}",
            harness,
            HARNESSES.join(" or ")
        )),
    }
}

pub fn render(harness: &str, bin: &str, existing: &[u8]) -> Result<(String, bool)> {
    match harness {
        "copilot" => {
            let entry = |cmd: String, timeout: i32| json!({"bash": cmd, "powershell": cmd, "timeoutSec": timeout, "type": "command"});
            let doc = json!({
                "hooks": {
                    "agentStop": [entry(format!("{bin}{STOP_CMD}copilot"), 10)],
                    "postToolUse": [entry(format!("{bin}{BEAT_CMD}copilot"), 5)],
                    "sessionStart": [entry(format!("{bin}{CONTEXT_CMD}copilot"), 30)]
                },
                "version": 1
            });
            let out = pretty(&doc)?;
            Ok((out.clone(), trim(existing) != out.trim()))
        }
        "claude" => {
            let mut settings: Map<String, Value> = if trim(existing).is_empty() {
                Map::new()
            } else {
                match serde_json::from_slice::<Value>(existing)
                    .map_err(|e| anyhow!("existing settings are not a JSON object: {e}"))?
                {
                    Value::Object(m) => m,
                    _ => {
                        return Err(anyhow!(
                            "existing settings are not a JSON object: json: cannot unmarshal non-object into Go value of type map[string]interface {{}}"
                        ));
                    }
                }
            };
            let mut hooks = settings
                .remove("hooks")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            let mut changed = false;
            add_claude(
                &mut hooks,
                "SessionStart",
                &format!("{bin}{CONTEXT_CMD}claude"),
                30,
                &mut changed,
            );
            add_claude(
                &mut hooks,
                "Stop",
                &format!("{bin}{STOP_CMD}claude"),
                10,
                &mut changed,
            );
            add_claude(
                &mut hooks,
                "PostToolUse",
                &format!("{bin}{BEAT_CMD}claude"),
                5,
                &mut changed,
            );
            settings.insert("hooks".to_string(), Value::Object(hooks));
            Ok((pretty(&Value::Object(settings))?, changed))
        }
        _ => Err(anyhow!("unknown harness {:?}", harness)),
    }
}

fn add_claude(
    hooks: &mut Map<String, Value>,
    event: &str,
    cmd: &str,
    timeout: i32,
    changed: &mut bool,
) {
    let groups = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(groups) = groups else {
        return;
    };
    if groups.iter().any(|g| has_command(g, cmd)) {
        return;
    }
    groups.push(json!({"hooks": [{"command": cmd, "timeout": timeout, "type": "command"}]}));
    *changed = true;
}

fn has_command(group: &Value, cmd: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|list| {
            list.iter()
                .any(|h| h.get("command").and_then(Value::as_str) == Some(cmd))
        })
}

fn render_removal(existing: &[u8]) -> Result<(String, bool)> {
    if trim(existing).is_empty() {
        return Ok((String::new(), false));
    }
    let mut settings = match serde_json::from_slice::<Value>(existing)
        .map_err(|e| anyhow!("existing settings are not a JSON object: {e}"))?
    {
        Value::Object(m) => m,
        _ => {
            return Err(anyhow!(
                "existing settings are not a JSON object: json: cannot unmarshal non-object into Go value of type map[string]interface {{}}"
            ));
        }
    };
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else {
        return Ok((pretty(&Value::Object(settings))?, false));
    };
    let mut changed = false;
    let keys = hooks.keys().cloned().collect::<Vec<_>>();
    for event in keys {
        let Some(Value::Array(groups)) = hooks.get_mut(&event) else {
            continue;
        };
        let mut kept_groups = Vec::new();
        for group in std::mem::take(groups) {
            let mut group_obj = match group {
                Value::Object(m) => m,
                other => {
                    kept_groups.push(other);
                    continue;
                }
            };
            let mut had_list = false;
            let mut keep = Vec::new();
            if let Some(Value::Array(list)) = group_obj.remove("hooks") {
                had_list = true;
                for h in list {
                    let c = h.get("command").and_then(Value::as_str).unwrap_or("");
                    if c.contains(CONTEXT_CMD) || c.contains(STOP_CMD) || c.contains(BEAT_CMD) {
                        changed = true;
                    } else {
                        keep.push(h);
                    }
                }
            }
            if keep.is_empty() && had_list {
                continue;
            }
            if had_list {
                group_obj.insert("hooks".to_string(), Value::Array(keep));
            }
            kept_groups.push(Value::Object(group_obj));
        }
        if kept_groups.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, Value::Array(kept_groups));
        }
    }
    if hooks.is_empty() {
        settings.remove("hooks");
    }
    Ok((pretty(&Value::Object(settings))?, changed))
}

pub fn install_harness(harness: &str, dir: &str, dry_run: bool) -> Result<Install> {
    let path = config_path(harness, dir)?;
    let existing = match std::fs::read(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let (content, changed) =
        render(harness, &pulse_bin(), &existing).map_err(|e| anyhow!("{path}: {e}"))?;
    let mut undo = format!("pulse hooks uninstall {harness}");
    if !dir.is_empty() {
        undo.push_str(" --dir ");
        undo.push_str(dir);
    }
    if harness == "copilot" {
        undo.push_str("  (or: rm ");
        undo.push_str(&path);
        undo.push(')');
    }
    let res = Install {
        harness: harness.to_string(),
        path: path.clone(),
        content: content.clone(),
        undo,
        changed,
    };
    if dry_run || !changed {
        return Ok(res);
    }
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(res)
}

pub fn uninstall_harness(harness: &str, dir: &str, dry_run: bool) -> Result<Install> {
    let path = config_path(harness, dir)?;
    let mut res = Install {
        harness: harness.to_string(),
        path: path.clone(),
        ..Default::default()
    };
    let existing = match std::fs::read(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(res),
        Err(e) => return Err(e.into()),
    };
    match harness {
        "copilot" => {
            res.changed = true;
            if !dry_run {
                std::fs::remove_file(&path)?;
            }
        }
        "claude" => {
            let (content, changed) =
                render_removal(&existing).map_err(|e| anyhow!("{path}: {e}"))?;
            res.content = content.clone();
            res.changed = changed;
            if changed && !dry_run {
                std::fs::write(&path, content)?;
            }
        }
        _ => return Err(anyhow!("unknown harness {:?}", harness)),
    }
    Ok(res)
}

pub fn context_json<F>(harness: &str, mut run: F) -> String
where
    F: FnMut(&[&str]) -> String,
{
    let mut text = String::new();
    text.push_str("PULSE - the board this checkout works on. Read before acting; post findings, vote, claim before you change things.\n\n");
    text.push_str(run(&["guide"]).trim());
    text.push_str("\n\n");
    text.push_str(run(&["board"]).trim());
    let doc = if harness == "claude" {
        json!({"hookSpecificOutput": {"additionalContext": text, "hookEventName": "SessionStart"}})
    } else {
        json!({"additionalContext": text})
    };
    plain_json(&doc)
}

pub fn run_pulse(home: &str, args: &[String]) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pulse"));
    let mut full = Vec::new();
    if !home.is_empty() {
        full.extend(["--home".to_string(), home.to_string()]);
    }
    full.push("--no-color".to_string());
    full.extend_from_slice(args);
    let out = std::process::Command::new(exe)
        .args(&full)
        .env("NO_COLOR", "1")
        .output();
    match out {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !out.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(String::from_utf8_lossy(&out.stderr).trim());
            }
            if !out.status.success() {
                text.push_str(&format!(
                    "\n(pulse {} failed: {})",
                    args.join(" "),
                    out.status
                ));
            }
            text
        }
        Err(e) => format!("(pulse {} failed: {e})", args.join(" ")),
    }
}

pub const STOP_REMINDER: &str = "Before you stop, PULSE asks one thing: if this session did tool-backed work, record what you learned - `pulse post finding --area AREA --thread THREAD --summary '...'` (or `pulse reflect`), and vote on what you have an opinion on (`pulse ballot`). If there is nothing worth recording, stop now.";

type ReminderFn = fn(Option<&pulse_store::Store>) -> String;
static REMINDER: OnceLock<RwLock<ReminderFn>> = OnceLock::new();

fn default_reminder(s: Option<&pulse_store::Store>) -> String {
    let text = pulse_evolve::stop_reminder(s).trim().to_string();
    if text.is_empty() {
        STOP_REMINDER.to_string()
    } else {
        text
    }
}

pub fn set_reminder(f: ReminderFn) {
    *REMINDER
        .get_or_init(|| RwLock::new(default_reminder))
        .write()
        .expect("reminder lock") = f;
}

pub fn reminder(s: Option<&pulse_store::Store>) -> String {
    let f = *REMINDER
        .get_or_init(|| RwLock::new(default_reminder))
        .read()
        .expect("reminder lock");
    f(s)
}

pub fn stop_json(
    harness: &str,
    home: &str,
    s: Option<&pulse_store::Store>,
    mut payload: impl Read,
) -> String {
    let mut raw = Vec::new();
    let _ = payload.by_ref().take(1 << 20).read_to_end(&mut raw);
    let input: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let active = ["stop_hook_active", "stopHookActive"]
        .iter()
        .any(|k| input.get(*k).and_then(Value::as_bool).unwrap_or(false));
    if active {
        return "{}".to_string();
    }
    let sid = ["session_id", "sessionId"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .unwrap_or("");
    if !sid.is_empty() && !home.is_empty() {
        let dir = Path::new(home).join("hooks/reminded");
        let marker = dir.join(sanitize(sid));
        if marker.exists() {
            return "{}".to_string();
        }
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(marker, format!("{sid}\n"));
        }
    }
    let text = reminder(s);
    if text.trim().is_empty() {
        return "{}".to_string();
    }
    let doc = if harness == "claude" {
        json!({"hookSpecificOutput": {"additionalContext": text, "hookEventName": "Stop"}})
    } else {
        json!({"decision": "block", "reason": text})
    };
    plain_json(&doc)
}

/// The harness session id a hook payload carries (Copilot spells it `sessionId`, Claude `session_id`).
pub fn payload_session(payload: &[u8]) -> String {
    let input: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    ["session_id", "sessionId"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

/// What the installed post-tool hook runs: a heartbeat for the session's agent.
/// The agent is `agent` when the hook's environment names one (`PULSE_AGENT`),
/// otherwise the active session agent whose `sessions` list holds the payload's
/// session id (`pulse agent join --session ID`). Hooks never fail the harness:
/// an unknown agent or a closed store beats nothing and the output is `{}`.
pub fn beat_json(home: &str, agent: &str, now: &str, mut payload: impl Read) -> String {
    let mut raw = Vec::new();
    let _ = payload.by_ref().take(1 << 20).read_to_end(&mut raw);
    let sid = payload_session(&raw);
    let Ok(s) = pulse_store::Store::open(pulse_store::Options {
        home: home.to_string(),
        create: false,
        read_only: false,
        skip_records: true,
        now: now.to_string(),
    }) else {
        return "{}".to_string();
    };
    let name = if agent.is_empty() {
        session_agent(&s, &sid)
    } else {
        agent.to_string()
    };
    if name.is_empty() {
        return "{}".to_string();
    }
    let _ = pulse_board::beat_agent(&s, &name, "hook");
    "{}".to_string()
}

/// The active session agent that joined with this harness session id, if any.
pub fn session_agent(s: &pulse_store::Store, sid: &str) -> String {
    if sid.is_empty() {
        return String::new();
    }
    let Ok(conn) = s.conn() else {
        return String::new();
    };
    conn.query_row(
        "SELECT name FROM agents a WHERE a.status = 'active' AND a.kind NOT IN ('human','legacy')
         AND EXISTS (SELECT 1 FROM json_each(a.sessions) WHERE value = ?)
         ORDER BY a.last_active_at DESC LIMIT 1",
        [sid],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_default()
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn plain_json(doc: &Value) -> String {
    pulse_store::gojson::to_string_no_html(doc).unwrap_or_else(|_| "{}".to_string())
}

fn pretty(doc: &Value) -> Result<String> {
    Ok(format!("{}\n", pulse_store::gojson::to_string_pretty(doc)?))
}

fn trim(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{context_args, register, set_stdin_for_tests};
    use crate::hooks::test_helpers::new_store;
    use pulse_cli::Context;

    #[test]
    fn install_copilot_and_claude() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "command",
            "hooks context --harness claude",
            "hooks stop --harness copilot",
            "json:\"bash\"",
            "json:\"hooks\"",
            "json:\"powershell\"",
            "json:\"timeoutSec\"",
            "json:\"type\"",
            "json:\"version\"",
            "pulse hooks uninstall copilot --dir",
            "timeout",
        );
        let repo = tempfile::tempdir().unwrap();
        let repo_s = repo.path().display().to_string();
        let res = install_harness("copilot", &repo_s, true).unwrap();
        assert_eq!(
            res.path,
            repo.path()
                .join(".github/hooks/pulse.json")
                .display()
                .to_string()
        );
        assert!(res.changed);
        assert!(!Path::new(&res.path).exists());
        let res = install_harness("copilot", &repo_s, false).unwrap();
        let raw = std::fs::read_to_string(&res.path).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["hooks"]["sessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(doc["hooks"]["agentStop"].as_array().unwrap().len(), 1);
        assert_eq!(doc["hooks"]["postToolUse"].as_array().unwrap().len(), 1);
        assert!(
            doc["hooks"]["sessionStart"][0]["bash"]
                .as_str()
                .unwrap()
                .ends_with(" hooks context --harness copilot")
        );
        assert!(
            doc["hooks"]["postToolUse"][0]["bash"]
                .as_str()
                .unwrap()
                .ends_with(" hooks beat --harness copilot")
        );
        assert_eq!(doc["hooks"]["postToolUse"][0]["timeoutSec"], 5);
        assert!(!install_harness("copilot", &repo_s, false).unwrap().changed);
        assert!(
            uninstall_harness("copilot", &repo_s, false)
                .unwrap()
                .changed
        );
        assert!(!Path::new(&res.path).exists());

        let settings = repo.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo bye"}]}]}}"#).unwrap();
        let res = install_harness("claude", &repo_s, false).unwrap();
        assert!(res.changed);
        let raw = std::fs::read_to_string(&settings).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["model"], "opus");
        assert_eq!(doc["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(doc["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert!(
            doc["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with(" hooks beat --harness claude")
        );
        assert!(!install_harness("claude", &repo_s, false).unwrap().changed);
        assert!(uninstall_harness("claude", &repo_s, false).unwrap().changed);
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(doc["hooks"].get("SessionStart").is_none());
        assert!(doc["hooks"].get("PostToolUse").is_none());
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(install_harness("cursor", &repo_s, true).is_err());
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("COPILOT_HOME", repo.path().join("chome"));
        }
        assert_eq!(
            config_path("copilot", "").unwrap(),
            repo.path()
                .join("chome/hooks/pulse.json")
                .display()
                .to_string()
        );
        assert!(
            config_path("claude", "")
                .unwrap()
                .ends_with(".claude/settings.json")
        );
    }

    fn ctx_for(
        s: Option<pulse_store::Store>,
        home: &str,
        json: bool,
    ) -> (Context, pulse_cli::Buffer) {
        let out = pulse_cli::Buffer::new();
        (
            Context {
                store: s,
                home: home.to_string(),
                out: Box::new(out.clone()),
                err: Box::new(out.clone()),
                json,
                quiet: false,
                color: false,
                agent: String::new(),
                project: String::new(),
                session: String::new(),
                now: "2026-09-06T21:00:00Z".into(),
                args: vec![],
            },
            out,
        )
    }

    fn run_cmd(c: &mut Context, name: &str, args: &[&str]) -> Result<()> {
        register();
        let cmd = pulse_cli::lookup(name).expect("command");
        let mut fs = pulse_cli::FlagSet::new(name);
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        let raw = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let positional = pulse_cli::flags::parse_interleaved(&mut fs, &raw)?;
        (cmd.run)(c, &fs, &positional)
    }

    #[test]
    fn context_and_stop_json() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "\"; cat >> \"",
            "\"hookEventName\":\"Stop\"",
            "--dry-run",
            "--harness",
            "OUT:board",
            "Undo: pulse hooks uninstall copilot",
            "cycle-1",
            "do not block",
            "event leaked the private path to the hook:",
            "event was not masked:",
            "evolve",
            "evolve.cycle",
            "evolve.heldout_dir",
            "heldout-fixtures",
            "install",
            "printf '' \"$PULSE_EVENT_DETAIL\" > \"",
            "script",
            "seen.txt",
            "would write",
        );
        let run = |args: &[&str]| format!("OUT:{}", args.join(" "));
        let cp: Value = serde_json::from_str(&context_json("copilot", run)).unwrap();
        assert!(
            cp["additionalContext"]
                .as_str()
                .unwrap()
                .contains("OUT:guide")
        );
        let cl: Value = serde_json::from_str(&context_json("claude", run)).unwrap();
        assert_eq!(cl["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().display().to_string();
        assert_eq!(
            stop_json(
                "claude",
                &home_s,
                None,
                r#"{"session_id":"abc","stop_hook_active":true}"#.as_bytes()
            ),
            "{}"
        );
        let first = stop_json(
            "claude",
            &home_s,
            None,
            r#"{"session_id":"abc"}"#.as_bytes(),
        );
        let st: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(st["hookSpecificOutput"]["hookEventName"], "Stop");
        assert!(
            st["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("pulse reflect")
        );
        assert_eq!(
            stop_json(
                "claude",
                &home_s,
                None,
                r#"{"session_id":"abc"}"#.as_bytes()
            ),
            "{}"
        );
        let cop = stop_json(
            "copilot",
            &home_s,
            None,
            r#"{"sessionId":"xyz/1"}"#.as_bytes(),
        );
        let cs: Value = serde_json::from_str(&cop).unwrap();
        assert_eq!(cs["decision"], "block");
        assert!(home.path().join("hooks/reminded/xyz_1").exists());
        assert!(
            stop_json("copilot", &home_s, None, b"not json" as &[u8])
                .starts_with(r#"{"decision":"block""#)
        );
        assert_eq!(cs["reason"], reminder(None));
        set_stdin_for_tests(Some(br#"{"session_id":"cmd"}"#.to_vec()));
        let (mut c, out) = ctx_for(None, &home_s, false);
        run_cmd(&mut c, "hooks", &["stop", "--harness", "claude"]).unwrap();
        assert!(
            out.string().contains(r#""hookEventName":"Stop""#),
            "{}",
            out.string()
        );
        let (mut c, out) = ctx_for(None, &home_s, false);
        run_cmd(
            &mut c,
            "hooks",
            &["install", "copilot", "--dir", &home_s, "--dry-run"],
        )
        .unwrap();
        assert!(out.string().contains("would write"));
        assert!(out.string().contains("Undo: pulse hooks uninstall copilot"));
        assert!(run_cmd(&mut c, "hooks", &["install", "vim"]).is_err());
        set_stdin_for_tests(None);
    }

    fn test_reminder_some(s: Option<&pulse_store::Store>) -> String {
        if s.is_some() {
            "record the parity finding on runner/compute".into()
        } else {
            "no store".into()
        }
    }
    fn test_reminder_empty(_: Option<&pulse_store::Store>) -> String {
        String::new()
    }

    #[test]
    fn beat_json_beats_the_session_agent_and_never_fails_the_harness() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let home = s.home();
        let now = "2026-09-06T21:00:00Z";
        for (name, session) in [("fable-a", "sess-a"), ("fable-b", "sess-b")] {
            pulse_board::join_agent(
                &s,
                pulse_board::JoinInput {
                    name: name.into(),
                    kind: "session".into(),
                    session: session.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(session_agent(&s, "sess-b"), "fable-b");
        assert_eq!(session_agent(&s, "nobody"), "");
        assert_eq!(session_agent(&s, ""), "");
        assert_eq!(
            payload_session(br#"{"sessionId":"sess-a","cwd":"/x"}"#),
            "sess-a"
        );
        assert_eq!(payload_session(br#"{"session_id":"sess-b"}"#), "sess-b");
        assert_eq!(payload_session(b"not json"), "");
        let beat_of = |name: &str| -> String {
            s.meta(&format!("{}{name}", pulse_board::BEAT_PREFIX))
                .unwrap()
                .0
        };
        // Copilot names the session; the payload resolves the agent.
        assert_eq!(
            beat_json(&home, "", now, br#"{"sessionId":"sess-a"}"#.as_ref()),
            "{}"
        );
        let a: pulse_board::AgentBeat = serde_json::from_str(&beat_of("fable-a")).unwrap();
        assert_eq!(a.source, "hook");
        assert_eq!(a.at, now);
        assert_eq!(beat_of("fable-b"), "");
        // PULSE_AGENT in the hook's environment wins over the payload.
        assert_eq!(
            beat_json(&home, "fable-b", now, br#"{"sessionId":"sess-a"}"#.as_ref()),
            "{}"
        );
        assert!(!beat_of("fable-b").is_empty());
        // Unknown sessions, humans and a missing store beat nothing and still answer `{}`.
        assert_eq!(
            beat_json(&home, "", now, br#"{"sessionId":"sess-x"}"#.as_ref()),
            "{}"
        );
        assert_eq!(beat_json(&home, "human", now, b"".as_ref()), "{}");
        assert_eq!(beat_json(&home, "", now, b"".as_ref()), "{}");
        let missing = tempfile::tempdir().unwrap();
        assert_eq!(
            beat_json(
                &missing.path().join("none").display().to_string(),
                "fable-a",
                now,
                b"{}".as_ref()
            ),
            "{}"
        );
        let beats: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'agent-beat:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(beats, 2);
        let p = pulse_board::agent_presences(&s, now).unwrap();
        assert_eq!(p["fable-a"].presence, "online");
        assert_eq!(p["fable-a"].heartbeat_source, "hook");
    }

    #[test]
    fn stop_json_uses_store_aware_reminder() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().display().to_string();
        set_reminder(test_reminder_some);
        let out = stop_json(
            "copilot",
            &home_s,
            Some(&s),
            r#"{"sessionId":"s1"}"#.as_bytes(),
        );
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["reason"], "record the parity finding on runner/compute");
        let out = stop_json("copilot", &home_s, None, r#"{"sessionId":"s2"}"#.as_bytes());
        assert!(out.contains("no store"));
        set_reminder(test_reminder_empty);
        assert_eq!(
            stop_json(
                "claude",
                &home_s,
                Some(&s),
                r#"{"session_id":"s3"}"#.as_bytes()
            ),
            "{}"
        );
        set_reminder(default_reminder);
    }

    #[test]
    fn project_goal_hooks_and_reflect_stay_current_and_read_only() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("PULSE_JOB", "");
            std::env::set_var("PULSE_AGENT", "");
            std::env::set_var("PULSE_PROJECT", "");
            std::env::set_var("PULSE_SESSION", "");
        }
        let s = new_store();
        {
            let conn = s.conn().unwrap();
            conn.execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo','Demo',?,?)",
                rusqlite::params![
                    Path::new(&s.home)
                        .join("missing-checkout")
                        .display()
                        .to_string(),
                    s.now()
                ],
            )
            .unwrap();
        }
        let ro = pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            read_only: true,
            ..Default::default()
        })
        .unwrap();
        for text in [
            "",
            "Improve useful search with measured regression outcomes.",
        ] {
            if !text.is_empty() {
                pulse_goals::set(&s, "demo", text, "human").unwrap();
            }
            let goal = pulse_goals::get(&ro, "demo").unwrap();
            let (before_events, before_settings): (i64, i64) = s
                .conn()
                .unwrap()
                .query_row(
                    "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM settings)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            let cancel = pulse_bus::CancellationToken::new();
            let reflected = pulse_evolve::reflect(&cancel, Some(&ro), "demo", "", "", "").unwrap();
            assert!(reflected.board.contains(&pulse_goals::context(&goal)));
            for harness in ["copilot", "claude"] {
                let ctx = Context {
                    project: "demo".into(),
                    agent: "reader".into(),
                    session: "interactive-session".into(),
                    store: None,
                    home: String::new(),
                    out: Box::new(pulse_cli::Buffer::new()),
                    err: Box::new(pulse_cli::Buffer::new()),
                    json: false,
                    quiet: false,
                    color: false,
                    now: String::new(),
                    args: vec![],
                };
                let result = context_json(harness, |args| {
                    let scoped = context_args(&ctx, args);
                    assert_eq!(
                        &scoped[1..],
                        [
                            "--project",
                            "demo",
                            "--agent",
                            "reader",
                            "--session",
                            "interactive-session"
                        ]
                    );
                    if args[0] == "guide" {
                        return "Existing PULSE rules apply.".to_string();
                    }
                    pulse_tally::digest_for(
                        &ro,
                        &ctx.project,
                        "",
                        pulse_tally::Thresholds::default(),
                        "",
                        &ctx.session,
                    )
                    .unwrap()
                });
                assert!(result.contains(&goal.text), "{result}");
                assert!(result.contains("not permission"), "{result}");
            }
            let (after_events, after_settings): (i64, i64) = s
                .conn()
                .unwrap()
                .query_row(
                    "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM settings)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                (before_events, before_settings),
                (after_events, after_settings)
            );
        }
    }
}
