//! `pulse ask QUESTION…`: the quick answer box from a terminal.

use crate::{AskAnswer, AskRequest, DEFAULT_TIMEOUT};
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, errorf, register as cli_register, write_json};
use std::io::Write;
use std::sync::Once;

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        cli_register(Command {
            name: "ask".into(),
            group: "human".into(),
            summary: "ask a quick question about a project; the answer comes from the store, with its sources".into(),
            usage: "pulse ask QUESTION... [--project SLUG | --all] [--model M] [--no-model] [--json]".into(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run,
            flags: Some(flags),
        });
    });
}

fn flags(fs: &mut FlagSet) {
    fs.bool_var("all", false, "ask about every project, not one");
    fs.string_var(
        "model",
        "",
        "the model that answers; empty means the ask.model setting, then the light model",
    );
    fs.bool_var(
        "no-model",
        false,
        "answer with the matching facts only, without a model call",
    );
}

fn run(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let question = args.join(" ");
    if let Err(why) = crate::check_question(&question) {
        return Err(errorf(
            2,
            "Nothing to ask",
            &format!("{why}."),
            "pulse ask \"what waits for me?\"",
        )
        .into());
    }
    let s = c
        .store
        .as_ref()
        .ok_or_else(|| errorf(1, "No store", "The store did not open.", "pulse init"))?;
    let all = fs.get_bool("all");
    if all && fs.is_set("project") {
        return Err(errorf(
            2,
            "Pick one scope",
            "--all and --project exclude each other.",
            "pulse help ask",
        )
        .into());
    }
    let project = if all {
        String::new()
    } else {
        pulse_board::current_project(s, &c.project)?.slug
    };

    let mut note_for_stderr = String::new();
    let opened: Option<Box<dyn pulse_provider::Provider>> = if fs.get_bool("no-model") {
        None
    } else {
        let name = s.setting("provider", pulse_provider::DEFAULT);
        match pulse_provider::open(&name, pulse_harness::harness_provider_options(c, Some(s))) {
            Ok(p) => Some(p),
            Err(e) => {
                note_for_stderr = format!("the model provider {name:?} did not open: {e}");
                None
            }
        }
    };
    let req = AskRequest {
        project,
        all_projects: all,
        question,
        history: vec![],
        model: fs.get_string("model"),
        actor: "human:cli".into(),
    };
    let answer = crate::ask(s, opened.as_deref(), &req, DEFAULT_TIMEOUT)?;

    if c.json {
        return write_json(c.out.as_mut(), &answer);
    }
    if !note_for_stderr.is_empty() {
        let _ = writeln!(c.err, "note: {note_for_stderr}");
    }
    print_text(c.out.as_mut(), &answer, c.quiet)
}

