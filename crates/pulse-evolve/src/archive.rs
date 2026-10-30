use anyhow::{Result, anyhow};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::api::{ArchiveRow, Scores};

/// ScorePair is the public archive fixture payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScorePair {
    #[serde(rename = "baseline")]
    pub baseline: Option<Scores>,
    #[serde(rename = "variant")]
    pub variant: Option<Scores>,
}

/// Archived is one archive row as shown by the CLI/API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Archived {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "suggestion_post")]
    pub suggestion_post: String,
    #[serde(rename = "hypothesis")]
    pub hypothesis: String,
    #[serde(rename = "branch")]
    pub branch: String,
    #[serde(rename = "parent", default)]
    pub parent: String,
    #[serde(rename = "patch", default)]
    pub patch: String,
    #[serde(rename = "base_commit", default)]
    pub base_commit: String,
    #[serde(rename = "baseline_score")]
    pub baseline_score: f64,
    #[serde(rename = "variant_score")]
    pub variant_score: f64,
    #[serde(rename = "tolerance")]
    pub tolerance: f64,
    #[serde(rename = "kept")]
    pub kept: bool,
    #[serde(rename = "fixtures")]
    pub fixtures: ScorePair,
    #[serde(rename = "sandbox")]
    pub sandbox: String,
    #[serde(rename = "run_id")]
    pub run_id: String,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "note")]
    pub note: String,
}

/// ArchiveOutcome is what the Keep rule saw in an archived row, derived from
/// its scores: `Improved` is a kept row whose delta reached the tolerance, or
/// a kept tie whose card's own check rose (`improved`); `Neutral` any other
/// kept tie (`neutral`: the branch may still land, but nothing was shown to
/// move); `Discarded` a row the rule did not keep. Lineage credits
/// productivity to `Improved` rows only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveOutcome {
    Improved,
    Neutral,
    Discarded,
}

impl ArchiveOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::Neutral => "neutral",
            Self::Discarded => "discarded",
        }
    }
}

impl std::fmt::Display for ArchiveOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl Archived {
    pub fn delta(&self) -> f64 {
        self.variant_score - self.baseline_score
    }

    /// Mirrors [`crate::api::keep`]: `delta >= tolerance` is `improved`, and
    /// so is a smaller kept delta whose card check rose; any other kept delta
    /// is `neutral`.
    pub fn outcome(&self) -> ArchiveOutcome {
        if !self.kept {
            ArchiveOutcome::Discarded
        } else if self.delta() >= self.tolerance || self.card_check_rose() {
            ArchiveOutcome::Improved
        } else {
            ArchiveOutcome::Neutral
        }
    }

    /// Whether the card's own check scored higher on the variant, read from
    /// the archived measurements.
    pub fn card_check_rose(&self) -> bool {
        match (&self.fixtures.baseline, &self.fixtures.variant) {
            (Some(b), Some(v)) => crate::api::card_check_rose(b, v),
            _ => false,
        }
    }

    /// The fixtures the variant's measurement counted only inside the card's
    /// changed files.
    pub fn diff_scoped(&self) -> &[String] {
        self.fixtures
            .variant
            .as_ref()
            .map(|v| v.diff_scoped.as_slice())
            .unwrap_or(&[])
    }
}

pub(crate) fn public_scores(input: Option<&Scores>) -> Result<Scores> {
    let Some(input) = input else {
        return Err(anyhow!("a complete finite measurement is required"));
    };
    if !crate::fixtures::score_value(input.total) {
        return Err(anyhow!("a complete finite measurement is required"));
    }
    let mut out = input.clone();
    let mut held = 0;
    for f in &mut out.fixtures {
        if !crate::fixtures::score_value(f.score)
            || !crate::fixtures::finite(f.weight)
            || f.weight <= 0.0
        {
            return Err(anyhow!(
                "fixture scores and weights must be finite and valid"
            ));
        }
        if f.kind == "heldout" {
            held += 1;
            f.name = format!("H{held}");
            f.note.clear();
        } else if f.kind != "public" && f.kind != "signal" && f.kind != "card" {
            return Err(anyhow!("unknown fixture kind"));
        }
    }
    out.heldout = held;
    Ok(out)
}

