use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::tally::{
    Amendment, Post, Proposal, Suggestion, TallyResult, compute, load_post, same_session,
};
use crate::text::{plural, trunc};
use crate::thresholds::Thresholds;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BallotItem {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<Suggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amendment: Option<Amendment>,
}

pub fn ballot(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
    session: &str,
    th: Thresholds,
) -> Result<Vec<BallotItem>> {
    let res = compute(s, project, "", th)?;
    let mut session = session.to_string();
    if !agent.is_empty() && session.is_empty() {
        session = agent_session(s, agent);
    }
    Ok(limit_ballot_items(
        res.ballot(agent, &session),
        res.params.quorum,
        ballot_cap(s),
    ))
}

impl TallyResult {
    pub fn ballot(&self, agent: &str, session: &str) -> Vec<BallotItem> {
        let mut voted: BTreeMap<String, bool> = BTreeMap::new();
        if !agent.is_empty() {
            for v in &self.svotes {
                if v.agent == agent {
                    voted.insert(v.on.clone(), true);
                }
            }
            for v in &self.avotes {
                if v.agent == agent {
                    voted.insert(v.id.clone(), true);
                }
            }
        }
        let mut out = Vec::new();
        for x in &self.suggestions {
            if x.status != "needs votes" || !crate::tally::duplicate_of(Some(self), x).is_empty() {
                continue;
            }
            if !agent.is_empty()
                && (x.agent == agent
                    || voted.contains_key(&x.id)
                    || same_session(session, &x.session))
            {
                continue;
            }
            out.push(BallotItem {
                kind: "suggestion".into(),
                suggestion: Some(x.clone()),
                amendment: None,
            });
        }
        for a in &self.amendments {
            if !a.open {
                continue;
            }
            if !agent.is_empty()
                && (a.agent == agent
                    || voted.contains_key(&a.id)
                    || same_session(session, &a.session))
            {
                continue;
            }
            out.push(BallotItem {
                kind: "amendment".into(),
                suggestion: None,
                amendment: Some(a.clone()),
            });
        }
        limit_ballot_items(out, self.params.quorum, 0)
    }
}

pub fn ballot_cap(s: &pulse_store::Store) -> usize {
    let n = s
        .setting("lead.ballot_cap", "10")
        .trim()
        .parse::<usize>()
        .unwrap_or(10);
    if n < 1 { 10 } else { n }
}

