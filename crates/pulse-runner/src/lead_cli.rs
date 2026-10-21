//! `pulse lead`: seat, list, pause, resume, end and run the role leads of
//! a project, and ask which roles the project is missing.

use anyhow::{Result, anyhow};
use pulse_cli::{Context, FlagSet};
use pulse_tasks::leads::{self, Lead};
use serde::Serialize;
use std::io::Write;

pub(crate) fn lead_flags(fs: &mut FlagSet) {
    fs.string_var(
        "role",
        "",
        "add: the role word (rsi|docs|security|monitor, or a word of your own)",
    );
    fs.string_var(
        "title",
        "",
        "add: the lead's title; a built-in role brings its own",
    );
    fs.string_var(
        "brief",
        "",
        "add: the standing brief; a built-in role brings its own, a custom role needs one",
    );
    fs.string_var(
        "brief-file",
        "",
        "add: read the standing brief from a file (- for stdin)",
    );
    fs.string_var(
        "every",
        "",
        "add: how often the lead sits (30m, 6h, 1d, 1w); a built-in role has a default",
    );
    fs.string_var("note", "", "pause|end: why");
    fs.string_var("status", "", "list: active|paused|ended");
}

#[derive(Serialize)]
struct LeadOut<'a> {
    lead: &'a Lead,
}

#[derive(Serialize)]
struct LeadsOut<'a> {
    leads: &'a [Lead],
}

#[derive(Serialize)]
struct LeadShowOut<'a> {
    lead: &'a Lead,
    jobs: &'a [pulse_tasks::Job],
}

#[derive(Serialize)]
struct SuggestOut<'a> {
    project: &'a str,
    suggestions: &'a [crate::lead_roles::LeadGap],
    roles: Vec<RoleOut>,
}

#[derive(Serialize)]
pub(crate) struct RoleOut {
    pub role: String,
    pub title: String,
    pub about: String,
    pub every: String,
}

pub(crate) fn builtin_roles_out() -> Vec<RoleOut> {
    leads::BUILTIN_ROLES
        .iter()
        .map(|r| RoleOut {
            role: r.name.into(),
            title: r.title.into(),
            about: r.about.into(),
            every: r.every.into(),
        })
        .collect()
}

fn store(c: &Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow!("lead: store is needed"))
}

fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

fn usage(sub: &str) -> anyhow::Error {
    let what = if sub.is_empty() {
        "pulse lead needs a subcommand".to_string()
    } else {
        format!("pulse lead: unknown subcommand {sub:?}")
    };
    pulse_cli::errorf(2, &what, "see the usage", "pulse help lead").into()
}

fn need(args: &[String], fix: &str) -> Result<()> {
    if args.is_empty() {
        Err(pulse_cli::errorf(
            2,
            "Missing argument",
            &format!("the command needs {fix}"),
            "pulse help lead",
        )
        .into())
    } else {
        Ok(())
    }
}

fn need_project(c: &Context) -> Result<String> {
    if c.project.is_empty() {
        return Err(pulse_cli::errorf(
            2,
            "Missing --project",
            "a lead belongs to one project",
            "pulse lead ... --project SLUG",
        )
        .into());
    }
    Ok(c.project.clone())
}

fn read_body(inline: &str, file: &str) -> Result<String> {
    if !inline.is_empty() {
        return Ok(inline.to_string());
    }
    if file.is_empty() {
        return Ok(String::new());
    }
    if file == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        return Ok(text);
    }
    std::fs::read_to_string(file).map_err(|e| {
        pulse_cli::errorf(
            2,
            &format!("Cannot read {file}"),
            &e.to_string(),
            "--brief-file FILE or --brief TEXT",
        )
        .into()
    })
}

