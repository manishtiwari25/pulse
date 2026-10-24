use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::tally::{Amendment, NoPostError, compute_light, load_post};
use crate::text::fenced_block;
use crate::thresholds::Thresholds;

pub const SYSTEM_AGENT: &str = "pulse";

pub fn constitution_id(s: &pulse_store::Store, project: &str) -> String {
    if s.brain().ok().flatten().is_some_and(|b| b.slug == project) {
        "EVO-constitution".into()
    } else {
        format!("EVO-constitution-{project}")
    }
}

fn has_records(s: &pulse_store::Store) -> bool {
    let Ok(conn) = s.conn() else {
        return false;
    };
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'records'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    n > 0
}

pub fn constitution_record(
    s: &pulse_store::Store,
    project: &str,
    rev: i64,
) -> Result<(pulse_store::Record, Option<pulse_store::Revision>)> {
    if !has_records(s) {
        return Err(anyhow!(pulse_store::ErrNoRecord));
    }
    let list = s.list_records(pulse_store::RecordFilter {
        project: project.into(),
        kind: "evolution".into(),
        slug: "constitution".into(),
        ..pulse_store::RecordFilter::default()
    })?;
    let mut id = String::new();
    if let Some(first) = list.first() {
        id = first.id.clone();
    } else {
        for cand in [
            format!("EVO-constitution-{project}"),
            "EVO-constitution".into(),
        ] {
            if let Ok((r, _)) = s.get_record(&cand, 0)
                && r.project == project
            {
                id = cand;
                break;
            }
        }
    }
    if id.is_empty() {
        return Err(anyhow!(pulse_store::ErrNoRecord));
    }
    s.get_record(&id, rev)
}

pub fn constitution_text(s: &pulse_store::Store, project: &str) -> String {
    if let Ok((_, Some(rev))) = constitution_record(s, project, 0) {
        return rev.body;
    }
    legacy_file(s, project, "constitution.md")
}

static LEGACY_FILES: AtomicBool = AtomicBool::new(true);

pub fn legacy_files() -> bool {
    LEGACY_FILES.load(Ordering::Relaxed)
}

pub fn set_legacy_files(v: bool) {
    LEGACY_FILES.store(v, Ordering::Relaxed);
}

fn legacy_file(s: &pulse_store::Store, project: &str, name: &str) -> String {
    if !legacy_files() || project.is_empty() || project == pulse_store::COMMONS_SLUG {
        return String::new();
    }
    let Ok(Some(p)) = s.project(project) else {
        return String::new();
    };
    if p.path.is_empty() {
        return String::new();
    }
    fs::read_to_string(Path::new(&p.path).join("docs").join("evolution").join(name))
        .unwrap_or_default()
}

pub fn stop_present(s: &pulse_store::Store, project: &str) -> bool {
    if s.setting("stop", "") == "1" {
        return true;
    }
    if !legacy_files() || project.is_empty() || project == pulse_store::COMMONS_SLUG {
        return false;
    }
    let Ok(Some(p)) = s.project(project) else {
        return false;
    };
    if p.path.is_empty() {
        return false;
    }
    Path::new(&p.path)
        .join("docs")
        .join("evolution")
        .join("STOP")
        .exists()
}

pub fn passed_amendments(
    s: &pulse_store::Store,
    project: &str,
    th: Thresholds,
) -> Result<Vec<Amendment>> {
    let res = compute_light(s, project, "", th)?;
    Ok(res
        .amendments
        .into_iter()
        .filter(|a| a.open && a.status.starts_with("passed"))
        .collect())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub post_id: String,
    pub project: String,
    pub record_id: String,
    pub rev: i64,
    pub hash: String,
    pub section: String,
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub already_at: String,
    pub notice: String,
}

