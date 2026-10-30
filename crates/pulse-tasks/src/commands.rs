use crate::compute::*;
use crate::manifest;
use crate::messages::*;
use crate::schedules;
use crate::suggestions;
use crate::tasks::*;
use anyhow::{Result, anyhow, bail};
use pulse_bus::{CancellationToken, Event, Filter};
use pulse_cli::{Command, Context, FlagSet};
use serde::Serialize;
use std::io::{IsTerminal, Read, Write};
use std::sync::{Mutex, Once};
use std::time::Duration;

static REGISTER: Once = Once::new();
static STDIN_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);

#[derive(Serialize)]
struct TaskOut<'a> {
    task: &'a Task,
}

#[derive(Serialize)]
struct ShowOut<'a> {
    jobs: &'a [Job],
    messages: &'a [Message],
    revisions: &'a [Revision],
    task: &'a Task,
}

#[derive(Serialize)]
struct TasksOut<'a> {
    tasks: &'a [Task],
}

#[derive(Serialize)]
struct SuggestionOut<'a> {
    suggestion: &'a suggestions::Suggestion,
}

#[derive(Serialize)]
struct SuggestionsOut<'a> {
    suggestions: &'a [suggestions::Suggestion],
}

#[derive(Serialize)]
struct ReviewOut<'a> {
    suggestion: &'a suggestions::Suggestion,
    task: Option<&'a Task>,
}

#[derive(Serialize)]
struct MessageOut<'a> {
    message: &'a Message,
}

#[derive(Serialize)]
struct MessagesOut<'a> {
    messages: &'a [Message],
}

#[derive(Serialize)]
struct MarkedOut {
    marked: i32,
}

#[derive(Serialize)]
struct RequestOut<'a> {
    request: &'a ComputeRequest,
}

#[derive(Serialize)]
struct ComputeListOut<'a> {
    caps: &'a Caps,
    requests: &'a [ComputeRequest],
}

#[derive(Serialize)]
struct ManifestCheckOut<'a> {
    path: &'a str,
    schedules: Vec<&'a str>,
    problems: &'a [String],
}

#[derive(Serialize)]
struct ManifestShowOut<'a> {
    project: &'a str,
    status: Option<&'a manifest::Status>,
    schedules: &'a [schedules::Schedule],
    skills: &'a [pulse_store::Skill],
}

#[derive(Serialize)]
struct ManifestSyncOut<'a> {
    synced: &'a manifest::Synced,
}

pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN_BUF.lock().expect("stdin lock poisoned") = data;
}

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command { name:"task".into(), group:"agents".into(), summary:"the owner's tasks for a project: new, list, show, amend, cancel, status, reply; and suggested work any agent proposes and another reviews: suggest, suggestions, review, withdraw".into(), usage:"pulse task new --project P --title T [--brief-file F | --brief TEXT | stdin] [--priority N]\npulse task list [--project P] [--status S] [--json]\npulse task show TASK-NNN|SUG-NNN [--json]\npulse task amend TASK-NNN [--title T] [--brief-file F | --brief TEXT | stdin] [--note WHY]\npulse task cancel TASK-NNN [--reason WHY]\npulse task status TASK-NNN <open|planned|in_progress|review|done|cancelled>\npulse task reply TASK-NNN <text> | --file F [--choice LABEL[=reply|done|amend]]...\npulse task suggest --project P --title T [--brief-file F | --brief TEXT | stdin] [--priority N]   (any agent; another one reviews it)\npulse task suggestions [--project P] [--status open|suggested|reviewing|accepted|declined|owner|withdrawn] [--json]\npulse task review SUG-NNN <accept|decline|owner> --reason WHY [--title T] [--brief-file F | --brief TEXT] [--priority N]   (not your own)\npulse task withdraw SUG-NNN [--reason WHY]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_task, flags:Some(task_flags) });
        pulse_cli::register(Command { name:"msg".into(), group:"agents".into(), summary:"the conversation between the owner and a project's lead: send, list, read".into(), usage:"pulse msg send --project P [--task TASK-NNN] <text> | --file F [--choice LABEL[=reply|done|amend]]...\npulse msg list [--project P] [--task TASK-NNN] [--since MSG-ID] [--unread] [--json]\npulse msg read MSG-ID... (mark read for --agent)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_msg, flags:Some(msg_flags) });
        pulse_cli::register(Command { name:"compute".into(), group:"agents".into(), summary:"more machines for agents: request (agents), list, approve|deny (the owner)".into(), usage:"pulse compute request --project P [--size S] --hours N --reason WHY\npulse compute list [--project P] [--status S] [--json]\npulse compute approve CR-NNN [--words WHY]   (owner)\npulse compute deny CR-NNN [--words WHY]      (owner)\npulse compute providers [--json]             (the providers this binary can provision with)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_compute, flags:Some(compute_flags) });
        pulse_cli::register(Command { name:"schedule".into(), group:"agents".into(), summary:"a standing job for a project - queued again every cadence - or a check/watch script the daemon runs without an LLM: add, list, show, log, pause, resume, end, run".into(), usage:"pulse schedule add --project P --title T --every 30m|6h|1d|2w [--at HH:MM] [--kind task|improve|clean|reflect] [--brief TEXT | --brief-file F | stdin] [--tier 0|1] [--priority N] [--times N] [--run-as AGENT] [--skill NAME]\npulse schedule add --project P --title T --every ... [--at HH:MM] --kind check|watch --script REL_PATH [--job-kind task|improve] [--brief TEXT | --brief-file F] [--tier 0|1] [--priority N] [--times N] [--run-as AGENT]\n    check: the script (committed on the default branch) exits 0 = green; anything else = red, one job to make it pass\n    watch: exit 0 and print nothing = nothing to do; print something = one job to handle it; non-zero = the watcher is broken\npulse schedule list [--project P] [--status active|paused|ended] [--json]\npulse schedule show SCHED-NNN [--json]\npulse schedule log SCHED-NNN                (the last output of a check or watch script)\npulse schedule pause SCHED-NNN [--note WHY]\npulse schedule resume SCHED-NNN\npulse schedule end SCHED-NNN [--note WHY]\npulse schedule run SCHED-NNN               (due now; the daemon queues its job, or runs its script, on the next tick)\n    --skill NAME: every job loads that pulse skill first and follows it; with no --brief the skill is the brief\n    a project's own .pulse/pulse.yaml keeps schedules too (pulse manifest); those show their origin and the file, not this command, changes them".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_schedule, flags:Some(schedule_flags) });
        pulse_cli::register(Command { name:"manifest".into(), group:"agents".into(), summary:"the project's own PULSE file, .pulse/pulse.yaml: the schedules it keeps and the SKILL.md folders the daemon loads from the default branch: check, show, sync".into(), usage:"pulse manifest check [PATH] [--project P] [--json]   (a working copy's file; PATH defaults to .pulse/pulse.yaml here; --project adds the store's checks: skills, agents, scripts)\npulse manifest show --project P [--json]            (what the daemon last applied: the commit, its problems, the schedules and skills the repository keeps)\npulse manifest sync --project P [--json]            (apply the default branch now instead of waiting for the daemon's next pass)\n    the file: schedules: [{name, title, every, at, kind, brief, skill, script, job_kind, tier, priority, run_as, times, paused}]\n    skills: every <dir>/<name>/SKILL.md under .pulse/skills, .claude/skills, .github/skills or .agents/skills on the default branch\n    exit 1 when check finds a problem; check without --project needs no store, so it runs on any machine".into(), aliases:vec![], no_store:true, read_only:false, raw:false, run:run_manifest, flags:None });
        pulse_cli::register(Command { name:"watch".into(), group:"agents".into(), summary:"stream store events as they happen (Ctrl-C exits 0)".into(), usage:"pulse watch [--project P] [--area A] [--thread T] [--action GLOB] [--since ID] [--detail] [--interval 1s] [--json]".into(), aliases:vec![], no_store:false, read_only:true, raw:false, run:run_watch, flags:Some(watch_flags) });
    });
}

fn schedule_flags(fs: &mut FlagSet) {
    fs.string_var("title", "", "the title of every job it queues (add)");
    fs.string_var(
        "every",
        "",
        "the cadence: 30m, 6h, 1d, 2w, hourly, daily, weekly (add)",
    );
    fs.string_var("at", "", "HH:MM UTC; anchors a cadence of whole days (add)");
    fs.string_var(
        "kind",
        "task",
        "task, improve, clean or reflect (a job every cadence); check or watch (a script, --script) (add)",
    );
    fs.string_var(
        "script",
        "",
        "check/watch: the script to run, relative to the repo root, committed on the default branch (add)",
    );
    fs.string_var(
        "job-kind",
        "",
        "check/watch: the kind of job a red check or a firing watch queues, task (default) or improve (add)",
    );
    fs.string_var(
        "brief",
        "",
        "the brief of every job (add); or --brief-file, or stdin; optional for check/watch",
    );
    fs.string_var("brief-file", "", "file holding the brief; - for stdin");
    fs.int_var("tier", 1, "0 or 1 (add)");
    fs.int_var("priority", 0, "priority, higher first (add)");
    fs.int_var("times", 0, "end after this many jobs; 0 keeps going (add)");
    fs.string_var(
        "run-as",
        "",
        "the custom agent every job it queues runs as, one that pulse agents list shows for the project (add)",
    );
    fs.string_var(
        "skill",
        "",
        "the pulse skill every job loads first and follows, by name (pulse skill list); with no --brief the skill is the brief (add)",
    );
    fs.string_var("note", "", "why (pause, end)");
    fs.string_var("status", "", "list only active, paused or ended schedules");
}

fn task_flags(fs: &mut FlagSet) {
    fs.string_var(
        "title",
        "",
        "task title (new, amend, suggest; review accept: a sharper title)",
    );
    fs.string_var(
        "brief",
        "",
        "brief text (new, amend, suggest, review accept); or --brief-file, or stdin",
    );
    fs.string_var("brief-file", "", "file holding the brief; - for stdin");
    fs.int_var(
        "priority",
        0,
        "priority, higher first (new, suggest, review accept)",
    );
    fs.string_var("status", "", "filter by status (list, suggestions)");
    fs.string_var("note", "", "why this revision (amend)");
    fs.string_var("reason", "", "why (cancel, review, withdraw)");
    fs.string_var("file", "", "file holding the reply text (reply)");
    fs.strings_var(
        "choice",
        "an answer the owner can pick instead of writing, repeatable: LABEL or LABEL=reply|done|amend (reply)",
    );
}

fn msg_flags(fs: &mut FlagSet) {
    fs.string_var("task", "", "the task the message is about");
    fs.string_var("since", "", "only messages after this id (list)");
    fs.bool_var(
        "unread",
        false,
        "only messages the acting agent has not read (list)",
    );
    fs.string_var("file", "", "file holding the message text (send)");
    fs.strings_var(
        "choice",
        "an answer the owner can pick instead of writing, repeatable: LABEL or LABEL=reply|done|amend (send, needs --task)",
    );
}

fn compute_flags(fs: &mut FlagSet) {
    fs.string_var(
        "size",
        "",
        "machine size (default: the provider's size setting, e.g. compute.azure.vm_size)",
    );
    fs.int_var("hours", 0, "hours the machine is needed");
    fs.string_var("reason", "", "why the work needs it");
    fs.string_var("status", "", "filter by status (list)");
    fs.string_var("words", "", "the owner's words (approve, deny)");
}

fn watch_flags(fs: &mut FlagSet) {
    fs.string_var("area", "", "only posts in this area");
    fs.string_var(
        "thread",
        "",
        "only posts in this thread (area/thread or thread)",
    );
    fs.string_var(
        "action",
        "",
        "only actions matching this glob, e.g. task.* (repeatable with commas)",
    );
    fs.i64_var("since", -1, "start after this event id (default: now)");
    fs.bool_var("detail", false, "append the detail column");
    fs.duration_var("interval", Duration::from_secs(1), "poll interval");
}

fn store(c: &Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow!("command requires a store"))
}

fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

fn read_body(inline: &str, file: &str) -> Result<String> {
    if !inline.is_empty() {
        return Ok(inline.to_string());
    }
    if !file.is_empty() && file != "-" {
        return std::fs::read_to_string(file).map_err(|e| {
            pulse_cli::errorf(
                2,
                &format!("Cannot read {file}"),
                &e.to_string(),
                "check the path",
            )
            .into()
        });
    }
    if let Some(data) = STDIN_BUF.lock().expect("stdin lock poisoned").clone() {
        return Ok(String::from_utf8_lossy(&data).to_string());
    }
    if file == "-" || !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        return Ok(s);
    }
    Ok(String::new())
}