fn print_text(out: &mut dyn Write, a: &AskAnswer, quiet: bool) -> Result<()> {
    writeln!(out, "{}", a.answer.trim_end())?;
    if quiet {
        return Ok(());
    }
    if !a.sources.is_empty() {
        writeln!(out, "\nSources:")?;
        for it in &a.sources {
            let day = it.date.get(..10).unwrap_or(&it.date);
            writeln!(
                out,
                "  {}  {}  {}  {}",
                it.id,
                day,
                it.kind,
                crate::context::clip(&it.summary, 100)
            )?;
        }
    }
    if a.facts_only {
        writeln!(
            out,
            "\n(facts only: {}; sign in with pulse harness login for a worded answer)",
            a.note
        )?;
    } else {
        writeln!(
            out,
            "\n({} via {}, {:.1} s)",
            a.model,
            a.provider,
            a.elapsed_ms as f64 / 1000.0
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_cli::Buffer;
    use pulse_store::{Options, Store};
    use std::path::Path;
    use tempfile::TempDir;

    const NOW: &str = "2026-10-13T06:00:00Z";

    fn set_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn clear_env(home: &Path) {
        for key in ["PULSE_JOB", "PULSE_AGENT", "PULSE_PROJECT", "PULSE_SESSION"] {
            set_env(key, "");
        }
        set_env("PULSE_HOME", home.to_str().unwrap());
    }

    fn command_store() -> (TempDir, Store) {
        let home = tempfile::tempdir().unwrap();
        clear_env(home.path());
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: NOW.into(),
            ..Options::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug,name,path,added_at,status) VALUES ('demo','Demo',?,?, 'active')",
            rusqlite::params![home.path().join("demo").display().to_string(), s.now()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO posts (id,project,area,thread,kind,agent,date,summary,body,created_at) VALUES ('20261012T100000Z-nimbus-1','demo','hub','hub/memory','finding','nimbus',?, 'The hub keeps the whole board in RAM','measured 2.1 GB',?)",
            [s.now(), s.now()],
        )
        .unwrap();
        drop(conn);
        (home, s)
    }

    fn sv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    /// Runs `pulse ask` the way the binary does, with the store already open.
    fn ask_command(s: &Store, project: &str, args: &[&str]) -> (String, String, Result<()>) {
        register();
        let cmd = pulse_cli::lookup("ask").expect("registered ask command");
        let out = Buffer::new();
        let err = Buffer::new();
        let mut c = Context {
            store: Some(s.clone()),
            home: s.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: project.into(),
            session: String::new(),
            now: NOW.into(),
            args: Vec::new(),
        };
        let mut fs = FlagSet::new("ask");
        if let Some(bind) = cmd.flags {
            bind(&mut fs);
        }
        fs.string_var("project", "", "project slug");
        fs.bool_var("json", false, "machine-readable output");
        let result = match pulse_cli::flags::parse_interleaved(&mut fs, &sv(args)) {
            Ok(positional) => {
                if fs.is_set("project") && !fs.get_string("project").is_empty() {
                    c.project = fs.get_string("project");
                }
                c.json = fs.get_bool("json");
                c.args = positional.clone();
                (cmd.run)(&mut c, &fs, &positional)
            }
            Err(e) => Err(e.into()),
        };
        (out.string(), err.string(), result)
    }

    #[test]
    fn registers_once_in_the_human_group() {
        register();
        register();
        let cmd = pulse_cli::lookup("ask").expect("ask registered");
        assert_eq!(cmd.group, "human");
        assert!(cmd.usage.contains("--no-model"));
    }

    #[test]
    fn text_output_prints_the_facts_and_sources_without_a_model() {
        let (_home, s) = command_store();
        let (out, _err, result) = ask_command(&s, "demo", &["--no-model", "memory", "in", "RAM?"]);
        result.unwrap();
        assert!(
            out.starts_with("No model answered (no model was asked)"),
            "{out}"
        );
        assert!(out.contains("Sources:\n  20261012T100000Z-nimbus-1  2026-10-13  finding  The hub keeps the whole board in RAM"), "{out}");
        assert!(out.contains("(facts only: no model was asked;"), "{out}");
    }

    #[test]
    fn json_output_is_the_answer_shape() {
        let (_home, s) = command_store();
        let (out, _err, result) = ask_command(&s, "", &["--all", "--no-model", "--json", "RAM"]);
        result.unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["facts_only"], true);
        assert_eq!(v["project"], "");
        assert_eq!(v["sources"][0]["id"], "20261012T100000Z-nimbus-1");
        assert_eq!(v["sources"][0]["source"], "post");
    }

    #[test]
    fn an_empty_question_and_mixed_scopes_are_refused() {
        let (_home, s) = command_store();
        let (_out, _err, result) = ask_command(&s, "demo", &["--no-model"]);
        let e = result.unwrap_err();
        assert!(e.to_string().contains("Nothing to ask"), "{e}");
        let (_out, _err, result) = ask_command(
            &s,
            "demo",
            &["--all", "--project", "demo", "--no-model", "x"],
        );
        let e = result.unwrap_err();
        assert!(e.to_string().contains("Pick one scope"), "{e}");
    }

    #[test]
    fn an_unknown_provider_still_answers_with_facts() {
        let (_home, s) = command_store();
        s.set_setting("human", "provider", "no-such-provider").unwrap_or_else(|_| {
            s.conn()
                .unwrap()
                .execute("INSERT OR REPLACE INTO settings (name,value,updated_at,updated_by) VALUES ('provider','no-such-provider',?, 'human')", [s.now()])
                .unwrap();
        });
        let (out, err, result) = ask_command(&s, "demo", &["RAM"]);
        result.unwrap();
        assert!(err.contains("did not open"), "{err}");
        assert!(out.contains("(facts only: no model was asked;"), "{out}");
    }
}
