use crate::register::{bool_flag, json_output, no_args, or_dash, store, string_flag, trunc};
use anyhow::Result;
use pulse_cli::Context;
use pulse_cli::flags::FlagSet;
use rusqlite::params_from_iter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn run_projects(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let projects = store(c)?.projects(false)?;
    if c.json {
        return json_output(c, &projects);
    }
    for p in projects {
        let brain = if p.brain { "  (brain)" } else { "" };
        c.printf(format_args!(
            "{:<18} {:<8} {}{}  {}\n",
            p.slug, p.status, p.path, brain, p.remote
        ))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub project: String,
    pub kind: String,
    pub tier: i64,
    pub title: String,
    pub brief: String,
    pub source: String,
    pub priority: i64,
    pub status: String,
    pub created_at: String,
    pub created_by: String,
    pub lease_agent: String,
    pub lease_until: String,
    pub generation: i64,
    pub attempts: i64,
    pub finished_at: String,
    pub result: String,
    pub branch: String,
    pub recovery: String,
}

pub fn jobs(s: &pulse_store::Store, project: &str, include_finished: bool) -> Result<Vec<Job>> {
    let mut q = String::from(
        r#"SELECT j.id, j.project, j.kind, j.tier, j.title, j.brief, j.source, j.priority, j.status,
		j.created_at, j.created_by, j.lease_agent, j.lease_until, j.generation, j.attempts, j.finished_at, j.result, j.branch,
		COALESCE(e.at, ''), COALESCE(e.detail, '')
		FROM jobs j
		LEFT JOIN events e ON e.seq = (
			SELECT seq FROM events WHERE action = 'job.recover' AND subject = j.id ORDER BY seq DESC LIMIT 1
		)
		WHERE 1=1"#,
    );
    let mut params = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND j.project = ?");
        params.push(project.to_string());
    }
    if !include_finished {
        q.push_str(" AND j.status IN ('queued', 'leased', 'blocked')");
    }
    q.push_str(" ORDER BY j.status, j.priority DESC, j.created_at");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        let recover_at: String = r.get(18)?;
        let recover_detail: String = r.get(19)?;
        Ok(Job {
            id: r.get(0)?,
            project: r.get(1)?,
            kind: r.get(2)?,
            tier: r.get(3)?,
            title: r.get(4)?,
            brief: r.get(5)?,
            source: r.get(6)?,
            priority: r.get(7)?,
            status: r.get(8)?,
            created_at: r.get(9)?,
            created_by: r.get(10)?,
            lease_agent: r.get(11)?,
            lease_until: r.get(12)?,
            generation: r.get(13)?,
            attempts: r.get(14)?,
            finished_at: r.get(15)?,
            result: r.get(16)?,
            branch: r.get(17)?,
            recovery: recovery_text(&recover_at, &recover_detail),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn recovery_text(at: &str, detail: &str) -> String {
    if detail.is_empty() {
        return String::new();
    }
    let Ok(doc) = serde_json::from_str::<BTreeMap<String, Value>>(detail) else {
        return String::new();
    };
    match doc.get("outcome").and_then(Value::as_str).unwrap_or("") {
        "requeued" => format!("ran again {at}"),
        "replanned" => doc
            .get("job")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|job| format!("re-planned as {job}"))
            .unwrap_or_else(|| "re-planned".into()),
        "held" => format!(
            "held: {}",
            doc.get("why").and_then(Value::as_str).unwrap_or("")
        ),
        "retried" => format!("retried by hand {at}"),
        _ => String::new(),
    }
}

pub fn run_jobs(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let jobs = jobs(store(c)?, &c.project, bool_flag(fs, "all"))?;
    if c.json {
        return json_output(c, &jobs);
    }
    if jobs.is_empty() {
        c.println("no jobs")?;
    }
    for j in jobs {
        let lease = if j.status == "leased" {
            format!(" by {} until {}", j.lease_agent, j.lease_until)
        } else {
            String::new()
        };
        c.printf(format_args!(
            "{:<9} {:<14} {:<8} tier {}  {:<34} {}{}\n",
            j.status,
            j.project,
            j.kind,
            j.tier,
            j.id,
            trunc(&j.title, 60),
            lease
        ))?;
        if !j.result.is_empty() || !j.recovery.is_empty() {
            let mut detail = trunc(&j.result, 100);
            if !j.recovery.is_empty() {
                if !detail.is_empty() {
                    detail.push_str("  ");
                }
                detail.push_str("recovery: ");
                detail.push_str(&trunc(&j.recovery, 100));
            }
            c.printf(format_args!("          {detail}\n"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentScorecard {
    #[serde(flatten)]
    pub agent: pulse_store::Agent,
    #[serde(flatten)]
    pub presence: pulse_board::AgentPresence,
    pub findings: i64,
    pub suggestions: i64,
    pub votes: i64,
    pub answers: i64,
    pub results: i64,
    pub n: i64,
    pub last_post: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<pulse_board::Focus>,
}

pub fn agent_scorecards(
    s: &pulse_store::Store,
    include_pruned: bool,
) -> Result<Vec<AgentScorecard>> {
    let presence = pulse_board::agent_presences(s, "")?;
    let agents = s.agents("")?;
    let focuses = pulse_board::agent_focuses(s, &s.now())?;
    let mut out = Vec::new();
    let mut indices = BTreeMap::new();
    for a in agents {
        if !include_pruned && a.status == "pruned" {
            continue;
        }
        let name = a.name.clone();
        indices.insert(name.clone(), out.len());
        out.push(AgentScorecard {
            presence: presence.get(&name).cloned().unwrap_or_default(),
            focus: focuses.get(&name).cloned(),
            agent: a,
            ..Default::default()
        });
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT agent, SUM(kind = 'finding'), SUM(kind = 'suggestion'), SUM(kind = 'vote'),
		SUM(kind = 'answer'), SUM(kind = 'result'), COUNT(*), MAX(date) FROM posts GROUP BY agent",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;
    for row in rows {
        let (name, findings, suggestions, votes, answers, results, n, last_post) = row?;
        if let Some(index) = indices.get(&name).copied() {
            out[index].findings = findings;
            out[index].suggestions = suggestions;
            out[index].votes = votes;
            out[index].answers = answers;
            out[index].results = results;
            out[index].n = n;
            out[index].last_post = last_post;
        }
    }
    out.sort_by(|a, b| {
        a.agent
            .status
            .cmp(&b.agent.status)
            .then_with(|| b.agent.last_active_at.cmp(&a.agent.last_active_at))
            .then_with(|| a.agent.name.cmp(&b.agent.name))
    });
    Ok(out)
}

pub fn run_agents(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let agents = agent_scorecards(store(c)?, bool_flag(fs, "all"))?;
    if c.json {
        return json_output(c, &agents);
    }
    for a in agents {
        c.printf(format_args!(
            "{:<16} {:<7} {:<8} presence {:<7} now {:<36} posts {:<4} findings {:<3} suggestions {:<3} votes {:<3} jobs {}/{}  last heartbeat {}  last useful {}\n",
            a.agent.name,
            a.agent.status,
            a.agent.kind,
            a.presence.presence,
            focus_text(a.focus.as_ref()),
            a.agent.posts,
            a.findings,
            a.suggestions,
            a.votes,
            a.agent.jobs_done,
            a.agent.jobs_done + a.agent.jobs_failed,
            or_dash(&a.presence.last_heartbeat_at),
            or_dash(&a.agent.last_useful_at)
        ))?;
    }
    Ok(())
}

pub fn focus_text(f: Option<&pulse_board::Focus>) -> String {
    let Some(f) = f else {
        return "-".into();
    };
    if !f.job_id.is_empty() {
        let mut text = f.job_id.clone();
        if !f.job_kind.is_empty() {
            text.push(' ');
            text.push_str(&f.job_kind);
        }
        if !f.thread.is_empty() {
            text.push_str(" @ ");
            text.push_str(&f.thread);
        }
        if !f.executor.is_empty() {
            text.push_str(" on ");
            text.push_str(&f.executor);
        }
        return text;
    }
    if let Some(claim) = f.claims.first() {
        return format!("claim {}", claim.thread);
    }
    "-".into()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: String,
}

pub fn events(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    limit: i32,
) -> Result<Vec<Event>> {
    let mut q =
        "SELECT e.seq, e.at, e.actor, e.action, e.subject, e.detail FROM events e WHERE 1=1"
            .to_string();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if !project.is_empty() {
        q.push_str(
            r#" AND (e.subject = ? OR e.subject IN (SELECT id FROM posts WHERE project = ?)
			OR e.subject IN (SELECT id FROM jobs WHERE project = ?)
			OR e.subject IN (SELECT id FROM runs WHERE project = ?)
			OR CASE WHEN json_valid(e.detail) THEN json_extract(e.detail, '$.project') END = ?)"#,
        );
        for _ in 0..5 {
            params.push(project.to_string().into());
        }
    }
    if !since.is_empty() {
        if let Ok(seq) = since.parse::<i64>()
            && seq >= 0
        {
            q.push_str(" AND e.seq > ?");
            params.push(seq.into());
        } else {
            let at = chrono::DateTime::parse_from_rfc3339(since).map_err(|_| {
                pulse_cli::errorf(
                    2,
                    "pulse: --since takes a non-negative event sequence or a UTC time like 2026-09-05T08:00:00Z",
                    "",
                    "",
                )
            })?;
            if at.offset().local_minus_utc() != 0 || since.len() != 20 || !since.ends_with('Z') {
                return Err(pulse_cli::errorf(
                    2,
                    "pulse: --since takes a non-negative event sequence or a UTC time like 2026-09-05T08:00:00Z",
                    "",
                    "",
                )
                .into());
            }
            q.push_str(" AND e.at > ?");
            params.push(at.format("%Y-%m-%dT%H:%M:%SZ").to_string().into());
        }
    }
    q.push_str(" ORDER BY e.seq DESC LIMIT ?");
    params.push((limit as i64).into());
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        Ok(Event {
            seq: r.get(0)?,
            at: r.get(1)?,
            actor: r.get(2)?,
            action: r.get(3)?,
            subject: r.get(4)?,
            detail: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn run_events(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let mut events = events(
        store(c)?,
        &c.project,
        &string_flag(fs, "since"),
        fs.get_int("limit"),
    )?;
    {
        let store_clone = c.store.clone();
        let redact = pulse_bus::redactor(store_clone.as_ref());
        for e in &mut events {
            let masked = redact(pulse_bus::Event {
                id: e.seq,
                at: e.at.clone(),
                actor: e.actor.clone(),
                action: e.action.clone(),
                subject: e.subject.clone(),
                detail: e.detail.clone(),
            });
            *e = Event {
                seq: masked.id,
                at: masked.at,
                actor: masked.actor,
                action: masked.action,
                subject: masked.subject,
                detail: masked.detail,
            };
        }
    }
    if c.json {
        return json_output(c, &events);
    }
    for e in events {
        c.printf(format_args!(
            "{} {:<8} {:<18} {:<16} {:<40} {}\n",
            e.at,
            e.seq,
            e.action,
            trunc(&e.actor, 16),
            trunc(&e.subject, 40),
            trunc(&e.detail, 80)
        ))?;
    }
    Ok(())
}

pub fn join_counts(counts: &[crate::status::StatusCount]) -> String {
    counts
        .iter()
        .map(|c| format!("{} {}", c.n, c.status))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn jobs_recovery_field_and_text() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,tier,title,brief,status,created_at,created_by,finished_at,result) VALUES ('job-held','demo','task',1,'Held job','','failed','2026-09-06T19:00:00Z','human','2026-09-06T19:00:00Z','failed'),('job-plain','demo','task',1,'Plain job','','failed','2026-09-06T19:00:00Z','human','2026-09-06T19:00:00Z','failed')",
        );
        s.event(
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            "job-held",
            serde_json::json!({"outcome":"replanned","root":"job-held","job":"job-copy"}),
        )
        .unwrap();
        s.event(
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            "job-held",
            serde_json::json!({"outcome":"held","why":"owner must inspect"}),
        )
        .unwrap();
        let rows = jobs(&s, "", true).unwrap();
        let by_id: std::collections::BTreeMap<_, _> =
            rows.into_iter().map(|j| (j.id.clone(), j)).collect();
        assert_eq!(by_id["job-held"].recovery, "held: owner must inspect");
        assert_eq!(by_id["job-plain"].recovery, "");
        let text = invoke(Some(s.clone()), false, "", "jobs", &["--all"]);
        assert!(
            text.out.contains("recovery: held: owner must inspect"),
            "{text:?}"
        );
        let js = invoke(Some(s), true, "", "jobs", &["--all"]);
        let decoded: Vec<Job> = serde_json::from_str(&js.out).unwrap();
        let by_id: std::collections::BTreeMap<_, _> =
            decoded.into_iter().map(|j| (j.id.clone(), j)).collect();
        assert_eq!(by_id["job-held"].recovery, "held: owner must inspect");
        assert_eq!(by_id["job-plain"].recovery, "");
    }

    #[test]
    fn recovery_text_phrases() {
        assert_eq!(
            recovery_text("2026-09-06T19:00:00Z", r#"{"outcome":"requeued"}"#),
            "ran again 2026-09-06T19:00:00Z"
        );
        assert_eq!(
            recovery_text("x", r#"{"outcome":"replanned","job":"job-new"}"#),
            "re-planned as job-new"
        );
        assert_eq!(
            recovery_text("x", r#"{"outcome":"held","why":"needs owner"}"#),
            "held: needs owner"
        );
        assert_eq!(
            recovery_text("2026-09-06T19:00:00Z", r#"{"outcome":"retried"}"#),
            "retried by hand 2026-09-06T19:00:00Z"
        );
        assert_eq!(recovery_text("x", ""), "");
    }

    #[test]
    fn agent_listings_keep_identity_history_and_separate_heartbeat() {
        let (_dir, s) = temporary_store();
        for (name, kind) in [
            ("idle-worker", "worker"),
            ("idle-lead", "lead"),
            ("old-session", "session"),
            ("legacy", "legacy"),
            ("pruned", "worker"),
        ] {
            pulse_board::join_agent(
                &s,
                pulse_board::JoinInput {
                    name: name.into(),
                    kind: kind.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        s.conn().unwrap().execute("UPDATE agents SET status='pruned', pruned_reason='no useful output for 14 days' WHERE name='pruned'", []).unwrap();
        let result = invoke(Some(s.clone()), true, "", "agents", &["--all"]);
        assert_eq!(result.code, 0, "{result:?}");
        assert!(result.out.contains("last_heartbeat_at"));
        let text = invoke(Some(s), false, "", "agents", &[]);
        assert!(text.out.contains("presence"));
        assert!(!text.out.contains("pruned"));
    }

    #[test]
    fn agent_listings_show_focus() {
        let (_dir, s) = temporary_store();
        pulse_board::join_agent(
            &s,
            pulse_board::JoinInput {
                name: "aurora".into(),
                kind: "worker".into(),
                ..Default::default()
            },
        )
        .unwrap();
        s.conn().unwrap().execute("INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until) VALUES('job-1','proj','task','Do it','leased',?,'aurora','2026-09-06T20:00:00Z')", [&TEST_NOW]).unwrap();
        let text = invoke(Some(s), false, "", "agents", &[]);
        assert!(text.out.contains("now job-1 task"), "{text:?}");
    }

    #[test]
    fn events_filters() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,title,created_at) VALUES ('job-one','one','task','test','2026-09-06T19:00:00Z'); INSERT INTO posts(id,project,area,thread,kind,agent,date,created_at) VALUES ('post-one','one','a','t','finding','human','2026-09-06T19:00:00Z','2026-09-06T19:00:00Z')",
        );
        for (subject, detail) in [
            ("one", "direct"),
            ("job-one", "job"),
            ("post-one", "post"),
            ("local", r#"{"project":"one"}"#),
            ("two", r#"{"project":"two"}"#),
        ] {
            s.event("human", "test", subject, detail).unwrap();
        }
        assert_eq!(events(&s, "one", "", -1).unwrap().len(), 4);
        assert_eq!(
            events(&s, "one", "2026-09-06T18:59:59Z", 40).unwrap().len(),
            4
        );
        assert_eq!(events(&s, "one", TEST_NOW, 40).unwrap().len(), 0);
        assert!(events(&s, "", "2026-99-99T19:00:00Z", 40).is_err());
        assert!(events(&s, "", "yesterday", 40).is_err());
    }

    #[test]
    fn scorecards_pruning_and_counts() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO agents(name,status,created_at,last_active_at,posts,jobs_done,jobs_failed) VALUES ('old','pruned','2026-09-06T19:00:00Z','2026-01-01T00:00:00Z',99,2,3),('a','active','2026-09-06T19:00:00Z','2026-09-06T19:00:00Z',0,0,0)",
        );
        exec_sql(
            &s,
            "INSERT INTO posts(id,project,area,thread,kind,agent,date,created_at) VALUES ('p1','p','a','t','finding','old','2026-09-06T19:00:00Z','2026-09-06T19:00:00Z')",
        );
        assert!(
            agent_scorecards(&s, true)
                .unwrap()
                .iter()
                .any(|c| c.agent.name == "old" && c.findings == 1)
        );
        let old = agent_scorecards(&s, true)
            .unwrap()
            .into_iter()
            .find(|c| c.agent.name == "old")
            .unwrap();
        assert_eq!(old.answers, 0, "answer");
        assert_eq!(old.results, 0, "result");
        assert_eq!(old.suggestions, 0, "suggestion");
        assert!(
            !agent_scorecards(&s, false)
                .unwrap()
                .iter()
                .any(|c| c.agent.name == "old")
        );
    }

    #[test]
    fn list_outputs_preserve_fixed_widths() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,title,status,created_at,result,lease_agent,lease_until) VALUES ('long','a-project-slug-longer-than-the-column','task','界界界界界界界界界界界界','leased','2026-09-06T19:00:00Z','success','worker','2026-09-07T19:00:00Z')",
        );
        let got = invoke(Some(s), false, "", "jobs", &["--all"]);
        assert!(got.out.contains("a-project-slug-longer-than-the-column"));
        assert!(got.out.contains("界"));
        for status in ["blocked", "finished", "queued"] {
            assert!(!got.out.contains(status));
        }
        assert!(got.out.contains(" by worker until 2026-09-07T19:00:00Z"));
    }

    #[test]
    fn events_redact_quoted_and_historical_private_paths() {
        let (_dir, s) = temporary_store();
        let old_root = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let old_path = old_root
            .path()
            .join("privé-🔒")
            .join("\"fixtures\"\\<heldout>&")
            .display()
            .to_string();
        let new_path = new_root
            .path()
            .join("replacement-fixtures")
            .display()
            .to_string();
        s.set_setting("human", "evolve.heldout_dir", &old_path)
            .unwrap();
        let payload =
            serde_json::json!({"path": old_path, "message": format!("scoring in {old_path}")});
        let python_json = pulse_store::py_json(&payload).unwrap();
        let html_json = serde_json::to_string(&payload)
            .unwrap()
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e");
        let plain_json = serde_json::to_string(&payload).unwrap();
        let double_json = serde_json::to_string(&python_json).unwrap();
        for detail in [
            python_json.as_str(),
            html_json.as_str(),
            plain_json.trim(),
            double_json.as_str(),
        ] {
            s.event(
                "human",
                "fixture.quoted",
                &format!("{old_path}/result"),
                detail,
            )
            .unwrap();
        }
        s.set_setting("human", "evolve.heldout_dir", &new_path)
            .unwrap();
        s.event(
            "human",
            "fixture.current",
            &format!("{new_path}/result"),
            format!("scoring in {new_path}").as_str(),
        )
        .unwrap();
        for cleared in [false, true] {
            if cleared {
                s.set_setting("human", "evolve.heldout_dir", "").unwrap();
            }
            let before = events(&s, "", "", -1).unwrap();
            for as_json in [false, true] {
                let got = invoke(Some(s.clone()), as_json, "", "events", &["--limit", "-1"]);
                assert_eq!(got.code, 0, "{got:?}");
                assert!(
                    !got.out.contains(&old_path)
                        && !got.err.contains(&old_path)
                        && !got.out.contains(&new_path)
                        && !got.err.contains(&new_path),
                    "JSON={as_json} cleared={cleared}: {got:?}"
                );
                assert!(
                    got.out.contains("[owner-private]/result"),
                    "ordinary event subjects were not redacted: {got:?}"
                );
                if as_json {
                    let decoded_events: Vec<Event> = serde_json::from_str(&got.out).unwrap();
                    for event in decoded_events {
                        if event.action != "fixture.quoted" {
                            continue;
                        }
                        let mut decoded: serde_json::Value =
                            serde_json::from_str(&event.detail).unwrap();
                        if let Some(inner) = decoded.as_str() {
                            decoded = serde_json::from_str(inner).unwrap();
                        }
                        let text = format!("{decoded}");
                        assert!(
                            !text.contains(&old_path)
                                && !text.contains(&new_path)
                                && text.contains("[owner-private]"),
                            "decoded event detail exposed a path: {text}"
                        );
                    }
                }
            }
            let after = events(&s, "", "", -1).unwrap();
            assert_eq!(before, after, "redaction changed canonical event rows");
        }
    }
}