fn usage(cmd: &str, sub: &str) -> anyhow::Error {
    let what = if sub.is_empty() {
        format!("pulse {cmd} needs a subcommand")
    } else {
        format!("pulse {cmd}: unknown subcommand {sub:?}")
    };
    pulse_cli::errorf(2, &what, "see the usage", &format!("pulse help {cmd}")).into()
}

fn need(cmd: &str, args: &[String], n: usize, fix: &str) -> Result<()> {
    if args.len() < n {
        Err(pulse_cli::errorf(
            2,
            "Missing argument",
            &format!("the command needs {fix}"),
            &format!("pulse help {cmd}"),
        )
        .into())
    } else {
        Ok(())
    }
}

fn printf(c: &mut Context, args: std::fmt::Arguments<'_>) -> Result<()> {
    c.printf(args)?;
    Ok(())
}

/// Prints the answers a message offers, one numbered line each, so a reader
/// of the terminal sees what the hub shows as buttons.
fn print_choices(c: &mut Context, choices: &[Choice], indent: &str) -> Result<()> {
    for (i, ch) in choices.iter().enumerate() {
        let action = if ch.action == "reply" {
            String::new()
        } else {
            format!("  ({})", ch.action)
        };
        printf(
            c,
            format_args!("{indent}[{}] {}{action}\n", i + 1, ch.label),
        )?;
    }
    Ok(())
}

fn print_task(c: &mut Context, t: &Task) -> Result<()> {
    let st = c.style();
    printf(
        c,
        format_args!("{}  {}  {}\n", st.bold(&t.id), st.cyan(&t.status), t.title),
    )?;
    printf(
        c,
        format_args!(
            "  project {}  priority {}  rev {}  by {}  created {}  updated {}\n",
            t.project, t.priority, t.current_rev, t.created_by, t.created_at, t.updated_at
        ),
    )?;
    if !t.lead.is_empty() || !t.job_id.is_empty() {
        printf(c, format_args!("  lead {}  job {}\n", t.lead, t.job_id))?;
    }
    Ok(())
}

fn print_suggestion(c: &mut Context, g: &suggestions::Suggestion) -> Result<()> {
    let st = c.style();
    printf(
        c,
        format_args!("{}  {}  {}\n", st.bold(&g.id), st.cyan(&g.status), g.title),
    )?;
    printf(
        c,
        format_args!(
            "  project {}  priority {}  suggested by {}  created {}  updated {}\n",
            g.project, g.priority, g.suggested_by, g.created_at, g.updated_at
        ),
    )?;
    if !g.job_id.is_empty() || g.reviews > 0 {
        printf(
            c,
            format_args!("  review job {}  reviews ended {}\n", g.job_id, g.reviews),
        )?;
    }
    if !g.reviewer.is_empty() || !g.verdict.is_empty() {
        printf(
            c,
            format_args!("  reviewer {}  verdict {}\n", g.reviewer, g.verdict),
        )?;
    }
    if !g.task_id.is_empty() {
        printf(c, format_args!("  task {}\n", g.task_id))?;
    }
    Ok(())
}

#[derive(Serialize)]
struct ScheduleOut<'a> {
    schedule: &'a schedules::Schedule,
}

#[derive(Serialize)]
struct SchedulesOut<'a> {
    schedules: &'a [schedules::Schedule],
}

#[derive(Serialize)]
struct ScheduleShowOut<'a> {
    jobs: &'a [Job],
    schedule: &'a schedules::Schedule,
}

#[derive(Serialize)]
struct ScheduleLogOut<'a> {
    schedule: &'a schedules::Schedule,
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    let skip = lines.len().saturating_sub(n);
    lines[skip..].join("\n")
}

/// The state column of `pulse schedule list`: green/red/ok/broken for a
/// script, or `-` before its first run; empty for the job kinds.
fn state_text(sc: &schedules::Schedule) -> String {
    if !sc.is_script() {
        String::new()
    } else if sc.state.is_empty() {
        "-".to_string()
    } else {
        sc.state.clone()
    }
}

fn print_schedule(c: &mut Context, sc: &schedules::Schedule) -> Result<()> {
    let st = c.style();
    let mut head = format!(
        "{} {} {} every {}",
        st.bold(&sc.id),
        sc.project,
        sc.status,
        sc.every()
    );
    if !sc.at_time.is_empty() {
        head.push_str(&format!(" at {} UTC", sc.at_time));
    }
    if sc.max_runs > 0 {
        head.push_str(&format!(", {} of {} runs", sc.runs, sc.max_runs));
    } else {
        head.push_str(&format!(", {} runs", sc.runs));
    }
    c.println(&head)?;
    if sc.is_script() {
        printf(
            c,
            format_args!(
                "{} `{}`, queues {} jobs at tier {}, priority {}; set by {} at {}\n",
                sc.kind, sc.script, sc.job_kind, sc.tier, sc.priority, sc.created_by, sc.created_at
            ),
        )?;
        if sc.state.is_empty() {
            c.println("state: not run yet")?;
        } else {
            printf(
                c,
                format_args!(
                    "state: {} since {}; last exit {}, {} failure{} in a row\n",
                    sc.state,
                    sc.state_since,
                    sc.last_rc.map_or("-".to_string(), |rc| rc.to_string()),
                    sc.fails,
                    if sc.fails == 1 { "" } else { "s" }
                ),
            )?;
        }
    } else {
        printf(
            c,
            format_args!(
                "{} job, tier {}, priority {}; set by {} at {}\n",
                sc.kind, sc.tier, sc.priority, sc.created_by, sc.created_at
            ),
        )?;
    }
    if sc.status == "active" {
        printf(c, format_args!("next at {}\n", sc.next_at))?;
    }
    if !sc.agent.is_empty() {
        printf(c, format_args!("jobs run as {}\n", sc.agent))?;
    }
    if !sc.skill.is_empty() {
        printf(c, format_args!("jobs follow the skill {}\n", sc.skill))?;
    }
    if sc.from_repo() {
        printf(
            c,
            format_args!(
                "kept by the repository: {} (the file on the default branch changes it; pulse manifest show --project {})\n",
                sc.origin.trim_start_matches(pulse_store::REPO_ORIGIN),
                sc.project
            ),
        )?;
    }
    if !sc.last_at.is_empty() {
        if sc.last_job.is_empty() {
            printf(c, format_args!("last run at {}\n", sc.last_at))?;
        } else if sc.is_script() {
            printf(
                c,
                format_args!("last run at {} (last job {})\n", sc.last_at, sc.last_job),
            )?;
        } else {
            printf(
                c,
                format_args!("last at {} ({})\n", sc.last_at, sc.last_job),
            )?;
        }
    }
    if !sc.note.is_empty() {
        printf(c, format_args!("note: {}\n", sc.note))?;
    }
    Ok(())
}

