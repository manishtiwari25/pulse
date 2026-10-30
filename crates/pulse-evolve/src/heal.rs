//! The loop heals a flat evaluator set itself. When the flat-fixture gate
//! pauses a project (every cycle in the window tied), the planner tries, in
//! order, before it asks the owner or waits on the question it asked:
//!
//! 1. **Derive.** Add the metric fixtures the project's own tree affords
//!    ([`crate::derive`]) and re-arm a derived fixture whose count reached
//!    its scale. A fixture dated after the window's oldest cycle lets the
//!    loop plan again; what was done is posted on `evolution/loop-gate`.
//! 2. **Self-review.** When nothing can be derived and the gate's question
//!    has waited `evolve.flat_wait_hours` without an owner answer, the
//!    planner answers it in the loop's name: run `evolve.flat_cycles` more
//!    cycles as the fixtures are. The gate asks again after that many more
//!    tied cycles, so the loop never waits forever on a question nobody
//!    answers, and never runs more than one window on its own say-so.
//!
//! Before either, the healer retires judge noise: a public fixture whose
//! archived scores flipped the same baseline commit both ways twice since it
//! was last enabled is disabled (event `evolution.fixture.retired`, one
//! finding on the thread); no gate can read such a fixture either way.
//!
//! Only the flat gate's own question is reviewed this way. The
//! failed-experiment gate and STOP are the owner's alone. `evolve.heal off`
//! turns the whole step off; `evolve.flat_wait_hours 0` keeps derivation and
//! leaves the review to the owner.

use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub const HEAL_SETTING: &str = "evolve.heal";
pub const FLAT_WAIT_SETTING: &str = "evolve.flat_wait_hours";
pub const HEALED_SUMMARY: &str = "Evolution healed: the loop derived fixtures from the tree";
pub const RESUMED_SUMMARY: &str = "Evolution resumed: the gate no longer holds";
pub const SELF_REVIEW_PREFIX: &str = "Reviewed by the loop: run";
pub const RETIRED_SUMMARY: &str =
    "Evolution fixture retired: it scored the same commit both ways twice";

/// What the healer did for a project, for the plan result and the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Healed {
    /// `derived`, `self-reviewed` or `retired`.
    pub how: String,
    /// The post that records it: an answer on the gate question when one is
    /// open, otherwise a finding on the thread.
    pub post: String,
    pub line: String,
}

pub fn heal_enabled(s: &pulse_store::Store) -> bool {
    s.setting(HEAL_SETTING, "on") != "off"
}

pub fn flat_wait_hours(s: &pulse_store::Store) -> i64 {
    s.setting_int(FLAT_WAIT_SETTING).max(0)
}

fn parse_time(text: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(text.trim()).ok()
}

