use crate::hooks::{deliveries, delivery_states, fire, get, list, set_enabled};
use crate::install::{
    Install, context_json, install_harness, run_pulse, stop_json, uninstall_harness,
};
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, register as cli_register, write_json};
use serde::Serialize;
use std::io::Write;
use std::sync::{Mutex, Once};

static REGISTER: Once = Once::new();
static STDIN: Mutex<Option<Vec<u8>>> = Mutex::new(None);

#[derive(Serialize)]
struct HookOut {
    hook: crate::hooks::Hook,
}

#[derive(Serialize)]
struct HookIdOut {
    hook: i64,
    status: String,
    ok: bool,
}

#[derive(Serialize)]
struct ListOut {
    deliveries: std::collections::BTreeMap<i64, crate::hooks::DeliveryState>,
    hooks: Vec<crate::hooks::Hook>,
}

#[derive(Serialize)]
struct DeliveriesOut {
    deliveries: Vec<crate::hooks::Delivery>,
}

pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN.lock().expect("stdin lock") = data;
}

pub fn register() {
    REGISTER.call_once(|| {
        cli_register(Command {
            name: "hook".into(),
            group: "agents".into(),
            summary: "scripts and webhooks that fire on store events: add, list, remove, enable, test".into(),
            usage: "pulse hook add <event-glob> --script PATH | --url URL [--project P]\n\
pulse hook list [--all] [--json]\n\
pulse hook deliveries <id> [--limit N] [--json]\n\
pulse hook remove <id>      (disables; rows are never deleted)\n\
pulse hook enable <id>\n\
pulse hook test <id>        (fires it with a synthetic event)".into(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run: run_hook,
            flags: Some(hook_flags),
        });
        cli_register(Command {
            name: "hooks".into(),
            group: "human".into(),
            summary: "harness hooks for interactive agents: install copilot|claude (session start reads the board, stop reminds to reflect)".into(),
            usage: "pulse hooks install copilot|claude [--dir PATH] [--dry-run]\n\
pulse hooks uninstall copilot|claude [--dir PATH] [--dry-run]\n\
pulse hooks context --harness copilot|claude   (what the installed sessionStart hook runs)\n\
pulse hooks stop --harness copilot|claude      (what the installed stop hook runs)".into(),
            aliases: vec![],
            no_store: true,
            read_only: false,
            raw: false,
            run: run_hooks,
            flags: Some(hooks_flags),
        });
    });
}

fn hook_flags(fs: &mut FlagSet) {
    fs.string_var(
        "script",
        "",
        "executable to run (env PULSE_EVENT_*, JSON on stdin, 30 s)",
    );
    fs.string_var("url", "", "URL to POST the event JSON to (10 s)");
    fs.bool_var("all", false, "list removed hooks too");
    fs.int_var("limit", 50, "delivery rows to show");
}

fn hooks_flags(fs: &mut FlagSet) {
    fs.string_var(
        "dir",
        "",
        "repository to install into (default: the user-level location)",
    );
    fs.bool_var(
        "dry-run",
        false,
        "print what would be written, write nothing",
    );
    fs.string_var("harness", "copilot", "copilot or claude (context, stop)");
}

fn cli_error(
    code: i32,
    what: impl Into<String>,
    why: impl Into<String>,
    fix: impl Into<String>,
) -> anyhow::Error {
    pulse_cli::Error {
        what: what.into(),
        why: why.into(),
        fix: fix.into(),
        code,
    }
    .into()
}

fn store(c: &Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("store is required"))
}

fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.to_string()
    } else {
        c.agent.clone()
    }
}

fn parse_id(arg: &str) -> Result<i64> {
    arg.parse::<i64>().map_err(|_| {
        cli_error(
            2,
            "A hook id is a number",
            format!("got {arg:?}"),
            "pulse hook list",
        )
    })
}