pub fn run_manifest(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("manifest", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "check" => {
            let s = if c.project.trim().is_empty() {
                None
            } else {
                Some(manifest_store(c, false)?)
            };
            manifest_check(
                c,
                s.as_ref(),
                rest.first().map(String::as_str).unwrap_or(""),
            )
        }
        "show" => {
            let s = manifest_store(c, false)?;
            let slug = need_project(c, &s, "show")?;
            let status = manifest::status(&s, &slug)?;
            let schedules: Vec<schedules::Schedule> = schedules::list(&s, &slug, "")?
                .into_iter()
                .filter(schedules::Schedule::from_repo)
                .collect();
            let skills: Vec<pulse_store::Skill> = s
                .skills(&slug)?
                .into_iter()
                .filter(pulse_store::Skill::from_repo)
                .collect();
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ManifestShowOut {
                        project: &slug,
                        status: status.as_ref(),
                        schedules: &schedules,
                        skills: &skills,
                    },
                );
            }
            print_manifest_status(c, &slug, status.as_ref())?;
            if schedules.is_empty() {
                c.println("Schedules the file keeps: none.")?;
            } else {
                c.println("Schedules the file keeps:")?;
                for sc in &schedules {
                    let name = manifest::origin_name(&sc.origin).unwrap_or(&sc.title);
                    let mut line =
                        format!("  {} {} {} every {}", sc.id, name, sc.status, sc.every());
                    if sc.is_script() {
                        line.push_str(&format!(", {} `{}`", sc.kind, sc.script));
                    } else if !sc.skill.is_empty() {
                        line.push_str(&format!(", skill `{}`", sc.skill));
                    }
                    if sc.status == "active" {
                        line.push_str(&format!(", next at {}", sc.next_at));
                    } else if !sc.note.is_empty() {
                        line.push_str(&format!(" ({})", sc.note));
                    }
                    c.println(&line)?;
                }
            }
            if skills.is_empty() {
                c.println("Skills the repository keeps: none.")?;
            } else {
                c.println("Skills the repository keeps:")?;
                for sk in &skills {
                    printf(
                        c,
                        format_args!(
                            "  {} {} ({}): {}\n",
                            sk.name,
                            sk.status,
                            sk.origin.trim_start_matches(pulse_store::REPO_ORIGIN),
                            sk.description
                        ),
                    )?;
                }
            }
            Ok(())
        }
        "sync" => {
            let s = manifest_store(c, true)?;
            let slug = need_project(c, &s, "sync")?;
            let Some(synced) = manifest::sync(&s, &slug, &actor(c), true)? else {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("No checkout of {slug} to read here"),
                    "the project's path is not a git repository on this machine, or its default branch is not in it",
                    "pulse projects show, then fetch the default branch",
                )
                .into());
            };
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ManifestSyncOut { synced: &synced });
            }
            c.println(synced.line())?;
            if !synced.problems.is_empty() {
                c.println(format!(
                    "Fix the file on {} and commit; the daemon applies the next commit by itself.",
                    synced.branch
                ))?;
            }
            Ok(())
        }
        other => Err(usage("manifest", other)),
    }
}

fn need_project(c: &Context, s: &pulse_store::Store, sub: &str) -> Result<String> {
    if c.project.trim().is_empty() {
        return Err(pulse_cli::errorf(
            2,
            &format!("pulse manifest {sub} needs --project"),
            "the file belongs to one project's repository",
            &format!("pulse manifest {sub} --project SLUG"),
        )
        .into());
    }
    crate::tasks::check_project(s, &c.project)?;
    Ok(c.project.trim().to_string())
}

/// The store for `pulse manifest`, which opens none by itself so that
/// `check` runs on a machine without one: the context's when the caller
/// opened it, else the one at the PULSE home.
fn manifest_store(c: &Context, writable: bool) -> Result<pulse_store::Store> {
    if let Some(s) = &c.store {
        return Ok(s.clone());
    }
    let path = std::path::Path::new(&c.home).join(pulse_store::STORE_NAME);
    if !path.exists() {
        return Err(pulse_cli::errorf(
            2,
            &format!("No store at {}", path.display()),
            "this machine has no PULSE store yet; pulse manifest check without --project needs none",
            "pulse init <project-path>",
        )
        .into());
    }
    pulse_store::Store::open(pulse_store::Options {
        home: c.home.clone(),
        read_only: !writable,
        skip_records: !writable,
        now: c.now.clone(),
        ..pulse_store::Options::default()
    })
}

