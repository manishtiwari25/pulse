use anyhow::Result;
use std::io::{self, Read, Write};
use std::sync::Once;

use crate::amend::{ApplyError, apply_amendment, constitution_record, ratify};
use crate::digest::{
    ballot, ballot_text, digest_for, post_json, project_goal, proposals, proposals_text,
    queue_text, show,
};
use crate::records::records;
use crate::tally::{NoPostError, NoProjectError, compute, load_post};
use crate::text::{block_hash, fenced_block, text_hash};
use crate::thresholds::{Thresholds, parse_majority};

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(pulse_cli::Command { name: "board".into(), group: "agents".into(), read_only: true, summary: "the digest an agent reads before work".into(), usage: "pulse board [--project SLUG] [--area AREA] [--agent NAME] [--now TIME] [--json]".into(), flags: Some(|fs| fs.string_var("area", "", "one area only")), run: run_board, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "show".into(), group: "agents".into(), read_only: true, summary: "print one post".into(), usage: "pulse show <post-id> [--json]".into(), run: run_show, flags: None, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "queue".into(), group: "agents".into(), read_only: true, summary: "the experiment queue of one project, or work wanted across every project".into(), usage: "pulse queue [--project SLUG] [--agent NAME] [--limit N] [--now TIME] [--json]".into(), flags: Some(|fs| fs.int_var("limit", 0, "show at most N work items across projects (0 = all)")), run: run_queue, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "ballot".into(), group: "agents".into(), read_only: true, summary: "open suggestions and amendments you can still vote on".into(), usage: "pulse ballot [--project SLUG] [--agent NAME] [--now TIME] [--json]".into(), run: run_ballot, flags: None, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "proposals".into(), group: "human".into(), read_only: true, summary: "what waits for the human's yes or no, across every project".into(), usage: "pulse proposals [--project SLUG] [--json]".into(), run: run_proposals, flags: None, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "tally".into(), aliases: vec!["records".into()], group: "inspect".into(), read_only: true, summary: "the tally as typed records (what the dashboard reads)".into(), usage: "pulse tally [--project SLUG] [--area AREA] [--now TIME] [--quorum N] [--expiry-days N] [--stale-days N] [--amend-quorum N] [--amend-majority N/M] [--json]".into(), flags: Some(threshold_flags), run: run_tally, no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "hash".into(), group: "agents".into(), read_only: true, summary: "the text_hash of an amendment's proposed text (a post id, a file, or - for stdin)".into(), usage: "pulse hash <post-id|file|->".into(), run: run_hash, flags: None, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "constitution".into(), group: "agents".into(), read_only: true, summary: "print a project's constitution record (EVO-constitution)".into(), usage: "pulse constitution [--project SLUG] [--rev N] [--json]".into(), flags: Some(|fs| fs.int_var("rev", 0, "a past revision (default: current)")), run: run_constitution, aliases: vec![], no_store: false, raw: false });
        pulse_cli::register(pulse_cli::Command { name: "amend".into(), group: "agents".into(), summary: "amend apply <post-id>: write a passed amendment into the constitution record (idempotent)".into(), usage: "pulse amend apply <post-id> [--now TIME] [--json]".into(), run: run_amend, flags: None, aliases: vec![], no_store: false, read_only: false, raw: false });
    });
}

fn threshold_flags(fs: &mut pulse_cli::FlagSet) {
    fs.string_var("area", "", "one area only");
    fs.int_var("quorum", 0, "override the quorum");
    fs.int_var("expiry-days", 0, "override expiry_days");
    fs.int_var("stale-days", 0, "override stale_days");
    fs.int_var("amend-quorum", 0, "override amend_quorum");
    fs.string_var(
        "amend-majority",
        "",
        "override amend_majority (a fraction such as 2/3)",
    );
}

fn store(c: &pulse_cli::Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing store"))
}

fn thresholds_from(c: &pulse_cli::Context, fs: &pulse_cli::FlagSet) -> Result<Thresholds> {
    let mut th = Thresholds {
        now: c.now.clone(),
        quorum: fs
            .lookup("quorum")
            .map(|_| fs.get_int("quorum"))
            .unwrap_or(0),
        expiry_days: fs
            .lookup("expiry-days")
            .map(|_| fs.get_int("expiry-days"))
            .unwrap_or(0),
        stale_days: fs
            .lookup("stale-days")
            .map(|_| fs.get_int("stale-days"))
            .unwrap_or(0),
        amend_quorum: fs
            .lookup("amend-quorum")
            .map(|_| fs.get_int("amend-quorum"))
            .unwrap_or(0),
        ..Thresholds::default()
    };
    if fs.lookup("amend-majority").is_some() {
        let v = fs.get_string("amend-majority");
        if !v.is_empty() {
            parse_majority(&v).map_err(|_| {
                pulse_cli::errorf(
                    2,
                    &format!("A majority must be a fraction such as 2/3, not {v:?}"),
                    "amend-majority is a share of the counted votes",
                    "pulse tally --amend-majority 2/3",
                )
            })?;
            th.amend_majority = v;
        }
    }
    Ok(th)
}