fn run_hook(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(cli_error(
            2,
            "pulse hook needs a subcommand",
            "add, list, deliveries, remove, enable or test",
            "pulse help hook",
        ));
    }
    let sub = args[0].as_str();
    let rest = &args[1..];
    match sub {
        "add" => {
            if rest.is_empty() {
                return Err(cli_error(
                    2,
                    "Missing event glob",
                    "the first argument names the actions the hook fires on",
                    "pulse hook add 'task.*' --script PATH",
                ));
            }
            let mut kind = "";
            let mut target = String::new();
            let script = fs.get_string("script");
            if !script.is_empty() {
                kind = "script";
                target = script;
            }
            let url = fs.get_string("url");
            if !url.is_empty() {
                if !kind.is_empty() {
                    return Err(cli_error(
                        2,
                        "A hook is a --script or a --url",
                        "not both",
                        "pulse help hook",
                    ));
                }
                kind = "webhook";
                target = url;
            }
            let h = crate::hooks::add(store(c)?, &rest[0], kind, &target, &c.project, &actor(c))?;
            if c.json {
                write_json(&mut c.out, &HookOut { hook: h })?;
            } else {
                c.printf(format_args!(
                    "hook {} added: {} -> {} {}\n",
                    h.id, h.event, h.kind, h.target
                ))?;
            }
        }
        "list" => {
            let rows = list(store(c)?, !fs.get_bool("all"))?;
            let states = delivery_states(store(c)?)?;
            if c.json {
                write_json(
                    &mut c.out,
                    &ListOut {
                        deliveries: states,
                        hooks: rows,
                    },
                )?;
            } else if rows.is_empty() {
                c.println("No hooks. Add one: pulse hook add 'task.*' --script PATH")?;
            } else {
                let table = rows
                    .iter()
                    .map(|h| {
                        let state = states.get(&h.id).cloned().unwrap_or_default();
                        vec![
                            h.id.to_string(),
                            if h.enabled { "on" } else { "off" }.to_string(),
                            h.event.clone(),
                            h.project.clone(),
                            h.kind.clone(),
                            h.target.clone(),
                            h.fires.to_string(),
                            state.pending.to_string(),
                            h.last_fired_at.clone(),
                            h.last_status.clone(),
                            state.last_error,
                        ]
                    })
                    .collect::<Vec<_>>();
                pulse_cli::table(
                    &mut c.out,
                    c.color,
                    Some(&[
                        "ID",
                        "ENABLED",
                        "EVENT",
                        "PROJECT",
                        "KIND",
                        "TARGET",
                        "FIRES",
                        "PENDING",
                        "LAST",
                        "STATUS",
                        "LAST ERROR",
                    ]),
                    &table,
                )?;
            }
        }
        "deliveries" => {
            if rest.is_empty() {
                return Err(cli_error(
                    2,
                    "Missing hook id",
                    "which hook?",
                    "pulse hook list",
                ));
            }
            let id = parse_id(&rest[0])?;
            let _ = get(store(c)?, id)?;
            let rows = deliveries(store(c)?, id, fs.get_int("limit"))?;
            if c.json {
                write_json(&mut c.out, &DeliveriesOut { deliveries: rows })?;
            } else {
                let table = rows
                    .iter()
                    .map(|d| {
                        vec![
                            d.id.to_string(),
                            d.event_seq.to_string(),
                            d.event_action.clone(),
                            d.attempt.to_string(),
                            d.status.clone(),
                            d.next_at.clone(),
                            d.detail.clone(),
                        ]
                    })
                    .collect::<Vec<_>>();
                pulse_cli::table(
                    &mut c.out,
                    c.color,
                    Some(&[
                        "ID", "EVENT", "ACTION", "ATTEMPT", "STATUS", "NEXT", "DETAIL",
                    ]),
                    &table,
                )?;
            }
        }
        "remove" | "enable" => {
            if rest.is_empty() {
                return Err(cli_error(
                    2,
                    "Missing hook id",
                    "which hook?",
                    "pulse hook list",
                ));
            }
            let id = parse_id(&rest[0])?;
            let h = set_enabled(store(c)?, id, sub == "enable", &actor(c))?;
            if c.json {
                write_json(&mut c.out, &HookOut { hook: h })?;
            } else if sub == "enable" {
                c.printf(format_args!("hook {} enabled\n", h.id))?;
            } else {
                c.printf(format_args!(
                    "hook {} removed (disabled; pulse hook enable {} brings it back)\n",
                    h.id, h.id
                ))?;
            }
        }
        "test" => {
            if rest.is_empty() {
                return Err(cli_error(
                    2,
                    "Missing hook id",
                    "which hook?",
                    "pulse hook list",
                ));
            }
            let id = parse_id(&rest[0])?;
            let h = get(store(c)?, id)?;
            let mut action = h.event.replace('*', "test").replace('?', "x");
            if action.contains(['[', ']']) || action.is_empty() {
                action = "hook.test".into();
            }
            let e = pulse_bus::Event {
                at: store(c)?.now(),
                actor: actor(c),
                action,
                subject: "hook-test".into(),
                detail: format!(r#"{{"project": "{}", "test": true}}"#, h.project),
                ..Default::default()
            };
            let (status, err) = fire(&pulse_bus::CancellationToken::new(), store(c)?, &h, &e);
            if c.json {
                write_json(
                    &mut c.out,
                    &HookIdOut {
                        hook: h.id,
                        status,
                        ok: err.is_ok(),
                    },
                )?;
            } else if let Err(e) = err {
                return Err(cli_error(
                    1,
                    format!("hook {} failed", h.id),
                    e.to_string(),
                    format!(
                        "fix the script or the URL, then pulse hook test {}",
                        rest[0]
                    ),
                ));
            } else {
                c.printf(format_args!("hook {} fired: {}\n", h.id, status))?;
            }
        }
        _ => {
            return Err(cli_error(
                2,
                format!("pulse hook: unknown subcommand {sub:?}"),
                "add, list, deliveries, remove, enable or test",
                "pulse help hook",
            ));
        }
    }
    Ok(())
}