/// Reads a working copy's file and lists what the daemon would refuse:
/// the YAML, every entry's fields, and with `--project` the store's checks
/// too (the skill, the agent, the script on the default branch).
fn manifest_check(c: &mut Context, s: Option<&pulse_store::Store>, arg: &str) -> Result<()> {
    let path = if arg.is_empty() {
        let found: Vec<&str> = manifest::MANIFEST_PATHS
            .iter()
            .copied()
            .filter(|p| std::path::Path::new(p).is_file())
            .collect();
        if found.len() > 1 {
            c.println(format!(
                "note: both {} and {} exist; the daemon reads {} and reports the other",
                found[0], found[1], found[0]
            ))?;
        }
        match found.first() {
            Some(p) => (*p).to_string(),
            None => {
                return Err(pulse_cli::errorf(
                    2,
                    "No PULSE file here",
                    &format!(
                        "none of {} exists under the current directory",
                        manifest::MANIFEST_PATHS.join(", ")
                    ),
                    "run it from the repository root, or name the file: pulse manifest check PATH",
                )
                .into());
            }
        }
    } else {
        arg.to_string()
    };
    let text = std::fs::read_to_string(&path).map_err(|e| {
        pulse_cli::errorf(
            2,
            &format!("Cannot read {path}"),
            &e.to_string(),
            "check the path",
        )
    })?;
    let mut problems = Vec::new();
    let mut names = Vec::new();
    let mut parsed = None;
    if text.len() > manifest::FILE_LIMIT {
        problems.push(format!(
            "{path} is {} bytes; the daemon reads at most {}",
            text.len(),
            manifest::FILE_LIMIT
        ));
    }
    match manifest::parse(&text) {
        Ok(m) => {
            problems.extend(manifest::static_problems(&m));
            parsed = Some(m);
        }
        Err(e) => problems.push(format!("{path} could not be read: {e}")),
    }
    if let Some(m) = &parsed {
        let held: std::collections::BTreeSet<String> = problems
            .iter()
            .filter_map(|p| p.strip_prefix("schedule `"))
            .filter_map(|p| p.split('`').next())
            .map(str::to_string)
            .collect();
        let slug = c.project.trim().to_string();
        if let Some(s) = s {
            crate::tasks::check_project(s, &slug)?;
        }
        for e in &m.schedules {
            let name = e.name.trim();
            names.push(name);
            let Some(s) = s else {
                continue;
            };
            if held.contains(name) {
                continue;
            }
            match schedules::check(s, &e.input(&slug, &path, &actor(c))) {
                Ok(warning) if !warning.is_empty() => {
                    problems.push(format!("schedule `{name}`: {warning}"));
                }
                Ok(_) => {}
                Err(err) => problems.push(format!(
                    "schedule `{name}`: {}",
                    manifest::first_line(&err.to_string())
                )),
            }
        }
    }
    if c.json {
        pulse_cli::write_json(
            &mut c.out,
            &ManifestCheckOut {
                path: &path,
                schedules: names,
                problems: &problems,
            },
        )?;
    } else if problems.is_empty() {
        let n = names.len();
        let mut line = format!(
            "{path}: sound; {n} schedule{}",
            if n == 1 { "" } else { "s" }
        );
        if c.project.trim().is_empty() {
            line.push_str(
                " (add --project P to check skills, agents and scripts against the store)",
            );
        }
        c.println(line)?;
    } else {
        c.println(format!(
            "{path}: {} problem{}",
            problems.len(),
            if problems.len() == 1 { "" } else { "s" }
        ))?;
        for p in &problems {
            c.println(format!("  - {p}"))?;
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(pulse_cli::errorf(
            1,
            &format!(
                "{} problem{} in {path}",
                problems.len(),
                if problems.len() == 1 { "" } else { "s" }
            ),
            "the daemon would hold or refuse these entries",
            "fix the file; pulse help manifest lists the fields",
        )
        .into())
    }
}

fn print_manifest_status(
    c: &mut Context,
    slug: &str,
    status: Option<&manifest::Status>,
) -> Result<()> {
    let Some(st) = status else {
        return c
            .println(format!(
                "{slug}: the daemon has not read a PULSE file for this project yet (pulse manifest sync --project {slug} reads it now)"
            ))
            .map_err(Into::into);
    };
    if st.path.is_empty() {
        c.println(format!(
            "{slug}: no {} on the default branch at {} (read {})",
            manifest::MANIFEST_PATHS[0],
            st.commit,
            st.at
        ))?;
    } else {
        c.println(format!(
            "{slug}: {} at {} (read {})",
            st.path, st.commit, st.at
        ))?;
    }
    if st.problems.is_empty() {
        c.println("Problems: none.")?;
    } else {
        c.println(format!("Problems ({}):", st.problems.len()))?;
        for p in &st.problems {
            c.println(format!("  - {p}"))?;
        }
    }
    Ok(())
}

pub fn run_schedule(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("schedule", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "add" => {
            let kind = fs.get_string("kind");
            let skill = fs.get_string("skill");
            let (brief_flag, brief_file) = (fs.get_string("brief"), fs.get_string("brief-file"));
            // A check or watch, or a job that follows a skill, needs no
            // brief, so stdin is read only when asked for.
            let brief = if (schedules::SCRIPT_KINDS.contains(&kind.as_str())
                || !skill.trim().is_empty())
                && brief_flag.is_empty()
                && brief_file.is_empty()
            {
                String::new()
            } else {
                read_body(&brief_flag, &brief_file)?
            };
            let (sc, warning) = schedules::add_checked(
                store(c)?,
                schedules::ScheduleInput {
                    project: c.project.clone(),
                    title: fs.get_string("title"),
                    kind,
                    brief,
                    tier: fs.get_int("tier"),
                    priority: fs.get_int("priority"),
                    every: fs.get_string("every"),
                    at: fs.get_string("at"),
                    times: fs.get_int("times"),
                    author: actor(c),
                    script: fs.get_string("script"),
                    job_kind: fs.get_string("job-kind"),
                    agent: fs.get_string("run-as"),
                    skill,
                    origin: String::new(),
                },
            )?;
            if !warning.is_empty() {
                writeln!(c.err, "{warning}")?;
            }
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
            if sc.is_script() {
                printf(
                    c,
                    format_args!(
                        "{} set in {}: {} `{}` every {}, first run at {}\n",
                        sc.id,
                        sc.project,
                        sc.kind,
                        sc.script,
                        sc.every(),
                        sc.next_at
                    ),
                )
            } else {
                printf(
                    c,
                    format_args!(
                        "{} set in {}: {} every {}, first at {}\n",
                        sc.id,
                        sc.project,
                        sc.title,
                        sc.every(),
                        sc.next_at
                    ),
                )
            }
        }
        "list" => {
            if !c.project.is_empty() {
                crate::tasks::check_project(store(c)?, &c.project)?;
            }
            let list = schedules::list(store(c)?, &c.project, &fs.get_string("status"))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SchedulesOut { schedules: &list });
            }
            if list.is_empty() {
                c.println("No schedules.")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|sc| {
                    vec![
                        sc.id.clone(),
                        sc.project.clone(),
                        sc.kind.clone(),
                        sc.status.clone(),
                        state_text(sc),
                        sc.every(),
                        if sc.status == "active" {
                            sc.next_at.clone()
                        } else {
                            String::new()
                        },
                        if sc.max_runs > 0 {
                            format!("{}/{}", sc.runs, sc.max_runs)
                        } else {
                            sc.runs.to_string()
                        },
                        if sc.from_repo() {
                            "repository".to_string()
                        } else {
                            sc.created_by.clone()
                        },
                        if sc.is_script() {
                            format!("{} ({})", sc.title, sc.script)
                        } else {
                            sc.title.clone()
                        },
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID", "PROJECT", "KIND", "STATUS", "STATE", "EVERY", "NEXT", "RUNS", "BY",
                    "TITLE",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need("schedule", rest, 1, "a schedule id")?;
            let sc = schedules::get(store(c)?, &rest[0])?;
            let js = schedules::jobs(store(c)?, &sc.id)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ScheduleShowOut {
                        jobs: &js,
                        schedule: &sc,
                    },
                );
            }
            print_schedule(c, &sc)?;
            if !sc.brief.trim().is_empty() {
                c.println("")?;
                c.println(sc.brief.trim_end_matches('\n'))?;
            }
            if sc.is_script() && sc.last_rc.is_some() {
                c.println("")?;
                c.println(c.style().bold("LAST OUTPUT"))?;
                let tail = last_lines(&sc.last_output, 12);
                if tail.is_empty() {
                    c.println("(no output)")?;
                } else {
                    c.println(tail.trim_end_matches('\n'))?;
                }
                if sc.last_output.lines().count() > 12 {
                    printf(
                        c,
                        format_args!("(last 12 lines; pulse schedule log {} has all)\n", sc.id),
                    )?;
                }
            }
            c.println("")?;
            c.println(c.style().bold("JOBS"))?;
            let rows = js
                .iter()
                .map(|j| {
                    vec![
                        j.id.clone(),
                        j.status.clone(),
                        j.agent.clone(),
                        j.created_at.clone(),
                        j.finished_at.clone(),
                        j.branch.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&["JOB", "STATUS", "AGENT", "CREATED", "FINISHED", "BRANCH"]),
                &rows,
            )?;
            Ok(())
        }
        "pause" | "end" => {
            need("schedule", rest, 1, "a schedule id")?;
            let s = store(c)?;
            let sc = if sub == "pause" {
                schedules::pause(s, &rest[0], &fs.get_string("note"), &actor(c))?
            } else {
                schedules::end(s, &rest[0], &fs.get_string("note"), &actor(c))?
            };
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
            printf(c, format_args!("{} {}\n", sc.id, sc.status))
        }
        "resume" => {
            need("schedule", rest, 1, "a schedule id")?;
            let sc = schedules::resume(store(c)?, &rest[0], &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
            printf(
                c,
                format_args!("{} {}, next at {}\n", sc.id, sc.status, sc.next_at),
            )
        }
        "run" => {
            need("schedule", rest, 1, "a schedule id")?;
            let sc = schedules::run_now(store(c)?, &rest[0], &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
            if sc.is_script() {
                printf(
                    c,
                    format_args!(
                        "{} due now; the daemon runs `{}` on the next tick\n",
                        sc.id, sc.script
                    ),
                )
            } else {
                printf(
                    c,
                    format_args!(
                        "{} due now; the daemon queues its job on the next tick\n",
                        sc.id
                    ),
                )
            }
        }
        "log" => {
            need("schedule", rest, 1, "a schedule id")?;
            let sc = schedules::get(store(c)?, &rest[0])?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleLogOut { schedule: &sc });
            }
            if !sc.is_script() {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("{} is a {} schedule, not a check or watch", sc.id, sc.kind),
                    "only a script leaves output behind",
                    &format!("pulse schedule show {}", sc.id),
                )
                .into());
            }
            match sc.last_rc {
                None => printf(
                    c,
                    format_args!(
                        "{} has not run `{}` yet; next at {}\n",
                        sc.id, sc.script, sc.next_at
                    ),
                ),
                Some(rc) => {
                    printf(
                        c,
                        format_args!(
                            "{} `{}` at {}: {} (exit {rc})\n",
                            sc.id, sc.script, sc.last_at, sc.state
                        ),
                    )?;
                    if sc.last_output.trim().is_empty() {
                        c.println("(no output)")?;
                    } else {
                        c.println(sc.last_output.trim_end_matches('\n'))?;
                    }
                    Ok(())
                }
            }
        }
        other => Err(usage("schedule", other)),
    }
}

