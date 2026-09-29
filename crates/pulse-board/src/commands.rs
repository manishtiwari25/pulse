use crate::agents::*;
use crate::board::*;
use crate::export::export_store;
use crate::jobs::*;
use crate::posts::*;
use crate::projects::*;
use crate::remove::*;
use crate::threads::*;
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, register as cli_register, write_json};
use std::io::Write;
use std::sync::{Mutex, Once, OnceLock, RwLock};

pub type UninstallService = fn(&mut dyn Write) -> String;
pub static UNINSTALL_SERVICE: OnceLock<RwLock<Option<UninstallService>>> = OnceLock::new();
pub fn set_uninstall_service(f: Option<UninstallService>) {
    *UNINSTALL_SERVICE
        .get_or_init(|| RwLock::new(None))
        .write()
        .expect("uninstall service lock") = f;
}
static STDIN_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);
pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN_BUF.lock().expect("stdin lock") = data;
}
pub fn stdin_is_tty() -> bool {
    STDIN_BUF.lock().expect("stdin lock").is_none()
}

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

pub fn register() {
    register_commands();
}

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

fn run_agent(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    match args.first().map(String::as_str) {
        Some("join") => {
            let kind = fs.get_string("kind");
            if kind != "session" && kind != "worker" {
                return Err(usage("agent", "--kind is session or worker"));
            }
            if fs.get_string("name").is_empty() {
                return Err(usage("agent", "join needs --name NAME"));
            }
            let (row, created) = join_agent(
                s,
                JoinInput {
                    name: fs.get_string("name"),
                    runner: fs.get_string("runner"),
                    model: fs.get_string("model"),
                    session: c.session.clone(),
                    display: fs.get_string("display"),
                    kind,
                    ..Default::default()
                },
            )?;
            if c.json {
                write_json(
                    &mut c.out,
                    &pulse_store::gomap! {"agent": row, "created": created},
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
            } else {
                for r in rows {
                    let useful = if r.agent.last_useful_at.is_empty() {
                        "-"
                    } else {
                        &r.agent.last_useful_at
                    };
                    let beat = if r.presence.last_heartbeat_at.is_empty() {
                        "-"
                    } else {
                        &r.presence.last_heartbeat_at
                    };
                    writeln!(
                        c.out,
                        "{:<16} {:<7} {:<8} presence {:<7} posts {:<4} jobs {}/{}  last heartbeat {}  last useful {}  {}/{}",
                        r.agent.name,
                        r.agent.status,
                        r.agent.kind,
                        r.presence.presence,
                        r.agent.posts,
                        r.agent.jobs_done,
                        r.agent.jobs_done + r.agent.jobs_failed,
                        beat,
                        useful,
                        r.agent.runner,
                        r.agent.model
                    )?;
                }
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
            } else {
                for p in rows {
                    writeln!(
                        c.out,
                        "{} {}: {}",
                        if fs.get_bool("dry-run") {
                            "would prune"
                        } else {
                            "pruned"
                        },
                        p.name,
                        p.reason
                    )?;
                }
            }
        }
        Some(other) => {
            return Err(usage(
                "agent",
                &format!(
                    "unknown action {}; join, spawn, list or prune",
                    py_repr(other)
                ),
            ));
        }
        None => return Err(usage("agent", "join, spawn, list or prune")),
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
    if !contains(KINDS, &args[0]) {
        return Err(refuse(
            2,
            format!("Kind must be one of: {}", KINDS.join(", ")),
        ));
    }
    if !fs.get_string("vote").is_empty()
        && fs.get_string("vote") != "up"
        && fs.get_string("vote") != "down"
    {
        return Err(usage("post", "--vote is up or down"));
    }
    if !fs.get_string("resolution").is_empty()
        && !contains(RESOLUTIONS, &fs.get_string("resolution"))
    {
        return Err(refuse(
            2,
            format!("Resolution must be one of: {}", RESOLUTIONS.join(", ")),
        ));
    }
    for need in ["area", "thread", "summary"] {
        if fs.get_string(need).is_empty() {
            return Err(usage("post", "--area, --thread and --summary are required"));
        }
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
    for need in ["area", "thread", "summary"] {
        if fs.get_string(need).is_empty() {
            return Err(usage(
                "claim",
                "--area, --thread and --summary are required",
            ));
        }
    }
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
    if args.len() != 2 || (args[1] != "up" && args[1] != "down") {
        return Err(usage("vote", "pulse vote POST_ID up|down --summary REASON"));
    }
    if fs.get_string("summary").is_empty() {
        return Err(usage("vote", "--summary REASON is required"));
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
fn run_thread(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    match args.first().map(String::as_str) {
        Some("list") => {
            if args.len() != 1 {
                return Err(usage("thread", "list takes no positional arguments"));
            }
            let p = current_project(s, &c.project)?;
            let mut rows = thread_summaries(s, &p.slug, &c.now)?;
            let status = fs.get_string("status");
            if !status.is_empty() {
                if status != "open" && status != "stale" && status != "closed" {
                    return Err(usage("thread", "--status is open, stale, or closed"));
                }
                rows.retain(|r| r.status == status);
            }
            let area = fs.get_string("area");
            if !area.is_empty() {
                rows.retain(|r| r.area == slugify(&area));
            }
            if c.json {
                write_json(&mut c.out, &rows)?;
            } else {
                render_thread_list(&mut c.out, c.color, &rows)?;
            }
        }
        Some("show") => {
            if args.len() != 2 {
                return Err(usage("thread", "show needs AREA/THREAD"));
            }
            let p = current_project(s, &c.project)?;
            let (sum, posts) = thread_detail(s, &p.slug, &args[1], &c.now)?;
            if c.json {
                write_json(
                    &mut c.out,
                    &pulse_store::gomap! {"thread": sum, "posts": posts},
                )?;
            } else {
                render_thread_detail(&mut c.out, c.color, &sum, &posts, fs.get_bool("bodies"))?;
            }
        }
        Some("resolve") => {
            if args.len() != 2 {
                return Err(usage("thread", "resolve needs AREA/THREAD"));
            }
            if fs.get_string("as").is_empty() || fs.get_string("summary").is_empty() {
                return Err(usage(
                    "thread",
                    "resolve needs --as RESOLUTION and --summary TEXT",
                ));
            }
            let p = current_project(s, &c.project)?;
            let body = read_body(fs)?;
            let row = resolve_thread(
                s,
                ResolveInput {
                    project: p.slug,
                    key: args[1].clone(),
                    resolution: fs.get_string("as"),
                    summary: fs.get_string("summary"),
                    body,
                    agent: c.agent.clone(),
                    session: c.session.clone(),
                },
            )?;
            print_post(c, &row)?;
        }
        Some(other) => {
            return Err(usage(
                "thread",
                &format!("unknown action {}; list, show or resolve", py_repr(other)),
            ));
        }
        None => return Err(usage("thread", "list, show or resolve")),
    }
    Ok(())
}
fn run_decide(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 2 || (args[1] != "yes" && args[1] != "no") {
        return Err(usage(
            "decide",
            "pulse decide POST_ID yes|no [--words '...']",
        ));
    }
    if !c.agent.is_empty() && c.agent != pulse_store::HUMAN_AGENT {
        return Err(refuse_fix(
            2,
            "pulse decide is the human's call",
            "an agent proposes and votes; the owner answers in the hub or here without PULSE_AGENT",
            format!("pulse vote {} up|down --summary REASON", args[0]),
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
    } else {
        writeln!(c.out, "recorded {} as {}", d.answer, d.result.id)?;
        if let Some(j) = d.job {
            writeln!(c.out, "queued {}: {}", j.id, j.title)?;
        }
    }
    Ok(())
}
fn run_job(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?;
    match args.first().map(String::as_str) {
        Some("add") => {
            if fs.get_string("title").is_empty() {
                return Err(usage("job", "add needs --title"));
            }
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
                write_json(
                    &mut c.out,
                    &pulse_store::gomap! {"job": j, "created": created},
                )?
            } else if created {
                writeln!(c.out, "{}", j.id)?;
            } else {
                writeln!(c.out, "already queued: {}", j.id)?;
            }
        }
        Some("finish") => {
            if args.len() != 3 {
                return Err(usage(
                    "job",
                    "finish JOB_ID done|failed|cancelled|blocked|queued",
                ));
            }
            if !contains(JOB_END_STATES, &args[2]) {
                return Err(refuse(
                    2,
                    "A job ends done, failed, cancelled, blocked, or back to queued",
                ));
            }
            let agent = resolve_agent(s, &c.agent, false)?;
            let name = agent
                .as_ref()
                .map(|a| a.name.as_str())
                .unwrap_or(pulse_store::HUMAN_AGENT);
            let j = finish_job(
                s,
                &args[1],
                name,
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
            let agent = resolve_agent(s, &c.agent, false)?;
            let name = agent
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| pulse_store::HUMAN_AGENT.to_string());
            let mut brief = fs.get_string("brief");
            let result = fs.get_string("result");
            if !result.is_empty() {
                if !brief.is_empty() {
                    brief.push_str("\n\n");
                }
                brief.push_str(&result);
            }
            let r = retry_job(
                s,
                RetryInput {
                    job_id: args[1].clone(),
                    as_new: fs.get_bool("as-new"),
                    brief,
                    actor: name,
                },
            )?;
            if c.json {
                write_json(&mut c.out, &r)?;
            } else if r.outcome == "retried" {
                writeln!(
                    c.out,
                    "job {} queued again (retry {} of this chain)",
                    r.job.id, r.n
                )?;
            } else if r.reused {
                writeln!(
                    c.out,
                    "job {} already has open corrected job {} (retry {} of this chain)",
                    args[1], r.job.id, r.n
                )?;
            } else {
                writeln!(c.out, "job {} re-planned as {}", args[1], r.job.id)?;
            }
        }
        Some(other) => {
            return Err(usage(
                "job",
                &format!("unknown action {}; add, finish or retry", py_repr(other)),
            ));
        }
        None => return Err(usage("job", "add, finish or retry")),
    }
    Ok(())
}
fn run_init(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let mut path = fs.get_string("path");
    if args.len() > 1 {
        return Err(usage("init", "at most one PATH"));
    }
    if let Some(a) = args.first() {
        if !path.is_empty() && path != *a {
            return Err(usage("init", "PATH and --path disagree"));
        }
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
        let brain = if r.project.brain { " (brain)" } else { "" };
        writeln!(
            c.out,
            "{} project {} at {}{}",
            if r.created { "registered" } else { "refreshed" },
            r.project.slug,
            r.project.path,
            brain
        )?;
        let q = if r.job_queued {
            "queued".to_string()
        } else if !contains(&["queued", "leased", "blocked"], &r.job.status) {
            format!("already {}", r.job.status)
        } else {
            "already queued".to_string()
        };
        writeln!(c.out, "{q} onboarding job {}", r.job.id)?;
        if !r.environment_record.is_empty() {
            writeln!(
                c.out,
                "imported docs/context/environment.md as {}",
                r.environment_record
            )?;
        }
        writeln!(c.out, "store: {}", r.store)?;
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
    } else if rows.is_empty() {
        writeln!(c.out, "registry: nothing to import")?;
    } else {
        for i in rows {
            writeln!(
                c.out,
                "registry: {} {}",
                if i.created { "added" } else { "kept" },
                i.slug
            )?;
        }
    }
    Ok(())
}
fn run_remove(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = store(c)?.clone();
    if args.is_empty() {
        return Err(usage(
            "remove",
            "project SLUG, executor NAME, duplicate-findings, or service",
        ));
    }
    let mut dry = fs.get_bool("dry-run");
    let yes = fs.get_bool("yes");
    let actor_name = actor(c);
    let project_flag = fs.get_string("project");
    let run = |d: bool| -> Result<RemovePreview> {
        match args.first().map(String::as_str) {
            Some("project") => {
                if args.len() != 2 {
                    return Err(usage("remove", "project needs one SLUG"));
                }
                remove_project(&s, &args[1], &actor_name, d)
            }
            Some("executor") => {
                if args.len() != 2 {
                    return Err(usage("remove", "executor needs one NAME"));
                }
                remove_executor(&s, &args[1], &actor_name, d)
            }
            Some("duplicate-findings") => {
                if args.len() != 1 {
                    return Err(usage(
                        "remove",
                        "duplicate-findings takes no positional argument; use --project SLUG",
                    ));
                }
                super::remove_duplicate_findings(&s, &project_flag, &actor_name, d)
            }
            Some(other) => Err(usage(
                "remove",
                &format!(
                    "unknown target {}; project, executor, duplicate-findings or service",
                    py_repr(other)
                ),
            )),
            None => unreachable!(),
        }
    };
    if args[0] == "duplicate-findings" {
        dry = !fs.get_bool("apply") || dry;
    }
    if args[0] == "service" {
        if args.len() != 1 {
            return Err(usage("remove", "service takes no positional argument"));
        }
        if dry {
            writeln!(c.out, "would run: pulse service uninstall")?;
            return Ok(());
        }
        let uninstall = *UNINSTALL_SERVICE
            .get_or_init(|| RwLock::new(None))
            .read()
            .expect("uninstall service lock");
        let Some(f) = uninstall else {
            return Err(refuse_fix(
                2,
                "The service uninstaller is not linked into this binary",
                "internal/runner registers it",
                "pulse service uninstall",
            ));
        };
        f(&mut c.out);
        return Ok(());
    }
    if !dry && !yes && stdin_is_tty() {
        let st = c.style();
        let pre = run(true)?;
        pre.render(&mut c.out, st);
        write!(c.out, "{} [y/N] ", st.bold("Apply?"))?;
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let a = line.trim().to_ascii_lowercase();
        if a != "y" && a != "yes" {
            writeln!(c.out, "not applied")?;
            return Err(pulse_cli::Error {
                what: String::new(),
                why: String::new(),
                fix: String::new(),
                code: 1,
            }
            .into());
        }
        let pre = run(false)?;
        if c.json {
            write_json(&mut c.out, &pre)?;
        } else {
            writeln!(
                c.out,
                "{}  {}",
                st.green("applied"),
                st.dim(&format!(
                    "{} {}; the Yours lines above are still yours",
                    pre.kind, pre.name
                ))
            )?;
        }
        return Ok(());
    }
    let pre = run(dry)?;
    if c.json {
        write_json(&mut c.out, &pre)?
    } else {
        let st = c.style();
        pre.render(&mut c.out, st);
    }
    Ok(())
}

fn display_time(iso: &str) -> &str {
    if iso.is_empty() { "-" } else { iso }
}

fn claim_agents(claims: &[ThreadClaim]) -> String {
    if claims.is_empty() {
        "-".to_string()
    } else {
        claims
            .iter()
            .map(|c| c.agent.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn render_thread_list(
    w: &mut dyn Write,
    color: bool,
    rows: &[ThreadSummary],
) -> std::io::Result<()> {
    let table = rows
        .iter()
        .map(|row| {
            vec![
                row.key.clone(),
                row.status.clone(),
                row.posts.to_string(),
                display_time(&row.last_at).to_string(),
                row.last_by.clone(),
                row.open_questions.to_string(),
                claim_agents(&row.active_claims),
                trunc(&row.last_summary, 60),
            ]
        })
        .collect::<Vec<_>>();
    pulse_cli::table(
        w,
        color,
        Some(&[
            "KEY", "STATUS", "POSTS", "LAST", "BY", "Q", "CLAIMS", "SUMMARY",
        ]),
        &table,
    )
}

fn render_thread_detail(
    w: &mut dyn Write,
    color: bool,
    sum: &ThreadSummary,
    posts: &[Post],
    bodies: bool,
) -> std::io::Result<()> {
    writeln!(
        w,
        "{}  {}  {} posts  last {} by {}",
        sum.key,
        sum.status,
        sum.posts,
        display_time(&sum.last_at),
        sum.last_by
    )?;
    writeln!(
        w,
        "participants: {}\nquestions open: {}\nhandoffs open: {}\nclaims: {}\n",
        sum.participants.join(", "),
        sum.open_questions,
        sum.open_handoffs,
        claim_agents(&sum.active_claims)
    )?;
    let mut table = Vec::new();
    for p in posts {
        table.push(vec![
            p.date.clone(),
            p.kind.clone(),
            p.agent.clone(),
            p.id.clone(),
            trunc(&p.summary, 80),
        ]);
        if bodies && !p.body.trim().is_empty() {
            table.push(vec![
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                p.body.trim().to_string(),
            ]);
        }
    }
    pulse_cli::table(
        w,
        color,
        Some(&["DATE", "KIND", "AGENT", "ID", "SUMMARY"]),
        &table,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use std::io::{self, Write};

    fn make_failed_job(s: &pulse_store::Store, project: &str, source: &str) -> Job {
        let (job, _) = add_job(
            s,
            JobInput {
                project: project.into(),
                kind: "task".into(),
                title: "Retry me".into(),
                brief: "first try".into(),
                tier: 1,
                source: source.into(),
                created_by: "human".into(),
                ..Default::default()
            },
        )
        .expect("add job");
        finish_job(s, &job.id, "human", "failed", "nope", "", None).expect("fail job")
    }

    fn seed_posts(s: &pulse_store::Store) -> (Post, Post, Post) {
        let suggestion = create_post(
            s,
            PostInput {
                kind: "suggestion".into(),
                project: "demo".into(),
                area: "runner".into(),
                thread: "faster-tests".into(),
                summary: "Run less".into(),
                body: "Try it".into(),
                agent: "alice".into(),
                ..Default::default()
            },
        )
        .expect("suggestion");
        let amendment = create_post(
            s,
            PostInput {
                kind: "amendment".into(),
                project: "demo".into(),
                area: "runner".into(),
                thread: "faster-tests".into(),
                summary: "Change wording".into(),
                body: "## Current text\n```\nold\n```\n\n## Proposed text\n```\nnew\n```\n".into(),
                agent: "alice".into(),
                ..Default::default()
            },
        )
        .expect("amendment");
        let finding = create_post(
            s,
            PostInput {
                kind: "finding".into(),
                project: "demo".into(),
                area: "runner".into(),
                thread: "faster-tests".into(),
                summary: "Found it".into(),
                agent: "bob".into(),
                ..Default::default()
            },
        )
        .expect("finding");
        (suggestion, amendment, finding)
    }

    #[test]
    fn command_registers() {
        let _serial = crate::test_helpers::serial();
        register_commands();
        assert!(pulse_cli::lookup("post").is_some());
    }

    #[test]
    fn post_summary_flag_help() {
        let _serial = crate::test_helpers::serial();
        let mut fs = FlagSet::new("post");
        post_flags(&mut fs);
        let help = fs.lookup("summary").expect("summary flag").usage.clone();
        assert_eq!(
            help,
            format!("one line, at most {} characters", MAX_SUMMARY_CHARS)
        );
    }

    #[test]
    fn retry_command_output_and_json() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _home, s, p, _repo) = fixture();
        let job = make_failed_job(&s, &p.slug, "task:cli");
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "alice".into(),
                ..test_context()
            }),
            &["job", "retry", &job.id, "--brief", "again"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert_eq!(
            out.trim(),
            format!("job {} queued again (retry 1 of this chain)", job.id)
        );

        let job = make_failed_job(&s, &p.slug, "task:cli-copy");
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "alice".into(),
                ..test_context()
            }),
            &["job", "retry", &job.id, "--as-new"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(
            out.trim()
                .starts_with(&format!("job {} re-planned as job-", job.id)),
            "{out}"
        );
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "alice".into(),
                ..test_context()
            }),
            &["job", "retry", &job.id, "--as-new"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(out.contains("already has open corrected job"), "{out}");

        let job = make_failed_job(&s, &p.slug, "task:json");
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "alice".into(),
                json: true,
                ..test_context()
            }),
            &["job", "retry", &job.id],
        );
        assert!(err.is_ok(), "{err:?}");
        let result: serde_json::Value = serde_json::from_str(&out).expect("retry json");
        assert_eq!(result["outcome"], "retried");
        assert_eq!(result["root"], job.id);
        assert_eq!(result["job"]["id"], job.id);
    }

    #[test]
    fn vote_summary_warnings() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        let (suggestion, amendment, _finding) = seed_posts(&s);
        for command in ["vote", "post"] {
            for (name, summary, vote, amendment_target, json, warning) in [
                ("bare up", "up", "up", false, false, true),
                ("bare down", "down", "down", false, false, true),
                ("padded mixed case", " \tDoWn\n", "down", false, false, true),
                ("opposite direction", "up", "down", false, false, true),
                (
                    "explained up",
                    "up: fewer retries",
                    "up",
                    false,
                    false,
                    false,
                ),
                (
                    "explained down",
                    "down: no measured improvement",
                    "down",
                    false,
                    false,
                    false,
                ),
                ("one word reason", "Agreed", "up", false, false, false),
                ("word containing up", "backup", "up", false, false, false),
                (
                    "normalized explanation",
                    " \tup\nbecause fewer retries ",
                    "up",
                    false,
                    false,
                    false,
                ),
                ("amendment bare", "up", "up", true, false, true),
                (
                    "amendment explained",
                    "clarifies the rule",
                    "up",
                    true,
                    false,
                    false,
                ),
                ("JSON bare up", "up", "up", false, true, false),
                ("JSON bare down", "down", "down", false, true, false),
                ("JSON amendment bare", "down", "down", true, true, false),
            ] {
                let votes_before =
                    crate::test_helpers::count_rows(&s, "posts", "kind = 'vote'", &[]);
                let target = if amendment_target {
                    &amendment
                } else {
                    &suggestion
                };
                let body = if amendment_target {
                    "Read: use the new line.\nThe explanation in the body is not the displayed reason."
                } else {
                    "The explanation in the body is not the displayed reason."
                };
                let mut argv = if command == "vote" {
                    vec!["vote", &target.id, vote]
                } else {
                    vec![
                        "post",
                        "vote",
                        "--area",
                        &target.area,
                        "--thread",
                        &target.thread,
                        "--on",
                        &target.id,
                        "--vote",
                        vote,
                    ]
                };
                argv.extend([
                    "--summary",
                    summary,
                    "--body",
                    body,
                    "--text-hash",
                    &target.text_hash,
                ]);
                let diagnostics = pulse_cli::Buffer::new();
                let (out, err) = run_cmd(
                    &s,
                    Some(Context {
                        agent: "bob".into(),
                        project: "demo".into(),
                        json,
                        err: Box::new(diagnostics.clone()),
                        ..test_context()
                    }),
                    &argv,
                );
                assert!(err.is_ok(), "{command}/{name}: {err:?}");
                let id = if json {
                    serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
                        .as_str()
                        .unwrap()
                        .to_string()
                } else {
                    out.trim().to_string()
                };
                let row = post_by_id(&s, &id).unwrap().expect("stored vote");
                assert_eq!(row.kind, "vote");
                assert_eq!(row.vote, vote);
                assert_eq!(row.on, target.id);
                assert_eq!(
                    row.summary,
                    summary.split_whitespace().collect::<Vec<_>>().join(" ")
                );
                assert_eq!(row.body, body);
                assert_eq!(row.text_hash, target.text_hash);
                assert_eq!(
                    crate::test_helpers::count_rows(&s, "posts", "kind = 'vote'", &[]),
                    votes_before + 1
                );
                let want_warning = if warning {
                    format!(
                        "warning: vote {} has a bare up/down summary; include an explanatory phrase in --summary\n",
                        row.id
                    )
                } else {
                    String::new()
                };
                assert_eq!(diagnostics.string(), want_warning, "{command}/{name}");
            }
        }
    }

    #[test]
    fn non_vote_summary_warnings() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        for summary in ["up", "down"] {
            let diagnostics = pulse_cli::Buffer::new();
            let (out, err) = run_cmd(
                &s,
                Some(Context {
                    agent: "bob".into(),
                    project: "demo".into(),
                    err: Box::new(diagnostics.clone()),
                    ..test_context()
                }),
                &[
                    "post",
                    "finding",
                    "--area",
                    "runner",
                    "--thread",
                    "status",
                    "--summary",
                    summary,
                ],
            );
            assert!(err.is_ok(), "{err:?}");
            assert!(!out.is_empty());
            assert_eq!(diagnostics.string(), "");
        }
    }

    #[test]
    fn rejected_vote_summary_does_not_warn() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        let (target, _am, _fin) = seed_posts(&s);
        for command in ["vote", "post"] {
            let mut argv = if command == "vote" {
                vec!["vote", &target.id, "up"]
            } else {
                vec![
                    "post",
                    "vote",
                    "--area",
                    &target.area,
                    "--thread",
                    &target.thread,
                    "--on",
                    &target.id,
                    "--vote",
                    "up",
                ]
            };
            argv.extend(["--summary", "up"]);
            let diagnostics = pulse_cli::Buffer::new();
            let (out, err) = run_cmd(
                &s,
                Some(Context {
                    agent: "alice".into(),
                    project: "demo".into(),
                    err: Box::new(diagnostics.clone()),
                    ..test_context()
                }),
                &argv,
            );
            let err = err.unwrap_err();
            assert_eq!(code(Some(&err)), 2);
            assert_eq!(
                what_any(&err),
                format!("No self-votes: alice wrote {}", target.id)
            );
            assert_eq!(out, "");
            assert_eq!(diagnostics.string(), "");
            assert_eq!(
                crate::test_helpers::count_rows(&s, "posts", "kind = 'vote'", &[]),
                0
            );
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CmdOutcome {
        code: i32,
        stdout: String,
        stderr: String,
    }

    fn scenario_env(agent: &str, project: &str, session: &str) -> Vec<(&'static str, String)> {
        vec![
            ("PULSE_AGENT", agent.to_string()),
            ("PULSE_PROJECT", project.to_string()),
            ("PULSE_SESSION", session.to_string()),
        ]
    }

    fn run_go_command(
        bin: &std::path::Path,
        home: &std::path::Path,
        project: &std::path::Path,
        args: &[String],
        env: &[(&str, String)],
    ) -> CmdOutcome {
        let out = std::process::Command::new(bin)
            .args(args)
            .current_dir(project)
            .env("PULSE_HOME", home)
            .env("PULSE_NOW", pulse_parity::FIXED_NOW)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("PULSE_AGENT", "")
            .env("PULSE_PROJECT", "")
            .env("PULSE_SESSION", "")
            .envs(env.iter().map(|(k, v)| (*k, v)))
            .output()
            .expect("go oracle command");
        CmdOutcome {
            code: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    fn run_rust_command(
        home: &std::path::Path,
        project: &std::path::Path,
        args: &[String],
        env: &[(&str, String)],
    ) -> CmdOutcome {
        register();
        let Some(cmd) = pulse_cli::lookup(&args[0]) else {
            return CmdOutcome {
                code: 2,
                stdout: String::new(),
                stderr: format!("pulse: unknown command {:?}\n  fix: pulse help\n", args[0]),
            };
        };
        let mut fs = FlagSet::new(&args[0]);
        if let Some(flags) = cmd.flags {
            flags(&mut fs);
        }
        for (name, default, usage) in [
            ("project", "", "project slug"),
            ("agent", "", "the agent you act as"),
            ("session", "", "the session you act in"),
            ("now", "", "fixed UTC clock"),
        ] {
            if fs.lookup(name).is_none() {
                fs.string_var(name, default, usage);
            }
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
        let raw: Vec<String> = args[1..].to_vec();
        let positional = match pulse_cli::flags::parse_interleaved(&mut fs, &raw) {
            Ok(v) => v,
            Err(e) => {
                return CmdOutcome {
                    code: 2,
                    stdout: String::new(),
                    stderr: format!("{e}\n"),
                };
            }
        };
        let env_get = |key: &str| {
            env.iter()
                .find_map(|(k, v)| (*k == key).then(|| v.clone()))
                .unwrap_or_default()
        };
        let out = pulse_cli::Buffer::new();
        let err = pulse_cli::Buffer::new();
        let store = match pulse_store::Store::open(pulse_store::Options {
            home: home.display().to_string(),
            create: args[0] == "init",
            now: pulse_parity::FIXED_NOW.into(),
            ..Default::default()
        }) {
            Ok(s) => s,
            Err(e) => {
                return CmdOutcome {
                    code: 2,
                    stdout: String::new(),
                    stderr: format!("{e}\n"),
                };
            }
        };
        let mut ctx = Context {
            store: Some(store),
            home: home.display().to_string(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json: fs.get_bool("json"),
            quiet: fs.get_bool("quiet"),
            color: false,
            agent: env_get("PULSE_AGENT"),
            project: env_get("PULSE_PROJECT"),
            session: env_get("PULSE_SESSION"),
            now: pulse_parity::FIXED_NOW.into(),
            args: positional.clone(),
        };
        if fs.is_set("project") && !fs.get_string("project").is_empty() {
            ctx.project = fs.get_string("project");
        }
        if fs.is_set("agent") && !fs.get_string("agent").is_empty() {
            ctx.agent = fs.get_string("agent");
        }
        if fs.is_set("session") && !fs.get_string("session").is_empty() {
            ctx.session = fs.get_string("session");
        }
        if fs.is_set("now") && !fs.get_string("now").is_empty() {
            ctx.now = fs.get_string("now");
        }
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
        let _ = project;
        CmdOutcome {
            code,
            stdout: out.string(),
            stderr: err.string(),
        }
    }

    fn canonical_scenario_text(
        s: &str,
        home: &std::path::Path,
        project: &std::path::Path,
    ) -> String {
        static JOB_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static EVENTS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static RECORDS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let text = pulse_parity::normalize(s, home, project);
        let text = JOB_RE
            .get_or_init(|| regex::Regex::new(r"job-\d{8}T\d{6}Z-[0-9a-f]{6}").unwrap())
            .replace_all(&text, "<JOB>")
            .into_owned();
        let text = EVENTS_RE
            .get_or_init(|| regex::Regex::new(r#""label": "\d+ events""#).unwrap())
            .replace_all(&text, r#""label": "<EVENTS> events""#)
            .into_owned();
        RECORDS_RE
            .get_or_init(|| {
                regex::Regex::new(
                    r#"(?s),\n    \{\n      "label": "\d+ records",\n      "detail": "decisions, plans and guides in the store"\n    \}"#,
                )
                .unwrap()
            })
            .replace_all(&text, "")
            .into_owned()
    }

    fn canonical_scenario_dump(
        s: &str,
        home: &std::path::Path,
        project: &std::path::Path,
    ) -> String {
        static JOB_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static EVENT_SEQ_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let text = pulse_parity::normalize(s, home, project);
        let text = JOB_RE
            .get_or_init(|| regex::Regex::new(r"job-\d{8}T\d{6}Z-[0-9a-f]{6}").unwrap())
            .replace_all(&text, "<JOB>")
            .into_owned();
        let event_seq_re = EVENT_SEQ_RE.get_or_init(|| {
            regex::Regex::new(r#"^\d+ \| ("[^"]+" \| "[^"]+" \| "[^"]+" \| )"#).unwrap()
        });
        let mut out = Vec::new();
        let mut skip_record_section = false;
        for line in text.lines() {
            if line.starts_with("== ") {
                skip_record_section = line.starts_with("== records ")
                    || line.starts_with("== record_revisions ")
                    || line.starts_with("== evolution_");
            }
            if skip_record_section {
                continue;
            }
            if line.contains(r#" | "record.created" | "#)
                || line.contains(r#" | "records.framework.seeded" | "#)
                || line.contains(r#" | "evolution.records.seeded" | "#)
                || line.contains(r#" | "evolution.fixture.added" | "#)
                || line.contains(r#" | "evolution.fixture.seeded" | "#)
            {
                continue;
            }
            out.push(event_seq_re.replace(line, r#"<SEQ> | $1"#).into_owned());
        }
        out.join("\n")
    }

    #[test]
    fn oracle_command_scenarios() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let Some(oracle_bin) = pulse_parity::oracle() else {
            eprintln!("no Go oracle; skipping oracle_command_scenarios");
            return;
        };
        let project = git_repo();
        let go_home = tempfile::tempdir().unwrap();
        let rust_home = tempfile::tempdir().unwrap();
        let export_go = go_home.path().join("dump.sql");
        let export_rust = rust_home.path().join("dump.sql");
        let empty_env: Vec<(&'static str, String)> = vec![];
        let alice_env = scenario_env("alice", "demo", "sess-alice");

        let mut go_suggestion = String::new();
        let mut rust_suggestion = String::new();
        let mut go_job = String::new();
        let mut rust_job = String::new();
        type ScenarioEnv = Vec<(&'static str, String)>;
        type ScenarioStep = (Vec<String>, Vec<String>, ScenarioEnv);
        let mut steps: Vec<ScenarioStep> = vec![
            (
                vec![
                    "init".into(),
                    project.path().display().to_string(),
                    "--name".into(),
                    "demo".into(),
                ],
                vec![
                    "init".into(),
                    project.path().display().to_string(),
                    "--name".into(),
                    "demo".into(),
                ],
                empty_env.clone(),
            ),
            (
                vec![
                    "agent".into(),
                    "join".into(),
                    "--name".into(),
                    "alice".into(),
                    "--runner".into(),
                    "r1".into(),
                    "--model".into(),
                    "m1".into(),
                ],
                vec![
                    "agent".into(),
                    "join".into(),
                    "--name".into(),
                    "alice".into(),
                    "--runner".into(),
                    "r1".into(),
                    "--model".into(),
                    "m1".into(),
                ],
                scenario_env("", "", "sess-alice"),
            ),
            (
                vec![
                    "post".into(),
                    "suggestion".into(),
                    "--area".into(),
                    "a".into(),
                    "--thread".into(),
                    "t".into(),
                    "--summary".into(),
                    "S".into(),
                    "--body".into(),
                    "B".into(),
                ],
                vec![
                    "post".into(),
                    "suggestion".into(),
                    "--area".into(),
                    "a".into(),
                    "--thread".into(),
                    "t".into(),
                    "--summary".into(),
                    "S".into(),
                    "--body".into(),
                    "B".into(),
                ],
                alice_env.clone(),
            ),
        ];

        for (i, (go_args, rust_args, env)) in steps.drain(..).enumerate() {
            let go = run_go_command(&oracle_bin, go_home.path(), project.path(), &go_args, &env);
            let rust = run_rust_command(rust_home.path(), project.path(), &rust_args, &env);
            assert_eq!(
                canonical_scenario_text(&go.stdout, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stdout, rust_home.path(), project.path()),
                "stdout step {i}: {go_args:?}"
            );
            assert_eq!(
                canonical_scenario_text(&go.stderr, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stderr, rust_home.path(), project.path()),
                "stderr step {i}: {go_args:?}"
            );
            assert_eq!(go.code, rust.code, "exit step {i}: {go_args:?}");
            if i == 2 {
                go_suggestion = go.stdout.trim().to_string();
                rust_suggestion = rust.stdout.trim().to_string();
            }
        }

        let dynamic_steps = vec![
            (
                vec![
                    "post",
                    "question",
                    "--area",
                    "a",
                    "--thread",
                    "t",
                    "--summary",
                    "Q",
                    "--body",
                    "?",
                    "--on",
                    &go_suggestion,
                ],
                vec![
                    "post",
                    "question",
                    "--area",
                    "a",
                    "--thread",
                    "t",
                    "--summary",
                    "Q",
                    "--body",
                    "?",
                    "--on",
                    &rust_suggestion,
                ],
                alice_env.clone(),
            ),
            (
                vec!["vote", &go_suggestion, "up", "--summary", "why"],
                vec!["vote", &rust_suggestion, "up", "--summary", "why"],
                scenario_env("human", "demo", ""),
            ),
            (
                vec!["thread", "list", "--project", "demo", "--json"],
                vec!["thread", "list", "--project", "demo", "--json"],
                empty_env.clone(),
            ),
            (
                vec!["thread", "show", "a/t", "--project", "demo"],
                vec!["thread", "show", "a/t", "--project", "demo"],
                empty_env.clone(),
            ),
            (
                vec!["job", "add", "--title", "T", "--project", "demo"],
                vec!["job", "add", "--title", "T", "--project", "demo"],
                scenario_env("human", "", ""),
            ),
        ];
        for (i, (go_raw, rust_raw, env)) in dynamic_steps.into_iter().enumerate() {
            let go_args = go_raw.into_iter().map(str::to_string).collect::<Vec<_>>();
            let rust_args = rust_raw.into_iter().map(str::to_string).collect::<Vec<_>>();
            let go = run_go_command(&oracle_bin, go_home.path(), project.path(), &go_args, &env);
            let rust = run_rust_command(rust_home.path(), project.path(), &rust_args, &env);
            assert_eq!(
                canonical_scenario_text(&go.stdout, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stdout, rust_home.path(), project.path()),
                "stdout dynamic step {i}: {go_args:?}"
            );
            assert_eq!(
                canonical_scenario_text(&go.stderr, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stderr, rust_home.path(), project.path()),
                "stderr dynamic step {i}: {go_args:?}"
            );
            assert_eq!(go.code, rust.code, "exit dynamic step {i}: {go_args:?}");
            if go_args.starts_with(&["job".into(), "add".into()]) {
                go_job = go.stdout.trim().to_string();
                rust_job = rust.stdout.trim().to_string();
            }
        }

        for (go_raw, rust_raw, env) in [
            (
                vec!["job", "finish", &go_job, "done"],
                vec!["job", "finish", &rust_job, "done"],
                scenario_env("human", "", ""),
            ),
            (
                vec!["job", "retry", &go_job],
                vec!["job", "retry", &rust_job],
                scenario_env("human", "", ""),
            ),
            (
                vec!["decide", &go_suggestion, "yes"],
                vec!["decide", &rust_suggestion, "yes"],
                scenario_env("human", "demo", ""),
            ),
            (
                vec!["remove", "project", "demo", "--dry-run", "--json"],
                vec!["remove", "project", "demo", "--dry-run", "--json"],
                scenario_env("human", "", ""),
            ),
            (
                vec!["export", "--sql", &export_go.display().to_string()],
                vec!["export", "--sql", &export_rust.display().to_string()],
                empty_env.clone(),
            ),
            (
                vec![
                    "post",
                    "finding",
                    "--area",
                    "a",
                    "--thread",
                    "t",
                    "--summary",
                    "missing",
                    "--project",
                    "zzz",
                ],
                vec![
                    "post",
                    "finding",
                    "--area",
                    "a",
                    "--thread",
                    "t",
                    "--summary",
                    "missing",
                    "--project",
                    "zzz",
                ],
                alice_env.clone(),
            ),
            (
                vec![
                    "agent", "join", "--name", "alice", "--runner", "r2", "--model", "m2",
                ],
                vec![
                    "agent", "join", "--name", "alice", "--runner", "r2", "--model", "m2",
                ],
                scenario_env("", "", "sess-alice"),
            ),
        ] {
            let go_args = go_raw.into_iter().map(str::to_string).collect::<Vec<_>>();
            let rust_args = rust_raw.into_iter().map(str::to_string).collect::<Vec<_>>();
            let go = run_go_command(&oracle_bin, go_home.path(), project.path(), &go_args, &env);
            let rust = run_rust_command(rust_home.path(), project.path(), &rust_args, &env);
            assert_eq!(
                canonical_scenario_text(&go.stdout, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stdout, rust_home.path(), project.path()),
                "stdout final step: {go_args:?}"
            );
            assert_eq!(
                canonical_scenario_text(&go.stderr, go_home.path(), project.path()),
                canonical_scenario_text(&rust.stderr, rust_home.path(), project.path()),
                "stderr final step: {go_args:?}"
            );
            assert_eq!(go.code, rust.code, "exit final step: {go_args:?}");
        }

        let go_dump =
            pulse_parity::dump_store(&go_home.path().join(pulse_store::STORE_NAME)).unwrap();
        let rust_dump =
            pulse_parity::dump_store(&rust_home.path().join(pulse_store::STORE_NAME)).unwrap();
        let go_dump = canonical_scenario_dump(&go_dump, go_home.path(), project.path());
        let rust_dump = canonical_scenario_dump(&rust_dump, rust_home.path(), project.path());
        if go_dump != rust_dump {
            let (go_diff, rust_diff) = pulse_parity::first_difference(&go_dump, &rust_dump);
            panic!("store dump\n--- go\n{go_diff}\n+++ rust\n{rust_diff}");
        }
    }

    #[test]
    fn thread_cli_list_json_and_resolve() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        create_post(
            &s,
            PostInput {
                kind: "finding".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Bug found".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:00:00Z".into(),
                ..Default::default()
            },
        )
        .expect("post");
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                json: true,
                ..test_context()
            }),
            &["thread", "list", "--project", "demo"],
        );
        assert!(err.is_ok(), "{err:?}");
        let rows: serde_json::Value = serde_json::from_str(&out).expect("thread rows");
        assert_eq!(rows.as_array().expect("rows array").len(), 1);
        assert_eq!(rows[0]["key"], "api/rate-limits");
        assert_eq!(rows[0]["last_by"], "alice");

        let (out, err) = run_cmd(
            &s,
            Some(test_context()),
            &[
                "thread",
                "resolve",
                "api/rate-limits",
                "--project",
                "demo",
                "--agent",
                "alice",
                "--as",
                "answered",
                "--summary",
                "429s fixed; measured",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let id = out.trim();
        let conn = s.conn().expect("conn");
        let (kind, resolution): (String, String) = conn
            .query_row(
                "SELECT kind, resolution FROM posts WHERE id = ?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("resolved post");
        assert_eq!((kind.as_str(), resolution.as_str()), ("result", "answered"));
    }

    #[test]
    fn post_for_owner_cli() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        for (args, want) in [
            (vec!["--for-owner"], true),
            (vec![], false),
            (vec!["--for-owner=false"], false),
        ] {
            let before = count_rows(&s, "posts", "kind = 'question'", &[]);
            let mut argv = vec![
                "post",
                "question",
                "--area",
                "routing",
                "--thread",
                "owner-review",
                "--summary",
                "Owner question for @owner",
                "--body",
                "for_owner: true",
            ];
            argv.extend(args);
            let (out, err) = run_cmd(
                &s,
                Some(Context {
                    agent: "alice".into(),
                    project: "demo".into(),
                    json: true,
                    ..test_context()
                }),
                &argv,
            );
            assert!(err.is_ok(), "{err:?}");
            let p: serde_json::Value = serde_json::from_str(&out).expect("post json");
            assert_eq!(
                p.get("for_owner")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                want
            );
            let id = p["id"].as_str().expect("post id");
            let extra = p["extra"].as_str().expect("extra");
            let loaded = post_by_id(&s, id).expect("load post").expect("post exists");
            assert_eq!(loaded.for_owner, want);
            assert_eq!(loaded.extra, extra);
            assert_eq!(
                count_rows(&s, "posts", "kind = 'question'", &[]),
                before + 1
            );
        }
    }

    #[test]
    fn vote_command() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _home, s, _p, _repo) = fixture();
        let (sug, am, _) = seed_posts(&s);
        let (_out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "bob".into(),
                ..test_context()
            }),
            &["vote", &sug.id, "sideways", "--summary", "r"],
        );
        assert_eq!(code(err.as_ref().err()), 2);
        let (_out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "bob".into(),
                ..test_context()
            }),
            &["vote", "nope", "up", "--summary", "r"],
        );
        assert_eq!(what_any(err.as_ref().unwrap_err()), "No post 'nope'");
        let (_out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "alice".into(),
                ..test_context()
            }),
            &["vote", &sug.id, "up", "--summary", "r"],
        );
        assert_eq!(
            what_any(err.as_ref().unwrap_err()),
            format!("No self-votes: alice wrote {}", sug.id)
        );
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "bob".into(),
                ..test_context()
            }),
            &["vote", &sug.id, "down", "--summary", "too slow"],
        );
        assert!(err.is_ok(), "{err:?}");
        let v = post_by_id(&s, out.trim())
            .expect("load vote")
            .expect("vote exists");
        assert_eq!(v.kind, "vote");
        assert_eq!(v.vote, "down");
        assert_eq!(v.on, sug.id);
        assert_eq!(v.session, "sess-bob");
        let (_out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "bob".into(),
                ..test_context()
            }),
            &[
                "vote",
                &am.id,
                "up",
                "--summary",
                "r",
                "--text-hash",
                &am.text_hash,
            ],
        );
        assert!(what_any(err.as_ref().unwrap_err()).contains("`Read:` line"));
        let (out, err) = run_cmd(
            &s,
            Some(Context {
                agent: "bob".into(),
                ..test_context()
            }),
            &[
                "vote",
                &am.id,
                "up",
                "--summary",
                "r",
                "--text-hash",
                &am.text_hash,
                "--body",
                "Read: new line",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let v = post_by_id(&s, out.trim())
            .expect("load vote")
            .expect("vote exists");
        assert!(v.has_read_line);
        assert_eq!(v.text_hash, am.text_hash);
    }

    struct ErrorWriter(io::ErrorKind);

    impl Write for ErrorWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_context() -> Context {
        Context {
            store: None,
            home: String::new(),
            out: Box::new(pulse_cli::Buffer::new()),
            err: Box::new(pulse_cli::Buffer::new()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: TEST_NOW.into(),
            args: vec![],
        }
    }

    #[test]
    fn print_post_output_errors() {
        let _serial = crate::test_helpers::serial();
        let err = print_post(
            &mut Context {
                out: Box::new(ErrorWriter(io::ErrorKind::Other)),
                ..test_context()
            },
            &Post {
                id: "vote-id".into(),
                kind: "vote".into(),
                summary: "up".into(),
                ..Default::default()
            },
        )
        .expect_err("stdout write error");
        assert_eq!(
            err.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::Other)
        );
        let err = print_post(
            &mut Context {
                err: Box::new(ErrorWriter(io::ErrorKind::BrokenPipe)),
                ..test_context()
            },
            &Post {
                id: "vote-id".into(),
                kind: "vote".into(),
                summary: "up".into(),
                ..Default::default()
            },
        )
        .expect_err("stderr write error");
        assert_eq!(
            err.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::BrokenPipe)
        );
    }
}