fn run_hooks(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(cli_error(
            2,
            "pulse hooks needs a subcommand",
            "install, uninstall, context or stop",
            "pulse help hooks",
        ));
    }
    let sub = args[0].as_str();
    let rest = &args[1..];
    match sub {
        "install" | "uninstall" => {
            if rest.is_empty() {
                return Err(cli_error(
                    2,
                    "Which harness?",
                    "copilot or claude",
                    format!("pulse hooks {sub} copilot"),
                ));
            }
            let res: Install = if sub == "install" {
                install_harness(&rest[0], &fs.get_string("dir"), fs.get_bool("dry-run"))
            } else {
                uninstall_harness(&rest[0], &fs.get_string("dir"), fs.get_bool("dry-run"))
            }
            .map_err(|e| {
                cli_error(
                    2,
                    format!("Cannot {sub} {} hooks", rest[0]),
                    e.to_string(),
                    "pulse help hooks",
                )
            })?;
            if c.json {
                write_json(&mut c.out, &res)?;
            } else {
                let st = c.style();
                match (sub, res.changed, fs.get_bool("dry-run")) {
                    ("install", false, _) => c.printf(format_args!(
                        "{} already has the PULSE hooks; nothing written.\n",
                        res.path
                    ))?,
                    ("install", true, true) => c.printf(format_args!(
                        "{} {}\n\n{}\n",
                        st.bold("would write"),
                        res.path,
                        res.content
                    ))?,
                    ("install", true, false) => c.printf(format_args!(
                        "{} {}\n\n{}\n",
                        st.bold("wrote"),
                        res.path,
                        res.content
                    ))?,
                    ("uninstall", false, _) => c.printf(format_args!(
                        "{} has no PULSE hooks; nothing to remove.\n",
                        res.path
                    ))?,
                    ("uninstall", true, true) => c.printf(format_args!(
                        "{} the PULSE hooks from {}\n",
                        st.bold("would remove"),
                        res.path
                    ))?,
                    _ => c.printf(format_args!(
                        "{} the PULSE hooks from {}\n",
                        st.bold("removed"),
                        res.path
                    ))?,
                }
                if sub == "install" {
                    c.printf(format_args!("What it does: session start -> `pulse guide` + `pulse board` become context; stop -> one reminder per session to record findings.\n"))?;
                    c.printf(format_args!(
                        "Takes effect on the next {} session. Undo: {}\n",
                        rest[0], res.undo
                    ))?;
                }
            }
        }
        "context" => {
            let harness = fs.get_string("harness");
            let home = c.home.clone();
            let scoped = |args: &[&str]| {
                let v = context_args(c, args);
                run_pulse(&home, &v)
            };
            writeln!(c.out, "{}", context_json(&harness, scoped))?;
        }
        "stop" => {
            let opened = pulse_store::Store::open(pulse_store::Options {
                home: c.home.clone(),
                read_only: true,
                ..Default::default()
            })
            .ok();
            let input = STDIN
                .lock()
                .expect("stdin lock")
                .clone()
                .unwrap_or_else(|| {
                    let mut buf = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf);
                    buf
                });
            writeln!(
                c.out,
                "{}",
                stop_json(
                    &fs.get_string("harness"),
                    &c.home,
                    opened.as_ref(),
                    &input[..]
                )
            )?;
        }
        _ => {
            return Err(cli_error(
                2,
                format!("pulse hooks: unknown subcommand {sub:?}"),
                "install, uninstall, context or stop",
                "pulse help hooks",
            ));
        }
    }
    Ok(())
}

