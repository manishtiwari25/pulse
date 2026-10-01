//! Port of internal/runner/idle.go.
#![allow(dead_code)]

use chrono::TimeDelta;
use rusqlite::params;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::util::Row;

/// Go: func idleNote(s *store.Store, slug, now string) string
pub(crate) fn idle_note(s: &pulse_store::Store, slug: String, now: String) -> String {
    let suggestions = pulse_tally::compute_light(
        s,
        &slug,
        "",
        pulse_tally::Thresholds {
            now: now.clone(),
            ..Default::default()
        },
    )
    .ok();
    idle_note_with_tally(s, slug, now, suggestions.as_ref())
}

pub(crate) fn idle_note_with_tally(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    tally: Option<&pulse_tally::TallyResult>,
) -> String {
    let suggestion_part = suggestion_idle_part_with_tally(s, slug.clone(), now.clone(), tally);
    let suggestions = tally.map(|r| r.suggestions.clone()).unwrap_or_default();
    idle_note_from_parts(s, slug, now, suggestion_part, suggestions)
}

pub(crate) fn idle_note_from_parts(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    suggestion_part: String,
    suggestions: Vec<pulse_tally::Suggestion>,
) -> String {
    let mut parts = vec![suggestion_part];
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
    suggestion_idle_part_with_tally(s, slug, now, res.as_ref())
}

