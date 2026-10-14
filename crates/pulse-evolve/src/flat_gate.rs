//! The flat-fixture gate. When a project's last `evolve.flat_cycles`
//! experiment cycles all tied (the variant scored exactly what its baseline
//! scored), the fixtures are not telling a better variant from a worse one:
//! every further cycle costs a worker run and a measurement and teaches
//! nothing. The gate pauses the loop with one owner question on the project's
//! `evolution/loop-gate` thread. A fixture enabled since the streak began, or
//! an answer or decision on that question, lets the loop run again; after a
//! review, `evolve.flat_cycles` new tied cycles must accumulate before it asks
//! again. The gate is a pure read: `plan` posts the question, the way it posts
//! the failed-experiment gate's.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

/// Summary of the owner question the gate asks for. The board coalesces open
/// owner questions by summary and the gate finds its own by it.
pub const FLAT_SUMMARY: &str = "Evolution paused: the fixtures do not tell variants apart";

/// The setting that sizes the window: tied cycles in a row before the gate
/// fires; 0 turns the gate off and anything below 2 counts as 2.
pub const FLAT_CYCLES_SETTING: &str = "evolve.flat_cycles";

/// One archived experiment cycle of a flat streak.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatCycle {
    pub id: String,
    pub baseline_score: f64,
    pub variant_score: f64,
    pub at: String,
}

/// The window that fires the gate: the project's last N experiment cycles,
/// newest first, every one a tie, with no enabled fixture and no reviewed gate
/// question dated at or after the oldest of them.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatStreak {
    pub cycles: Vec<FlatCycle>,
}

const QUESTION_SQL: &str = "q.project = ? AND q.kind = 'question' AND q.summary = ?";

/// The owner reviewed the question: decided it, answered it, or answered
/// later in its thread (the hub inbox folds the question away on any of
/// these, so the gate must not wait on a question nobody sees).
const REVIEWED_SQL: &str = "(EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = q.id)
	OR EXISTS (SELECT 1 FROM posts a WHERE a.kind = 'answer' AND a.project = q.project
		AND (a.on_post = q.id OR (a.area = q.area AND a.thread = q.thread AND a.date > q.date))))";

/// Closed without a review: the owner-inbox retire sweep ends a duplicate
/// question with a `result` post.
const CLOSED_SQL: &str =
    "EXISTS (SELECT 1 FROM posts r WHERE r.kind = 'result' AND r.on_post = q.id)";

/// The window size the store asks for: 0 when the gate is off.
pub fn flat_cycles(s: &pulse_store::Store) -> usize {
    let n = s.setting_int(FLAT_CYCLES_SETTING);
    if n <= 0 { 0 } else { n.max(2) as usize }
}

pub fn tied(cycle: &FlatCycle) -> bool {
    (cycle.variant_score - cycle.baseline_score).abs() < 1e-9
}

/// The streak that fires the gate now, or `None` while the loop may plan:
/// the gate is off, fewer than N experiment cycles exist, one of the last N
/// moved, an enabled fixture is dated at or after the window's oldest cycle
/// (it was never measured by the whole window, so it may still discriminate),
/// or the owner already reviewed a gate question dated inside the window.
/// Baseline refresh rows are not cycles: they tie by construction.
pub fn flat_streak(s: &pulse_store::Store, project: &str) -> Result<Option<FlatStreak>> {
    let n = flat_cycles(s);
    if n == 0 {
        return Ok(None);
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT a.id, a.baseline_score, a.variant_score, a.at FROM evolution_archive a
		WHERE a.project = ? AND NOT {}
		ORDER BY a.at DESC, a.rowid DESC LIMIT ?",
        crate::status::BASELINE_ROW_SQL
    ))?;
    let cycles: Vec<FlatCycle> = stmt
        .query_map(params![project, n as i64], |r| {
            Ok(FlatCycle {
                id: r.get(0)?,
                baseline_score: r.get(1)?,
                variant_score: r.get(2)?,
                at: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if cycles.len() < n || !cycles.iter().all(tied) {
        return Ok(None);
    }
    let oldest = cycles[cycles.len() - 1].at.clone();
    let newer_fixture: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM evolution_fixtures WHERE project = ? AND enabled = 1 AND at >= ?)",
        params![project, oldest],
        |r| r.get(0),
    )?;
    if newer_fixture {
        return Ok(None);
    }
    let reviewed: bool = conn.query_row(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM posts q WHERE {QUESTION_SQL} AND q.date >= ? AND {REVIEWED_SQL})"
        ),
        params![project, FLAT_SUMMARY, oldest],
        |r| r.get(0),
    )?;
    if reviewed {
        return Ok(None);
    }
    Ok(Some(FlatStreak { cycles }))
}