pub fn context_args(c: &Context, args: &[&str]) -> Vec<String> {
    let mut out = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    for (name, value) in [
        ("--project", &c.project),
        ("--agent", &c.agent),
        ("--session", &c.session),
    ] {
        if !value.is_empty() {
            out.push(name.to_string());
            out.push(value.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::test_helpers::{new_store, script};

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
                err: Box::new(pulse_cli::Buffer::new()),
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
        let mut fs = FlagSet::new(name);
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        let raw = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let positional = pulse_cli::flags::parse_interleaved(&mut fs, &raw)?;
        (cmd.run)(c, &fs, &positional)
    }

    #[test]
    fn hook_commands() {
        let s = new_store();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("fired");
        let sh = script(&format!(
            "echo \"$PULSE_EVENT_ACTION\" > \"{}\"",
            marker.display()
        ));
        let (mut c, out) = ctx_for(Some(s.clone()), &s.home, false);
        run_cmd(&mut c, "hook", &["add", "task.*", "--script", &sh]).unwrap();
        assert!(out.string().contains("hook 1 added: task.* -> script"));
        assert!(
            run_cmd(
                &mut c,
                "hook",
                &["add", "post", "--script", &sh, "--url", "https://x.test"]
            )
            .is_err()
        );
        run_cmd(&mut c, "hook", &["test", "1"]).unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap().trim(), "task.test");
        let (mut c, out) = ctx_for(Some(s.clone()), &s.home, true);
        run_cmd(&mut c, "hook", &["list"]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
        assert_eq!(v["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"][0]["fires"], 1);
        let (mut c, out) = ctx_for(Some(s.clone()), &s.home, false);
        run_cmd(&mut c, "hook", &["remove", "1"]).unwrap();
        run_cmd(&mut c, "hook", &["list"]).unwrap();
        assert!(out.string().contains("No hooks"));
        run_cmd(&mut c, "hook", &["list", "--all"]).unwrap();
        assert!(out.string().contains("off"));
        run_cmd(&mut c, "hook", &["enable", "1"]).unwrap();
        assert!(run_cmd(&mut c, "hook", &["remove", "x"]).is_err());
    }

    #[test]
    fn hooks_stop_and_install_commands() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().display().to_string();
        set_stdin_for_tests(Some(br#"{"session_id":"cmd"}"#.to_vec()));
        let (mut c, out) = ctx_for(None, &home_s, false);
        run_cmd(&mut c, "hooks", &["stop", "--harness", "claude"]).unwrap();
        assert!(out.string().contains(r#""hookEventName":"Stop""#));
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

    #[test]
    fn oracle_command_scenarios() {
        let Some(oracle) = pulse_parity::oracle() else {
            eprintln!("skipped: no Go oracle");
            return;
        };
        #[derive(Debug, PartialEq, Eq)]
        struct Outcome {
            code: i32,
            stdout: String,
            stderr: String,
        }
        fn go(oracle: &std::path::Path, home: &std::path::Path, args: &[&str]) -> Outcome {
            let out = std::process::Command::new(oracle)
                .args(args)
                .env("PULSE_HOME", home)
                .env("PULSE_NOW", "2026-09-06T21:00:00Z")
                .env("NO_COLOR", "1")
                .output()
                .unwrap();
            Outcome {
                code: out.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            }
        }
        fn rust(home: &std::path::Path, args: &[&str]) -> Outcome {
            register();
            let cmd = pulse_cli::lookup(args[0]).unwrap();
            let mut fs = FlagSet::new(args[0]);
            if let Some(flags) = cmd.flags {
                flags(&mut fs);
            }
            for (name, default, usage) in [
                ("json", "false", "machine-readable output"),
                ("quiet", "false", "print only what a script needs"),
                ("no-color", "false", "no colour"),
            ] {
                if fs.lookup(name).is_none() && default == "false" {
                    fs.bool_var(name, false, usage);
                }
            }
            let raw = args[1..]
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>();
            let positional = match pulse_cli::flags::parse_interleaved(&mut fs, &raw) {
                Ok(v) => v,
                Err(e) => {
                    return Outcome {
                        code: 2,
                        stdout: String::new(),
                        stderr: format!("{e}\n"),
                    };
                }
            };
            let out = pulse_cli::Buffer::new();
            let err = pulse_cli::Buffer::new();
            let store = if cmd.no_store {
                None
            } else {
                Some(
                    pulse_store::Store::open(pulse_store::Options {
                        home: home.display().to_string(),
                        now: "2026-09-06T21:00:00Z".into(),
                        ..Default::default()
                    })
                    .unwrap(),
                )
            };
            let mut ctx = Context {
                store,
                home: home.display().to_string(),
                out: Box::new(out.clone()),
                err: Box::new(err.clone()),
                json: fs.get_bool("json"),
                quiet: fs.get_bool("quiet"),
                color: false,
                agent: String::new(),
                project: String::new(),
                session: String::new(),
                now: "2026-09-06T21:00:00Z".into(),
                args: positional.clone(),
            };
            let code = match (cmd.run)(&mut ctx, &fs, &positional) {
                Ok(()) => 0,
                Err(e) => {
                    if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
                        let _ = writeln!(ctx.err, "{ce}");
                        ce.exit_code()
                    } else {
                        let _ = writeln!(ctx.err, "pulse {}: {e}", args[0]);
                        1
                    }
                }
            };
            Outcome {
                code,
                stdout: out.string(),
                stderr: err.string(),
            }
        }
        fn norm(s: &str, home: &std::path::Path, sh: &std::path::Path) -> String {
            s.replace(&home.display().to_string(), "<HOME>")
                .replace(&sh.display().to_string(), "<SCRIPT>")
        }
        let go_home = tempfile::tempdir().unwrap();
        let rust_home = tempfile::tempdir().unwrap();
        pulse_store::Store::create(go_home.path().join(pulse_store::STORE_NAME)).unwrap();
        pulse_store::Store::create(rust_home.path().join(pulse_store::STORE_NAME)).unwrap();
        let sh_dir = tempfile::tempdir().unwrap();
        let sh = sh_dir.path().join("x.sh");
        std::fs::write(&sh, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for args in [
            vec!["hook", "add"],
            vec!["hook", "add", "job.done", "--script", sh.to_str().unwrap()],
            vec!["hook", "list"],
            vec!["hook", "list", "--json"],
            vec!["hook", "disable", "1"],
            vec!["hook", "enable", "1"],
            vec!["hook", "deliveries", "1"],
            vec!["hook", "add", "job.done", "--url", "ftp://x"],
        ] {
            let g = go(&oracle, go_home.path(), &args);
            let r = rust(rust_home.path(), &args);
            assert_eq!(g.code, r.code, "{args:?}");
            assert_eq!(
                norm(&g.stdout, go_home.path(), &sh),
                norm(&r.stdout, rust_home.path(), &sh),
                "stdout {args:?}"
            );
            assert_eq!(
                norm(&g.stderr, go_home.path(), &sh),
                norm(&r.stderr, rust_home.path(), &sh),
                "stderr {args:?}"
            );
        }
    }
}