pub(crate) fn baseline_shape(row: &ArchiveRow, baseline: &Scores, variant: &Scores) -> bool {
    if !crate::api::is_baseline_row(&row.hypothesis, &row.suggestion_post)
        || baseline.total != variant.total
        || baseline.fixtures.len() != variant.fixtures.len()
    {
        return false;
    }
    baseline
        .fixtures
        .iter()
        .zip(&variant.fixtures)
        .all(|(b, v)| {
            b.name == v.name && b.kind == v.kind && b.score == v.score && b.weight == v.weight
        })
}

pub(crate) fn archive(s: &pulse_store::Store, row: ArchiveRow) -> Result<String> {
    crate::fixtures::valid_project(s, &row.project)?;
    let baseline = public_scores(row.baseline.as_ref())?;
    let variant = public_scores(row.variant.as_ref())?;
    if baseline.shadow || variant.shadow {
        return Err(crate::fixtures::input_error(
            "Cannot archive a shadow run",
            "only runner experiment landing may write the archive",
            "pulse evolve archive",
        ));
    }
    if !crate::fixtures::score_value(row.tolerance) {
        return Err(crate::fixtures::input_error(
            "Cannot archive experiment",
            "tolerance must be a finite fraction in [0,1]",
            "pulse settings set evolve.tolerance 0.02",
        ));
    }
    let (keep, _) = crate::api::keep(Some(&baseline), Some(&variant), row.tolerance);
    if row.kept && !keep && !baseline_shape(&row, &baseline, &variant) {
        return Err(anyhow!(
            "a kept archive row must satisfy the unchanged Keep rule"
        ));
    }
    if (!baseline.project.is_empty() && baseline.project != row.project)
        || (!variant.project.is_empty() && variant.project != row.project)
    {
        return Err(anyhow!(
            "measurement project does not match the archive project"
        ));
    }
    for value in [
        &row.hypothesis,
        &row.note,
        &row.branch,
        &row.parent,
        &row.suggestion_post,
        &row.run_id,
    ] {
        if pulse_store::secret_shaped(value) {
            return Err(anyhow!("archive text must not contain credentials"));
        }
    }
    let fixtures = pulse_store::gojson::to_string(&ScorePair {
        baseline: Some(baseline.clone()),
        variant: Some(variant.clone()),
    })?;
    let tx = s.begin()?;
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(CAST(SUBSTR(id, 3) AS INTEGER)), 0) + 1
		FROM evolution_archive WHERE project = ?",
        params![row.project],
        |r| r.get(0),
    )?;
    let id = format!("B-{next:03}");
    let sandbox = if row.sandbox.is_empty() {
        variant.sandbox.clone()
    } else {
        row.sandbox.clone()
    };
    tx.execute(
        "INSERT INTO evolution_archive
		(id, project, suggestion_post, hypothesis, branch, baseline_score, variant_score, tolerance, kept,
		 fixtures_json, sandbox, run_id, at, note, parent, patch, base_commit) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            row.project,
            row.suggestion_post,
            row.hypothesis,
            row.branch,
            baseline.total,
            variant.total,
            row.tolerance,
            row.kept,
            fixtures,
            sandbox,
            row.run_id,
            s.now(),
            row.note,
            row.parent,
            row.patch,
            row.base_commit
        ],
    )?;
    let outcome = Archived {
        kept: row.kept,
        baseline_score: baseline.total,
        variant_score: variant.total,
        tolerance: row.tolerance,
        fixtures: ScorePair {
            baseline: Some(baseline.clone()),
            variant: Some(variant.clone()),
        },
        ..Default::default()
    }
    .outcome();
    tx.event(
        "pulse",
        "evolution.archived",
        &id,
        serde_json::json!({
            "project": row.project, "kept": row.kept, "outcome": outcome.label(), "baseline": baseline.total, "variant": variant.total,
            "tolerance": row.tolerance, "suggestion_post": row.suggestion_post, "run_id": row.run_id,
            "parent": row.parent, "patch": row.patch, "base_commit": row.base_commit,
            "diff_scoped": variant.diff_scoped, "card_check": variant.card_check
        }),
    )?;
    tx.commit()?;
    Ok(id)
}