/// `(question, reason)`: both empty while the loop may plan. When the gate
/// fires, `question` is the open gate question to coalesce on, or empty when
/// the caller should post one, and `reason` says what was measured.
pub fn flat_fixture_gate(s: &pulse_store::Store, project: &str) -> Result<(String, String)> {
    let Some(streak) = flat_streak(s, project)? else {
        return Ok((String::new(), String::new()));
    };
    let conn = s.conn()?;
    let question: String = conn
        .query_row(
            &format!(
                "SELECT q.id FROM posts q WHERE {QUESTION_SQL} AND NOT {REVIEWED_SQL} AND NOT {CLOSED_SQL}
				ORDER BY q.date DESC, q.id DESC LIMIT 1"
            ),
            params![project, FLAT_SUMMARY],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    Ok((question, flat_reason(&streak)))
}

pub fn flat_reason(streak: &FlatStreak) -> String {
    let ids: Vec<&str> = streak.cycles.iter().rev().map(|c| c.id.as_str()).collect();
    format!(
        "the last {} experiment cycles all tied (variant score = baseline score in {}); the fixtures cannot tell a better variant from a worse one, so the loop pauses until a fixture whose score moves is added or the question is answered",
        streak.cycles.len(),
        ids.join(", ")
    )
}

/// What the archive shows about one fixture across a project's experiment
/// cycles (a held-out fixture by its archive name, `H1`, `H2`, ...). The
/// owner reads it to pick the fixture to replace: one that never told a
/// variant from its baseline cannot pick a variant, and one that scored a
/// frozen tree two ways reports differences that are not signal.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FixtureEvidence {
    pub name: String,
    /// Cycles whose archived matrix scored it on both trees.
    pub cycles: usize,
    /// Cycles where the variant scored differently from its baseline.
    pub told_apart: usize,
    /// Times its baseline score changed along with the baseline commit: it
    /// moves with the default branch, whatever the experiments do.
    pub moved_with_main: usize,
    /// It scored the same baseline commit two ways.
    pub flaky: bool,
}

