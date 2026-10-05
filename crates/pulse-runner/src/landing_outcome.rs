use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use serde_json::Value;

use crate::util::Row;

#[derive(Debug, Clone)]
struct Landing {
    seq: i64,
    at: String,
    detail: Value,
    sha: String,
    job: String,
    project: String,
}

struct ThreadTarget<'a> {
    area: &'a str,
    thread: &'a str,
}

pub(crate) fn tend_landing_outcomes(
    s: &pulse_store::Store,
    projects: &[Row],
    now: String,
    actor: String,
) -> Vec<String> {
    let hours = s
        .setting("evolve.outcome_hours", "24")
        .parse::<i64>()
        .unwrap_or(24)
        .max(1);
    let due_before = match offset_hours(&now, -hours) {
        Ok(v) => v,
        Err(err) => return vec![format!("landing outcome: invalid now {now}: {err}")],
    };
    let since = match offset_hours(&now, -(24 * 14)) {
        Ok(v) => v,
        Err(err) => return vec![format!("landing outcome: invalid horizon {now}: {err}")],
    };
    let mut lines = Vec::new();
    for project in projects.iter().filter(|p| p.int("brain") == 1) {
        match due_landings(s, &project.str("slug"), &since, &due_before) {
            Ok(landings) => {
                for landing in landings {
                    match measure_landing(s, &landing, hours, &now, &actor) {
                        Ok(line) => lines.push(line),
                        Err(err) => lines.push(format!(
                            "landing outcome: {} failed: {err}",
                            landing_label(&landing)
                        )),
                    }
                }
            }
            Err(err) => lines.push(format!(
                "landing outcome: {} scan failed: {err}",
                project.str("slug")
            )),
        }
    }
    lines
}

fn due_landings(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    due_before: &str,
) -> anyhow::Result<Vec<Landing>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT e.seq, e.at, e.detail
		FROM events e
		WHERE e.action = 'land.landed'
		AND e.at >= ?1 AND e.at <= ?2
		AND json_extract(e.detail, '$.project') = ?3
		AND NOT EXISTS (
			SELECT 1 FROM events m
			WHERE m.action = 'land.measured'
			AND (
				json_extract(m.detail, '$.landing_event') = e.seq
				OR (json_extract(e.detail, '$.sha') IS NOT NULL
					AND json_extract(m.detail, '$.sha') = json_extract(e.detail, '$.sha'))
			)
		)
		ORDER BY e.at, e.seq"#,
    )?;
    let mut rows = stmt.query(params![since, due_before, project])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let at: String = row.get(1)?;
        let detail_text: String = row.get(2)?;
        let detail: Value = serde_json::from_str(&detail_text).unwrap_or_default();
        if !touches_improver(&detail) {
            continue;
        }
        out.push(Landing {
            seq,
            at,
            sha: detail
                .get("sha")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            job: detail
                .get("job")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            project: project.to_string(),
            detail,
        });
    }
    Ok(out)
}

fn measure_landing(
    s: &pulse_store::Store,
    landing: &Landing,
    hours: i64,
    now: &str,
    actor: &str,
) -> anyhow::Result<String> {
    let outcome = pulse_evolve::outcome::outcome_for(s, &landing.at, hours)?;
    let regressed = pulse_evolve::outcome::regressed(&outcome);
    let (area, thread, on_post) = landing_thread(s, &landing.project, &landing.job)?;
    let short = short_sha(&landing.sha);
    let summary = crate::util::trunc(
        &format!(
            "Outcome of {short} after {hours}h: landings/worker-h {:.2} -> {:.2}{}, first pass {:.0}% -> {:.0}%, lead {:.0} -> {:.0} min",
            outcome.before.landings_per_hour,
            outcome.after.landings_per_hour,
            pulse_evolve::outcome::format_delta_pct(outcome.landings_per_hour_delta_pct),
            outcome.before.first_pass_pct,
            outcome.after.first_pass_pct,
            outcome.before.lead_minutes_median,
            outcome.after.lead_minutes_median
        ),
        240,
    );
    let mut body = pulse_evolve::outcome::outcome_lines(&outcome).join("\n");
    body.push_str(&format!(
        "\n\nMeasured across all projects; windows of {hours} h either side of the landing."
    ));
    crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "result".into(),
            project: landing.project.clone(),
            area: area.clone(),
            thread: thread.clone(),
            summary,
            body,
            agent: crate::board::actor_agent(s, actor.to_string()),
            on: on_post.clone(),
            resolution: "proceeded".into(),
            date: now.to_string(),
            ..Default::default()
        },
    )?;
    if regressed {
        post_revert_suggestion(
            s,
            landing,
            &outcome,
            hours,
            now,
            actor,
            ThreadTarget {
                area: &area,
                thread: &thread,
            },
        )?;
    }
    s.event(
        actor,
        "land.measured",
        landing_subject(landing).as_str(),
        serde_json::json!({
            "landing_event": landing.seq,
            "sha": landing.sha,
            "job": landing.job,
            "project": landing.project,
            "hours": hours,
            "before": outcome.before,
            "after": outcome.after,
            "regressed": regressed,
        }),
    )?;
    Ok(format!(
        "landing outcome: {short} landings/h {:.2}->{:.2}{}",
        outcome.before.landings_per_hour,
        outcome.after.landings_per_hour,
        pulse_evolve::outcome::format_delta_pct(outcome.landings_per_hour_delta_pct)
    ))
}