pub fn run_task(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("task", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "new" => {
            let brief = read_body(&fs.get_string("brief"), &fs.get_string("brief-file"))?;
            let t = new(
                store(c)?,
                &c.project,
                &fs.get_string("title"),
                &brief,
                fs.get_int("priority"),
                &actor(c),
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(
                c,
                format_args!("{} created in {}: {}\n", t.id, t.project, t.title),
            )
        }
        "list" => {
            let s = store(c)?;
            if !c.project.is_empty() {
                check_project(s, &c.project)?;
            }
            let list = list(s, &c.project, &fs.get_string("status"))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TasksOut { tasks: &list });
            }
            if list.is_empty() {
                c.println("No tasks.")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|t| {
                    vec![
                        t.id.clone(),
                        t.project.clone(),
                        t.status.clone(),
                        t.priority.to_string(),
                        t.lead.clone(),
                        t.updated_at.clone(),
                        t.title.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&["ID", "PROJECT", "STATUS", "PRI", "LEAD", "UPDATED", "TITLE"]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need("task", rest, 1, "a task id")?;
            if suggestions::is_id(&rest[0]) {
                let g = suggestions::get(store(c)?, &rest[0])?;
                if c.json {
                    return pulse_cli::write_json(&mut c.out, &SuggestionOut { suggestion: &g });
                }
                print_suggestion(c, &g)?;
                c.println("")?;
                c.println(g.brief.trim_end_matches('\n'))?;
                return Ok(());
            }
            let (t, revs) = get(store(c)?, &rest[0])?;
            let msgs = messages(store(c)?, "", &t.id, "", "")?;
            let js = jobs(store(c)?, &t.id)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ShowOut {
                        jobs: &js,
                        messages: &msgs,
                        revisions: &revs,
                        task: &t,
                    },
                );
            }
            print_task(c, &t)?;
            c.println("")?;
            c.println(t.brief.trim_end_matches('\n'))?;
            c.println("")?;
            let mut rows = revs
                .iter()
                .map(|r| {
                    vec![
                        r.rev.to_string(),
                        r.at.clone(),
                        r.author.clone(),
                        r.note.clone(),
                        r.title.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            c.println(c.style().bold("REVISIONS"))?;
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&["REV", "AT", "AUTHOR", "NOTE", "TITLE"]),
                &rows,
            )?;
            c.println("")?;
            c.println(c.style().bold("MESSAGES"))?;
            if msgs.is_empty() {
                c.println("  none")?;
            }
            for m in msgs {
                let tag = if m.is_report() {
                    format!(" (report on job {})", m.job_id)
                } else {
                    String::new()
                };
                printf(
                    c,
                    format_args!("  {} {} {}{}: {}\n", m.at, m.id, m.author, tag, m.body),
                )?;
                print_choices(c, &m.choices, "    ")?;
            }
            c.println("")?;
            c.println(c.style().bold("JOBS"))?;
            if js.is_empty() {
                c.println("  none")?;
            } else {
                rows = js
                    .iter()
                    .map(|j| {
                        vec![
                            j.id.clone(),
                            j.kind.clone(),
                            j.status.clone(),
                            j.agent.clone(),
                            j.branch.clone(),
                            j.title.clone(),
                        ]
                    })
                    .collect();
                pulse_cli::table(
                    &mut c.out,
                    c.color,
                    Some(&["ID", "KIND", "STATUS", "AGENT", "BRANCH", "TITLE"]),
                    &rows,
                )?;
            }
            Ok(())
        }
        "amend" => {
            need("task", rest, 1, "a task id")?;
            let brief = read_body(&fs.get_string("brief"), &fs.get_string("brief-file"))?;
            let t = amend(
                store(c)?,
                &rest[0],
                &fs.get_string("title"),
                &brief,
                &fs.get_string("note"),
                &actor(c),
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(c, format_args!("{} amended: rev {}\n", t.id, t.current_rev))
        }
        "cancel" => {
            need("task", rest, 1, "a task id")?;
            let t = cancel(store(c)?, &rest[0], &fs.get_string("reason"), &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(c, format_args!("{} cancelled\n", t.id))
        }
        "status" => {
            need("task", rest, 2, "a task id and a status")?;
            let t = set_status(store(c)?, &rest[0], &rest[1], &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(c, format_args!("{} is {}\n", t.id, t.status))
        }
        "reply" => {
            need("task", rest, 1, "a task id")?;
            let body = read_body(&rest[1..].join(" "), &fs.get_string("file"))?;
            let choices = choices_from_flags(&fs.get_strings("choice"))?;
            let m = reply_with_choices(store(c)?, &rest[0], &actor(c), &body, &choices)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MessageOut { message: &m });
            }
            printf(c, format_args!("{} sent on {}\n", m.id, m.task_id))?;
            print_choices(c, &m.choices, "")
        }
        "suggest" => {
            let brief = read_body(&fs.get_string("brief"), &fs.get_string("brief-file"))?;
            let g = suggestions::suggest(
                store(c)?,
                &c.project,
                &fs.get_string("title"),
                &brief,
                fs.get_int("priority"),
                &actor(c),
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SuggestionOut { suggestion: &g });
            }
            printf(
                c,
                format_args!(
                    "{} suggested in {}: {}\nAnother agent reviews it; pulse task show {} follows the verdict.\n",
                    g.id, g.project, g.title, g.id
                ),
            )
        }
        "suggestions" => {
            let s = store(c)?;
            if !c.project.is_empty() {
                check_project(s, &c.project)?;
            }
            let list = suggestions::list(s, &c.project, &fs.get_string("status"))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SuggestionsOut { suggestions: &list });
            }
            if list.is_empty() {
                c.println("No suggestions.")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|g| {
                    vec![
                        g.id.clone(),
                        g.project.clone(),
                        g.status.clone(),
                        g.priority.to_string(),
                        g.suggested_by.clone(),
                        g.reviewer.clone(),
                        g.task_id.clone(),
                        g.updated_at.clone(),
                        g.title.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID", "PROJECT", "STATUS", "PRI", "BY", "REVIEWER", "TASK", "UPDATED", "TITLE",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "review" => {
            need(
                "task",
                rest,
                2,
                "a suggestion id and a verdict (accept, decline or owner)",
            )?;
            let brief = if fs.is_set("brief") || fs.is_set("brief-file") {
                read_body(&fs.get_string("brief"), &fs.get_string("brief-file"))?
            } else {
                String::new()
            };
            let v = suggestions::Verdict {
                verdict: rest[1].clone(),
                reason: fs.get_string("reason"),
                title: fs.get_string("title"),
                brief,
                priority: fs.is_set("priority").then(|| fs.get_int("priority")),
            };
            let (g, task) = suggestions::review(store(c)?, &rest[0], &v, &actor(c))?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ReviewOut {
                        suggestion: &g,
                        task: task.as_ref(),
                    },
                );
            }
            match task {
                Some(t) => printf(
                    c,
                    format_args!("{} accepted: {} created in {}\n", g.id, t.id, t.project),
                ),
                None => printf(c, format_args!("{} {}: {}\n", g.id, g.status, g.verdict)),
            }
        }
        "withdraw" => {
            need("task", rest, 1, "a suggestion id")?;
            let g =
                suggestions::withdraw(store(c)?, &rest[0], &fs.get_string("reason"), &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &SuggestionOut { suggestion: &g });
            }
            printf(c, format_args!("{} withdrawn\n", g.id))
        }
        _ => Err(usage("task", sub)),
    }
}

