//! Skeleton port of internal/runner/idle.go.
#![allow(dead_code)]

use chrono::TimeDelta;
use rusqlite::params;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::util::Row;

/// Go: func idleNote(s *store.Store, slug, now string) string
pub(crate) fn idle_note(s: &pulse_store::Store, slug: String, now: String) -> String {
    let mut parts = vec![suggestion_idle_part(s, slug.clone(), now.clone())];
    if let Ok(handoffs) = pulse_tally::open_handoffs_for(s, &slug) {
        let part = handoff_idle_part(s, handoffs);
        if !part.is_empty() {
            parts.push(part);
        }
    }
    let part = question_idle_part(s, slug.clone(), now.clone());
    if !part.is_empty() {
        parts.push(part);
    }
    let suggestions = crate::tally::suggestion_records(s, slug.clone(), now.clone());
    let part = ballot_idle_part(s, slug.clone(), now.clone(), suggestions);
    if !part.is_empty() {
        parts.push(part);
    }
    parts.push(reflect_idle_part(s, slug.clone(), now.clone()));
    parts.push(research_idle_part(s, slug.clone(), now));
    if let Ok(st) = pulse_evolve::status(s, &slug) {
        if st.level == "D" {
            parts.push("evolve: level D".into());
        } else {
            parts.push(format!("evolve: {}", crate::util::trunc(&st.reason, 120)));
        }
    }
    parts.join("; ")
}

/// Go: func suggestionIdlePart(s *store.Store, slug, now string) string
pub(crate) fn suggestion_idle_part(s: &pulse_store::Store, slug: String, now: String) -> String {
    let res = pulse_tally::compute_light(
        s,
        &slug,
        "",
        pulse_tally::Thresholds {
            now: now.clone(),
            ..Default::default()
        },
    )
    .ok();
    let suggestions = res
        .as_ref()
        .map(|r| r.suggestions.clone())
        .unwrap_or_default();
    let plan_ctx = pulse_tally::plan_context_for(s, &slug, &now, res.as_ref());
    let mut eligible = 0;
    let mut plannable = 0;
    let mut in_flight = 0;
    let mut net_non_positive = 0;
    let mut owner = 0;
    let mut no_evidence = 0;
    let mut over_operating_cap = 0;
    let mut duplicates = 0;
    let mut needs_votes = 0;
    let mut rejected = 0;
    let mut tried = 0;
    let project_path = s
        .conn()
        .ok()
        .map(|conn| crate::board::project_by_slug(&conn, slug.clone()).str("path"))
        .unwrap_or_default();
    for sg in suggestions {
        match sg.status.as_str() {
            "eligible" => {
                eligible += 1;
                let gate = pulse_tally::plan_gate_with(
                    s,
                    plan_ctx.clone(),
                    &project_path,
                    &sg,
                    crate::plan::tally_post(s, sg.id.clone()).as_ref(),
                );
                if !pulse_tally::in_flight_gate(&sg).is_empty() {
                    in_flight += 1;
                } else if gate == pulse_tally::GATE_NET_VOTES {
                    net_non_positive += 1;
                } else if gate == pulse_tally::GATE_OWNER {
                    owner += 1;
                } else if gate.starts_with(pulse_tally::GATE_DUPLICATE) {
                    duplicates += 1;
                } else if gate.starts_with(pulse_tally::GATE_OPERATING_LAYER_CAP) {
                    over_operating_cap += 1;
                } else if gate == pulse_tally::GATE_NO_EVIDENCE
                    || gate == pulse_tally::GATE_OWNER_PATHS
                    || gate == pulse_tally::GATE_UNLANDED
                    || gate == pulse_tally::GATE_NO_OP_CHECK
                    || !gate.is_empty()
                {
                    no_evidence += 1;
                } else {
                    plannable += 1;
                }
            }
            "needs votes" => needs_votes += 1,
            "rejected" => rejected += 1,
            "tried" | "built" | "carded" => tried += 1,
            _ => {}
        }
    }
    if eligible == 0 {
        let mut held = Vec::new();
        if needs_votes > 0 {
            held.push(format!("{needs_votes} need votes"));
        }
        if rejected > 0 {
            held.push(format!("{rejected} rejected"));
        }
        if tried > 0 {
            held.push(format!("{tried} tried"));
        }
        if held.is_empty() {
            return "no suggestions".into();
        }
        return format!("no eligible suggestion ({})", held.join(", "));
    }
    let mut held = Vec::new();
    if net_non_positive > 0 {
        held.push(format!("{net_non_positive} net votes <= 0"));
    }
    if owner > 0 {
        held.push(format!("{owner} for the owner"));
    }
    if no_evidence > 0 {
        held.push(format!("{no_evidence} without evidence"));
    }
    if duplicates > 0 {
        held.push(format!("{duplicates} duplicates"));
    }
    if over_operating_cap > 0 {
        held.push(format!("{over_operating_cap} over operating-layer cap"));
    }
    if in_flight > 0 {
        held.push(format!("{in_flight} in flight"));
    }
    if rejected > 0 {
        held.push(format!("{rejected} rejected"));
    }
    if held.is_empty() {
        format!("{eligible} eligible suggestions, {plannable} plannable")
    } else {
        format!(
            "{eligible} eligible suggestions, {plannable} plannable ({})",
            held.join(", ")
        )
    }
}