pub fn apply_amendment(s: &pulse_store::Store, post_id: &str, th: Thresholds) -> Result<Applied> {
    let post = load_post(s, post_id)?;
    if post.kind != "amendment" {
        return Err(ApplyError {
            what: format!("{post_id} is a {}, not an amendment", post.kind),
            why: String::new(),
            fix: "pulse board".into(),
        }
        .into());
    }
    if let Some(at) = applied_at(s, post_id) {
        let (rec, _) = constitution_record(s, &post.project, 0)?;
        return Ok(Applied {
            post_id: post_id.into(),
            project: post.project,
            record_id: rec.id,
            rev: rec.current_rev,
            author: post.agent,
            already_at: at.clone(),
            notice: format!("amendment {post_id} was already applied at {at}"),
            ..Applied::default()
        });
    }
    let res = compute_light(s, &post.project, "", th)?;
    let am = res
        .amendments
        .iter()
        .find(|a| a.id == post_id)
        .ok_or_else(|| ApplyError {
            what: format!(
                "amendment {post_id} is not in the tally of {}",
                post.project
            ),
            why: String::new(),
            fix: format!("pulse board --project {}", post.project),
        })?;
    if am.status.starts_with("blocked: protected section") {
        return Err(ApplyError {
            what: format!("amendment {post_id} touches a protected section"),
            why: am.status.clone(),
            fix: format!("only the human ratifies it: pulse decide {post_id} yes --words '...'"),
        }
        .into());
    }
    if am.status.starts_with("passed; ratification paused") {
        return Err(ApplyError {
            what: format!("amendment {post_id} passed but ratification is paused"),
            why: am.status.clone(),
            fix: "pulse start".into(),
        }
        .into());
    }
    if !am.status.starts_with("passed") {
        return Err(ApplyError {
            what: format!("amendment {post_id} has not passed"),
            why: am.status.clone(),
            fix: format!("pulse ballot --project {}", post.project),
        }
        .into());
    }
    let (rec, rev) = constitution_record(s, &post.project, 0).map_err(|e| {
        if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() {
            ApplyError {
                what: format!("{} has no constitution record", post.project),
                why: "nothing to amend".into(),
                fix: format!(
                    "pulse migrate --records --repo PATH --project {}",
                    post.project
                ),
            }
            .into()
        } else {
            e
        }
    })?;
    let rev = rev.ok_or_else(|| anyhow!("record {} has no current revision", rec.id))?;
    let (body, ok) = replace_current_text(&rev.body, &post.body);
    if !ok {
        return Err(ApplyError {
            what: "the replaced text is not in the constitution exactly once".into(),
            why: "the record changed since the tally".into(),
            fix: format!("pulse constitution --project {}", post.project),
        }
        .into());
    }
    let new_rev = s.put_record(rec.clone(), rev.frontmatter, &body, &post.agent, post_id)?;
    let detail = pulse_store::Detail::json(serde_json::json!({
        "project": post.project,
        "record": rec.id,
        "rev": new_rev,
        "hash": am.hash,
        "section": am.section,
    }))?;
    s.event(&post.agent, "constitution.amended", post_id, detail)?;
    let rec_id = rec.id.clone();
    Ok(Applied {
        post_id: post_id.into(),
        project: post.project,
        record_id: rec_id.clone(),
        rev: new_rev,
        hash: am.hash.clone(),
        section: am.section.clone(),
        author: post.agent,
        notice: format!(
            "amendment {post_id} ratified: {rec_id} revision {new_rev} replaces the quoted text (section {}; {} up, {} down, {} voters)",
            am.section, am.up, am.down, am.voters
        ),
        ..Applied::default()
    })
}

#[derive(Debug, Clone)]
pub struct ApplyError {
    pub what: String,
    pub why: String,
    pub fix: String,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.what)?;
        if !self.why.is_empty() {
            write!(f, "\n  why: {}", self.why)?;
        }
        if !self.fix.is_empty() {
            write!(f, "\n  fix: {}", self.fix)?;
        }
        Ok(())
    }
}

impl std::error::Error for ApplyError {}

fn applied_at(s: &pulse_store::Store, post_id: &str) -> Option<String> {
    let conn = s.conn().ok()?;
    conn.query_row("SELECT at FROM events WHERE action = 'constitution.amended' AND subject = ? ORDER BY seq DESC LIMIT 1", [post_id], |r| r.get(0)).ok()
}

pub(crate) fn replace_current_text(constitution: &str, amendment_body: &str) -> (String, bool) {
    let Some(cur) = fenced_block(amendment_body, "Current text") else {
        return (String::new(), false);
    };
    let Some(prop) = fenced_block(amendment_body, "Proposed text") else {
        return (String::new(), false);
    };
    let mut lines: Vec<String> = constitution
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']).to_string())
        .collect();
    if lines.last().is_some_and(|s| s.is_empty()) {
        lines.pop();
    }
    let hay = format!("\n{}\n", lines.join("\n"));
    let needle = format!("\n{}\n", cur.join("\n"));
    if hay.matches(&needle).count() != 1 {
        return (String::new(), false);
    }
    (
        hay.replacen(&needle, &format!("\n{}\n", prop.join("\n")), 1)
            .trim_start_matches('\n')
            .to_string(),
        true,
    )
}

pub fn ratify(s: &pulse_store::Store, applied: &Applied, agent: &str) -> Result<pulse_board::Post> {
    let am = load_post(s, &applied.post_id).map_err(|e| {
        if let Some(np) = e.downcast_ref::<NoPostError>() {
            anyhow!(NoPostError { id: np.id.clone() })
        } else {
            e
        }
    })?;
    let mut summary = format!("ratified: {}", am.summary);
    if summary.chars().count() > pulse_board::MAX_SUMMARY_CHARS {
        summary = summary
            .chars()
            .take(pulse_board::MAX_SUMMARY_CHARS - 3)
            .collect::<String>()
            + "...";
    }
    let mut input = pulse_board::PostInput {
        kind: "result".into(),
        project: am.project,
        area: am.area,
        thread: am.thread,
        summary,
        body: format!(
            "{}\n\nRecord: {} revision {}\nHash: {}\n",
            applied.notice, applied.record_id, applied.rev, applied.hash
        ),
        on: applied.post_id.clone(),
        resolution: "ratified".into(),
        agent: agent.into(),
        ..pulse_board::PostInput::default()
    };
    if agent.is_empty() {
        input.agent = SYSTEM_AGENT.into();
        input.no_agent_check = true;
        input.runner = "pulse".into();
    }
    pulse_board::create_post(s, input)
}