pub fn run_msg(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("msg", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "send" => {
            let body = read_body(&rest.join(" "), &fs.get_string("file"))?;
            let choices = choices_from_flags(&fs.get_strings("choice"))?;
            let m = send_message_with_choices(
                store(c)?,
                &c.project,
                &fs.get_string("task"),
                &actor(c),
                &body,
                &choices,
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MessageOut { message: &m });
            }
            printf(c, format_args!("{} sent in {}\n", m.id, m.project))?;
            print_choices(c, &m.choices, "")
        }
        "list" => {
            let unread = if fs.get_bool("unread") {
                actor(c)
            } else {
                String::new()
            };
            let list = messages(
                store(c)?,
                &c.project,
                &fs.get_string("task"),
                &fs.get_string("since"),
                &unread,
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MessagesOut { messages: &list });
            }
            if list.is_empty() {
                c.println("No messages.")?;
                return Ok(());
            }
            for m in list {
                let task = if m.task_id.is_empty() {
                    "-"
                } else {
                    &m.task_id
                };
                let tag = if m.is_report() {
                    format!(" (report on job {})", m.job_id)
                } else {
                    String::new()
                };
                printf(
                    c,
                    format_args!(
                        "{} {} {} {} {}{}: {}\n",
                        m.at,
                        m.id,
                        m.project,
                        task,
                        c.style().bold(&m.author),
                        tag,
                        m.body
                    ),
                )?;
                print_choices(c, &m.choices, "  ")?;
            }
            Ok(())
        }
        "read" => {
            need("msg", rest, 1, "one or more message ids")?;
            let n = mark_read(store(c)?, rest, &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MarkedOut { marked: n });
            }
            printf(c, format_args!("{} marked read for {}\n", n, actor(c)))
        }
        _ => Err(usage("msg", sub)),
    }
}

