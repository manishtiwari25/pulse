use crate::compute::*;
use crate::messages::*;
use crate::schedules;
use crate::tasks::*;
use anyhow::{Result, anyhow};
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

pub fn set_stdin_for_tests(data: Option<Vec<u8>>) {
    *STDIN_BUF.lock().expect("stdin lock poisoned") = data;
}

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command { name:"task".into(), group:"agents".into(), summary:"the owner's tasks for a project: new, list, show, amend, cancel, status, reply".into(), usage:"pulse task new --project P --title T [--brief-file F | --brief TEXT | stdin] [--priority N]\npulse task list [--project P] [--status S] [--json]\npulse task show TASK-NNN [--json]\npulse task amend TASK-NNN [--title T] [--brief-file F | --brief TEXT | stdin] [--note WHY]\npulse task cancel TASK-NNN [--reason WHY]\npulse task status TASK-NNN <open|planned|in_progress|review|done|cancelled>\npulse task reply TASK-NNN <text> | --file F".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_task, flags:Some(task_flags) });
        pulse_cli::register(Command { name:"msg".into(), group:"agents".into(), summary:"the conversation between the owner and a project's lead: send, list, read".into(), usage:"pulse msg send --project P [--task TASK-NNN] <text> | --file F\npulse msg list [--project P] [--task TASK-NNN] [--since MSG-ID] [--unread] [--json]\npulse msg read MSG-ID... (mark read for --agent)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_msg, flags:Some(msg_flags) });
        pulse_cli::register(Command { name:"compute".into(), group:"agents".into(), summary:"more machines for agents: request (agents), list, approve|deny (the owner)".into(), usage:"pulse compute request --project P [--size S] --hours N --reason WHY\npulse compute list [--project P] [--status S] [--json]\npulse compute approve CR-NNN [--words WHY]   (owner)\npulse compute deny CR-NNN [--words WHY]      (owner)\npulse compute providers [--json]             (the providers this binary can provision with)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_compute, flags:Some(compute_flags) });
        pulse_cli::register(Command { name:"schedule".into(), group:"agents".into(), summary:"a standing job for a project - queued again every cadence: add, list, show, pause, resume, end, run".into(), usage:"pulse schedule add --project P --title T --every 30m|6h|1d|2w [--at HH:MM] [--kind task|improve|clean|reflect] [--brief TEXT | --brief-file F | stdin] [--tier 0|1] [--priority N] [--times N]\npulse schedule list [--project P] [--status active|paused|ended] [--json]\npulse schedule show SCHED-NNN [--json]\npulse schedule pause SCHED-NNN [--note WHY]\npulse schedule resume SCHED-NNN\npulse schedule end SCHED-NNN [--note WHY]\npulse schedule run SCHED-NNN               (due now; the daemon queues its job on the next tick)".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:run_schedule, flags:Some(schedule_flags) });
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
    fs.string_var("kind", "task", "task, improve, clean or reflect (add)");
    fs.string_var(
        "brief",
        "",
        "the brief of every job (add); or --brief-file, or stdin",
    );
    fs.string_var("brief-file", "", "file holding the brief; - for stdin");
    fs.int_var("tier", 1, "0 or 1 (add)");
    fs.int_var("priority", 0, "priority, higher first (add)");
    fs.int_var("times", 0, "end after this many jobs; 0 keeps going (add)");
    fs.string_var("note", "", "why (pause, end)");
    fs.string_var("status", "", "list only active, paused or ended schedules");
}

fn task_flags(fs: &mut FlagSet) {
    fs.string_var("title", "", "task title (new, amend)");
    fs.string_var(
        "brief",
        "",
        "brief text (new, amend); or --brief-file, or stdin",
    );
    fs.string_var("brief-file", "", "file holding the brief; - for stdin");
    fs.int_var("priority", 0, "priority, higher first (new)");
    fs.string_var("status", "", "filter by status (list)");
    fs.string_var("note", "", "why this revision (amend)");
    fs.string_var("reason", "", "why (cancel)");
    fs.string_var("file", "", "file holding the reply text (reply)");
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

fn need(args: &[String], n: usize, fix: &str) -> Result<()> {
    if args.len() < n {
        Err(pulse_cli::errorf(
            2,
            "Missing argument",
            &format!("the command needs {fix}"),
            "pulse help task",
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
    printf(
        c,
        format_args!(
            "{} job, tier {}, priority {}; set by {} at {}\n",
            sc.kind, sc.tier, sc.priority, sc.created_by, sc.created_at
        ),
    )?;
    if sc.status == "active" {
        printf(c, format_args!("next at {}\n", sc.next_at))?;
    }
    if !sc.last_at.is_empty() {
        printf(
            c,
            format_args!("last at {} ({})\n", sc.last_at, sc.last_job),
        )?;
    }
    if !sc.note.is_empty() {
        printf(c, format_args!("note: {}\n", sc.note))?;
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
            let brief = read_body(&fs.get_string("brief"), &fs.get_string("brief-file"))?;
            let sc = schedules::add(
                store(c)?,
                schedules::ScheduleInput {
                    project: c.project.clone(),
                    title: fs.get_string("title"),
                    kind: fs.get_string("kind"),
                    brief,
                    tier: fs.get_int("tier"),
                    priority: fs.get_int("priority"),
                    every: fs.get_string("every"),
                    at: fs.get_string("at"),
                    times: fs.get_int("times"),
                    author: actor(c),
                },
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
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
                        sc.status.clone(),
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
                        sc.created_by.clone(),
                        sc.title.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID", "PROJECT", "STATUS", "EVERY", "NEXT", "RUNS", "BY", "TITLE",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need(rest, 1, "a schedule id")?;
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
            c.println("")?;
            c.println(sc.brief.trim_end_matches('\n'))?;
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
            need(rest, 1, "a schedule id")?;
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
            need(rest, 1, "a schedule id")?;
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
            need(rest, 1, "a schedule id")?;
            let sc = schedules::run_now(store(c)?, &rest[0], &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &ScheduleOut { schedule: &sc });
            }
            printf(
                c,
                format_args!(
                    "{} due now; the daemon queues its job on the next tick\n",
                    sc.id
                ),
            )
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
            need(rest, 1, "a task id")?;
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
                printf(
                    c,
                    format_args!("  {} {} {}: {}\n", m.at, m.id, m.author, m.body),
                )?;
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
            need(rest, 1, "a task id")?;
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
            need(rest, 1, "a task id")?;
            let t = cancel(store(c)?, &rest[0], &fs.get_string("reason"), &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(c, format_args!("{} cancelled\n", t.id))
        }
        "status" => {
            need(rest, 2, "a task id and a status")?;
            let t = set_status(store(c)?, &rest[0], &rest[1], &actor(c))?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &TaskOut { task: &t });
            }
            printf(c, format_args!("{} is {}\n", t.id, t.status))
        }
        "reply" => {
            need(rest, 1, "a task id")?;
            let body = read_body(&rest[1..].join(" "), &fs.get_string("file"))?;
            let m = reply(store(c)?, &rest[0], &actor(c), &body)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MessageOut { message: &m });
            }
            printf(c, format_args!("{} sent on {}\n", m.id, m.task_id))
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
            let m = send_message(
                store(c)?,
                &c.project,
                &fs.get_string("task"),
                &actor(c),
                &body,
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &MessageOut { message: &m });
            }
            printf(c, format_args!("{} sent in {}\n", m.id, m.project))
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
                printf(
                    c,
                    format_args!(
                        "{} {} {} {} {}: {}\n",
                        m.at,
                        m.id,
                        m.project,
                        task,
                        c.style().bold(&m.author),
                        m.body
                    ),
                )?;
            }
            Ok(())
        }
        "read" => {
            need(rest, 1, "one or more message ids")?;
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
            let r = request_compute(
                store(c)?,
                &c.project,
                &actor(c),
                &fs.get_string("size"),
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
            need(rest, 1, "a request id")?;
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