fn area_flag(fs: &pulse_cli::FlagSet) -> String {
    fs.lookup("area")
        .map(|_| fs.get_string("area"))
        .unwrap_or_default()
}

fn current_project(c: &pulse_cli::Context) -> Result<String> {
    let s = store(c)?;
    if !c.project.is_empty() {
        if c.project == pulse_store::COMMONS_SLUG {
            return Ok(c.project.clone());
        }
        if let Some(p) = s.project(&c.project)? {
            return Ok(p.slug);
        }
        return Err(pulse_cli::errorf(
            2,
            &format!("No project {:?}", c.project),
            "it is not registered in this store",
            "pulse projects",
        )
        .into());
    }
    if let Some(p) = s.project_for_dir(&pulse_store::cwd())? {
        return Ok(p.slug);
    }
    Err(pulse_cli::errorf(
        2,
        "The current directory is not inside a registered project",
        "pulse needs to know which board you mean",
        "pulse <cmd> --project SLUG (or set PULSE_PROJECT, or run pulse init here)",
    )
    .into())
}

fn resolve_agent(c: &pulse_cli::Context) -> Result<String> {
    let ident = &c.agent;
    if ident.is_empty() {
        return Ok(String::new());
    }
    let s = store(c)?;
    let mut row = s.agent(ident)?;
    if row.is_none()
        && ident.contains('/')
        && let Ok(name) = s.conn()?.query_row(
            "SELECT name FROM agents WHERE legacy_id = ? ORDER BY last_active_at DESC LIMIT 1",
            [ident],
            |r| r.get::<_, String>(0),
        )
    {
        row = s.agent(&name)?;
    }
    let Some(row) = row else {
        return Ok(String::new());
    };
    if row.status == "pruned" {
        return Err(pulse_cli::errorf(
            2,
            &format!("Agent {:?} was pruned on {}", row.name, row.pruned_at),
            &row.pruned_reason,
            "pulse agent join --name NEW-NAME --runner R --model M",
        )
        .into());
    }
    Ok(row.name)
}

fn wrap(err: anyhow::Error) -> anyhow::Error {
    if let Some(np) = err.downcast_ref::<NoProjectError>() {
        return pulse_cli::errorf(
            2,
            &format!("No project {:?}", np.slug),
            "it is not registered in this store",
            "pulse projects",
        )
        .into();
    }
    if let Some(nn) = err.downcast_ref::<NoPostError>() {
        return pulse_cli::errorf(
            2,
            &format!("No post {:?}", nn.id),
            "no post has that id",
            "pulse board",
        )
        .into();
    }
    if let Some(ae) = err.downcast_ref::<ApplyError>() {
        return pulse_cli::errorf(2, &ae.what, &ae.why, &ae.fix).into();
    }
    err
}

fn run_board(c: &mut pulse_cli::Context, fs: &pulse_cli::FlagSet, _: &[String]) -> Result<()> {
    let slug = current_project(c)?;
    let agent = resolve_agent(c)?;
    let area = area_flag(fs);
    let warning_res = compute(
        store(c)?,
        &slug,
        &area,
        Thresholds {
            now: c.now.clone(),
            ..Thresholds::default()
        },
    )
    .map_err(wrap)?;
    let bare = warning_res
        .posts
        .iter()
        .filter(|p| pulse_board::is_bare_vote_summary(&p.kind, &p.summary))
        .count();
    if bare > 0 {
        writeln!(
            c.err,
            "warning: {bare} vote post(s) have a bare up/down summary; include an explanatory phrase in the vote summary"
        )?;
    }
    if c.json {
        let goal = project_goal(store(c)?, &slug)?;
        // Go: struct { *Result; Goal *goals.Goal `json:"goal,omitempty"` }
        #[derive(serde::Serialize)]
        struct BoardJson<'a> {
            #[serde(flatten)]
            result: &'a crate::TallyResult,
            #[serde(skip_serializing_if = "Option::is_none")]
            goal: Option<pulse_goals::Goal>,
        }
        return pulse_cli::write_json(
            &mut *c.out,
            &BoardJson {
                result: &warning_res,
                goal,
            },
        );
    }
    let session = if !agent.is_empty() && c.session.is_empty() {
        crate::digest::agent_session(store(c)?, &agent)
    } else {
        c.session.clone()
    };
    let text = digest_for(
        store(c)?,
        &slug,
        &area,
        Thresholds {
            now: c.now.clone(),
            ..Thresholds::default()
        },
        &agent,
        &session,
    )
    .map_err(wrap)?;
    c.out.write_all(text.as_bytes())?;
    Ok(())
}

