use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, errorf, register as cli_register, write_json};
use pulse_store::{HUMAN_AGENT, Options, Store};
use std::io::Write;
use std::sync::Once;

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        cli_register(Command {
            name: "goal".into(),
            group: "human".into(),
            summary: "read the shared project goal, set/reset it as the owner, or review how far the work has moved it".into(),
            usage: "pulse goal [show] [--project SLUG] [--json]\n       pulse goal set --text TEXT [--project SLUG] [--json]\n       pulse goal reset [--project SLUG] [--json]\n       pulse goal review [--project SLUG] [--json]   score the last work against the goal; the lead's reading and proposed goal, if any".into(),
            aliases: vec![],
            no_store: true,
            read_only: false,
            raw: false,
            run,
            flags: Some(flags),
        });
    });
}

fn flags(fs: &mut FlagSet) {
    fs.string_var("text", "", "owner goal (1..4000 Unicode code points)");
}

fn run(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let mut sub = "show";
    if args.len() == 1 {
        sub = &args[0];
    }
    let text_given = fs.is_set("text");
    if args.len() > 1
        || !matches!(sub, "show" | "set" | "reset" | "review")
        || (text_given != (sub == "set"))
    {
        return Err(errorf(
            2,
            "Invalid goal command",
            "Use show, set --text TEXT, reset, or review.",
            "pulse help goal",
        )
        .into());
    }
    let reads = matches!(sub, "show" | "review");

    let mut actor = c.agent.clone();
    if actor.is_empty() {
        actor = std::env::var("PULSE_AGENT").unwrap_or_default();
    }
    if actor.is_empty() {
        actor = HUMAN_AGENT.to_string();
    }

    let mut home = c.home.clone();
    if let Some(s) = c.store.as_ref() {
        home = s.home.clone();
    }
    if !reads {
        pulse_goals::require_owner(&home, &actor)?;
    }

    let opened;
    let s = if let Some(s) = c.store.as_ref() {
        s
    } else {
        opened = Store::open(Options {
            home,
            read_only: reads,
            skip_records: true,
            now: c.now.clone(),
            ..Options::default()
        })?;
        &opened
    };

    let p = pulse_board::current_project(s, &c.project)?;
    if sub == "review" {
        let review = pulse_goals::review::review(s, &p.slug)?;
        if c.json {
            return write_json(c.out.as_mut(), &review);
        }
        if c.quiet {
            return Ok(());
        }
        write!(c.out, "{}", pulse_goals::review::render(&review))?;
        return Ok(());
    }
    let goal = match sub {
        "show" => pulse_goals::get(s, &p.slug)?,
        "set" => pulse_goals::set(s, &p.slug, &fs.get_string("text"), &actor)?,
        "reset" => pulse_goals::reset(s, &p.slug, &actor)?,
        _ => unreachable!(),
    };

    if c.json {
        return write_json(
            c.out.as_mut(),
            &pulse_goals::View {
                project: p.slug.clone(),
                goal,
                can_edit: pulse_goals::can_edit(s, &p, &actor),
            },
        );
    }
    if c.quiet {
        return Ok(());
    }
    write!(c.out, "{}", pulse_goals::context(&goal))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_cli::Buffer;
    use rusqlite::params;
    use std::path::Path;
    use tempfile::TempDir;

    const NOW: &str = "2026-09-13T06:00:00Z";

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
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,added_at) VALUES ('demo','Demo',?,?)",
                params![home.path().join("missing").display().to_string(), s.now()],
            )
            .unwrap();
        (home, s)
    }

    fn sv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn context(home: impl Into<String>, project: impl Into<String>, json: bool) -> Context {
        Context {
            store: None,
            home: home.into(),
            out: Box::new(Buffer::new()),
            err: Box::new(Buffer::new()),
            json,
            quiet: false,
            color: false,
            agent: String::new(),
            project: project.into(),
            session: String::new(),
            now: NOW.into(),
            args: Vec::new(),
        }
    }

    fn bind_late(fs: &mut FlagSet) {
        if fs.lookup("project").is_none() {
            fs.string_var("project", "", "project slug");
        }
        if fs.lookup("agent").is_none() {
            fs.string_var("agent", "", "the agent you act as");
        }
        if fs.lookup("session").is_none() {
            fs.string_var("session", "", "the session you act in");
        }
        if fs.lookup("now").is_none() {
            fs.string_var("now", "", "fixed UTC clock");
        }
        if fs.lookup("json").is_none() {
            fs.bool_var("json", false, "machine-readable output");
        }
        if fs.lookup("quiet").is_none() {
            fs.bool_var("quiet", false, "print only what a script needs");
        }
        if fs.lookup("no-color").is_none() {
            fs.bool_var("no-color", false, "no colour");
        }
    }

    fn apply_late(fs: &FlagSet, c: &mut Context) {
        if fs.is_set("project") && !fs.get_string("project").is_empty() {
            c.project = fs.get_string("project");
        }
        if fs.is_set("agent") && !fs.get_string("agent").is_empty() {
            c.agent = fs.get_string("agent");
        }
        if fs.is_set("session") && !fs.get_string("session").is_empty() {
            c.session = fs.get_string("session");
        }
        if fs.is_set("now") && !fs.get_string("now").is_empty() {
            c.now = fs.get_string("now");
        }
        if fs.get_bool("json") {
            c.json = true;
        }
        if fs.get_bool("quiet") {
            c.quiet = true;
        }
        if fs.get_bool("no-color") {
            c.color = false;
        }
    }

    fn goal_command(c: &mut Context, args: &[&str]) -> (String, Result<()>) {
        register();
        let cmd = pulse_cli::lookup("goal").expect("registered goal command");
        assert!(cmd.no_store, "goal must choose a read-only store for reads");
        let out = Buffer::new();
        c.out = Box::new(out.clone());
        c.err = Box::new(Buffer::new());
        let mut fs = FlagSet::new("goal");
        if let Some(bind) = cmd.flags {
            bind(&mut fs);
        }
        bind_late(&mut fs);
        let parsed = pulse_cli::flags::parse_interleaved(&mut fs, &sv(args));
        let err = match parsed {
            Ok(positional) => {
                apply_late(&fs, c);
                c.args = positional.clone();
                (cmd.run)(c, &fs, &positional)
            }
            Err(e) => Err(e.into()),
        };
        (out.string(), err)
    }

    fn json_goal(out: &str) -> pulse_goals::View {
        serde_json::from_str(out).unwrap()
    }

    #[test]
    fn goal_command_canonical_slug() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (_home, s) = command_store();
        let slug = "customer-support-ticket-management-syste";
        assert_eq!(slug.len(), 40);
        s.conn()
            .unwrap()
            .execute("UPDATE projects SET slug = ? WHERE slug = 'demo'", [slug])
            .unwrap();

        let mut ctx = context(s.home.clone(), slug, true);
        let (out, err) = goal_command(&mut ctx, &["show"]);
        assert!(
            err.is_ok(),
            "CLI cannot read a canonical registered slug: {:?}",
            err
        );
        let got = json_goal(&out);
        assert_eq!(got.project, slug);
        assert_eq!(got.goal.source, pulse_goals::GENERATED);

        let want = "Make /api/search return correct ticket results.";
        let (_, err) = goal_command(&mut ctx, &["--text", want, "set"]);
        assert!(err.is_ok(), "{err:?}");
        let (out, err) = goal_command(&mut ctx, &["show"]);
        assert!(err.is_ok(), "{err:?}");
        let got = json_goal(&out);
        assert_eq!(got.project, slug);
        assert_eq!(got.goal.source, pulse_goals::OWNER);
        assert_eq!(got.goal.text, want);
    }

    #[test]
    fn goal_command_read_set_reset_and_reopen() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (_home, s) = command_store();
        let mut ctx = context(s.home.clone(), "demo", true);
        let (out, err) = goal_command(&mut ctx, &[]);
        assert!(err.is_ok(), "{err:?}");
        let initial = json_goal(&out);
        assert_eq!(initial.project, "demo");
        assert!(initial.can_edit);
        assert_eq!(initial.goal.source, pulse_goals::GENERATED);

        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "read command persisted a legacy fallback");

        let (out, err) = goal_command(
            &mut ctx,
            &["--text", "  Make search useful and correct.  ", "set"],
        );
        assert!(err.is_ok(), "{err:?}");
        let saved = json_goal(&out);
        assert_eq!(saved.goal.text, "Make search useful and correct.");
        assert_eq!(saved.goal.source, pulse_goals::OWNER);
        assert_eq!(saved.goal.updated_by, HUMAN_AGENT);

        let (out, err) = goal_command(&mut ctx, &["show"]);
        assert!(
            err.is_ok() && out.contains(&saved.goal.text),
            "reopened command lost goal: {out}, {err:?}"
        );

        let (out, err) = goal_command(&mut ctx, &["reset"]);
        assert!(err.is_ok(), "{err:?}");
        let reset = json_goal(&out);
        assert_eq!(reset.goal.text, initial.goal.text);
        assert_eq!(reset.goal.source, pulse_goals::GENERATED);
        assert_eq!(reset.goal.updated_by, HUMAN_AGENT);
    }

    #[test]
    fn goal_command_rejects_workers_and_invalid_modes() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (_home, s) = command_store();
        for actor in ["aurora", "demo-lead"] {
            for args in [vec!["--text", "Overwrite direction.", "set"], vec!["reset"]] {
                let mut ctx = context("", "demo", false);
                ctx.store = Some(s.clone());
                ctx.agent = actor.into();
                let (_, err) = goal_command(&mut ctx, &args);
                assert!(err.is_err(), "agent {actor} could mutate a goal");
            }
        }
        for args in [
            vec!["set"],
            vec!["--text", "", "set"],
            vec!["--text", "Ignored text.", "reset"],
            vec!["--text", "Ignored text.", "show"],
            vec!["set", "reset"],
            vec!["unknown"],
        ] {
            let mut ctx = context("", "demo", false);
            ctx.store = Some(s.clone());
            let (_, err) = goal_command(&mut ctx, &args);
            assert!(err.is_err(), "invalid command accepted: {args:?}");
        }

        set_env("PULSE_AGENT", "aurora");
        let home = tempfile::tempdir().unwrap();
        let mut ctx = context(home.path().display().to_string(), "demo", false);
        ctx.agent = "human".into();
        let (_, err) = goal_command(&mut ctx, &["--text", "Impersonate owner.", "set"]);
        assert!(
            err.as_ref()
                .is_err_and(|e| e.to_string().contains("Only the owner")),
            "CLI did not guard impersonation before opening the store: {err:?}"
        );

        set_env("PULSE_AGENT", "");
        set_env("PULSE_JOB", "job-worker");
        let (_, err) = goal_command(&mut ctx, &["reset"]);
        assert!(
            err.as_ref()
                .is_err_and(|e| e.to_string().contains("Only the owner")),
            "worker job impersonated owner: {err:?}"
        );
        assert!(
            !home.path().join(pulse_store::STORE_NAME).exists(),
            "rejected command created a store"
        );
    }

    #[test]
    fn goal_command_review_reads_without_the_owner() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (_home, s) = command_store();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO tasks (id, project, title, priority, status, created_by, created_at, updated_at) VALUES ('TASK-001', 'demo', 'Ship search', 3, 'done', 'human', ?, ?)",
                params![NOW, NOW],
            )
            .unwrap();
        set_env("PULSE_AGENT", "reader");
        let mut ctx = context(s.home.clone(), "demo", true);
        let (out, err) = goal_command(&mut ctx, &["review"]);
        assert!(err.is_ok(), "{err:?}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["project"], "demo");
        assert_eq!(v["review_days"], 7);
        assert!(
            v["signals"].as_array().is_some_and(|a| a.len() == 4),
            "{out}"
        );
        assert!(v["verdict"].is_string());

        let mut ctx = context(s.home.clone(), "demo", false);
        let (out, err) = goal_command(&mut ctx, &["review"]);
        assert!(err.is_ok(), "{err:?}");
        assert!(
            out.contains("Goal review") && out.contains("Delivery"),
            "{out}"
        );
        let (_, err) = goal_command(&mut ctx, &["--text", "x", "review"]);
        assert!(err.is_err(), "review took --text");
    }

    #[test]
    fn goal_command_project_selection_and_read_only_missing_store() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (_home, s) = command_store();
        set_env("PULSE_PROJECT", "demo");
        set_env("PULSE_AGENT", "reader");
        let mut ctx = context(s.home.clone(), "", true);
        let (out, err) = goal_command(&mut ctx, &[]);
        assert!(
            err.is_ok()
                && out.contains("\"project\": \"demo\"")
                && out.contains("\"can_edit\": false"),
            "project-scoped agent read: {out}, {err:?}"
        );

        let home = tempfile::tempdir().unwrap();
        let mut ctx = context(home.path().display().to_string(), "", false);
        let (_, err) = goal_command(&mut ctx, &["show"]);
        assert!(err.is_err(), "missing store read succeeded");
        assert!(
            !home.path().join(pulse_store::STORE_NAME).exists(),
            "goal read created a store"
        );
    }
}