pub fn list_archive(s: &pulse_store::Store, project: &str) -> Result<Vec<Archived>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, project, suggestion_post, hypothesis, branch, baseline_score, variant_score,
		tolerance, kept, fixtures_json, sandbox, run_id, at, note, parent, patch, base_commit FROM evolution_archive
		WHERE (? = '' OR project = ?) ORDER BY at DESC, CAST(SUBSTR(id, 3) AS INTEGER) DESC, project",
    )?;
    let mut rows = stmt.query(params![project, project])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let fixtures_json: String = r.get(9)?;
        let mut row = Archived {
            id: r.get(0)?,
            project: r.get(1)?,
            suggestion_post: r.get(2)?,
            hypothesis: r.get(3)?,
            branch: r.get(4)?,
            parent: r.get(14)?,
            patch: r.get(15)?,
            base_commit: r.get(16)?,
            baseline_score: r.get(5)?,
            variant_score: r.get(6)?,
            tolerance: r.get(7)?,
            kept: r.get::<_, i64>(8)? != 0,
            fixtures: ScorePair::default(),
            sandbox: r.get(10)?,
            run_id: r.get(11)?,
            at: r.get(12)?,
            note: r.get(13)?,
        };
        if fixtures_json.trim() != "[]" && !fixtures_json.trim().is_empty() {
            row.fixtures = serde_json::from_str(&fixtures_json)
                .map_err(|_| anyhow!("archive fixture data is malformed"))?;
            if let Some(b) = row.fixtures.baseline.as_ref() {
                row.fixtures.baseline = Some(public_scores(Some(b))?);
            }
            if let Some(v) = row.fixtures.variant.as_ref() {
                row.fixtures.variant = Some(public_scores(Some(v))?);
            }
        }
        out.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ArchiveRow, BASELINE_HYPOTHESIS, FixtureScore, Scores};
    use crate::test_helpers::*;
    use std::collections::BTreeMap;

    #[test]
    fn archive_immutable_atomic_event_and_per_project_ids() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        test_project(&s, "other");
        let (ch, cancel) = pulse_bus::default().subscribe(
            pulse_bus::Filter {
                actions: vec!["evolution.archived".into()],
                ..Default::default()
            },
            16,
        );
        let row = ArchiveRow {
            project: "demo".into(),
            hypothesis: "one variable".into(),
            branch: "evolve/example".into(),
            parent: "B-000".into(),
            patch: "/home/me/.pulse/patches/evolve/demo/B-001.patch".into(),
            base_commit: "abc123".into(),
            baseline: Some(Scores {
                project: "demo".into(),
                total: 0.2,
                ..Default::default()
            }),
            variant: Some(Scores {
                project: "demo".into(),
                total: 0.4,
                fixtures: vec![FixtureScore {
                    name: "private-file-must-not-escape".into(),
                    kind: "heldout".into(),
                    score: 0.4,
                    weight: 1.0,
                    metric: false,
                    note: "private annotation".into(),
                }],
                ..Default::default()
            }),
            tolerance: 0.02,
            kept: true,
            run_id: "run-one".into(),
            ..Default::default()
        };
        let id = crate::api::archive_row(&s, row.clone()).unwrap();
        assert_eq!(id, "B-001");
        let event = ch.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(event.subject, id);
        let detail: serde_json::Value = serde_json::from_str(&event.detail).unwrap();
        assert_eq!(detail["project"], "demo");
        assert_eq!(detail["run_id"], "run-one");
        assert_eq!(detail["parent"], "B-000");
        assert_eq!(detail["base_commit"], "abc123");
        let conn = s.conn().unwrap();
        for query in [
            "UPDATE evolution_archive SET note = 'changed'",
            "DELETE FROM evolution_archive",
        ] {
            assert!(
                conn.execute(query, []).is_err(),
                "immutable archive accepted a mutation"
            );
        }
        let rows = list_archive(&s, "demo").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].parent, "B-000");
        assert_eq!(
            rows[0].patch,
            "/home/me/.pulse/patches/evolve/demo/B-001.patch"
        );
        assert_eq!(rows[0].base_commit, "abc123");
        assert_eq!(
            rows[0].fixtures.variant.as_ref().unwrap().fixtures[0].name,
            "H1"
        );
        let data = pulse_store::gojson::to_string(&rows).unwrap();
        assert!(!data.contains("private-file-must-not-escape"));
        assert!(!data.contains("private annotation"));
        let mut row = row;
        row.project = "other".into();
        row.baseline.as_mut().unwrap().project = "other".into();
        row.variant.as_mut().unwrap().project = "other".into();
        assert_eq!(crate::api::archive_row(&s, row).unwrap(), "B-001");
        let schema: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(schema, "3");
        cancel();
    }

    #[test]
    fn archive_keeps_a_row_whose_suggestion_id_outgrows_the_token_length() {
        // The post id carries the agent's name: `copilot-fable-1012-hub-2`
        // makes it 41 characters, which the credential shape once refused as
        // "archive text must not contain credentials" - and the lane then
        // re-measured the finished experiment every retry, never archiving.
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let scores = |total: f64| {
            Some(Scores {
                project: "demo".into(),
                total,
                ..Default::default()
            })
        };
        let row = ArchiveRow {
            project: "demo".into(),
            suggestion_post: "20261012T092238Z-copilot-fable-1012-hub-2".into(),
            hypothesis: "Signal: 20261012T092238Z-copilot-fable-1012-hub-2".into(),
            branch: "evolve/20261012T092238Z-copilot-fable-1012-hub-2".into(),
            baseline: scores(0.2),
            variant: scores(0.4),
            tolerance: 0.02,
            kept: true,
            run_id: "run-20261029T091718Z-f2e5da".into(),
            ..Default::default()
        };
        assert_eq!(crate::api::archive_row(&s, row.clone()).unwrap(), "B-001");
        let mut leaked = row;
        leaked.note = "token AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGh= set".into();
        let err = crate::api::archive_row(&s, leaked).unwrap_err();
        assert!(
            err.to_string().contains("must not contain credentials"),
            "{err}"
        );
    }

    #[test]
    fn archive_concurrent_ids_and_shadow_refusal() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let row = ArchiveRow {
            project: "demo".into(),
            baseline: Some(Scores {
                total: 0.2,
                ..Default::default()
            }),
            variant: Some(Scores {
                total: 0.4,
                ..Default::default()
            }),
            tolerance: 0.02,
            kept: true,
            ..Default::default()
        };
        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = s.clone();
            let row = row.clone();
            handles.push(std::thread::spawn(move || crate::api::archive_row(&s, row)));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let rows = list_archive(&s, "demo").unwrap();
        assert_eq!(rows.len(), 8);
        let ids: BTreeMap<_, _> = rows.iter().map(|r| (r.id.clone(), true)).collect();
        for i in 1..=8 {
            assert!(ids.contains_key(&format!("B-{i:03}")));
        }
        let mut shadow = row;
        shadow.variant.as_mut().unwrap().shadow = true;
        assert!(crate::api::archive_row(&s, shadow).is_err());
        assert_eq!(count_rows(&s, "evolution_archive"), 8);
    }

    #[test]
    fn keep_contract_no_regression() {
        let _serial = crate::test_helpers::serial();
        let scores = |total: f64| crate::api::Scores {
            total,
            ..Default::default()
        };
        let improved = crate::api::keep(Some(&scores(0.5)), Some(&scores(0.6)), 0.02);
        assert!(
            improved.0 && improved.1.starts_with("improved: "),
            "{improved:?}"
        );
        let tie = crate::api::keep(Some(&scores(0.5)), Some(&scores(0.51)), 0.02);
        assert!(
            tie.0
                && tie
                    .1
                    .starts_with("neutral (a tie: the hypothesis went unmeasured"),
            "a tie within the tolerance still goes to landing, labelled neutral: {tie:?}"
        );
        assert!(
            !tie.1.contains("kept"),
            "a tie is not called a win: {tie:?}"
        );
        let exact = crate::api::keep(Some(&scores(0.875)), Some(&scores(0.875)), 0.02);
        assert!(
            exact.0 && exact.1.starts_with("neutral"),
            "pass/fail fixtures tie exactly: {exact:?}"
        );
        let regressed = crate::api::keep(Some(&scores(0.5)), Some(&scores(0.47)), 0.02);
        assert!(
            !regressed.0 && regressed.1.starts_with("discarded: "),
            "a regression beyond the tolerance is discarded: {regressed:?}"
        );
        assert!(!crate::api::keep(None, Some(&scores(1.0)), 0.02).0);
    }

    #[test]
    fn a_tie_whose_card_check_rose_is_improved() {
        let scores = |total: f64, check: Option<f64>| crate::api::Scores {
            total,
            card_check: check,
            ..Default::default()
        };
        let rose = crate::api::keep(
            Some(&scores(0.5, Some(0.5))),
            Some(&scores(0.51, Some(0.75))),
            0.02,
        );
        assert!(
            rose.0
                && rose.1.starts_with(
                    "improved (a tie on the composite, but the card's own check rose)"
                ),
            "{rose:?}"
        );
        let flat = crate::api::keep(
            Some(&scores(0.5, Some(0.5))),
            Some(&scores(0.51, Some(0.5))),
            0.02,
        );
        assert!(flat.0 && flat.1.starts_with("neutral"), "{flat:?}");
        let fell = crate::api::keep(
            Some(&scores(0.5, Some(1.0))),
            Some(&scores(0.51, Some(0.0))),
            0.02,
        );
        assert!(fell.0 && fell.1.starts_with("neutral"), "{fell:?}");
        let unmeasured = crate::api::keep(
            Some(&scores(0.5, None)),
            Some(&scores(0.51, Some(1.0))),
            0.02,
        );
        assert!(
            unmeasured.0 && unmeasured.1.starts_with("neutral"),
            "a check that only the variant ran proves nothing: {unmeasured:?}"
        );
        let regressed = crate::api::keep(
            Some(&scores(0.5, Some(0.0))),
            Some(&scores(0.4, Some(1.0))),
            0.02,
        );
        assert!(
            !regressed.0,
            "a rising check never rescues a regression: {regressed:?}"
        );
        // The archive reads the same rule back from the stored measurements.
        let archived = Archived {
            baseline_score: 0.5,
            variant_score: 0.51,
            tolerance: 0.02,
            kept: true,
            fixtures: ScorePair {
                baseline: Some(scores(0.5, Some(0.5))),
                variant: Some(scores(0.51, Some(0.75))),
            },
            ..Default::default()
        };
        assert_eq!(archived.outcome(), ArchiveOutcome::Improved);
        let mut tied = archived.clone();
        tied.fixtures.variant.as_mut().unwrap().card_check = Some(0.5);
        assert_eq!(tied.outcome(), ArchiveOutcome::Neutral);
        let mut without = archived;
        without.fixtures.variant = None;
        assert_eq!(without.outcome(), ArchiveOutcome::Neutral);
    }

    #[test]
    fn outcome_mirrors_the_keep_rule_boundary() {
        // Dyadic scores, so `delta == tolerance` is exact in binary floating point.
        let archived = |baseline: f64, variant: f64, kept: bool| Archived {
            baseline_score: baseline,
            variant_score: variant,
            tolerance: 0.25,
            kept,
            ..Default::default()
        };
        let improved = archived(0.5, 0.75, true);
        assert_eq!(improved.delta(), 0.25);
        assert_eq!(
            improved.outcome(),
            ArchiveOutcome::Improved,
            "a delta equal to the tolerance is kept as an improvement"
        );
        assert_eq!(archived(0.5, 1.0, true).outcome(), ArchiveOutcome::Improved);
        assert_eq!(
            archived(0.5, 0.625, true).outcome(),
            ArchiveOutcome::Neutral,
            "a kept change inside the tolerance is a tie, not an improvement"
        );
        assert_eq!(
            archived(0.875, 0.875, true).outcome(),
            ArchiveOutcome::Neutral,
            "pass/fail fixtures tie exactly"
        );
        assert_eq!(
            archived(0.5, 0.125, false).outcome(),
            ArchiveOutcome::Discarded
        );
        assert_eq!(
            archived(0.5, 1.0, false).outcome(),
            ArchiveOutcome::Discarded,
            "the stored kept flag decides, not the scores alone"
        );
        for (outcome, label) in [
            (ArchiveOutcome::Improved, "improved"),
            (ArchiveOutcome::Neutral, "neutral"),
            (ArchiveOutcome::Discarded, "discarded"),
        ] {
            assert_eq!(outcome.label(), label);
            assert_eq!(outcome.to_string(), label);
            assert_eq!(
                serde_json::to_string(&outcome).unwrap(),
                format!("\"{label}\"")
            );
        }
    }

    #[test]
    fn archive_accepts_the_baseline_shape_only() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let measured = || Scores {
            project: "demo".into(),
            total: 1.0,
            sandbox: "fleet/measure-1".into(),
            fixtures: vec![
                FixtureScore {
                    name: "starter-one".into(),
                    kind: "public".into(),
                    score: 1.0,
                    weight: 1.0,
                    ..Default::default()
                },
                FixtureScore {
                    name: "starter-two".into(),
                    kind: "public".into(),
                    score: 1.0,
                    weight: 2.0,
                    ..Default::default()
                },
                FixtureScore {
                    name: "private-name".into(),
                    kind: "heldout".into(),
                    score: 1.0,
                    weight: 1.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let row = ArchiveRow {
            project: "demo".into(),
            hypothesis: BASELINE_HYPOTHESIS.into(),
            branch: "main".into(),
            baseline: Some(measured()),
            variant: Some(measured()),
            tolerance: 0.02,
            kept: true,
            run_id: "run-baseline".into(),
            note: "baseline measurement".into(),
            ..Default::default()
        };
        let id = crate::api::archive_row(&s, row.clone()).unwrap();
        assert_eq!(id, "B-001");
        let rows = list_archive(&s, "demo").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].kept);
        assert_eq!(rows[0].hypothesis, BASELINE_HYPOTHESIS);
        assert_eq!(rows[0].branch, "main");
        assert_eq!(rows[0].baseline_score, 1.0);
        assert_eq!(rows[0].variant_score, 1.0);
        assert_eq!(rows[0].note, "baseline measurement");
        assert_eq!(
            rows[0].fixtures.variant.as_ref().unwrap().fixtures[2].name,
            "H1"
        );
        assert!(crate::api::is_baseline_row(
            &rows[0].hypothesis,
            &rows[0].suggestion_post
        ));
        assert!(!crate::api::is_baseline_row("one variable", ""));
        assert!(!crate::api::is_baseline_row(BASELINE_HYPOTHESIS, "S-1"));

        // A kept row that regresses the score is refused unless it has the
        // baseline shape (same measurement on both sides, no suggestion).
        let regressed = || Scores {
            project: "demo".into(),
            total: 0.9,
            ..Default::default()
        };
        let refused = vec![
            ArchiveRow {
                project: "demo".into(),
                hypothesis: "one variable".into(),
                branch: "evolve/x".into(),
                baseline: Some(measured()),
                variant: Some(regressed()),
                tolerance: 0.02,
                kept: true,
                ..Default::default()
            },
            ArchiveRow {
                project: "demo".into(),
                branch: "evolve/x".into(),
                baseline: Some(measured()),
                variant: Some(regressed()),
                tolerance: 0.02,
                kept: true,
                ..Default::default()
            },
            ArchiveRow {
                project: "demo".into(),
                hypothesis: BASELINE_HYPOTHESIS.into(),
                suggestion_post: "S-1".into(),
                branch: "main".into(),
                baseline: Some(measured()),
                variant: Some(regressed()),
                tolerance: 0.02,
                kept: true,
                ..Default::default()
            },
            ArchiveRow {
                project: "demo".into(),
                hypothesis: BASELINE_HYPOTHESIS.into(),
                branch: "main".into(),
                baseline: Some(measured()),
                variant: Some(regressed()),
                tolerance: 0.02,
                kept: true,
                ..Default::default()
            },
        ];
        for row in refused {
            let err = crate::api::archive_row(&s, row).expect_err("kept row accepted");
            assert!(err.to_string().contains("Keep rule"));
        }
        assert_eq!(count_rows(&s, "evolution_archive"), 1);
        // A tie on a real hypothesis is a kept row under the no-regression rule.
        let tie = ArchiveRow {
            project: "demo".into(),
            hypothesis: "one variable".into(),
            suggestion_post: "S-1".into(),
            branch: "evolve/x".into(),
            baseline: Some(measured()),
            variant: Some(measured()),
            tolerance: 0.02,
            kept: true,
            note: "kept (no regression)".into(),
            ..Default::default()
        };
        assert_eq!(crate::api::archive_row(&s, tie).unwrap(), "B-002");
        let mut discarded = row;
        discarded.kept = false;
        discarded.note = "not kept".into();
        assert_eq!(crate::api::archive_row(&s, discarded).unwrap(), "B-003");
    }
}
