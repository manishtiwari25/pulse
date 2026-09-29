use crate::agents::*;
use crate::board::*;
use crate::export::export_store;
use crate::jobs::*;
use crate::posts::*;
use crate::projects::*;
use crate::remove::*;
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, register as cli_register, write_json};
use std::io::Write;
use std::sync::{Mutex, Once};

pub static mut UNINSTALL_SERVICE: Option<fn(&mut dyn Write) -> String> = None;
static STDIN_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);
pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN_BUF.lock().expect("stdin lock") = data;
}
pub fn stdin_is_tty() -> bool {
    STDIN_BUF.lock().expect("stdin lock").is_none()
}
pub use stdin_is_tty as StdinIsTTY;

static REGISTER: Once = Once::new();
pub fn register_commands() {
    REGISTER.call_once(|| {
        cli_register(Command { name:"agent".into(), group:"agents".into(), summary:"join with a unique name, spawn a worker, list, or prune".into(), usage:"pulse agent join --name NAME [--runner R] [--model M] [--session S] [--display D] [--kind session|worker]\n       pulse agent spawn [--runner R] [--model M]\n       pulse agent list [--all] [--json]\n       pulse agent prune [--days N] [--dry-run] [--now T]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_agent, flags:Some(agent_flags) });
        cli_register(Command { name:"post".into(), group:"agents".into(), summary:"write one immutable post".into(), usage:format!("pulse post KIND --area A --thread T --summary S [--body TEXT | --body-file F | < stdin] [--on POST] [--vote up|down]\n       [--expires T] [--resolution R] [--links L] [--files F] [--text-hash H] [--origin-project P] [--claim-hours H] [--for-owner] [--project SLUG]\n       kinds: {}", KINDS.join(", ")), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_post, flags:Some(post_flags) });
        cli_register(Command { name:"claim".into(), group:"agents".into(), summary:"shortcut: post a claim that expires after claim_hours".into(), usage:"pulse claim --area A --thread T --summary S [--hours H] [--project SLUG]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_claim, flags:Some(claim_flags) });
        cli_register(Command { name:"vote".into(), group:"agents".into(), summary:"shortcut: vote up or down on a suggestion or amendment".into(), usage:"pulse vote POST_ID up|down --summary REASON [--body TEXT] [--text-hash H]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_vote, flags:Some(vote_flags) });
        cli_register(Command { name:"thread".into(), group:"agents".into(), summary:"list, show, or resolve board threads".into(), usage:"pulse thread list --project SLUG [--status open|stale|closed] [--area A] [--json]\n       pulse thread show AREA/THREAD --project SLUG [--bodies]\n       pulse thread resolve AREA/THREAD --project SLUG --as RESOLUTION --summary TEXT [--body TEXT|--body-file PATH]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_thread, flags:Some(thread_flags) });
        cli_register(Command { name:"decide".into(), group:"human".into(), summary:"the human records yes or no on a proposal (a yes queues one job, WU-022)".into(), usage:"pulse decide POST_ID yes|no [--words '...']".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_decide, flags:Some(decide_flags) });
        cli_register(Command { name:"job".into(), group:"runner".into(), summary:"add, finish, or retry a job by hand".into(), usage:"pulse job add --title T [--kind K] [--brief B] [--tier 0|1] [--priority N] [--source S] [--project SLUG]\n       pulse job finish JOB_ID done|failed|cancelled|blocked|queued [--result TEXT] [--branch B]\n       pulse job retry JOB_ID [--as-new] [--brief TEXT] [--result TEXT]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_job, flags:Some(job_flags) });
        cli_register(Command { name:"init".into(), group:"human".into(), summary:"register a checkout as a project, queue its onboarding job, import its environment recipe".into(), usage:"pulse init [PATH] [--name N] [--remote URL] [--verify CMD] [--brain]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_init, flags:Some(init_flags) });
        cli_register(Command { name:"export".into(), group:"records".into(), summary:"copy the store to a .sql or .sqlite file outside every project".into(), usage:"pulse export --sql PATH | --db PATH [--force]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_export, flags:Some(export_flags) });
        cli_register(Command { name:"import".into(), group:"records".into(), summary:"register the projects of a JSON registry (the pre-store ~/.pulse/projects.json)".into(), usage:"pulse import --registry PATH".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_import, flags:Some(import_flags) });
        cli_register(Command { name:"remove".into(), group:"human".into(), summary:"deactivate a project, retire an executor, or remove duplicate system findings".into(), usage:"pulse remove project SLUG [--dry-run] [--yes] [--json]\n       pulse remove executor NAME [--dry-run] [--yes] [--json]   (the old spelling was: pulse executor remove NAME)\n       pulse remove duplicate-findings [--project SLUG] [--apply] [--yes] [--json]\n       pulse remove service                                     (the daemon's user service; today: pulse service uninstall)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_remove, flags:Some(remove_flags) });
    });
}
pub use register_commands as register;

fn agent_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "the agent's unique name (join)");
    fs.string_var("runner", "", "the runner the agent runs in (join, spawn)");
    fs.string_var("model", "", "the model behind the agent (join, spawn)");
    fs.string_var("display", "", "a display name (join)");
    fs.string_var("kind", "session", "session or worker (join)");
    fs.bool_var("all", false, "include pruned agents (list)");
    fs.int_var(
        "days",
        -1,
        "prune after this many days without useful output (default: setting prune_after_days)",
    );
    fs.bool_var("dry-run", false, "prune: report only");
}
pub fn post_flags(fs: &mut FlagSet) {
    fs.string_var("area", "", "the board area (lowercase words with dashes)");
    fs.string_var("thread", "", "the thread");
    fs.string_var(
        "summary",
        "",
        &format!("one line, at most {MAX_SUMMARY_CHARS} characters"),
    );
    fs.string_var("body", "", "the body text");
    fs.string_var("body-file", "", "read the body from a file");
    fs.string_var("on", "", "the post this answers");
    fs.string_var("vote", "", "up or down (votes only)");
    fs.string_var("expires", "", "UTC time like 2026-09-05T08:00:00Z");
    fs.string_var(
        "resolution",
        "",
        &format!("one of {}", RESOLUTIONS.join(", ")),
    );
    fs.string_var("links", "", "related links");
    fs.string_var("files", "", "files touched");
    fs.string_var("text-hash", "", "an amendment's text hash");
    fs.string_var("origin-project", "", "the project a commons post came from");
    fs.float64_var("claim-hours", 0.0, "for a claim: expiry in hours from now");
    fs.bool_var(
        "for-owner",
        false,
        "reserve this post for the owner, not a worker job",
    );
}
fn claim_flags(fs: &mut FlagSet) {
    fs.string_var("area", "", "the board area");
    fs.string_var("thread", "", "the thread");
    fs.string_var("summary", "", "one line");
    fs.float64_var(
        "hours",
        0.0,
        "expiry in hours from now (default: setting claim_hours)",
    );
}
fn vote_flags(fs: &mut FlagSet) {
    fs.string_var("summary", "", "the reason, one line");
    fs.string_var(
        "body",
        "",
        "the body (an amendment vote carries a `Read:` line)",
    );
    fs.string_var(
        "text-hash",
        "",
        "the amendment's text hash (see pulse hash)",
    );
}
fn thread_flags(fs: &mut FlagSet) {
    fs.string_var("status", "", "list only open, stale, or closed threads");
    fs.string_var("area", "", "list only one board area");
    fs.bool_var("bodies", false, "show post bodies");
    fs.string_var("as", "", &format!("resolution: {}", RESOLUTIONS.join(", ")));
    fs.string_var("summary", "", "one-line result summary");
    fs.string_var("body", "", "the result body");
    fs.string_var("body-file", "", "read the result body from a file");
}
fn decide_flags(fs: &mut FlagSet) {
    fs.string_var("words", "", "the words that go with the answer");
}
fn job_flags(fs: &mut FlagSet) {
    fs.string_var(
        "kind",
        "task",
        &format!("job kind: {}", JOB_KINDS.join(", ")),
    );
    fs.string_var("title", "", "one line (add)");
    fs.string_var(
        "brief",
        "",
        "what the worker reads (add); retry note (retry)",
    );
    fs.int_var("tier", 1, "0 or 1 (add)");
    fs.int_var("priority", 0, "higher runs first (add)");
    fs.string_var(
        "source",
        "",
        "what queued it; an open job with the same source is reused (add)",
    );
    fs.string_var(
        "result",
        "",
        "the outcome text (finish); retry note (retry)",
    );
    fs.string_var("branch", "", "the branch the work is on (finish)");
    fs.bool_var(
        "as-new",
        false,
        "retry by queuing one corrected copy (retry)",
    );
}
fn init_flags(fs: &mut FlagSet) {
    fs.string_var("path", "", "the checkout (default: the current directory)");
    fs.string_var("name", "", "the project's name (default: the folder name)");
    fs.string_var("remote", "", "the remote URL (default: remote.origin.url)");
    fs.string_var(
        "verify",
        "",
        "the project's own check command, e.g. 'npm test'",
    );
    fs.bool_var(
        "brain",
        false,
        "this checkout is the PULSE framework itself",
    );
}
fn export_flags(fs: &mut FlagSet) {
    fs.string_var("sql", "", "write a SQL dump here");
    fs.string_var("db", "", "write a SQLite copy here");
    fs.bool_var("force", false, "overwrite an existing file");
}
fn import_flags(fs: &mut FlagSet) {
    fs.string_var(
        "registry",
        "",
        "the JSON registry (default: PULSE_HOME/projects.json)",
    );
}
fn remove_flags(fs: &mut FlagSet) {
    fs.bool_var("dry-run", false, "show the preview and change nothing");
    fs.bool_var(
        "yes",
        false,
        "apply without the confirmation prompt (only asked on a terminal)",
    );
    fs.bool_var(
        "apply",
        false,
        "duplicate-findings: delete after preview/confirmation",
    );
    fs.string_var("project", "", "duplicate-findings: limit to one project");
}

fn usage(name: &str, what: &str) -> anyhow::Error {
    refuse_fix(
        2,
        format!("pulse {name}: {what}"),
        "",
        format!("pulse help {name}"),
    )
}
fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}
fn actor_or(c: &Context, def: &str) -> String {
    if c.agent.is_empty() {
        def.into()
    } else {
        c.agent.clone()
    }
}

pub fn current_project(s: &pulse_store::Store, explicit: &str) -> Result<pulse_store::Project> {
    let explicit = if explicit.is_empty() {
        std::env::var("PULSE_PROJECT").unwrap_or_default()
    } else {
        explicit.into()
    };
    if !explicit.is_empty() {
        if explicit == pulse_store::COMMONS_SLUG {
            return Ok(pulse_store::Project {
                slug: pulse_store::COMMONS_SLUG.into(),
                name: "the commons".into(),
                ..Default::default()
            });
        }
        return s.project(&explicit)?.ok_or_else(|| {
            refuse(
                2,
                format!(
                    "No project {} (pulse projects lists them)",
                    py_repr(&explicit)
                ),
            )
        });
    }
    s.project_for_dir(&pulse_store::cwd())?.ok_or_else(|| refuse(2, "The current directory is not inside a registered project; pass --project SLUG, set PULSE_PROJECT, or run `pulse init` here"))
}
pub use current_project as CurrentProject;

fn run_agent(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    match args.first().map(String::as_str) {
        Some("join") => {
            let (row, created) = join_agent(
                s,
                JoinInput {
                    name: fs.get_string("name"),
                    runner: fs.get_string("runner"),
                    model: fs.get_string("model"),
                    session: c.session.clone(),
                    display: fs.get_string("display"),
                    kind: fs.get_string("kind"),
                    ..Default::default()
                },
            )?;
            if c.json {
                write_json(
                    &mut c.out,
                    &serde_json::json!({"agent":row,"created":created}),
                )?
            } else {
                writeln!(
                    c.out,
                    "{} {} ({}/{}); export PULSE_AGENT={}",
                    if created { "joined" } else { "welcome back" },
                    row.name,
                    row.runner,
                    row.model,
                    row.name
                )?;
            }
        }
        Some("spawn") => {
            let row = spawn_agent(
                s,
                &fs.get_string("runner"),
                &fs.get_string("model"),
                "worker",
            )?;
            if c.json {
                write_json(&mut c.out, &row)?
            } else {
                writeln!(c.out, "{}", row.name)?;
            }
        }
        Some("list") => {
            let rows = scorecards(s, fs.get_bool("all"))?;
            if c.json {
                write_json(&mut c.out, &rows)?
            }
        }
        Some("prune") => {
            let rows = prune_agents(
                s,
                &c.now,
                fs.get_int("days") as i64,
                &actor_or(c, "daemon"),
                fs.get_bool("dry-run"),
            )?;
            if c.json {
                write_json(&mut c.out, &rows)?
            }
        }
        _ => return Err(usage("agent", "join, spawn, list or prune")),
    }
    Ok(())
}

pub fn read_body(fs: &FlagSet) -> Result<String> {
    if !fs.get_string("body-file").is_empty() {
        return std::fs::read_to_string(fs.get_string("body-file")).map_err(|e| {
            refuse_fix(
                2,
                format!("Cannot read --body-file {}", fs.get_string("body-file")),
                e.to_string(),
                "pass a readable file, or --body TEXT",
            )
        });
    }
    if fs.is_set("body") {
        return Ok(fs.get_string("body"));
    }
    if let Some(buf) = STDIN_BUF.lock().expect("stdin lock").clone() {
        return Ok(String::from_utf8_lossy(&buf).into());
    }
    Ok(String::new())
}
pub use read_body as ReadBody;
fn print_post(c: &mut Context, row: &Post) -> Result<()> {
    if c.json {
        write_json(&mut c.out, row)?
    } else {
        writeln!(c.out, "{}", row.id)?;
        if is_bare_vote_summary(&row.kind, &row.summary) {
            writeln!(
                c.err,
                "warning: vote {} has a bare up/down summary; include an explanatory phrase in --summary",
                row.id
            )?;
        }
    }
    Ok(())
}
fn run_post(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err(usage(
            "post",
            &format!("one positional KIND: {}", KINDS.join(", ")),
        ));
    }
    let s = store(c)?;
    let project = current_project(s, &c.project)?;
    let mut expires = fs.get_string("expires");
    if args[0] == "claim" && expires.is_empty() {
        expires = claim_expiry(s, fs.get_f64("claim-hours"))?;
    }
    let row = create_post(
        s,
        PostInput {
            kind: args[0].clone(),
            project: project.slug,
            area: fs.get_string("area"),
            thread: fs.get_string("thread"),
            summary: fs.get_string("summary"),
            body: read_body(fs)?,
            agent: c.agent.clone(),
            session: c.session.clone(),
            on: fs.get_string("on"),
            vote: fs.get_string("vote"),
            expires,
            resolution: fs.get_string("resolution"),
            links: fs.get_string("links"),
            files: fs.get_string("files"),
            text_hash: fs.get_string("text-hash"),
            origin_project: fs.get_string("origin-project"),
            for_owner: fs.get_bool("for-owner"),
            ..Default::default()
        },
    )?;
    print_post(c, &row)
}
fn run_claim(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = store(c)?;
    let p = current_project(s, &c.project)?;
    let row = create_post(
        s,
        PostInput {
            kind: "claim".into(),
            project: p.slug,
            area: fs.get_string("area"),
            thread: fs.get_string("thread"),
            summary: fs.get_string("summary"),
            agent: c.agent.clone(),
            session: c.session.clone(),
            expires: claim_expiry(s, fs.get_f64("hours"))?,
            ..Default::default()
        },
    )?;
    print_post(c, &row)
}
fn run_vote(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err(usage("vote", "pulse vote POST_ID up|down --summary REASON"));
    }
    let s = store(c)?;
    let target = post_by_id(s, &args[0])?
        .ok_or_else(|| refuse(2, format!("No post {}", py_repr(&args[0]))))?;
    let row = create_post(
        s,
        PostInput {
            kind: "vote".into(),
            project: target.project,
            area: target.area,
            thread: target.thread,
            summary: fs.get_string("summary"),
            body: fs.get_string("body"),
            agent: c.agent.clone(),
            session: c.session.clone(),
            on: args[0].clone(),
            vote: args[1].clone(),
            text_hash: fs.get_string("text-hash"),
            ..Default::default()
        },
    )?;
    print_post(c, &row)
}
fn run_thread(_c: &mut Context, _fs: &FlagSet, _args: &[String]) -> Result<()> {
    Ok(())
}
fn run_decide(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err(usage(
            "decide",
            "pulse decide POST_ID yes|no [--words '...']",
        ));
    }
    let d = decide(
        store(c)?,
        &args[0],
        &args[1],
        &fs.get_string("words"),
        pulse_store::HUMAN_AGENT,
    )?;
    if c.json {
        write_json(&mut c.out, &d)?;
    }
    Ok(())
}
fn run_job(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    match args.first().map(String::as_str) {
        Some("add") => {
            let p = current_project(s, &c.project)?;
            let (j, created) = add_job(
                s,
                JobInput {
                    project: p.slug,
                    kind: fs.get_string("kind"),
                    title: fs.get_string("title"),
                    brief: fs.get_string("brief"),
                    tier: fs.get_int("tier") as i64,
                    priority: fs.get_int("priority") as i64,
                    source: fs.get_string("source"),
                    created_by: c.agent.clone(),
                    ..Default::default()
                },
            )?;
            if c.json {
                write_json(&mut c.out, &serde_json::json!({"job":j,"created":created}))?
            } else {
                writeln!(c.out, "{}", j.id)?;
            }
        }
        Some("finish") => {
            if args.len() != 3 {
                return Err(usage(
                    "job",
                    "finish JOB_ID done|failed|cancelled|blocked|queued",
                ));
            }
            let j = finish_job(
                s,
                &args[1],
                &actor(c),
                &args[2],
                &fs.get_string("result"),
                &fs.get_string("branch"),
                None,
            )?;
            if c.json {
                write_json(&mut c.out, &j)?
            } else {
                writeln!(c.out, "{} {}", j.id, j.status)?;
            }
        }
        Some("retry") => {
            if args.len() != 2 {
                return Err(usage("job", "retry JOB_ID"));
            }
            let r = retry_job(
                s,
                RetryInput {
                    job_id: args[1].clone(),
                    as_new: fs.get_bool("as-new"),
                    brief: fs.get_string("brief"),
                    actor: actor(c),
                },
            )?;
            if c.json {
                write_json(&mut c.out, &r)?;
            }
        }
        _ => return Err(usage("job", "add, finish or retry")),
    }
    Ok(())
}
fn run_init(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let mut path = fs.get_string("path");
    if let Some(a) = args.first() {
        path = a.clone();
    }
    let r = init(
        store(c)?,
        &path,
        &fs.get_string("name"),
        &fs.get_string("remote"),
        fs.get_bool("brain"),
        &fs.get_string("verify"),
        &actor(c),
    )?;
    if c.json {
        write_json(&mut c.out, &r)?
    } else {
        writeln!(
            c.out,
            "{} project {} at {}",
            if r.created { "registered" } else { "refreshed" },
            r.project.slug,
            r.project.path
        )?;
    }
    Ok(())
}
fn run_export(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let p = export_store(
        store(c)?,
        &fs.get_string("sql"),
        &fs.get_string("db"),
        fs.get_bool("force"),
    )?;
    if c.json {
        write_json(&mut c.out, &serde_json::json!({"path":p}))?
    } else {
        writeln!(c.out, "{p}")?;
    }
    Ok(())
}
fn run_import(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let rows = import_registry(
        store(c)?,
        &fs.get_string("registry"),
        &actor_or(c, "migrate"),
    )?;
    if c.json {
        write_json(&mut c.out, &rows)?;
    }
    Ok(())
}
fn run_remove(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    let pre = match args.first().map(String::as_str) {
        Some("project") if args.len() == 2 => {
            remove_project(s, &args[1], &actor(c), fs.get_bool("dry-run"))?
        }
        Some("executor") if args.len() == 2 => {
            remove_executor(s, &args[1], &actor(c), fs.get_bool("dry-run"))?
        }
        Some("duplicate-findings") => remove_duplicate_findings(
            s,
            &fs.get_string("project"),
            &actor(c),
            !fs.get_bool("apply") || fs.get_bool("dry-run"),
        )?,
        _ => {
            return Err(usage(
                "remove",
                "project SLUG, executor NAME, duplicate-findings, or service",
            ));
        }
    };
    if c.json {
        write_json(&mut c.out, &pre)?
    } else {
        let st = c.style();
        pre.render(&mut c.out, st);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_registers() {
        register_commands();
        assert!(pulse_cli::lookup("post").is_some());
    }
}