/// Go: func handoffIdlePart(s *store.Store, handoffs []tally.Post) string
pub(crate) fn handoff_idle_part(
    s: &pulse_store::Store,
    handoffs: Vec<pulse_tally::Post>,
) -> String {
    if handoffs.is_empty() {
        return String::new();
    }
    let tried: i32 = handoffs
        .iter()
        .map(|h| job_source_count(s, format!("handoff:{}", h.id)))
        .sum();
    if tried == 0 {
        format!("{} handoffs", handoffs.len())
    } else {
        format!("{} handoffs ({} tried)", handoffs.len(), tried)
    }
}

/// Go: func questionIdlePart(s *store.Store, slug, now string) string
pub(crate) fn question_idle_part(s: &pulse_store::Store, slug: String, now: String) -> String {
    let questions = crate::plan::open_questions(s, slug, now);
    if questions.is_empty() {
        return String::new();
    }
    let tried: i32 = questions
        .iter()
        .map(|q| job_source_count(s, format!("question:{}", q.str("id"))))
        .sum();
    if tried == 0 {
        format!("{} questions", questions.len())
    } else {
        format!("{} questions ({} tried)", questions.len(), tried)
    }
}

/// Go: func ballotIdlePart(s *store.Store, slug, now string, suggestions []tally.Suggestion) string
pub(crate) fn ballot_idle_part(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    suggestions: Vec<pulse_tally::Suggestion>,
) -> String {
    if !crate::plan::ballot_due(s, slug.clone(), now.clone(), suggestions.clone()).is_empty() {
        return "ballot due".into();
    }
    if !suggestions.iter().any(|sg| sg.status == "needs votes") {
        return String::new();
    }
    let last = match s.conn() {
        Ok(conn) => crate::util::must_one(
            &conn,
            "SELECT created_at FROM jobs WHERE project = ? AND kind = 'ballot' ORDER BY created_at DESC LIMIT 1",
            &[&slug],
        ),
        Err(_) => Row::default(),
    };
    let created_at = last.str("created_at");
    if created_at.is_empty() {
        return String::new();
    }
    let due =
        crate::util::must_iso(&created_at) + TimeDelta::hours(crate::plan::ballot_hours(s) as i64);
    let left = due.signed_duration_since(crate::util::must_iso(&now));
    let mut hours =
        ((left.num_nanoseconds().unwrap_or(0) + 3_600_000_000_000 - 1) / 3_600_000_000_000) as i32;
    if hours < 1 {
        hours = 1;
    }
    format!("ballot in {hours}h")
}

/// Go: func reflectIdlePart(s *store.Store, slug, now string) string
pub(crate) fn reflect_idle_part(s: &pulse_store::Store, slug: String, now: String) -> String {
    let source = format!("reflect:{}:{}", slug, &now[..10.min(now.len())]);
    let day = format!("{}%", &now[..10.min(now.len())]);
    let job = match s.conn() {
        Ok(conn) => crate::util::must_one(
            &conn,
            "SELECT status FROM jobs WHERE project = ? AND kind = 'reflect'
		AND (source = ? OR (source = ? AND created_at LIKE ?)) ORDER BY created_at DESC, rowid DESC LIMIT 1",
            &[&slug, &source, &format!("onboard-reflect:{slug}"), &day],
        ),
        Err(_) => Row::default(),
    };
    if !job.str("status").is_empty() {
        return format!("reflect {} today", job.str("status"));
    }
    "reflect due".into()
}

/// Go: func researchIdlePart(s *store.Store, slug, now string) string
pub(crate) fn research_idle_part(s: &pulse_store::Store, slug: String, now: String) -> String {
    let (focus, reason, ok) = crate::research::research_due(s, slug, now);
    if ok {
        format!("research due: {}", focus.name)
    } else {
        format!("research: {reason}")
    }
}

/// Go: func jobSourceCount(s *store.Store, source string) int
pub(crate) fn job_source_count(s: &pulse_store::Store, source: String) -> i32 {
    let Ok(conn) = s.conn() else {
        return 0;
    };
    crate::util::must_one(
        &conn,
        "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
        &[&source],
    )
    .int("n")
}

/// Go: func recordIdleNotes(s *store.Store, notes map[string]string, now string)
pub(crate) fn record_idle_notes(
    s: &pulse_store::Store,
    notes: BTreeMap<String, String>,
    now: String,
) {
    #[derive(Serialize)]
    struct Note {
        at: String,
        note: String,
    }
    let out: BTreeMap<String, Note> = notes
        .into_iter()
        .map(|(slug, text)| {
            (
                slug,
                Note {
                    at: now.clone(),
                    note: text,
                },
            )
        })
        .collect();
    if let Ok(data) = pulse_store::gojson::to_string(&out)
        && let Ok(conn) = s.conn()
    {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('planner', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![data],
        );
    }
}