pub(crate) fn cmd_lead(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(usage(""));
    }
    let (sub, rest) = (args[0].as_str(), &args[1..]);
    match sub {
        "add" => {
            let project = need_project(c)?;
            let brief = read_body(
                &crate::commands::str_flag(fs, "brief"),
                &crate::commands::str_flag(fs, "brief-file"),
            )?;
            let lead = leads::add(
                store(c)?,
                leads::LeadInput {
                    project,
                    role: crate::commands::str_flag(fs, "role"),
                    title: crate::commands::str_flag(fs, "title"),
                    brief,
                    every: crate::commands::str_flag(fs, "every"),
                    author: actor(c),
                },
            )?;
            // The agent exists from the first minute, so the board and the
            // hub can name it before its first sitting.
            crate::lead_roles::ensure_role_lead(store(c)?, &lead)?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &LeadOut { lead: &lead });
            }
            writeln!(
                c.out,
                "{} seated in {}: {} ({}) as agent {}, sits every {}; the daemon queues the first sitting on its next tick",
                lead.id,
                lead.project,
                lead.title,
                lead.role,
                lead.agent,
                lead.every(),
            )?;
            Ok(())
        }
        "list" => {
            let list = leads::list(
                store(c)?,
                &c.project,
                &crate::commands::str_flag(fs, "status"),
            )?;
            if c.json {
                return pulse_cli::write_json(&mut c.out, &LeadsOut { leads: &list });
            }
            if list.is_empty() {
                writeln!(c.out, "No leads.")?;
                if !c.project.is_empty() {
                    writeln!(
                        c.out,
                        "pulse lead suggest --project {} names the roles this project is missing.",
                        c.project
                    )?;
                }
                return Ok(());
            }
            let rows = list
                .iter()
                .map(|l| {
                    vec![
                        l.id.clone(),
                        l.project.clone(),
                        l.role.clone(),
                        l.status.clone(),
                        l.every(),
                        if l.status == "active" {
                            l.next_at.clone()
                        } else {
                            String::new()
                        },
                        l.runs.to_string(),
                        l.agent.clone(),
                        l.title.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            pulse_cli::table(
                &mut c.out,
                c.color,
                Some(&[
                    "ID", "PROJECT", "ROLE", "STATUS", "EVERY", "NEXT", "RUNS", "AGENT", "TITLE",
                ]),
                &rows,
            )?;
            Ok(())
        }
        "show" => {
            need(rest, "a lead id")?;
            let lead = leads::get(store(c)?, &rest[0])?;
            let jobs = leads::jobs(store(c)?, &lead)?;
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &LeadShowOut {
                        lead: &lead,
                        jobs: &jobs,
                    },
                );
            }
            let st = c.style();
            writeln!(
                c.out,
                "{} {} {} lead ({}) {} every {}",
                st.bold(&lead.id),
                lead.project,
                lead.title,
                lead.role,
                lead.status,
                lead.every()
            )?;
            writeln!(
                c.out,
                "agent {}, seated by {} at {}",
                lead.agent, lead.created_by, lead.created_at
            )?;
            if lead.status == "active" {
                writeln!(c.out, "next sitting {}", lead.next_at)?;
            }
            if !lead.last_at.is_empty() {
                writeln!(
                    c.out,
                    "last sitting {} ({}), {} so far",
                    lead.last_at, lead.last_job, lead.runs
                )?;
            }
            if !lead.note.is_empty() {
                writeln!(c.out, "note: {}", lead.note)?;
            }
            writeln!(c.out)?;
            if lead.brief.trim().is_empty() {
                let about = leads::builtin(&lead.role).map(|r| r.about).unwrap_or("");
                writeln!(c.out, "Built-in brief: {about}")?;
            } else {
                writeln!(c.out, "{}", lead.brief.trim_end_matches('\n'))?;
            }
            if !jobs.is_empty() {
                writeln!(c.out)?;
                writeln!(c.out, "{}", st.bold("SITTINGS"))?;
                let rows = jobs
                    .iter()
                    .map(|j| {
                        vec![
                            j.id.clone(),
                            j.status.clone(),
                            j.created_at.clone(),
                            j.finished_at.clone(),
                            j.branch.clone(),
                        ]
                    })
                    .collect::<Vec<_>>();
                pulse_cli::table(
                    &mut c.out,
                    c.color,
                    Some(&["JOB", "STATUS", "CREATED", "FINISHED", "BRANCH"]),
                    &rows,
                )?;
            }
            Ok(())
        }
        "pause" | "end" | "resume" | "run" => {
            need(rest, "a lead id")?;
            let s = store(c)?;
            let who = actor(c);
            let note = crate::commands::str_flag(fs, "note");
            let lead = match sub {
                "pause" => leads::pause(s, &rest[0], &note, &who)?,
                "end" => leads::end(s, &rest[0], &note, &who)?,
                "resume" => leads::resume(s, &rest[0], &who)?,
                _ => leads::run_now(s, &rest[0], &who)?,
            };
            if c.json {
                return pulse_cli::write_json(&mut c.out, &LeadOut { lead: &lead });
            }
            if sub == "run" {
                writeln!(
                    c.out,
                    "{} sits at the daemon's next pass (within its interval)",
                    lead.id
                )?;
            } else {
                writeln!(c.out, "{} {}", lead.id, lead.status)?;
            }
            Ok(())
        }
        "suggest" => {
            let project = need_project(c)?;
            let s = store(c)?;
            if s.project(&project)?.is_none() {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("No project {project:?}"),
                    "it is not registered in this store",
                    "pulse projects",
                )
                .into());
            }
            let now = s.now();
            let gaps = crate::lead_roles::missing_leads(s, &project, &now);
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &SuggestOut {
                        project: &project,
                        suggestions: &gaps,
                        roles: builtin_roles_out(),
                    },
                );
            }
            if gaps.is_empty() {
                writeln!(
                    c.out,
                    "{project} shows no gap a built-in role would fill; a word of your own still works: pulse lead add --project {project} --role content --title \"Content lead\" --brief \"...\""
                )?;
                return Ok(());
            }
            for g in &gaps {
                writeln!(c.out, "{} ({}): {}", g.title, g.role, g.why)?;
                writeln!(c.out, "  {}", g.command)?;
            }
            Ok(())
        }
        _ => Err(usage(sub)),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{new_bare_repo, new_store};
    use crate::test_helpers_provider::{invoke, invoke_ok};

    #[test]
    fn lead_command_seats_lists_shows_and_ends() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let out = invoke_ok(&s, "lead", "proj", &["add", "--role", "monitor"]);
        assert!(
            out.contains("LEAD-001 seated in proj: Monitoring lead (monitor) as agent proj-lead-monitor, sits every 6h"),
            "{out}"
        );
        let agent = s.agent("proj-lead-monitor").unwrap().unwrap();
        assert_eq!(agent.kind, "lead");
        let (_, err, code) = invoke(&s, "lead", "proj", &["add", "--role", "monitor"]);
        assert!(
            code != 0 && err.contains("already has a monitor lead"),
            "{code} {err}"
        );
        let (_, err, code) = invoke(&s, "lead", "proj", &["add", "--role", "content"]);
        assert!(
            code != 0 && err.contains("title"),
            "custom role needs words: {code} {err}"
        );
        let out = invoke_ok(
            &s,
            "lead",
            "proj",
            &[
                "add",
                "--role",
                "content",
                "--title",
                "Content lead",
                "--brief",
                "Write one post a week.",
                "--every",
                "1w",
            ],
        );
        assert!(
            out.contains("LEAD-002 seated in proj: Content lead (content)"),
            "{out}"
        );
        let out = invoke_ok(&s, "lead", "proj", &["list"]);
        assert!(
            out.contains("LEAD-001") && out.contains("LEAD-002") && out.contains("monitor"),
            "{out}"
        );
        let out = invoke_ok(&s, "lead", "", &["show", "LEAD-002"]);
        assert!(out.contains("Write one post a week."), "{out}");
        let out = invoke_ok(&s, "lead", "proj", &["suggest"]);
        assert!(
            !out.contains("Monitoring lead"),
            "seated roles are not suggested: {out}"
        );
        let (_, err, code) = invoke(&s, "lead", "nope", &["suggest"]);
        assert!(code != 0 && err.contains("No project"), "{code} {err}");
        let out = invoke_ok(&s, "lead", "", &["pause", "LEAD-001", "--note", "holiday"]);
        assert!(out.contains("LEAD-001 paused"), "{out}");
        let out = invoke_ok(&s, "lead", "", &["resume", "LEAD-001"]);
        assert!(out.contains("LEAD-001 active"), "{out}");
        let out = invoke_ok(&s, "lead", "", &["run", "LEAD-001"]);
        assert!(
            out.contains("LEAD-001 sits at the daemon's next pass"),
            "{out}"
        );
        let out = invoke_ok(&s, "lead", "", &["end", "LEAD-002"]);
        assert!(out.contains("LEAD-002 ended"), "{out}");
        let out = invoke_ok(&s, "lead", "proj", &["list", "--status", "ended"]);
        assert!(
            out.contains("LEAD-002") && !out.contains("LEAD-001"),
            "{out}"
        );
        let (_, _, code) = invoke(&s, "lead", "", &["dance"]);
        assert_ne!(code, 0);
    }
}