pub fn run_compute(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage("compute", ""));
    }
    let (sub, rest) = (&args[0], &args[1..]);
    match sub.as_str() {
        "request" => {
            let size = fs.get_string("size");
            // One variable, checked before any provider call: can this host
            // ship a linux hands binary? On a host that cannot, refuse now
            // with the fix instead of approving a request whose machine fails
            // after provisioning (CR-001/CR-002). The probe is registered by
            // pulse-runner::compute::init; without a runner it stays silent.
            let arch = crate::compute::default_hands_arch(&size);
            if let Some(line) = crate::hands::hands_binary_probe_line(&arch) {
                bail!(
                    "compute request refused before a machine: {line}\nSet PULSE_HANDS_BINARY, place pulse-linux-{arch} beside the pulse binary, or install the cross toolchain, then retry."
                );
            }
            let r = request_compute(
                store(c)?,
                &c.project,
                &actor(c),
                &size,
                fs.get_int("hours"),
                &fs.get_string("reason"),
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &RequestOut { request: &r });
            }
            if r.status == "approved" {
                printf(
                    c,
                    format_args!(
                        "{} approved: {} for {}h (auto, under the caps)\n",
                        r.id, r.size, r.hours
                    ),
                )
            } else {
                printf(
                    c,
                    format_args!(
                        "{} pending: {} for {}h - proposal {} waits for the owner\n",
                        r.id, r.size, r.hours, r.proposal_post
                    ),
                )
            }
        }
        "list" => {
            if !c.project.is_empty() {
                crate::tasks::check_project(store(c)?, &c.project)?;
            }
            let list = list_compute(store(c)?, &c.project, &fs.get_string("status"))?;
            let caps = get_caps(store(c)?)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &ComputeListOut {
                        caps: &caps,
                        requests: &list,
                    },
                );
            }
            printf(
                c,
                format_args!(
                    "caps: provider {}  max_vms {} (in use {})  max_hours_per_day {} (today {})  auto_approve {}  budget_credits {}\n",
                    caps.provider,
                    caps.max_vms,
                    caps.vms_now,
                    caps.max_hours_per_day,
                    caps.hours_today,
                    caps.auto_approve,
                    caps.budget_credits
                ),
            )?;
            if list.is_empty() {
                c.println("No compute requests.")?;
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|r| {
                    vec![
                        r.id.clone(),
                        r.project.clone(),
                        r.status.clone(),
                        r.requested_by.clone(),
                        r.size.clone(),
                        r.hours.to_string(),
                        r.executor.clone(),
                        r.at.clone(),
                        r.reason.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID", "PROJECT", "STATUS", "BY", "SIZE", "HOURS", "EXECUTOR", "AT", "REASON",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "approve" | "deny" => {
            need("compute", rest, 1, "a request id")?;
            let who = actor(c);
            let r = if sub == "approve" {
                approve(store(c)?, &rest[0], &fs.get_string("words"), &who)?
            } else {
                deny(store(c)?, &rest[0], &fs.get_string("words"), &who)?
            };
            if c.json {
                return pulse_cli::write_json(&mut c.out, &RequestOut { request: &r });
            }
            printf(
                c,
                format_args!("{} {} by {}\n", r.id, r.status, r.decided_by),
            )
        }
        "providers" => {
            let f = *COMPUTE_PROVIDERS
                .read()
                .expect("compute providers lock poisoned");
            if let Some(f) = f {
                f(c)
            } else {
                Err(pulse_cli::errorf(
                    2,
                    "No compute provider is built into this binary",
                    "the runner registers them",
                    "use a full pulse build",
                )
                .into())
            }
        }
        _ => Err(usage("compute", sub)),
    }
}

fn run_watch(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let mut filter = Filter {
        project: c.project.clone(),
        area: fs.get_string("area"),
        thread: fs.get_string("thread"),
        ..Filter::default()
    };
    let action = fs.get_string("action");
    if !action.is_empty() {
        filter.actions = action.split(',').map(ToString::to_string).collect();
    }
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])?;
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            signal_cancel.cancel();
        }
    });
    watch(
        cancel,
        c,
        fs.get_i64("since"),
        filter,
        fs.get_duration("interval"),
        fs.get_bool("detail"),
    )
}

pub fn watch(
    cancel: CancellationToken,
    c: &mut Context,
    since: i64,
    filter: Filter,
    interval: Duration,
    detail: bool,
) -> Result<()> {
    let st = store(c)?.clone();
    let redact = pulse_bus::redactor(Some(&st));
    let rx = pulse_bus::tail(cancel, st.clone(), since, filter, interval);
    while let Ok(mut e) = rx.recv() {
        e = redact(e);
        if c.json {
            c.out.write_all(&json_line(&e)?)?;
            c.out.write_all(b"\n")?;
        } else {
            write!(c.out, "{} {} {} {}", e.at, e.actor, e.action, e.subject)?;
            if detail && !e.detail.is_empty() {
                write!(c.out, " {}", c.style().dim(&e.detail))?;
            }
            c.out.write_all(b"\n")?;
        }
        if let Err(e) = c.out.flush() {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e.into());
        }
    }
    Ok(())
}

fn json_line(e: &Event) -> Result<Vec<u8>> {
    Ok(pulse_store::gojson::to_vec(e)?)
}