fn run_show(c: &mut pulse_cli::Context, _: &pulse_cli::FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err(pulse_cli::errorf(
            2,
            "show needs one post id",
            "usage: pulse show <post-id>",
            "pulse board",
        )
        .into());
    }
    if c.json {
        let p = load_post(store(c)?, &args[0]).map_err(wrap)?;
        return pulse_cli::write_json(&mut *c.out, &post_json(&p));
    }
    let text = show(store(c)?, &args[0]).map_err(wrap)?;
    c.out.write_all(text.as_bytes())?;
    Ok(())
}

fn run_queue(c: &mut pulse_cli::Context, fs: &pulse_cli::FlagSet, _: &[String]) -> Result<()> {
    let limit = fs
        .lookup("limit")
        .map(|_| fs.get_int("limit").max(0) as usize)
        .unwrap_or(0);
    if c.project.is_empty() {
        let agent = resolve_agent(c)?;
        let mut q = crate::queue::work_wanted(
            store(c)?,
            None,
            &agent,
            Thresholds {
                now: c.now.clone(),
                ..Thresholds::default()
            },
        )
        .map_err(wrap)?;
        if c.json {
            if limit > 0 && limit < q.work.len() {
                q.work.truncate(limit);
            }
            return pulse_cli::write_json(&mut *c.out, &q);
        }
        let text = crate::queue::work_text(&q, limit);
        c.out.write_all(text.as_bytes())?;
        return Ok(());
    }
    let slug = current_project(c)?;
    let res = compute(
        store(c)?,
        &slug,
        "",
        Thresholds {
            now: c.now.clone(),
            ..Thresholds::default()
        },
    )
    .map_err(wrap)?;
    if c.json {
        let open: Vec<_> = res
            .suggestions
            .into_iter()
            .filter(|x| x.status != "closed")
            .collect();
        return pulse_cli::write_json(&mut *c.out, &open);
    }
    c.out.write_all(queue_text(&res).as_bytes())?;
    Ok(())
}

fn run_ballot(c: &mut pulse_cli::Context, _: &pulse_cli::FlagSet, _: &[String]) -> Result<()> {
    let slug = current_project(c)?;
    let agent = resolve_agent(c)?;
    let session = if !agent.is_empty() && c.session.is_empty() {
        crate::digest::agent_session(store(c)?, &agent)
    } else {
        c.session.clone()
    };
    let items = ballot(
        store(c)?,
        &slug,
        &agent,
        &session,
        Thresholds {
            now: c.now.clone(),
            ..Thresholds::default()
        },
    )
    .map_err(wrap)?;
    if c.json {
        let (sugs, ams) = crate::digest::split_ballot(&items);
        let goal = project_goal(store(c)?, &slug)?;
        // Go: result := map[string]any{...}; if goal != nil { result["goal"] = goal }
        return match goal {
            Some(goal) => pulse_cli::write_json(
                &mut *c.out,
                &pulse_store::gomap! {"project": slug, "agent": agent, "suggestions": sugs, "amendments": ams, "goal": goal},
            ),
            None => pulse_cli::write_json(
                &mut *c.out,
                &pulse_store::gomap! {"project": slug, "agent": agent, "suggestions": sugs, "amendments": ams},
            ),
        };
    }
    if let Some(goal) = project_goal(store(c)?, &slug)? {
        write!(c.out, "{}", pulse_goals::context(&goal))?;
        writeln!(c.out)?;
    }
    c.out
        .write_all(ballot_text(&slug, &agent, &items).as_bytes())?;
    Ok(())
}

fn run_proposals(c: &mut pulse_cli::Context, _: &pulse_cli::FlagSet, _: &[String]) -> Result<()> {
    if !c.project.is_empty()
        && c.project != pulse_store::COMMONS_SLUG
        && store(c)?.project(&c.project)?.is_none()
    {
        return Err(pulse_cli::errorf(
            2,
            &format!("No project {:?}", c.project),
            "it is not registered in this store",
            "pulse projects",
        )
        .into());
    }
    let items = proposals(store(c)?, &c.project).map_err(wrap)?;
    if c.json {
        // Go: if items == nil { items = []Proposal{} }
        return pulse_cli::write_json(&mut *c.out, &items.into_vec());
    }
    c.out.write_all(proposals_text(&items).as_bytes())?;
    Ok(())
}