fn post_revert_suggestion(
    s: &pulse_store::Store,
    landing: &Landing,
    outcome: &pulse_evolve::outcome::Outcome,
    hours: i64,
    now: &str,
    actor: &str,
    target: ThreadTarget<'_>,
) -> anyhow::Result<()> {
    let file_class = first_improver_path(&landing.detail).unwrap_or("crates/pulse-runner/");
    let short = short_sha(&landing.sha);
    let body = format!(
        "Class: operating-layer; File: {file_class}; Variable: revert landing {}; Hypothesis: restoring the previous improver state recovers landings per worker-hour from {:.2} toward {:.2}; Tier: 1; Signal: land.measured event {}\n\nEvidence: after {hours} h, landings per worker-hour fell {:.2} -> {:.2}{}, with {} attempt(s) before and {} attempt(s) after. The daemon should not revert automatically; this card asks the normal loop to evaluate a revert.",
        landing.sha,
        outcome.after.landings_per_hour,
        outcome.before.landings_per_hour,
        landing.seq,
        outcome.before.landings_per_hour,
        outcome.after.landings_per_hour,
        pulse_evolve::outcome::format_delta_pct(outcome.landings_per_hour_delta_pct),
        outcome.before.attempts,
        outcome.after.attempts,
    );
    crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "suggestion".into(),
            project: landing.project.clone(),
            area: target.area.to_string(),
            thread: target.thread.to_string(),
            summary: crate::util::trunc(
                &format!("Consider reverting {short}: landing throughput regressed"),
                240,
            ),
            body,
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )?;
    Ok(())
}

fn landing_thread(
    s: &pulse_store::Store,
    _project: &str,
    job: &str,
) -> anyhow::Result<(String, String, String)> {
    if !job.is_empty() {
        let conn = s.conn()?;
        let source: String = conn
            .query_row("SELECT source FROM jobs WHERE id = ?", params![job], |r| {
                r.get(0)
            })
            .unwrap_or_default();
        if let Some(post_id) = source.strip_prefix("suggestion:") {
            let post = crate::board::post_row(&conn, post_id.to_string());
            if !post.0.is_empty() {
                return Ok((post.str("area"), post.str("thread"), post_id.to_string()));
            }
        }
    }
    Ok((
        "evolution".to_string(),
        "landing-outcomes".to_string(),
        String::new(),
    ))
}

fn touches_improver(detail: &Value) -> bool {
    first_improver_path(detail).is_some()
}

fn first_improver_path(detail: &Value) -> Option<&str> {
    detail
        .pointer("/code_delivery/code_paths")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .find(|p| pulse_evolve::report::is_improver_path(p))
}