/// When the loop reviews an unanswered gate question itself: `(due, now)`
/// with `due` in RFC 3339 and `now` true once that time has passed. `None`
/// while the review is off or the question cannot be read.
pub fn self_review_due(s: &pulse_store::Store, question: &str) -> Result<Option<(String, bool)>> {
    let wait = flat_wait_hours(s);
    if wait == 0 || !heal_enabled(s) || question.is_empty() {
        return Ok(None);
    }
    let date: Option<String> = s
        .conn()?
        .query_row("SELECT date FROM posts WHERE id = ?", [question], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(asked) = date.as_deref().and_then(parse_time) else {
        return Ok(None);
    };
    let Some(now) = parse_time(&s.now()) else {
        return Ok(None);
    };
    let due = asked + chrono::Duration::hours(wait);
    Ok(Some((
        due.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        now >= due,
    )))
}

/// Whether the loop may write on the project's gate thread now: healing is
/// on, no other owner question (STOP, a hard stop) is open there and the
/// failed-experiment gate is quiet. Every gate on the thread reads a later
/// answer in it as its review, so while the owner is asked about a
/// violation the loop neither answers nor changes the evaluators.
fn may_write(s: &pulse_store::Store, project: &str) -> Result<bool> {
    if !heal_enabled(s) || crate::flat_gate::other_owner_question_open(s, project)? {
        return Ok(false);
    }
    let (_, _, failed) = crate::failed_gate::failed_experiment_gate(s, project)?;
    Ok(failed.is_empty())
}

/// Heal the project's evaluator set if the flat gate holds it, or close the
/// gate's question once it no longer holds. `None` when there is nothing to
/// do, healing is off, or the thread is the owner's right now; the planner
/// then asks or holds as before.
pub fn heal_flat(
    s: &pulse_store::Store,
    project: &str,
    opts: &crate::plan::PlanOptions,
) -> Result<Option<Healed>> {
    if !may_write(s, project)? {
        return Ok(None);
    }
    let retired = retire_flaky_fixtures(s, project, opts)?;
    Ok(heal_flat_gate(s, project, opts)?.or(retired))
}

/// Disables the enabled public fixtures whose archived scores flipped the
/// same baseline commit both ways twice or more since they were last
/// enabled ([`crate::flat_gate::RETIRE_FLIPS`]): judge noise that no gate
/// can read as a signal either way. One event per fixture and one finding
/// for all of them; `pulse evolve fixture enable NAME` is the owner's way
/// back, after which only later cycles count.
fn retire_flaky_fixtures(
    s: &pulse_store::Store,
    project: &str,
    opts: &crate::plan::PlanOptions,
) -> Result<Option<Healed>> {
    let evidence = crate::flat_gate::fixture_evidence(s, project)?;
    let suspects: Vec<String> = evidence
        .iter()
        .filter(|e| e.flips >= crate::flat_gate::RETIRE_FLIPS)
        .map(|e| e.name.clone())
        .collect();
    if suspects.is_empty() {
        return Ok(None);
    }
    let enabled = crate::fixtures::list_fixtures(s, project, true)?;
    let mut retired = Vec::new();
    for name in suspects {
        if !enabled.iter().any(|f| f.name == name) {
            continue;
        }
        // Only the cycles since the owner last enabled it count: the
        // evidence that retired it once must not retire it again.
        let since = last_enabled(s, project, &name)?;
        let flips = if since.is_empty() {
            evidence.iter().find(|e| e.name == name).map(|e| e.flips)
        } else {
            crate::flat_gate::fixture_evidence_since(s, project, &since)?
                .into_iter()
                .find(|e| e.name == name)
                .map(|e| e.flips)
        }
        .unwrap_or(0);
        if flips < crate::flat_gate::RETIRE_FLIPS {
            continue;
        }
        crate::fixtures::set_fixture_enabled(s, project, &name, false, &opts.agent)?;
        s.event(
            &opts.agent,
            "evolution.fixture.retired",
            &name,
            serde_json::json!({"project": project, "flips": flips, "since": since}),
        )?;
        retired.push(format!("{name} ({flips} flips on one baseline commit)"));
    }
    if retired.is_empty() {
        return Ok(None);
    }
    let line = format!("retired {}", retired.join(", "));
    let body = format!(
        "The loop disabled {} of {project}'s fixtures (evolve.heal on): {}. Each scored the same baseline commit both ways at least twice across the archived cycles, so a difference it reports is judge noise, not a signal, and a tie it reports is not a reading either; the flat-fixture gate had already stopped counting it. The baseline is measured again on the next cycle because the fixture set changed.\n\npulse evolve fixture enable NAME --project {project} brings one back - only cycles after that count for it again; pulse evolve fixture list --project {project} shows the set; pulse settings set evolve.heal off leaves the evaluators to you.\n",
        retired.len(),
        retired.join(", ")
    );
    let post = heal_post(s, project, opts, "", RETIRED_SUMMARY, body)?;
    Ok(Some(Healed {
        how: "retired".to_string(),
        post,
        line,
    }))
}

/// When the owner last enabled the fixture (the `evolution.fixture.enabled`
/// event with `enabled: true`), or empty.
fn last_enabled(s: &pulse_store::Store, project: &str, name: &str) -> Result<String> {
    let at: Option<String> = s.conn()?.query_row(
        "SELECT MAX(at) FROM events WHERE action = 'evolution.fixture.enabled' AND subject = ?
		AND json_valid(detail) AND json_extract(detail, '$.project') = ?
		AND json_extract(detail, '$.enabled') = 1",
        rusqlite::params![name, project],
        |r| r.get(0),
    )?;
    Ok(at.unwrap_or_default())
}

fn heal_flat_gate(
    s: &pulse_store::Store,
    project: &str,
    opts: &crate::plan::PlanOptions,
) -> Result<Option<Healed>> {
    let (question, reason) = crate::flat_gate::flat_fixture_gate(s, project)?;
    if reason.is_empty() {
        return close_moot_question(s, project, opts);
    }
    let n = crate::flat_gate::flat_cycles(s).max(1);
    let tired = tired_fixtures(s, project, n)?;
    let derived = crate::derive::derive_fixtures_sparing(s, project, &opts.agent, &tired)?;
    if derived.changed() {
        let mut body = format!(
            "The flat-fixture gate paused {project}: {reason}.\n\nThe loop healed its evaluator set itself (evolve.heal on): {}.\n\n",
            derived.line()
        );
        for spec in crate::derive::specs(&crate::derive::detect_stacks(std::path::Path::new(
            &crate::fixtures::valid_project(s, project)?.path,
        ))) {
            body.push_str(&format!("- {}: {}\n", spec.name, spec.about));
        }
        body.push_str(&format!(
            "\nA derived fixture is a metric fixture read off the project's own tree with a scale set from today's count, so a voted change that brings tests or settles a marker scores differently from one that does not; a derived fixture whose count reaches its scale is re-armed with a larger one and a fresh date, unless it told no variant from its baseline in {n} cycles - that one stays as it is, and the check a card names (Signal: check:<command>) is the fixture that can move. The baseline is measured again on the next cycle because the fixture set changed. pulse evolve fixture list --project {project} shows the set; pulse evolve fixture disable NAME --project {project} retires one for good (the loop never re-adds a disabled fixture); pulse settings set evolve.heal off leaves this gate to you.\n"
        ));
        let post = heal_post(s, project, opts, &question, HEALED_SUMMARY, body)?;
        s.event(
            &opts.agent,
            "evolution.healed",
            project,
            serde_json::json!({
                "project": project,
                "how": "derived",
                "post": post,
                "question": question,
                "added": derived.added,
                "rearmed": derived.rearmed,
            }),
        )?;
        return Ok(Some(Healed {
            how: "derived".to_string(),
            post,
            line: derived.line(),
        }));
    }
    let Some((due, now)) = self_review_due(s, &question)? else {
        return Ok(None);
    };
    if !now {
        return Ok(None);
    }
    let wait = flat_wait_hours(s);
    let evidence =
        crate::flat_gate::evidence_summary(&crate::flat_gate::fixture_evidence(s, project)?);
    let summary = format!("{SELF_REVIEW_PREFIX} {n} more cycles");
    let mut body = format!(
        "Question {question} had no owner answer in {wait} h (evolve.flat_wait_hours), so the loop reviewed it itself and runs {n} more experiment cycles with the fixtures as they are; it asks again after {n} more tied cycles.\n\nWhat the archive shows: {}.\nNothing could be derived from the tree: {}.\n\n",
        if evidence.is_empty() {
            "no archived cycle scored a fixture on both trees".to_string()
        } else {
            evidence
        },
        derived.line()
    );
    body.push_str(&format!(
        "To make the cycles count, have each card name the check its hypothesis moves - a Signal: check:<command> line is that card's card-check fixture, scored on the base and on the variant - or add a fixture whose score moves with the change: pulse evolve fixture add --project {project} --name NAME --metric --scorer 'pulse evolve score metric --higher --scale S -- CMD ARGS...'. pulse settings set evolve.flat_wait_hours 0 leaves the next review to you.\n"
    ));
    let post = heal_post(s, project, opts, &question, &summary, body)?;
    s.event(
        &opts.agent,
        "evolution.flat.self_reviewed",
        project,
        serde_json::json!({
            "project": project,
            "how": "self-reviewed",
            "post": post,
            "question": question,
            "due": due,
            "cycles": n,
        }),
    )?;
    Ok(Some(Healed {
        how: "self-reviewed".to_string(),
        post,
        line: format!(
            "the gate question {question} waited {wait} h unanswered; the loop runs {n} more cycles and asks again"
        ),
    }))
}

/// Derived count fixtures that told no variant from its baseline in `n` or
/// more archived cycles. Re-arming one would date it after the window and
/// lift the gate while it still measures nothing a card does, so the healer
/// leaves them and points at the card's own check instead.
fn tired_fixtures(s: &pulse_store::Store, project: &str, n: usize) -> Result<Vec<String>> {
    Ok(crate::flat_gate::fixture_evidence(s, project)?
        .into_iter()
        .filter(|e| {
            e.name.starts_with(crate::derive::DERIVED_PREFIX) && e.told_apart == 0 && e.cycles >= n
        })
        .map(|e| e.name)
        .collect())
}

/// A gate question still open after the gate went quiet - a fixture was
/// added by hand or by `fixture seed`, the gate was turned off, or a later
/// cycle moved - asks the owner for nothing. The loop answers it with the
/// reason, so the inbox folds it away.
fn close_moot_question(
    s: &pulse_store::Store,
    project: &str,
    opts: &crate::plan::PlanOptions,
) -> Result<Option<Healed>> {
    let question = crate::flat_gate::open_flat_question(s, project)?;
    if question.is_empty() {
        return Ok(None);
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT f.name FROM evolution_fixtures f WHERE f.project = ? AND f.enabled = 1
		AND f.at >= (SELECT q.date FROM posts q WHERE q.id = ?) ORDER BY f.at, f.name",
    )?;
    let newer: Vec<String> = stmt
        .query_map(rusqlite::params![project, question], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    let why = if crate::flat_gate::flat_cycles(s) == 0 {
        "the gate is off (evolve.flat_cycles 0)".to_string()
    } else if !newer.is_empty() {
        format!(
            "a fixture dated after the question is in the set: {}",
            newer.join(", ")
        )
    } else {
        "the window no longer holds: a later cycle told variants apart or the question was reviewed on another post".to_string()
    };
    let body = format!(
        "Question {question} asked for a fixture that moves or a review; {why}. The loop plans again, so this question needs no answer. pulse evolve status --project {project} shows what it does next.\n"
    );
    let post = heal_post(s, project, opts, &question, RESUMED_SUMMARY, body)?;
    s.event(
        &opts.agent,
        "evolution.flat.resumed",
        project,
        serde_json::json!({
            "project": project,
            "how": "resumed",
            "post": post,
            "question": question,
            "fixtures": newer,
        }),
    )?;
    Ok(Some(Healed {
        how: "resumed".to_string(),
        post,
        line: format!("closed gate question {question}: {why}"),
    }))
}

/// An answer on the open gate question when there is one (the inbox folds
/// the question away and the gate reads it as reviewed), else a finding on
/// the thread.
fn heal_post(
    s: &pulse_store::Store,
    project: &str,
    opts: &crate::plan::PlanOptions,
    question: &str,
    summary: &str,
    body: String,
) -> Result<String> {
    let post = pulse_board::create_post(
        s,
        pulse_board::PostInput {
            kind: if question.is_empty() {
                "finding".to_string()
            } else {
                "answer".to_string()
            },
            project: project.to_string(),
            area: "evolution".to_string(),
            thread: "loop-gate".to_string(),
            summary: summary.to_string(),
            body,
            agent: opts.agent.clone(),
            agent_row: opts.agent_row.clone(),
            session: opts.session.clone(),
            branch: Some(String::new()),
            on: question.to_string(),
            ..Default::default()
        },
    )?;
    Ok(post.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use rusqlite::params;

    fn stamp(s: &pulse_store::Store, offset_secs: i64) -> String {
        let now = chrono::DateTime::parse_from_rfc3339(&s.now()).unwrap();
        (now + chrono::Duration::seconds(offset_secs))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn tie(s: &pulse_store::Store, id: &str, offset: i64) {
        tie_with(s, id, offset, "{}");
    }

    fn tie_with(s: &pulse_store::Store, id: &str, offset: i64, fixtures_json: &str) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO evolution_archive (id, project, suggestion_post, hypothesis, branch,
				baseline_score, variant_score, tolerance, kept, fixtures_json, at)
				VALUES (?, 'demo', 'S-1', 'one constant', '', 0.5, 0.5, 0.02, 1, ?, ?)",
                params![id, fixtures_json, stamp(s, offset)],
            )
            .unwrap();
    }

    /// A project whose last three cycles tied, with one old hand-made
    /// fixture and no stack in its tree.
    fn flat_store() -> (pulse_store::Store, String) {
        let (s, repo) = flat_base();
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        (s, repo)
    }

    /// [`flat_store`] before its tied cycles.
    fn flat_base() -> (pulse_store::Store, String) {
        let s = test_store();
        crate::test_helpers_plan::disable_project_hook();
        let repo = test_project(&s, "demo");
        fixture(&s, "demo", "F01", "printf 1", 1.0);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = ? WHERE project = 'demo'",
                [stamp(&s, -1000)],
            )
            .unwrap();
        join_test_agent(&s, "tester");
        s.set_setting("human", crate::flat_gate::FLAT_CYCLES_SETTING, "3")
            .unwrap();
        (s, repo)
    }

    fn opts() -> crate::plan::PlanOptions {
        crate::plan::PlanOptions {
            agent: "tester".to_string(),
            ..Default::default()
        }
    }

    fn question(s: &pulse_store::Store, offset: i64) -> String {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "evolution".into(),
                thread: "loop-gate".into(),
                for_owner: true,
                agent: "tester".into(),
                summary: crate::flat_gate::FLAT_SUMMARY.into(),
                body: "gate".into(),
                date: stamp(s, offset),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn events(s: &pulse_store::Store, action: &str) -> i64 {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = ?",
                [action],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn post_kind(s: &pulse_store::Store, id: &str) -> (String, String, String) {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT kind, summary, on_post FROM posts WHERE id = ?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    #[test]
    fn a_quiet_gate_or_heal_off_does_nothing() {
        let _serial = serial();
        let s = test_store();
        crate::test_helpers_plan::disable_project_hook();
        test_project(&s, "demo");
        join_test_agent(&s, "tester");
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        let (s, repo) = flat_store();
        std::fs::write(format!("{repo}/Cargo.toml"), "[package]\n").unwrap();
        s.set_setting("human", HEAL_SETTING, "off").unwrap();
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        assert_eq!(count_rows(&s, "evolution_fixtures"), 1);
        assert_eq!(self_review_due(&s, "missing").unwrap(), None);
    }

    #[test]
    fn deriving_a_fixture_heals_the_gate_and_says_so_on_the_question() {
        let _serial = serial();
        let (s, repo) = flat_store();
        let q = question(&s, -5);
        std::fs::write(format!("{repo}/Cargo.toml"), "[package]\n").unwrap();
        std::fs::create_dir_all(format!("{repo}/src")).unwrap();
        std::fs::write(format!("{repo}/src/lib.rs"), "#[test]\nfn a() {}\n").unwrap();
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("healed");
        assert_eq!(healed.how, "derived");
        assert!(
            healed
                .line
                .starts_with("added derived-tests (1 now), derived-todo-markers (0 now)"),
            "{}",
            healed.line
        );
        let (kind, summary, on) = post_kind(&s, &healed.post);
        assert_eq!(
            (kind.as_str(), summary.as_str(), on.as_str()),
            ("answer", HEALED_SUMMARY, q.as_str())
        );
        let body: String = s
            .conn()
            .unwrap()
            .query_row("SELECT body FROM posts WHERE id = ?", [&healed.post], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            body.contains("The loop healed its evaluator set itself")
                && body.contains("- derived-tests: test declarations in the tree (rust)")
                && body.contains("pulse settings set evolve.heal off"),
            "{body}"
        );
        // The gate is quiet now: the new fixtures are dated after the window.
        let (_, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(reason.is_empty(), "{reason}");
        assert_eq!(events(&s, "evolution.healed"), 1);
        // Nothing more to heal.
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
    }

    /// Three tied cycles that scored `name` at `base` on the baseline and
    /// at `variant` on the variant tree.
    fn scored_on(s: &pulse_store::Store, name: &str, base: f64, variant: f64) {
        for (id, offset) in [("B-001", -30), ("B-002", -20), ("B-003", -10)] {
            scored_row(s, id, offset, name, base, variant);
        }
    }

    /// One archived cycle whose matrix scored `name` `base` on the baseline
    /// (commit `abc`) and `variant` on the variant tree.
    fn scored_row(
        s: &pulse_store::Store,
        id: &str,
        offset: i64,
        name: &str,
        base: f64,
        variant: f64,
    ) {
        let tree = |r#ref: &str, score: f64| {
            let fixtures = vec![crate::api::FixtureScore {
                name: name.into(),
                kind: "public".into(),
                score,
                weight: 1.0,
                metric: true,
                note: String::new(),
            }];
            crate::api::Scores {
                project: "demo".into(),
                ref_name: r#ref.into(),
                total: score,
                fixtures,
                ..Default::default()
            }
        };
        let pair = crate::archive::ScorePair {
            baseline: Some(tree("abc", base)),
            variant: Some(tree(&format!("v-{id}"), variant)),
        };
        tie_with(s, id, offset, &serde_json::to_string(&pair).unwrap());
    }

    /// A flat store whose derived set dates from before the window, with
    /// derived-tests saturated since (the tree grew past its scale) and the
    /// window's three cycles scoring it `base` / `variant`.
    fn saturated_store(base: f64, variant: f64) -> (pulse_store::Store, String) {
        let (s, repo) = flat_base();
        scored_on(&s, "derived-tests", base, variant);
        std::fs::write(format!("{repo}/Cargo.toml"), "[package]\n").unwrap();
        std::fs::create_dir_all(format!("{repo}/src")).unwrap();
        std::fs::write(format!("{repo}/src/lib.rs"), "#[test]\nfn a() {}\n").unwrap();
        crate::derive::derive_fixtures(&s, "demo", "tester").unwrap();
        let many: String = (0..25)
            .map(|i| format!("#[test]\nfn t{i}() {{}}\n"))
            .collect();
        std::fs::write(format!("{repo}/src/more.rs"), many).unwrap();
        let dated = stamp(&s, -1000);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = ? WHERE project = 'demo'",
                [&dated],
            )
            .unwrap();
        (s, dated)
    }

    fn derived_tests(s: &pulse_store::Store) -> crate::fixtures::Fixture {
        crate::fixtures::list_fixtures(s, "demo", true)
            .unwrap()
            .into_iter()
            .find(|f| f.name == "derived-tests")
            .unwrap()
    }

    #[test]
    fn a_derived_fixture_that_tied_the_whole_window_is_not_re_armed() {
        let _serial = serial();
        let (s, dated) = saturated_store(0.5, 0.5);
        // It tied every cycle of the window: the healer leaves it and its
        // date alone, nothing lifts the gate, and with no question due the
        // loop holds.
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        let tests = derived_tests(&s);
        assert_eq!(tests.at, dated);
        assert!(tests.scorer.contains("--scale 20 --"), "{}", tests.scorer);
        let (_, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(!reason.is_empty());
        // The self-review, once due, says why and points at the card's check.
        question(&s, -3 * 3600 - 1);
        s.set_setting("human", FLAT_WAIT_SETTING, "3").unwrap();
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("reviewed");
        assert_eq!(healed.how, "self-reviewed");
        let body: String = s
            .conn()
            .unwrap()
            .query_row("SELECT body FROM posts WHERE id = ?", [&healed.post], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            body.contains("derived-tests not re-armed: it told no variant from its baseline")
                && body
                    .contains("a Signal: check:<command> line is that card's card-check fixture"),
            "{body}"
        );
        // A fixture that told a variant apart in an earlier cycle is still
        // re-armed as before.
        let (s, dated) = saturated_store(0.5, 0.5);
        scored_row(&s, "B-000", -40, "derived-tests", 0.5, 0.6);
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("healed");
        assert_eq!(healed.how, "derived");
        assert!(
            healed.line.starts_with(
                "re-armed derived-tests (its count reached its scale; 26 now, scale 52)"
            ),
            "{}",
            healed.line
        );
        assert!(derived_tests(&s).at > dated);
    }

    /// One archived cycle whose matrix scored `request` `rb` on baseline
    /// commit `commit` and `rv` on the variant, with `metric` tied at 0.5.
    fn request_cycle(
        s: &pulse_store::Store,
        id: &str,
        commit: &str,
        rb: f64,
        rv: f64,
        offset: i64,
    ) {
        let tree = |r#ref: &str, request: f64| {
            let fixtures = vec![
                crate::api::FixtureScore {
                    name: "metric".into(),
                    kind: "public".into(),
                    score: 0.5,
                    weight: 1.0,
                    metric: true,
                    note: String::new(),
                },
                crate::api::FixtureScore {
                    name: "request".into(),
                    kind: "public".into(),
                    score: request,
                    weight: 1.0,
                    metric: false,
                    note: String::new(),
                },
            ];
            crate::api::Scores {
                project: "demo".into(),
                ref_name: r#ref.into(),
                total: crate::dispute::weighted_total(&fixtures),
                fixtures,
                ..Default::default()
            }
        };
        let pair = crate::archive::ScorePair {
            baseline: Some(tree(commit, rb)),
            variant: Some(tree(&format!("v-{id}"), rv)),
        };
        tie_with(s, id, offset, &serde_json::to_string(&pair).unwrap());
    }

    #[test]
    fn a_fixture_that_flips_the_same_commit_both_ways_twice_is_retired() {
        let _serial = serial();
        let (s, _repo) = flat_base();
        fixture(&s, "demo", "metric", "printf 1", 1.0);
        fixture(&s, "demo", "request", "printf 1", 1.0);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = ? WHERE project = 'demo'",
                [stamp(&s, -1000)],
            )
            .unwrap();
        // request scored commit c2 1, 0, 1: two flips on one commit.
        request_cycle(&s, "B-001", "c1", 1.0, 1.0, -60);
        request_cycle(&s, "B-002", "c2", 1.0, 1.0, -50);
        request_cycle(&s, "B-003", "c2", 0.0, 0.0, -40);
        let (_, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(!reason.is_empty(), "the window tied on what could be read");
        // One flip is evidence, not a verdict: nothing is retired yet, and
        // the gate goes on as before (derived set, then the owner).
        let healed = heal_flat(&s, "demo", &opts()).unwrap();
        assert!(healed.is_none(), "{healed:?}");
        assert!(fixture_named(&s, "request").enabled);
        request_cycle(&s, "B-004", "c2", 1.0, 0.0, -30);
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("retired");
        assert_eq!(healed.how, "retired");
        assert_eq!(
            healed.line,
            "retired request (2 flips on one baseline commit)"
        );
        assert!(!fixture_named(&s, "request").enabled);
        assert!(fixture_named(&s, "metric").enabled);
        let (kind, summary, on) = post_kind(&s, &healed.post);
        assert_eq!(
            (kind.as_str(), summary.as_str(), on.as_str()),
            ("finding", RETIRED_SUMMARY, "")
        );
        let body: String = s
            .conn()
            .unwrap()
            .query_row("SELECT body FROM posts WHERE id = ?", [&healed.post], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            body.contains("request (2 flips on one baseline commit)")
                && body.contains("pulse evolve fixture enable NAME --project demo"),
            "{body}"
        );
        assert_eq!(events(&s, "evolution.fixture.retired"), 1);
        // Disabled, it is no candidate; re-enabled by the owner, the old
        // evidence does not retire it again - only later cycles count.
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        crate::fixtures::set_fixture_enabled(&s, "demo", "request", true, "human").unwrap();
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        assert!(fixture_named(&s, "request").enabled);
        request_cycle(&s, "B-005", "c3", 1.0, 1.0, 10);
        request_cycle(&s, "B-006", "c3", 0.0, 0.0, 20);
        request_cycle(&s, "B-007", "c3", 1.0, 1.0, 30);
        let healed = heal_flat(&s, "demo", &opts())
            .unwrap()
            .expect("retired again");
        assert_eq!(healed.how, "retired");
        assert_eq!(events(&s, "evolution.fixture.retired"), 2);
        // Heal off: the evaluators are the owner's.
        crate::fixtures::set_fixture_enabled(&s, "demo", "request", true, "human").unwrap();
        request_cycle(&s, "B-008", "c4", 1.0, 1.0, 40);
        request_cycle(&s, "B-009", "c4", 0.0, 0.0, 50);
        request_cycle(&s, "B-010", "c4", 1.0, 1.0, 60);
        s.set_setting("human", HEAL_SETTING, "off").unwrap();
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        assert!(fixture_named(&s, "request").enabled);
    }

    fn fixture_named(s: &pulse_store::Store, name: &str) -> crate::fixtures::Fixture {
        crate::fixtures::list_fixtures(s, "demo", false)
            .unwrap()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap()
    }

    #[test]
    fn the_loop_writes_nothing_while_a_stop_question_or_failed_gate_is_open() {
        let _serial = serial();
        let (s, repo) = flat_store();
        std::fs::write(format!("{repo}/Cargo.toml"), "[package]\n").unwrap();
        let stop = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "evolution".into(),
                thread: "loop-gate".into(),
                for_owner: true,
                agent: "tester".into(),
                summary: crate::cycle::STOP_SUMMARY.into(),
                body: "a hard stop".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        assert_eq!(
            count_rows(&s, "evolution_fixtures"),
            1,
            "no fixture derived"
        );
        assert!(crate::flat_gate::other_owner_question_open(&s, "demo").unwrap());
        // The owner's decision frees the thread.
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO decisions (post_id, project, answer, decided_at) VALUES (?, 'demo', 'yes', ?)",
                params![stop, s.now()],
            )
            .unwrap();
        assert!(!crate::flat_gate::other_owner_question_open(&s, "demo").unwrap());
        // A failed experiment with a hard-stop marker holds it too.
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO jobs (id, project, kind, title, status, result, created_at) VALUES ('job-1', 'demo', 'experiment', 'x', 'failed', 'violation: wrote outside scope', ?)",
                [s.now()],
            )
            .unwrap();
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        assert_eq!(count_rows(&s, "evolution_fixtures"), 1);
    }

    #[test]
    fn a_question_the_gate_no_longer_needs_is_closed_by_the_loop() {
        let _serial = serial();
        let (s, _repo) = flat_store();
        let q = question(&s, -5);
        // Nothing to derive and the question is young: the loop holds.
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        // The owner adds a fixture by hand; the gate goes quiet.
        fixture(&s, "demo", "metric-x", "printf 0.7", 1.0);
        let (_, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(reason.is_empty(), "{reason}");
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("closed");
        assert_eq!(healed.how, "resumed");
        assert_eq!(
            healed.line,
            format!(
                "closed gate question {q}: a fixture dated after the question is in the set: metric-x"
            )
        );
        let (kind, summary, on) = post_kind(&s, &healed.post);
        assert_eq!(
            (kind.as_str(), summary.as_str(), on.as_str()),
            ("answer", RESUMED_SUMMARY, q.as_str())
        );
        assert!(
            crate::flat_gate::open_flat_question(&s, "demo")
                .unwrap()
                .is_empty()
        );
        assert_eq!(events(&s, "evolution.flat.resumed"), 1);
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
    }

    #[test]
    fn with_no_question_yet_the_record_is_a_finding() {
        let _serial = serial();
        let (s, repo) = flat_store();
        std::fs::write(format!("{repo}/go.mod"), "module demo\n").unwrap();
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("healed");
        let (kind, summary, on) = post_kind(&s, &healed.post);
        assert_eq!(
            (kind.as_str(), summary.as_str(), on.as_str()),
            ("finding", HEALED_SUMMARY, "")
        );
    }

    #[test]
    fn an_unanswered_question_is_reviewed_by_the_loop_after_the_wait() {
        let _serial = serial();
        let (s, _repo) = flat_store();
        // No stack: nothing to derive; no question yet: the planner asks first.
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None);
        let q = question(&s, -3600 * 2);
        s.set_setting("human", FLAT_WAIT_SETTING, "3").unwrap();
        let asked: String = s
            .conn()
            .unwrap()
            .query_row("SELECT date FROM posts WHERE id = ?", [&q], |r| r.get(0))
            .unwrap();
        let expected = (chrono::DateTime::parse_from_rfc3339(&asked).unwrap()
            + chrono::Duration::hours(3))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let (due, now) = self_review_due(&s, &q).unwrap().unwrap();
        assert!(!now && due == expected, "{due} vs {expected}");
        assert_eq!(heal_flat(&s, "demo", &opts()).unwrap(), None, "too young");
        s.set_setting("human", FLAT_WAIT_SETTING, "0").unwrap();
        assert_eq!(self_review_due(&s, &q).unwrap(), None);
        assert_eq!(
            heal_flat(&s, "demo", &opts()).unwrap(),
            None,
            "waits for the owner"
        );
        s.set_setting("human", FLAT_WAIT_SETTING, "1").unwrap();
        assert!(self_review_due(&s, &q).unwrap().unwrap().1);
        let healed = heal_flat(&s, "demo", &opts()).unwrap().expect("reviewed");
        assert_eq!(healed.how, "self-reviewed");
        assert_eq!(
            healed.line,
            format!(
                "the gate question {q} waited 1 h unanswered; the loop runs 3 more cycles and asks again"
            )
        );
        let (kind, summary, on) = post_kind(&s, &healed.post);
        assert_eq!(
            (kind.as_str(), summary.as_str(), on.as_str()),
            (
                "answer",
                "Reviewed by the loop: run 3 more cycles",
                q.as_str()
            )
        );
        let body: String = s
            .conn()
            .unwrap()
            .query_row("SELECT body FROM posts WHERE id = ?", [&healed.post], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            body.contains("had no owner answer in 1 h")
                && body.contains("no known stack in the tree, nothing derived")
                && body.contains("evolve.flat_wait_hours 0"),
            "{body}"
        );
        let (_, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(reason.is_empty(), "{reason}");
        assert_eq!(events(&s, "evolution.flat.self_reviewed"), 1);
        // Three more ties fire the gate again; the old answer is outside the window.
        tie(&s, "B-004", 10);
        tie(&s, "B-005", 20);
        tie(&s, "B-006", 30);
        let (open, reason) = crate::flat_gate::flat_fixture_gate(&s, "demo").unwrap();
        assert!(!reason.is_empty() && open.is_empty(), "{open} {reason}");
    }
}