/// Go: func idleNotesChanged(prev, cur map[string]string) []string
pub(crate) fn idle_notes_changed(
    prev: BTreeMap<String, String>,
    cur: BTreeMap<String, String>,
) -> Vec<String> {
    cur.into_iter()
        .filter_map(|(slug, note)| {
            (prev.get(&slug) != Some(&note)).then(|| format!("idle {slug}: {note}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    #[ignore = "cross-slice: plan::open_questions"]
    fn idle_note_empty_project() {
        let s = new_store();
        new_bare_repo(&s);
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(got.contains("no suggestions") && got.contains("reflect due"));
    }

    #[test]
    fn record_idle_notes_readable_by_inspect() {
        let s = new_store();
        new_bare_repo(&s);
        record_idle_notes(
            &s,
            BTreeMap::from([("proj".into(), "no suggestions; reflect due".into())]),
            TEST_NOW.into(),
        );
        let (raw, exists) = s.meta("planner").unwrap();
        assert!(exists, "planner meta missing");
        #[derive(serde::Deserialize)]
        struct Note {
            at: String,
            note: String,
        }
        let decoded: BTreeMap<String, Note> = serde_json::from_str(&raw).unwrap();
        assert_eq!(decoded["proj"].at, TEST_NOW);
        assert_eq!(decoded["proj"].note, "no suggestions; reflect due");
    }

    #[test]
    fn idle_notes_changed() {
        let got = super::idle_notes_changed(
            BTreeMap::from([
                ("a".into(), "same".into()),
                ("b".into(), "old".into()),
                ("gone".into(), "old".into()),
            ]),
            BTreeMap::from([
                ("b".into(), "new".into()),
                ("a".into(), "same".into()),
                ("c".into(), "newer".into()),
            ]),
        );
        assert_eq!(got, vec!["idle b: new", "idle c: newer"]);
    }

    #[test]
    fn reflect_idle_part_status_truth() {
        for source in [
            format!("reflect:proj:{}", &TEST_NOW[..10]),
            "onboard-reflect:proj".into(),
        ] {
            for status in [
                "scheduled",
                "queued",
                "leased",
                "running",
                "blocked",
                "done",
                "failed",
                "cancelled",
            ] {
                let s = new_store();
                status_truth_project(&s);
                s.conn()
                    .unwrap()
                    .execute(
                        "INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-reflect', 'proj', 'reflect', 'Daily reflect', ?, ?, ?)",
                        params![status, source, TEST_NOW],
                    )
                    .unwrap();
                assert_eq!(
                    reflect_idle_part(&s, "proj".into(), TEST_NOW.into()),
                    format!("reflect {status} today")
                );
            }
        }
    }

    #[test]
    fn reflect_idle_part_status_truth_due() {
        for (project, kind, source, created) in [
            ("", "", "", ""),
            (
                "proj",
                "reflect",
                "reflect:proj:2026-09-05",
                "2026-09-05T10:00:00Z",
            ),
            (
                "proj",
                "reflect",
                "onboard-reflect:proj",
                "2026-09-05T10:00:00Z",
            ),
            (
                "other",
                "reflect",
                &format!("reflect:proj:{}", &TEST_NOW[..10]),
                TEST_NOW,
            ),
            (
                "proj",
                "task",
                &format!("reflect:proj:{}", &TEST_NOW[..10]),
                TEST_NOW,
            ),
            ("proj", "reflect", "manual-reflect", TEST_NOW),
        ] {
            let s = new_store();
            status_truth_project(&s);
            if !source.is_empty() {
                s.conn()
                    .unwrap()
                    .execute(
                        "INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-other', ?, ?, 'Earlier work', 'done', ?, ?)",
                        params![project, kind, source, created],
                    )
                    .unwrap();
            }
            assert_eq!(
                reflect_idle_part(&s, "proj".into(), TEST_NOW.into()),
                "reflect due"
            );
        }
    }

    #[test]
    fn reflect_idle_part_status_truth_latest_attempt() {
        for created in [TEST_NOW, "2026-09-06T09:00:00Z"] {
            let s = new_store();
            s.conn()
                .unwrap()
                .execute(
                    "INSERT INTO jobs(id, project, kind, title, status, source, created_at) VALUES
				('J-first', 'proj', 'reflect', 'First attempt', 'done', 'onboard-reflect:proj', ?),
				('J-latest', 'proj', 'reflect', 'Latest attempt', 'failed', ?, ?)",
                    params![
                        created,
                        format!("reflect:proj:{}", &TEST_NOW[..10]),
                        TEST_NOW
                    ],
                )
                .unwrap();
            assert_eq!(
                reflect_idle_part(&s, "proj".into(), TEST_NOW.into()),
                "reflect failed today"
            );
        }
    }

    fn status_truth_project(s: &pulse_store::Store) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects(slug, name, path, added_at) VALUES('proj', 'proj', ?, ?)",
                params![
                    tempfile::tempdir().unwrap().keep().display().to_string(),
                    TEST_NOW
                ],
            )
            .unwrap();
    }
}