pub(crate) fn suggestion_idle_part_with_tally(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    res: Option<&pulse_tally::TallyResult>,
) -> String {
    let suggestions = res.map(|r| r.suggestions.clone()).unwrap_or_default();
    let plan_ctx = pulse_tally::plan_context_for(s, &slug, &now, res);
    let mut eligible = 0;
    let mut plannable = 0;
    let mut in_flight = 0;
    let mut net_non_positive = 0;
    let mut owner = 0;
    let mut no_evidence = 0;
    let mut file_gone = 0;
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
                } else if gate == pulse_tally::GATE_FILE_GONE {
                    file_gone += 1;
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
    if file_gone > 0 {
        held.push(format!("{file_gone} name gone paths"));
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
            let changed = prev.get(&slug).is_none_or(|old| {
                crate::idle_log::normalize_countdowns(old)
                    != crate::idle_log::normalize_countdowns(&note)
            });
            changed.then(|| format!("idle {slug}: {note}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f3::{row_agent, status_truth_project};

    #[test]
    fn idle_note_empty_project() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(got.contains("no suggestions"));
        assert!(got.contains("reflect due"));
    }

    #[test]
    fn idle_note_rejected_suggestion() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let author = join(&s, "author");
        let voter1 = join(&s, "voter-one");
        let voter2 = join(&s, "voter-two");
        let card = crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "suggestion".into(),
                project: "proj".into(),
                area: "code".into(),
                thread: "plan".into(),
                summary: "try a measured change".into(),
                body: "one variable\n".into(),
                agent: Some(Box::new(row_agent(author))),
                ..Default::default()
            },
        )
        .unwrap();
        for voter in [voter1, voter2] {
            crate::board::create_post(
                &s,
                TEST_NOW.into(),
                crate::board::PostOpts {
                    kind: "vote".into(),
                    project: "proj".into(),
                    area: "code".into(),
                    thread: "plan".into(),
                    summary: "down".into(),
                    body: "not ready\n".into(),
                    agent: Some(Box::new(row_agent(voter))),
                    on: card.str("id"),
                    vote: "down".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(
            got.contains("no eligible suggestion (1 rejected)"),
            "downvoted idle note: {got:?}"
        );
    }

    #[test]
    fn ballot_due_skips_when_only_author_could_vote() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let author = join(&s, "author");
        let card = crate::board::create_post(
            &s,
            "2026-09-06T08:00:00Z".into(),
            crate::board::PostOpts {
                kind: "suggestion".into(),
                project: "proj".into(),
                area: "code".into(),
                thread: "plan".into(),
                summary: "try a measured change".into(),
                body: "one variable\n".into(),
                agent: Some(Box::new(row_agent(author))),
                ..Default::default()
            },
        )
        .unwrap();
        let suggestions = crate::tally::suggestion_records(&s, "proj".into(), TEST_NOW.into());
        let cards =
            crate::plan::ballot_due(&s, "proj".into(), TEST_NOW.into(), suggestions.clone());
        assert!(
            cards.is_empty(),
            "ballot due with only author voter: {cards:?}"
        );
        join(&s, "voter");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE agents SET kind = 'worker', status = 'active' WHERE name = 'voter'",
                [],
            )
            .unwrap();
        let suggestions = crate::tally::suggestion_records(&s, "proj".into(), TEST_NOW.into());
        let cards =
            crate::plan::ballot_due(&s, "proj".into(), TEST_NOW.into(), suggestions.clone());
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].id, card.str("id"));
        assert!(
            cards.len() <= 1,
            "ballot due with available voter: {cards:?}"
        );
    }

    #[test]
    fn idle_note_handoffs_and_tried() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let worker = join(&s, "worker-one");
        let h = crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "handoff".into(),
                project: "proj".into(),
                area: "code".into(),
                thread: "handoff".into(),
                summary: "continue this".into(),
                body: "leftover work\n".into(),
                agent: Some(Box::new(row_agent(worker))),
                ..Default::default()
            },
        )
        .unwrap();
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(got.contains("1 handoffs"), "handoff idle note: {got:?}");
        assert!(!got.contains("1 handoffs ("), "handoff idle note: {got:?}");
        crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "try handoff".into(),
            "brief".into(),
            0,
            1,
            format!("handoff:{}", h.str("id")),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(
            got.contains("1 handoffs (1 tried)"),
            "tried handoff idle note: {got:?}"
        );
    }

    #[test]
    fn idle_note_questions() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let worker = join(&s, "worker-one");
        crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "question".into(),
                project: "proj".into(),
                area: "code".into(),
                thread: "q".into(),
                summary: "what next?".into(),
                body: "what next?\n".into(),
                agent: Some(Box::new(row_agent(worker))),
                ..Default::default()
            },
        )
        .unwrap();
        let got = idle_note(&s, "proj".into(), TEST_NOW.into());
        assert!(got.contains("1 questions"), "question idle note: {got:?}");
    }

    #[test]
    fn record_idle_notes_readable_by_inspect() {
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
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
    fn idle_notes_changed_ignores_countdowns() {
        let _serial = crate::test_helpers::serial();
        let got = super::idle_notes_changed(
            BTreeMap::from([
                (
                    "a".into(),
                    "2 eligible suggestions, 0 plannable; ballot in 2h; research: last research 47m ago, next after 13m".into(),
                ),
                ("b".into(), "1 eligible suggestion; ballot in 2h".into()),
            ]),
            BTreeMap::from([
                (
                    "a".into(),
                    "2 eligible suggestions, 0 plannable; ballot in 1h; research: last research 52m ago, next after 8m".into(),
                ),
                ("b".into(), "2 eligible suggestions; ballot in 1h".into()),
            ]),
        );
        assert_eq!(got, vec!["idle b: 2 eligible suggestions; ballot in 1h"]);
    }

    #[test]
    fn reflect_idle_part_status_truth() {
        let _serial = crate::test_helpers::serial();
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
                if let Err(err) = s.conn().unwrap().execute(
                    "INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-reflect', 'proj', 'reflect', 'Daily reflect', ?, ?, ?)",
                    params![status, source, TEST_NOW],
                ) {
                    panic!("{err}");
                }
                let (got, want) = (
                    reflect_idle_part(&s, "proj".into(), TEST_NOW.into()),
                    format!("reflect {status} today"),
                );
                if got != want {
                    panic!("idle diagnostic = {got:?}, want {want:?}");
                }
                let planned =
                    crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
                match &planned {
                    Ok(planned) if planned.is_empty() => {}
                    _ => {
                        panic!("an existing daily attempt must not be scheduled again: {planned:?}")
                    }
                }
                let n = count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND kind = 'reflect'",
                    &[],
                );
                if n != 1 {
                    panic!("daily attempts = {n}, want 1");
                }
            }
        }
    }

    #[test]
    fn reflect_idle_part_status_truth_due() {
        let _serial = crate::test_helpers::serial();
        struct Tc {
            name: &'static str,
            project: &'static str,
            kind: &'static str,
            source: String,
            created: &'static str,
        }
        for tc in [
            Tc {
                name: "no reflect",
                project: "",
                kind: "",
                source: "".into(),
                created: "",
            },
            Tc {
                name: "previous daily attempt",
                project: "proj",
                kind: "reflect",
                source: "reflect:proj:2026-09-05".into(),
                created: "2026-09-05T10:00:00Z",
            },
            Tc {
                name: "previous onboarding attempt",
                project: "proj",
                kind: "reflect",
                source: "onboard-reflect:proj".into(),
                created: "2026-09-05T10:00:00Z",
            },
            Tc {
                name: "another project",
                project: "other",
                kind: "reflect",
                source: format!("reflect:proj:{}", &TEST_NOW[..10]),
                created: TEST_NOW,
            },
            Tc {
                name: "another kind",
                project: "proj",
                kind: "task",
                source: format!("reflect:proj:{}", &TEST_NOW[..10]),
                created: TEST_NOW,
            },
            Tc {
                name: "unrelated source",
                project: "proj",
                kind: "reflect",
                source: "manual-reflect".into(),
                created: TEST_NOW,
            },
        ] {
            let s = new_store();
            status_truth_project(&s);
            if !tc.source.is_empty()
                && let Err(err) = s.conn().unwrap().execute(
                    "INSERT INTO jobs(id, project, kind, title, status, source, created_at)
					VALUES('J-other', ?, ?, 'Earlier work', 'done', ?, ?)",
                    params![tc.project, tc.kind, tc.source, tc.created],
                )
            {
                panic!("{}: {err}", tc.name);
            }
            let got = reflect_idle_part(&s, "proj".into(), TEST_NOW.into());
            if got != "reflect due" {
                panic!("{}: idle diagnostic = {got:?}, want reflect due", tc.name);
            }
            let planned =
                crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
            match &planned {
                Ok(planned) if planned.len() == 1 && planned[0].str("kind") == "reflect" => {}
                _ => panic!(
                    "{}: a due reflect must still be scheduled: {planned:?}",
                    tc.name
                ),
            }
        }
    }

    #[test]
    fn reflect_idle_part_status_truth_latest_attempt() {
        let _serial = crate::test_helpers::serial();
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
}