fn offset_hours(at: &str, hours: i64) -> anyhow::Result<String> {
    Ok(
        (DateTime::parse_from_rfc3339(at)?.with_timezone(&Utc) + Duration::hours(hours))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 12 {
        sha[..12].to_string()
    } else if sha.is_empty() {
        "unknown".to_string()
    } else {
        sha.to_string()
    }
}

fn landing_subject(landing: &Landing) -> String {
    if !landing.sha.is_empty() {
        landing.sha.clone()
    } else {
        landing.job.clone()
    }
}

fn landing_label(landing: &Landing) -> String {
    let label = landing_subject(landing);
    if label.is_empty() {
        format!("event {}", landing.seq)
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{count, new_bare_repo, serial};
    use crate::util::{event_at, exec1, must_rows};

    const LANDING_AT: &str = "2026-09-04T10:00:00Z";
    const NOW: &str = "2026-09-05T11:00:00Z";

    fn brain_store() -> (crate::test_helpers::TestStore, Row) {
        let s = crate::test_helpers::new_store();
        let _ = new_bare_repo(&s);
        let conn = s.conn().unwrap();
        exec1(
            &conn,
            "UPDATE projects SET brain = 1 WHERE slug = 'proj'",
            &[],
        );
        let project = crate::board::project_by_slug(&conn, "proj".into());
        drop(conn);
        (s, project)
    }

    fn suggestion_job(s: &pulse_store::Store) -> String {
        let post = crate::board::create_post(
            s,
            LANDING_AT.to_string(),
            crate::board::PostOpts {
                kind: "suggestion".into(),
                project: "proj".into(),
                area: "evolution".into(),
                thread: "loop".into(),
                summary: "Improve runner".into(),
                body: "Class: operating-layer; File: crates/pulse-runner/src/lib.rs; Variable: tune; Hypothesis: better; Tier: 1; Signal: test".into(),
                agent: crate::board::actor_agent(s, "tester".into()),
                date: LANDING_AT.into(),
                ..Default::default()
            },
        )
        .unwrap();
        let job = "job-outcome";
        let conn = s.conn().unwrap();
        exec1(
            &conn,
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at, finished_at) VALUES (?, 'proj', 'improve', 'Improve runner', '', ?, 'done', ?, ?)",
            &[
                &job,
                &format!("suggestion:{}", post.str("id")),
                &LANDING_AT,
                &LANDING_AT,
            ],
        );
        job.to_string()
    }

    fn run_row(s: &pulse_store::Store, id: &str, job: &str, at: &str, seconds: f64) {
        let conn = s.conn().unwrap();
        exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, seconds, commits) VALUES (?, ?, 'agent', 'proj', ?, ?, 'done', ?, 1)",
            &[&id, &job, &at, &at, &seconds],
        );
    }

    fn landing(s: &pulse_store::Store, at: &str, job: &str, paths: Vec<&str>) {
        let conn = s.conn().unwrap();
        event_at(
            &conn,
            at,
            "daemon",
            "land.landed",
            job,
            serde_json::json!({
                "project": "proj",
                "job": job,
                "sha": "1234567890abcdef1234567890abcdef12345678",
                "code_delivery": {"code_paths": paths},
            }),
        );
    }

    #[test]
    fn due_improver_landing_posts_result_once() {
        let _serial = serial();
        let (s, project) = brain_store();
        let job = suggestion_job(&s);
        run_row(&s, "run-before", &job, "2026-09-04T09:00:00Z", 3600.0);
        run_row(&s, "run-after", &job, "2026-09-04T11:00:00Z", 3600.0);
        landing(&s, LANDING_AT, &job, vec!["crates/pulse-runner/src/lib.rs"]);

        let lines = tend_landing_outcomes(
            &s,
            std::slice::from_ref(&project),
            NOW.into(),
            "daemon".into(),
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.measured'",
                &[]
            ),
            1
        );
        let posts = must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'result'",
            &[],
        );
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0].str("area"), "evolution");
        assert_eq!(posts[0].str("thread"), "loop");
        assert_eq!(posts[0].str("resolution"), "proceeded");

        let again = tend_landing_outcomes(&s, &[project], NOW.into(), "daemon".into());
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.measured'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn non_improver_landing_is_ignored() {
        let _serial = serial();
        let (s, project) = brain_store();
        let job = suggestion_job(&s);
        landing(&s, LANDING_AT, &job, vec!["web/app.tsx"]);
        let lines = tend_landing_outcomes(&s, &[project], NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.measured'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn young_landing_waits_for_threshold() {
        let _serial = serial();
        let (s, project) = brain_store();
        let job = suggestion_job(&s);
        landing(
            &s,
            "2026-09-05T10:30:00Z",
            &job,
            vec!["crates/pulse-runner/src/lib.rs"],
        );
        let lines = tend_landing_outcomes(&s, &[project], NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn regression_posts_revert_suggestion() {
        let _serial = serial();
        let (s, project) = brain_store();
        let job = suggestion_job(&s);
        for i in 0..10 {
            let id = format!("before-{i}");
            let conn = s.conn().unwrap();
            exec1(
                &conn,
                "INSERT INTO jobs (id, project, kind, title, status, created_at, finished_at) VALUES (?, 'proj', 'improve', 'Before', 'done', '2026-09-04T08:00:00Z', '2026-09-04T09:00:00Z')",
                &[&id],
            );
            drop(conn);
            run_row(
                &s,
                &format!("run-{id}"),
                &id,
                "2026-09-04T09:00:00Z",
                3600.0,
            );
            let conn = s.conn().unwrap();
            event_at(
                &conn,
                "2026-09-04T09:30:00Z",
                "daemon",
                "land.landed",
                &id,
                serde_json::json!({"project": "proj", "job": id, "sha": format!("before{i}"), "code_delivery": {"code_paths": ["README.md"]}}),
            );
        }
        for i in 0..9 {
            let conn = s.conn().unwrap();
            event_at(
                &conn,
                "2026-09-04T11:00:00Z",
                "daemon",
                "land.failed-verify",
                &format!("after-failed-{i}"),
                serde_json::json!({"project": "proj", "job": format!("after-failed-{i}")}),
            );
        }
        for i in 0..10 {
            run_row(
                &s,
                &format!("run-after-{i}"),
                &job,
                "2026-09-04T11:00:00Z",
                3600.0,
            );
        }
        landing(&s, LANDING_AT, &job, vec!["crates/pulse-runner/src/lib.rs"]);

        let lines = tend_landing_outcomes(&s, &[project], NOW.into(), "daemon".into());
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'suggestion' AND summary LIKE 'Consider reverting%'",
                &[]
            ),
            1
        );
    }
}