pub fn order_ballot_suggestions(sugs: &[Suggestion], quorum: i32) -> Vec<Suggestion> {
    let mut out = sugs.to_vec();
    out.sort_by(|a, b| {
        (quorum - a.voters)
            .cmp(&(quorum - b.voters))
            .then_with(|| a.date.cmp(&b.date))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

pub fn limit_ballot_suggestions(sugs: &[Suggestion], quorum: i32, cap: usize) -> Vec<Suggestion> {
    let mut out = order_ballot_suggestions(sugs, quorum);
    if cap > 0 && out.len() > cap {
        out.truncate(cap);
    }
    out
}

pub fn limit_ballot_items(items: Vec<BallotItem>, quorum: i32, cap: usize) -> Vec<BallotItem> {
    let mut sugs = Vec::new();
    let mut ams = Vec::new();
    for it in items {
        if let Some(sg) = it.suggestion {
            sugs.push(sg);
        } else {
            ams.push(it);
        }
    }
    let sugs = limit_ballot_suggestions(&sugs, quorum, cap);
    let mut out: Vec<BallotItem> = sugs
        .into_iter()
        .map(|sg| BallotItem {
            kind: "suggestion".into(),
            suggestion: Some(sg),
            amendment: None,
        })
        .collect();
    out.extend(ams);
    out
}

pub(crate) fn split_ballot(items: &[BallotItem]) -> (Vec<Suggestion>, Vec<Amendment>) {
    let mut sugs = Vec::new();
    let mut ams = Vec::new();
    for it in items {
        if let Some(sg) = &it.suggestion {
            sugs.push(sg.clone());
        }
        if let Some(a) = &it.amendment {
            ams.push(a.clone());
        }
    }
    (sugs, ams)
}

pub(crate) fn agent_session(s: &pulse_store::Store, agent: &str) -> String {
    if let Ok(env) = std::env::var("PULSE_SESSION")
        && !env.is_empty()
    {
        return env;
    }
    s.agent(agent)
        .ok()
        .flatten()
        .and_then(|a| a.sessions.last().cloned())
        .unwrap_or_default()
}

pub fn proposals(
    s: &pulse_store::Store,
    project: &str,
) -> Result<pulse_store::gojson::GoSlice<Proposal>> {
    let slugs = if project.is_empty() {
        s.projects(true)?
            .into_iter()
            .map(|p| p.slug)
            .collect::<Vec<_>>()
    } else {
        vec![project.to_string()]
    };
    // JSON null, not [], when nothing was appended.
    let mut out = pulse_store::gojson::GoSlice::nil();
    for slug in slugs {
        out.extend(compute(s, &slug, "", Thresholds::default())?.proposals);
    }
    Ok(out)
}

pub fn proposals_text(items: &[Proposal]) -> String {
    let mut b = String::new();
    for pr in items {
        let _ = writeln!(
            b,
            "{:<10} {:<14} {:<38} {}  {}",
            pr.status,
            pr.project,
            pr.key,
            pr.id,
            trunc(&pr.summary, 70)
        );
        if pr.status == "awaiting" {
            b.push_str("           decide: pulse decide ");
            b.push_str(&pr.id);
            b.push_str(" yes|no --words '...'\n");
        }
    }
    if items.is_empty() {
        b.push_str("no proposals\n");
    }
    b
}

pub fn queue_text(res: &TallyResult) -> String {
    let mut b = String::new();
    let mut any = false;
    for x in &res.suggestions {
        if x.status == "closed" {
            continue;
        }
        any = true;
        let _ = writeln!(
            b,
            "{:+3}  {:<12} {:<38} {:<44} up {} down {} voters {}  {}",
            x.score,
            x.status,
            x.key,
            x.id,
            x.up,
            x.down,
            x.voters,
            trunc(&x.summary, 50)
        );
    }
    if !any {
        b.push_str(
            "  nothing queued - post a card with `pulse post suggestion --project <name> --area <area> --thread <thread> --summary \"<one line>\" ...` or run `pulse ballot` to vote on open cards\n",
        );
    }
    b
}

pub fn ballot_text(project: &str, agent: &str, items: &[BallotItem]) -> String {
    let who = if agent.is_empty() { "everyone" } else { agent };
    let mut b = String::new();
    let _ = writeln!(
        b,
        "BALLOT for {who} in {project} (vote with: pulse vote POST_ID up|down --summary REASON)"
    );
    let (sugs, ams) = split_ballot(items);
    for x in sugs {
        let _ = writeln!(
            b,
            "  suggestion  {:<12} {:<38} {:<44} up {} down {} voters {}  {}",
            x.status,
            x.key,
            x.id,
            x.up,
            x.down,
            x.voters,
            trunc(&x.summary, 50)
        );
    }
    for a in ams {
        let _ = writeln!(
            b,
            "  amendment   {:<12} {:<38} {:<44} up {} down {} voters {}/{}  hash {}  {}",
            trunc(&a.status, 12),
            a.key,
            a.id,
            a.up,
            a.down,
            a.voters,
            a.quorum,
            prefix(&a.hash, 12),
            trunc(&a.summary, 40)
        );
    }
    if items.is_empty() {
        b.push_str("  nothing to vote on\n");
    }
    b
}

fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn digest(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
    agent: &str,
) -> Result<String> {
    let session = if agent.is_empty() {
        String::new()
    } else {
        agent_session(s, agent)
    };
    digest_for(s, project, area, th, agent, &session)
}

pub(crate) fn project_goal(
    s: &pulse_store::Store,
    project: &str,
) -> Result<Option<pulse_goals::Goal>> {
    if project == pulse_store::COMMONS_SLUG {
        return Ok(None);
    }
    Ok(Some(pulse_goals::get(s, project)?))
}

pub fn digest_for(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
    agent: &str,
    session: &str,
) -> Result<String> {
    let res = compute(s, project, area, th.clone())?;
    let goal = project_goal(s, project)?;
    let shared = if project == pulse_store::COMMONS_SLUG {
        crate::shared::SharedBoard::default()
    } else {
        crate::shared::shared_board(s, th.clone())?
    };
    let brain_slug = s.brain()?.map(|b| b.slug).unwrap_or_default();
    let params = &res.params;
    let mut b = String::new();
    macro_rules! w {
        ($($arg:tt)*) => {{
            let _ = writeln!(b, $($arg)*);
        }};
    }
    let area_note = if area.is_empty() {
        String::new()
    } else {
        format!(" area {area}")
    };
    w!(
        "PULSE {}{}  (now {}; store {})",
        project,
        area_note,
        res.now,
        real_path(&s.path)
    );
    if let Some(goal) = goal {
        w!("{}", pulse_goals::context(&goal));
    }
    w!(
        "NOTICES (who is asking for a vote; one vote per agent, latest wins, no self-votes - pulse guide)"
    );
    if project == pulse_store::COMMONS_SLUG {
        w!(
            "  this is the commons: posts here are data for every project; it has no queue and no loop"
        );
    } else {
        let mut items = res.ballot(agent, session);
        if !area.is_empty() {
            items = compute(s, project, "", th.clone())?.ballot(agent, session);
        }
        let (sg, am) = split_ballot(&items);
        let agent_note = if agent.is_empty() {
            String::new()
        } else {
            format!(" --agent {agent}")
        };
        w!(
            "  votes wanted here: {} {}, {} {} open - pulse ballot --project {}{}",
            sg.len(),
            plural(sg.len(), "suggestion"),
            am.len(),
            plural(am.len(), "amendment"),
            project,
            agent_note
        );
        if let Some(brain) = s.brain()? {
            if brain.slug != project {
                let bres = compute(s, &brain.slug, "", th.clone())?;
                let (bs, ba) = split_ballot(&bres.ballot(agent, session));
                w!(
                    "  brain {}: {} {}, {} {} open for votes - pulse ballot --project {}",
                    brain.slug,
                    bs.len(),
                    plural(bs.len(), "suggestion"),
                    ba.len(),
                    plural(ba.len(), "amendment"),
                    brain.slug
                );
            } else {
                w!("  this project is the brain: suggestions about PULSE itself belong here");
            }
        }
        w!("  {}", shared.notice(&res.now));
    }
    let awaiting = res
        .proposals
        .iter()
        .filter(|p| p.status == "awaiting")
        .count();
    w!(
        "  for the human: {} {} awaiting a yes or no - pulse proposals",
        awaiting,
        plural(awaiting, "proposal")
    );
    if s.setting("stop", "") == "1" {
        w!("  the runner is stopped (pulse start to resume)");
    }
    if params.stop_present {
        w!("  STOP is present: the loop and ratification are paused until the owner lifts it");
    }
    if !agent.is_empty() {
        let mentions = pulse_board::mentions_for(s, project, agent, 10)?;
        let total = pulse_board::mention_count(s, project, agent)?;
        if total > 0 {
            w!("");
            w!("MENTIONS FOR YOU ({total})");
            for p in mentions {
                w!(
                    "  {:<44} {:<38} by {:<16} {}",
                    p.id,
                    format!("{}/{}", p.area, p.thread),
                    trunc(&p.agent, 16),
                    trunc(&p.summary, 60)
                );
            }
        }
    }
    w!("");
    w!(
        "ACTIVE CLAIMS (advisory, never locks; claim_hours {})",
        s.setting("claim_hours", "")
    );
    let mut n = 0;
    for c in &res.claims {
        if c.status == "active" {
            w!(
                "  {:<38} {:<16} until {:<20} {}",
                c.key,
                trunc(&c.agent, 16),
                c.expires,
                trunc(&c.summary, 60)
            );
            n += 1;
        }
    }
    if n == 0 {
        w!("  none");
    }
    w!("");
    w!("OPEN THREADS");
    n = 0;
    for t in &res.threads {
        if t.status != "closed" {
            w!(
                "  {:<38} {:>2} posts  last {:<20} {}",
                t.key,
                t.posts,
                t.last,
                trunc(&t.last_summary, 60)
            );
            n += 1;
        }
    }
    if n == 0 {
        w!("  none");
    }
    w!("");
    if project != pulse_store::COMMONS_SLUG {
        b.push_str(&crate::shared::shared_board_text(
            &shared,
            project,
            &brain_slug,
        ));
        w!("");
    }
    w!(
        "PROPOSALS FOR THE HUMAN (the agents agreed; only the human decides - `pulse decide POST_ID yes|no`)"
    );
    n = 0;
    for pr in &res.proposals {
        if pr.status == "awaiting" {
            w!(
                "  awaiting   {:<38} since {:<20} {}",
                pr.key,
                pr.date,
                trunc(&pr.summary, 60)
            );
            w!("             read: pulse show {}", pr.id);
            n += 1;
        }
    }
    for pr in &res.proposals {
        if pr.status != "awaiting" {
            w!(
                "  {:<10} {:<38} on {:<23} {}",
                pr.status,
                pr.key,
                pr.result_date,
                trunc(&pr.summary, 60)
            );
            n += 1;
        }
    }
    if n == 0 {
        w!("  none");
    }
    w!("");
    w!(
        "EXPERIMENT QUEUE (score = up - down; quorum {} distinct voters; unvoted expire after {} days; built = a run landed it, tried = its job failed)",
        params.quorum,
        params.expiry_days
    );
    n = 0;
    for x in &res.suggestions {
        if x.status != "closed" {
            w!(
                "  {:+3}  {:<12} {:<38} {:<44} up {} down {} voters {}  {}",
                x.score,
                x.status,
                x.key,
                x.id,
                x.up,
                x.down,
                x.voters,
                trunc(&x.summary, 50)
            );
            n += 1;
        }
    }
    if n == 0 {
        w!("  none");
    }
    w!("");
    w!(
        "AMENDMENTS (supermajority {}/{} of counted votes; quorum {} distinct sessions, proposer excluded)",
        params.amend_num,
        params.amend_den,
        params.amend_quorum
    );
    n = 0;
    for a in &res.amendments {
        if a.open {
            w!("  {:<38} {}", a.key, a.id);
            w!("      {}", a.status);
            w!(
                "      up {} down {}  voters {}/{}  {}% up  identities {}  uncounted: {} self, {} hash mismatch, {} no Read line",
                a.up,
                a.down,
                a.voters,
                a.quorum,
                a.pct,
                a.nid,
                a.self_,
                a.badhash,
                a.noread
            );
            n += 1;
        }
    }
    if n == 0 {
        w!("  none open");
    }
    w!("");
    w!(
        "STALE (expired claims, threads idle > {} days)",
        params.stale_days
    );
    n = 0;
    for c in &res.claims {
        if c.status == "expired" || c.status == "no expiry" {
            let exp = if c.expires.is_empty() {
                "(no expiry)"
            } else {
                &c.expires
            };
            w!(
                "  claim      {:<38} {:<16} expired {}",
                c.key,
                trunc(&c.agent, 16),
                exp
            );
            n += 1;
        }
    }
    for t in &res.threads {
        if t.status == "stale" {
            w!("  thread     {:<38} idle since {}", t.key, t.last);
            n += 1;
        }
    }
    if res.nokind > 0 {
        w!("  {} post(s) without a kind", res.nokind);
        n += 1;
    }
    if n == 0 {
        w!("  none");
    }
    w!("");
    if project != pulse_store::COMMONS_SLUG {
        w!("JOBS (the runner's queue for this project; pulse jobs for every project)");
        n = 0;
        for j in open_jobs(s, project)? {
            let extra = if j.status == "leased" {
                format!(" by {} until {}", j.lease_agent, j.lease_until)
            } else {
                String::new()
            };
            w!(
                "  {:<7} {:<8} {:<28} tier {}  {}{}",
                j.status,
                j.kind,
                j.id,
                j.tier,
                trunc(&j.title, 50),
                extra
            );
            n += 1;
        }
        if n == 0 {
            w!("  none queued");
        }
        w!("");
    }
    w!(
        "AGENTS (unique names; pruned after {} days without useful output)",
        s.setting("prune_after_days", "")
    );
    let rows = scorecards(s)?;
    let mut others = 0;
    for r in &rows {
        if r.posts > 1 && r.kind != "legacy" {
            let last = if r.last_useful.is_empty() {
                "-"
            } else {
                &r.last_useful
            };
            w!(
                "  {:<16} {:<7} {:<8} posts {:<4} jobs {}/{}  last useful {}",
                r.name,
                r.status,
                r.kind,
                r.posts,
                r.jobs_done,
                r.jobs_done + r.jobs_failed,
                last
            );
        } else {
            others += 1;
        }
    }
    if others > 0 {
        w!("  +{} single-post or legacy agents", others);
    }
    if rows.is_empty() {
        w!("  none joined yet: pulse agent join --name NAME --runner R --model M");
    }
    Ok(b)
}

fn real_path(p: &str) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| PathBuf::from(p))
        .display()
        .to_string()
}

#[derive(Default)]
struct JobRow {
    id: String,
    kind: String,
    status: String,
    title: String,
    lease_agent: String,
    lease_until: String,
    tier: i32,
}
fn open_jobs(s: &pulse_store::Store, project: &str) -> Result<Vec<JobRow>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT id, kind, status, tier, title, lease_agent, lease_until FROM jobs WHERE project = ? AND status IN ('queued', 'leased', 'blocked') ORDER BY status, priority DESC, created_at")?;
    Ok(stmt
        .query_map([project], |r| {
            Ok(JobRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                status: r.get(2)?,
                tier: r.get(3)?,
                title: r.get(4)?,
                lease_agent: r.get(5)?,
                lease_until: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

#[derive(Default)]
struct Scorecard {
    name: String,
    status: String,
    kind: String,
    last_useful: String,
    posts: i32,
    jobs_done: i32,
    jobs_failed: i32,
}
fn scorecards(s: &pulse_store::Store) -> Result<Vec<Scorecard>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT name, status, kind, posts, jobs_done, jobs_failed, last_useful_at FROM agents WHERE status != 'pruned' ORDER BY status, last_active_at DESC, name")?;
    Ok(stmt
        .query_map([], |r| {
            Ok(Scorecard {
                name: r.get(0)?,
                status: r.get(1)?,
                kind: r.get(2)?,
                posts: r.get(3)?,
                jobs_done: r.get(4)?,
                jobs_failed: r.get(5)?,
                last_useful: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn show(s: &pulse_store::Store, post_id: &str) -> Result<String> {
    let p = load_post(s, post_id)?;
    let mut b = String::new();
    b.push_str("---\n");
    for (k, v) in frontmatter(&p) {
        let _ = writeln!(b, "{k}: {v}");
    }
    let _ = writeln!(b, "project: {}", p.project);
    b.push_str("---\n");
    b.push_str(p.body.trim_end_matches('\n'));
    b.push('\n');
    Ok(b)
}

pub(crate) fn frontmatter(p: &Post) -> Vec<(String, String)> {
    let mut out = vec![
        ("kind".into(), p.kind.clone()),
        ("agent".into(), p.agent.clone()),
        ("runner".into(), p.runner.clone()),
        ("model".into(), p.model.clone()),
        ("session".into(), p.session.clone()),
        ("date".into(), p.date.clone()),
        ("branch".into(), p.branch.clone()),
        ("summary".into(), p.summary.clone()),
    ];
    for (k, v) in [
        ("expires", &p.expires),
        ("on", &p.on_post),
        ("vote", &p.vote),
        ("resolution", &p.resolution),
        ("text_hash", &p.text_hash),
        ("links", &p.links),
        ("files", &p.files),
        ("name", &p.name),
        ("project", &p.origin_project),
    ] {
        if !v.is_empty() {
            out.push((k.into(), v.clone()));
        }
    }
    if p.for_owner {
        out.push(("for_owner".into(), "true".into()));
    }
    out.push(("id".into(), p.id.clone()));
    out.push(("area".into(), p.area.clone()));
    out.push(("thread".into(), p.thread.clone()));
    out
}

pub fn post_json(p: &Post) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    for (k, v) in frontmatter(p) {
        m.insert(k, Value::String(v));
    }
    m.insert("path".into(), Value::String(format!("store:{}", p.id)));
    m.insert("project".into(), Value::String(p.project.clone()));
    if !p.origin_project.is_empty() {
        m.insert(
            "origin_project".into(),
            Value::String(p.origin_project.clone()),
        );
    }
    m.insert("body".into(), Value::String(p.body.clone()));
    m
}

#[cfg(test)]
mod empty_queue_tests {
    use super::*;
    use crate::tally::TallyResult;

    #[test]
    fn queue_text_reports_empty_state() {
        let res = TallyResult::default();
        let text = queue_text(&res);
        assert!(text.starts_with("  nothing queued - "), "{text}");
        assert!(text.contains("pulse post suggestion"), "{text}");
        for flag in ["--project", "--area", "--thread", "--summary"] {
            assert!(text.contains(flag), "hint lacks {flag}: {text}");
        }
        assert!(text.contains("pulse ballot"), "{text}");
    }
}