/// The evidence for every fixture the project's archived experiment cycles
/// scored, oldest cycle first, in the order the fixtures first appeared.
pub fn fixture_evidence(s: &pulse_store::Store, project: &str) -> Result<Vec<FixtureEvidence>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT a.fixtures_json FROM evolution_archive a
		WHERE a.project = ? AND NOT {}
		ORDER BY a.at ASC, a.rowid ASC",
        crate::status::BASELINE_ROW_SQL
    ))?;
    let rows: Vec<String> = stmt
        .query_map(params![project], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out: Vec<FixtureEvidence> = Vec::new();
    // Per fixture: the last baseline (commit, score) and every score seen
    // per baseline commit.
    let mut last: Vec<(String, f64)> = Vec::new();
    let mut by_commit: Vec<std::collections::HashMap<String, f64>> = Vec::new();
    for json in rows {
        let Ok(pair) = serde_json::from_str::<crate::archive::ScorePair>(&json) else {
            continue;
        };
        let (Ok(baseline), Ok(variant)) = (
            crate::archive::public_scores(pair.baseline.as_ref()),
            crate::archive::public_scores(pair.variant.as_ref()),
        ) else {
            continue;
        };
        for b in &baseline.fixtures {
            let Some(v) = variant
                .fixtures
                .iter()
                .find(|v| v.name == b.name && v.kind == b.kind)
            else {
                continue;
            };
            let i = match out.iter().position(|e| e.name == b.name) {
                Some(i) => i,
                None => {
                    out.push(FixtureEvidence {
                        name: b.name.clone(),
                        ..Default::default()
                    });
                    last.push((String::new(), f64::NAN));
                    by_commit.push(Default::default());
                    out.len() - 1
                }
            };
            let e = &mut out[i];
            e.cycles += 1;
            if (v.score - b.score).abs() > 1e-9 {
                e.told_apart += 1;
            }
            let (commit, score) = &last[i];
            if !commit.is_empty() && *commit != baseline.ref_name && (b.score - score).abs() > 1e-9
            {
                e.moved_with_main += 1;
            }
            last[i] = (baseline.ref_name.clone(), b.score);
            if !baseline.ref_name.is_empty() {
                match by_commit[i].get(&baseline.ref_name) {
                    Some(seen) if (seen - b.score).abs() > 1e-9 => e.flaky = true,
                    Some(_) => {}
                    None => {
                        by_commit[i].insert(baseline.ref_name.clone(), b.score);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// One fixture's evidence as the question body shows it.
pub fn evidence_line(e: &FixtureEvidence) -> String {
    let mut line = if e.told_apart == 0 {
        format!(
            "told no variant from its baseline in {} cycle{}",
            e.cycles,
            if e.cycles == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "told a variant from its baseline in {} of {} cycles",
            e.told_apart, e.cycles
        )
    };
    if e.moved_with_main > 0 {
        line.push_str(&format!(
            "; moved {} time{} with the default branch",
            e.moved_with_main,
            if e.moved_with_main == 1 { "" } else { "s" }
        ));
    }
    if e.flaky {
        line.push_str(
            "; scored the same baseline commit two ways, so a difference it reports is not a signal on its own",
        );
    }
    line
}

/// The one sentence `pulse evolve status` adds to the gate's reason: what
/// the archive says about the fixtures, or empty before any cycle was scored.
pub fn evidence_summary(evidence: &[FixtureEvidence]) -> String {
    let scored: Vec<&FixtureEvidence> = evidence.iter().filter(|e| e.cycles > 0).collect();
    if scored.is_empty() {
        return String::new();
    }
    let cycles = scored.iter().map(|e| e.cycles).max().unwrap_or(0);
    let moving: Vec<&str> = scored
        .iter()
        .filter(|e| e.told_apart > 0)
        .map(|e| e.name.as_str())
        .collect();
    let flaky: Vec<&str> = scored
        .iter()
        .filter(|e| e.flaky)
        .map(|e| e.name.as_str())
        .collect();
    let mut line = if moving.is_empty() {
        format!("no fixture told a variant from its baseline in {cycles} cycles")
    } else {
        format!(
            "only {} told a variant from its baseline in {cycles} cycles",
            moving.join(", ")
        )
    };
    if !flaky.is_empty() {
        line.push_str(&format!(
            "; {} scored the same baseline commit two ways",
            flaky.join(", ")
        ));
    }
    line
}

/// The one-line owner step `pulse evolve status` names while the gate holds.
pub fn flat_owner_step(project: &str, question: &str) -> String {
    let answer = if question.is_empty() {
        "answer the loop's question on the evolution/loop-gate thread".to_string()
    } else {
        format!("answer question {question}")
    };
    format!(
        "add a fixture whose score moves (pulse evolve fixture add --project {project} --name NAME --metric --scorer CMD) or {answer}"
    )
}

/// The body of the gate question: the tied cycles with their scores, the
/// enabled fixtures, and the next human step with the commands that take it.
pub fn flat_fixture_question_body(s: &pulse_store::Store, project: &str) -> Result<String> {
    let streak = flat_streak(s, project)?;
    let n = streak
        .as_ref()
        .map(|st| st.cycles.len())
        .unwrap_or_else(|| flat_cycles(s))
        .max(1);
    let mut body = format!(
        "The last {n} experiment cycles of {project} all tied: each variant scored exactly what its baseline scored, so the fixtures cannot tell a better variant from a worse one. Every further cycle would cost a worker run and a measurement and teach nothing, so the loop pauses.\n\nTied cycles (oldest first; baseline -> variant score, archived at):\n"
    );
    for c in streak.iter().flat_map(|st| st.cycles.iter().rev()) {
        body.push_str(&format!(
            "- {}: {:.4} -> {:.4}, {}\n",
            c.id, c.baseline_score, c.variant_score, c.at
        ));
    }
    body.push_str(
        "\nEnabled fixtures (name: kind, metric, weight; what the archived cycles show):\n",
    );
    let fixtures = crate::fixtures::list_fixtures(s, project, true)?;
    let evidence = fixture_evidence(s, project)?;
    if fixtures.is_empty() {
        body.push_str("- none\n");
    }
    for f in &fixtures {
        body.push_str(&format!(
            "- {}: {}, metric {}, weight {}",
            f.name,
            f.kind,
            if f.metric { "yes" } else { "no" },
            f.weight
        ));
        if let Some(e) = evidence.iter().find(|e| e.name == f.name && e.cycles > 0) {
            body.push_str(&format!("; {}", evidence_line(e)));
        }
        body.push('\n');
    }
    for e in evidence
        .iter()
        .filter(|e| e.cycles > 0 && heldout_name(&e.name))
    {
        body.push_str(&format!("- {} (held-out): {}\n", e.name, evidence_line(e)));
    }
    body.push_str(&format!(
        "\nNext human step, one of:\n\
         - Add a fixture whose score moves with the change, usually a metric fixture that scores the frozen tree directly:\n    \
         pulse evolve fixture add --project {project} --name NAME --metric --scorer 'pulse evolve score metric --higher --scale S -- CMD ARGS...'\n  \
         (--lower scores a number that should shrink; any --scorer whose score differs between variants works). The loop runs again on its own once the fixture is enabled; pulse evolve fixture list --project {project} shows the current set.\n\
         - Or answer this question to let the loop run {n} more cycles as they are; it asks again after {n} more tied cycles:\n    \
         pulse post answer --project {project} --area evolution --thread loop-gate --on QUESTION_ID --summary 'Reviewed: run {n} more cycles'\n  \
         (QUESTION_ID is this post's id).\n\
         - pulse settings set evolve.flat_cycles 0 turns this gate off.\n"
    ));
    Ok(body)
}

fn heldout_name(name: &str) -> bool {
    crate::measure::heldout_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(s: &pulse_store::Store, offset: i64) -> String {
        let now = chrono::DateTime::parse_from_rfc3339(&s.now()).unwrap();
        (now + chrono::Duration::seconds(offset)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// A project with one enabled fixture dated well before every cycle the
    /// tests archive, one agent to post as, and a window of `n` cycles.
    fn gate_store(n: &str) -> pulse_store::Store {
        let s = crate::test_helpers::test_store();
        crate::test_helpers_plan::disable_project_hook();
        crate::test_helpers::test_project(&s, "demo");
        crate::test_helpers::fixture(&s, "demo", "F01", "printf 1", 1.0);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = ? WHERE project = 'demo'",
                [stamp(&s, -1000)],
            )
            .unwrap();
        crate::test_helpers::join_test_agent(&s, "tester");
        s.set_setting("human", FLAT_CYCLES_SETTING, n).unwrap();
        s
    }

    fn cycle(s: &pulse_store::Store, id: &str, baseline: f64, variant: f64, offset: i64) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO evolution_archive (id, project, suggestion_post, hypothesis, branch,
				baseline_score, variant_score, tolerance, kept, fixtures_json, at)
				VALUES (?, 'demo', 'S-1', 'one constant', '', ?, ?, 0.02, ?, '{}', ?)",
                params![
                    id,
                    baseline,
                    variant,
                    (variant + 0.02 >= baseline) as i64,
                    stamp(s, offset)
                ],
            )
            .unwrap();
    }

    fn tie(s: &pulse_store::Store, id: &str, offset: i64) {
        cycle(s, id, 0.5, 0.5, offset);
    }

    fn gate_post(s: &pulse_store::Store, kind: &str, on: &str, offset: i64) -> String {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                kind: kind.into(),
                project: "demo".into(),
                area: "evolution".into(),
                thread: "loop-gate".into(),
                for_owner: kind == "question",
                agent: "tester".into(),
                summary: if kind == "question" {
                    FLAT_SUMMARY.into()
                } else {
                    "Reviewed: run 3 more cycles".into()
                },
                body: "gate".into(),
                on: on.into(),
                date: stamp(s, offset),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn fires_after_n_tied_cycles_and_names_them() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        let (question, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(question.is_empty(), "no question exists yet: {question}");
        assert!(
            reason.starts_with("the last 3 experiment cycles all tied")
                && reason.contains("B-001, B-002, B-003"),
            "{reason}"
        );
        let body = flat_fixture_question_body(&s, "demo").unwrap();
        for want in [
            "- B-001: 0.5000 -> 0.5000, ",
            "- B-003: 0.5000 -> 0.5000, ",
            "- F01: public, metric no, weight 1\n",
            "pulse evolve fixture add --project demo --name NAME --metric --scorer",
            "pulse post answer --project demo --area evolution --thread loop-gate --on QUESTION_ID",
            "pulse settings set evolve.flat_cycles 0",
        ] {
            assert!(body.contains(want), "body lacks {want:?}:\n{body}");
        }
        assert_eq!(
            flat_owner_step("demo", "Q-1"),
            "add a fixture whose score moves (pulse evolve fixture add --project demo --name NAME --metric --scorer CMD) or answer question Q-1"
        );
    }

    /// One archived cycle with a real matrix: `metric` scores `m` on both
    /// trees, `request` scores `rb` on the baseline (commit `commit`) and `rv`
    /// on the variant, and one held-out fixture ties at 1.
    fn scored_cycle(
        s: &pulse_store::Store,
        id: &str,
        commit: &str,
        m: f64,
        rb: f64,
        rv: f64,
        offset: i64,
    ) {
        let tree = |r#ref: &str, request: f64| {
            let fixtures = vec![
                crate::api::FixtureScore {
                    name: "metric".into(),
                    kind: "public".into(),
                    score: m,
                    weight: 2.0,
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
                crate::api::FixtureScore {
                    name: "H1".into(),
                    kind: "heldout".into(),
                    score: 1.0,
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
                heldout: 1,
                ..Default::default()
            }
        };
        let baseline = tree(commit, rb);
        let variant = tree(&format!("v-{id}"), rv);
        let pair = crate::archive::ScorePair {
            baseline: Some(baseline.clone()),
            variant: Some(variant.clone()),
        };
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO evolution_archive (id, project, suggestion_post, hypothesis, branch,
				baseline_score, variant_score, tolerance, kept, fixtures_json, at)
				VALUES (?, 'demo', 'S-1', 'one constant', '', ?, ?, 0.02, ?, ?, ?)",
                params![
                    id,
                    baseline.total,
                    variant.total,
                    (variant.total + 0.02 >= baseline.total) as i64,
                    serde_json::to_string(&pair).unwrap(),
                    stamp(s, offset)
                ],
            )
            .unwrap();
    }

    #[test]
    fn the_question_says_which_fixture_never_moves_and_which_one_is_noise() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        crate::test_helpers::fixture(&s, "demo", "metric", "printf 1", 2.0);
        crate::test_helpers::fixture(&s, "demo", "request", "printf 1", 1.0);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_fixtures SET at = ? WHERE project = 'demo'",
                [stamp(&s, -1000)],
            )
            .unwrap();
        // The metric moved only when the default branch did (c1 -> c2); the
        // request fixture flipped the variant once (a discard) and scored
        // commit c2 two ways across cycles; the rest tied.
        scored_cycle(&s, "B-001", "c1", 0.50, 1.0, 1.0, -70);
        scored_cycle(&s, "B-002", "c1", 0.50, 1.0, 0.0, -60);
        scored_cycle(&s, "B-003", "c2", 0.46, 1.0, 1.0, -50);
        scored_cycle(&s, "B-004", "c2", 0.46, 0.0, 0.0, -40);
        scored_cycle(&s, "B-005", "c2", 0.46, 1.0, 1.0, -30);
        scored_cycle(&s, "B-006", "c2", 0.46, 1.0, 1.0, -20);
        let evidence = fixture_evidence(&s, "demo").unwrap();
        assert_eq!(
            evidence,
            vec![
                FixtureEvidence {
                    name: "metric".into(),
                    cycles: 6,
                    told_apart: 0,
                    moved_with_main: 1,
                    flaky: false,
                },
                FixtureEvidence {
                    name: "request".into(),
                    cycles: 6,
                    told_apart: 1,
                    moved_with_main: 0,
                    flaky: true,
                },
                FixtureEvidence {
                    name: "H1".into(),
                    cycles: 6,
                    told_apart: 0,
                    moved_with_main: 0,
                    flaky: false,
                },
            ]
        );
        assert_eq!(
            evidence_summary(&evidence),
            "only request told a variant from its baseline in 6 cycles; request scored the same baseline commit two ways"
        );
        let (question, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(question.is_empty());
        assert!(reason.contains("B-004, B-005, B-006"), "{reason}");
        let body = flat_fixture_question_body(&s, "demo").unwrap();
        for want in [
            "- F01: public, metric no, weight 1\n",
            "- metric: public, metric no, weight 2; told no variant from its baseline in 6 cycles; moved 1 time with the default branch\n",
            "- request: public, metric no, weight 1; told a variant from its baseline in 1 of 6 cycles; scored the same baseline commit two ways, so a difference it reports is not a signal on its own\n",
            "- H1 (held-out): told no variant from its baseline in 6 cycles\n",
        ] {
            assert!(body.contains(want), "body lacks {want:?}:\n{body}");
        }
        let status = crate::readiness::planning_status(
            &s,
            &crate::status::LoopStatus {
                project: "demo".into(),
                level: "D".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(status.state, "blocked", "{}", status.reason);
        assert!(
            status.reason.contains(
                "; only request told a variant from its baseline in 6 cycles; request scored the same baseline commit two ways; owner: add a fixture"
            ),
            "{}",
            status.reason
        );
        // Before any scored cycle the summary is empty and the body says
        // nothing it cannot show.
        assert_eq!(evidence_summary(&[]), "");
        let plain = evidence_summary(&[FixtureEvidence {
            name: "metric".into(),
            cycles: 3,
            ..Default::default()
        }]);
        assert_eq!(
            plain,
            "no fixture told a variant from its baseline in 3 cycles"
        );
    }

    #[test]
    fn baseline_refresh_rows_are_not_cycles() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO evolution_archive (id, project, suggestion_post, hypothesis, branch,
				baseline_score, variant_score, tolerance, kept, fixtures_json, at)
				VALUES ('B-003', 'demo', '', 'baseline', '', 0.5, 0.5, 0.02, 1, '{}', ?)",
                [stamp(&s, -10)],
            )
            .unwrap();
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            reason.is_empty(),
            "a baseline row counted as a cycle: {reason}"
        );
        tie(&s, "B-004", -5);
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(reason.contains("B-001, B-002, B-004"), "{reason}");
    }

    #[test]
    fn quiet_while_one_cycle_in_the_window_moved() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -40);
        cycle(&s, "B-002", 0.5, 0.6, -30);
        tie(&s, "B-003", -20);
        tie(&s, "B-004", -10);
        let (question, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            question.is_empty() && reason.is_empty(),
            "a moved cycle inside the window fired the gate: {reason}"
        );
        cycle(&s, "B-005", 0.5, 0.5 + 1e-12, -5);
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            reason.contains("B-003, B-004, B-005"),
            "a difference below 1e-9 is a tie: {reason}"
        );
    }

    #[test]
    fn quiet_with_fewer_than_n_cycles() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -20);
        tie(&s, "B-002", -10);
        let (question, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            question.is_empty() && reason.is_empty(),
            "two ties fired a three-cycle gate: {reason}"
        );
        tie(&s, "B-003", -5);
        assert!(!flat_fixture_gate(&s, "demo").unwrap().1.is_empty());
    }

    #[test]
    fn quiet_while_an_enabled_fixture_is_newer_than_the_window() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        let added = crate::test_helpers::fixture(&s, "demo", "F02", "printf 1", 1.0);
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            reason.is_empty(),
            "a fixture the window never measured did not hold the gate: {reason}"
        );
        crate::fixtures::set_fixture_enabled(&s, "demo", &added.name, false, "human").unwrap();
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(!reason.is_empty(), "a disabled fixture held the gate");
        crate::fixtures::set_fixture_enabled(&s, "demo", &added.name, true, "human").unwrap();
        tie(&s, "B-004", 1);
        tie(&s, "B-005", 2);
        assert!(flat_fixture_gate(&s, "demo").unwrap().1.is_empty());
        tie(&s, "B-006", 3);
        let (_, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            reason.contains("B-004, B-005, B-006"),
            "a window measured entirely after the fixture should fire: {reason}"
        );
    }

    #[test]
    fn coalesces_on_the_open_question() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        let question = gate_post(&s, "question", "", -9);
        let (found, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert_eq!(found, question, "{reason}");
        assert!(!reason.is_empty());
        tie(&s, "B-004", -5);
        assert_eq!(flat_fixture_gate(&s, "demo").unwrap().0, question);
    }

    #[test]
    fn quiet_after_the_review_until_n_new_tied_cycles() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        let question = gate_post(&s, "question", "", -9);
        gate_post(&s, "answer", &question, -8);
        let (found, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            found.is_empty() && reason.is_empty(),
            "the answered question did not clear the gate: {found} {reason}"
        );
        tie(&s, "B-004", -7);
        tie(&s, "B-005", -6);
        assert!(
            flat_fixture_gate(&s, "demo").unwrap().1.is_empty(),
            "two new ties re-fired a three-cycle gate"
        );
        tie(&s, "B-006", -5);
        let (found, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            found.is_empty() && reason.contains("B-004, B-005, B-006"),
            "three new ties should ask again with a fresh question: {found} {reason}"
        );
    }

    #[test]
    fn a_decision_is_a_review_too() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("3");
        tie(&s, "B-001", -30);
        tie(&s, "B-002", -20);
        tie(&s, "B-003", -10);
        let question = gate_post(&s, "question", "", -9);
        pulse_board::decide(&s, &question, "no", "keep going", "human").unwrap();
        let (found, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            found.is_empty() && reason.is_empty(),
            "a decided question did not clear the gate: {found} {reason}"
        );
    }

    #[test]
    fn zero_turns_the_gate_off_and_one_counts_as_two() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store("0");
        for (i, offset) in (-60..0).step_by(10).enumerate() {
            tie(&s, &format!("B-{:03}", i + 1), offset);
        }
        assert_eq!(flat_cycles(&s), 0);
        let (question, reason) = flat_fixture_gate(&s, "demo").unwrap();
        assert!(
            question.is_empty() && reason.is_empty(),
            "evolve.flat_cycles=0 still fired: {reason}"
        );
        s.conn()
            .unwrap()
            .execute("DELETE FROM settings WHERE name = ?", [FLAT_CYCLES_SETTING])
            .unwrap();
        assert_eq!(flat_cycles(&s), 6, "the default window is six cycles");
        assert!(
            flat_fixture_gate(&s, "demo")
                .unwrap()
                .1
                .contains("the last 6 experiment cycles")
        );
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, '1', ?, 'test')",
                params![FLAT_CYCLES_SETTING, s.now()],
            )
            .unwrap();
        assert_eq!(flat_cycles(&s), 2, "a window below two counts as two");
    }
}