fn run_tally(c: &mut pulse_cli::Context, fs: &pulse_cli::FlagSet, _: &[String]) -> Result<()> {
    let slug = current_project(c)?;
    let th = thresholds_from(c, fs)?;
    if c.json {
        let res = compute(store(c)?, &slug, &area_flag(fs), th).map_err(wrap)?;
        return pulse_cli::write_json(&mut *c.out, &res);
    }
    let text = records(store(c)?, &slug, &area_flag(fs), th).map_err(wrap)?;
    c.out.write_all(text.as_bytes())?;
    Ok(())
}

fn run_hash(c: &mut pulse_cli::Context, _: &pulse_cli::FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err(pulse_cli::errorf(
            2,
            "hash needs a post id, a file, or -",
            "usage: pulse hash <post-id|file|->",
            "pulse ballot",
        )
        .into());
    }
    let arg = &args[0];
    if arg == "-" {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        writeln!(c.out, "{}", text_hash(&text))?;
        return Ok(());
    }
    if let Ok(p) = load_post(store(c)?, arg) {
        if let Some(block) = fenced_block(&p.body, "Proposed text") {
            writeln!(c.out, "{}", block_hash(&block))?;
            return Ok(());
        }
        return Err(pulse_cli::errorf(
            1,
            &format!("No fenced block under '## Proposed text' in {arg}"),
            "an amendment quotes its proposed text in a fenced block under that heading",
            &format!("pulse show {arg}"),
        )
        .into());
    }
    let text = std::fs::read_to_string(arg).map_err(|_| {
        pulse_cli::errorf(
            2,
            &format!("No post or file {arg:?}"),
            "hash takes a post id, a file path, or - for stdin",
            "pulse ballot",
        )
    })?;
    writeln!(c.out, "{}", text_hash(&text))?;
    Ok(())
}

fn run_constitution(
    c: &mut pulse_cli::Context,
    fs: &pulse_cli::FlagSet,
    _: &[String],
) -> Result<()> {
    let slug = current_project(c)?;
    let rev = fs
        .lookup("rev")
        .map(|_| fs.get_int("rev") as i64)
        .unwrap_or(0);
    let (rec, revision) = constitution_record(store(c)?, &slug, rev).map_err(|e| {
        if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() {
            pulse_cli::errorf(2, &format!("{slug} has no constitution record"), "no record of kind evolution with slug constitution exists for it", &format!("pulse migrate --records --repo PATH --project {slug}  (or pulse record new evolution --title 'Constitution')")).into()
        } else { e }
    })?;
    let revision =
        revision.ok_or_else(|| anyhow::anyhow!("record {} has no requested revision", rec.id))?;
    if c.json {
        return pulse_cli::write_json(
            &mut *c.out,
            &serde_json::json!({"record": rec, "revision": revision}),
        );
    }
    c.out.write_all(revision.body.as_bytes())?;
    if !revision.body.ends_with('\n') {
        c.out.write_all(b"\n")?;
    }
    Ok(())
}

fn run_amend(c: &mut pulse_cli::Context, _: &pulse_cli::FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 2 || args[0] != "apply" {
        return Err(pulse_cli::errorf(
            2,
            "amend takes: apply <post-id>",
            "the only verb today is apply",
            "pulse amend apply <post-id>",
        )
        .into());
    }
    let applied = apply_amendment(
        store(c)?,
        &args[1],
        Thresholds {
            now: c.now.clone(),
            ..Thresholds::default()
        },
    )
    .map_err(wrap)?;
    if c.json {
        return pulse_cli::write_json(&mut *c.out, &applied);
    }
    if !applied.already_at.is_empty() {
        c.printf(format_args!(
            "already applied at {}: {} is at revision {}\n",
            applied.already_at, applied.record_id, applied.rev
        ))?;
        return Ok(());
    }
    let notice = ratify(store(c)?, &applied, &c.agent)?;
    c.printf(format_args!(
        "applied {}: {} revision {} by {}\n",
        applied.post_id, applied.record_id, applied.rev, applied.author
    ))?;
    c.printf(format_args!(
        "ratified: {} closes the amendment\n",
        notice.id
    ))?;
    Ok(())
}
