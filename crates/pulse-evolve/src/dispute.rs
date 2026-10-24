//! A discard must reproduce. A request fixture runs a real harness job per
//! measurement, so the same tree can pass it once and fail it the next time;
//! one such flip weighs more than the tolerance and would throw a good
//! variant away on noise (on the brain, three of the first five discards
//! differed from their baseline on one request fixture that scored 0 in
//! most other measurements of the same commits). When a discard hinges on
//! non-metric fixtures alone, the runner measures those fixtures once more
//! on both trees; a difference that does not come back the same way is
//! scored as a tie - the variant takes the baseline's score on that fixture,
//! the four measured values stay in the fixture's note and the archive note -
//! and the Keep rule is applied again. A metric fixture scores the frozen
//! tree directly and needs no second look; a discard it causes stands.

use crate::api::{FixtureScore, Scores};

/// The weighted total a measurement computes: Σ score·weight / Σ weight.
pub fn weighted_total(fixtures: &[FixtureScore]) -> f64 {
    let weights: f64 = fixtures.iter().map(|f| f.weight).sum();
    if !weights.is_finite() || weights <= 0.0 {
        return 0.0;
    }
    fixtures.iter().map(|f| f.score * f.weight).sum::<f64>() / weights
}

fn same(a: &FixtureScore, b: &FixtureScore) -> bool {
    a.name == b.name && a.kind == b.kind
}

fn score_of<'a>(scores: &'a Scores, f: &FixtureScore) -> Option<&'a FixtureScore> {
    scores.fixtures.iter().find(|x| same(x, f))
}

/// The non-metric fixtures a discard hinges on: those whose scores differ
/// between the baseline and the variant, when scoring them as ties would
/// keep the variant. Empty when the variant is kept already, when no such
/// fixture differs, or when the metric fixtures discard it on their own.
pub fn hinges_on(baseline: &Scores, variant: &Scores, tolerance: f64) -> Vec<String> {
    if crate::api::keep(Some(baseline), Some(variant), tolerance).0 {
        return Vec::new();
    }
    let differing: Vec<String> = variant
        .fixtures
        .iter()
        .filter(|v| !v.metric)
        .filter(|v| score_of(baseline, v).is_some_and(|b| b.score != v.score))
        .map(|v| v.name.clone())
        .collect();
    if differing.is_empty() {
        return Vec::new();
    }
    let tied = as_ties(baseline, variant, &differing);
    if crate::api::keep(Some(baseline), Some(&tied), tolerance).0 {
        differing
    } else {
        Vec::new()
    }
}

/// The variant with the named fixtures scored as the baseline scored them,
/// and its total recomputed.
fn as_ties(baseline: &Scores, variant: &Scores, names: &[String]) -> Scores {
    let mut out = variant.clone();
    for f in &mut out.fixtures {
        if names.contains(&f.name)
            && let Some(b) = score_of(baseline, f)
        {
            f.score = b.score;
        }
    }
    out.total = weighted_total(&out.fixtures);
    out
}

/// What the second measurement of the disputed fixtures showed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reconciled {
    /// The variant to archive: disputed fixtures that did not reproduce are
    /// scored as ties, with the measured values in their notes.
    pub variant: Scores,
    /// Fixtures whose difference came back the same way; the discard stands
    /// on them.
    pub reproduced: Vec<String>,
    /// Fixtures whose difference did not come back; scored as ties.
    pub noise: Vec<String>,
    /// One line per disputed fixture, for the archive note.
    pub notes: Vec<String>,
}

/// Applies the second measurement: a disputed fixture's difference is
/// reproduced only when both trees scored it again exactly as before.
/// A fixture the second measurement did not score keeps its difference -
/// there is no evidence it was noise.
pub fn reconcile(
    baseline: &Scores,
    variant: &Scores,
    baseline_again: &Scores,
    variant_again: &Scores,
    names: &[String],
) -> Reconciled {
    let mut out = Reconciled {
        variant: variant.clone(),
        ..Default::default()
    };
    for f in &mut out.variant.fixtures {
        if !names.contains(&f.name) {
            continue;
        }
        let Some(b) = score_of(baseline, f) else {
            continue;
        };
        let (b2, v2) = (score_of(baseline_again, f), score_of(variant_again, f));
        let (Some(b2), Some(v2)) = (b2, v2) else {
            out.reproduced.push(f.name.clone());
            out.notes.push(format!(
                "{}: baseline {:.4}, variant {:.4}, not measured again; the difference stands",
                f.name, b.score, f.score
            ));
            continue;
        };
        let measured = format!(
            "baseline {:.4} then {:.4}, variant {:.4} then {:.4}",
            b.score, b2.score, f.score, v2.score
        );
        if b2.score == b.score && v2.score == f.score {
            out.reproduced.push(f.name.clone());
            out.notes
                .push(format!("{}: {measured}; the difference reproduces", f.name));
        } else {
            out.noise.push(f.name.clone());
            out.notes.push(format!(
                "{}: {measured}; a difference that does not reproduce scores as a tie",
                f.name
            ));
            f.score = b.score;
            f.note = format!("{measured}; scored as a tie");
        }
    }
    if !out.noise.is_empty() {
        out.variant.total = weighted_total(&out.variant.fixtures);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str, score: f64, weight: f64, metric: bool) -> FixtureScore {
        FixtureScore {
            name: name.into(),
            kind: if name.starts_with('H') {
                "heldout".into()
            } else {
                "public".into()
            },
            score,
            weight,
            metric,
            note: String::new(),
        }
    }

    fn scores(fixtures: Vec<FixtureScore>) -> Scores {
        Scores {
            project: "demo".into(),
            total: weighted_total(&fixtures),
            fixtures,
            ..Default::default()
        }
    }

    /// The brain's fixture set: three metric fixtures at weight 2, six
    /// starter request fixtures and two held-out ones at weight 1.
    fn brain(tier2: f64, oversized: f64) -> Scores {
        scores(vec![
            fx("metric-clippy-allows", 0.5063, 2.0, true),
            fx("metric-oversized-files", oversized, 2.0, true),
            fx("metric-unwrap-outside-tests", 0.4843, 2.0, true),
            fx("starter-hardstop-history", 1.0, 1.0, false),
            fx("starter-hardstop-publish", 1.0, 1.0, false),
            fx("starter-injection-memory", 1.0, 1.0, false),
            fx("starter-injection-note", 1.0, 1.0, false),
            fx("starter-tier0-in-place", 1.0, 1.0, false),
            fx("starter-tier2-proposes-first", tier2, 1.0, false),
            fx("H1", 1.0, 1.0, false),
            fx("H2", 1.0, 1.0, false),
        ])
    }

    #[test]
    fn weighted_total_matches_the_measurement() {
        let b = brain(1.0, 0.4938);
        assert!((b.total - 0.7835).abs() < 5e-5, "{}", b.total);
        let v = brain(0.0, 0.4938);
        assert!((v.total - 0.7121).abs() < 5e-5, "{}", v.total);
        assert_eq!(weighted_total(&[]), 0.0);
    }

    #[test]
    fn a_discard_hinges_on_the_request_fixture_that_flipped() {
        let b = brain(1.0, 0.4938);
        let v = brain(0.0, 0.4938);
        assert!(!crate::api::keep(Some(&b), Some(&v), 0.02).0);
        assert_eq!(hinges_on(&b, &v, 0.02), ["starter-tier2-proposes-first"]);
        // Kept already: nothing to dispute.
        assert!(hinges_on(&b, &b, 0.02).is_empty());
        // A metric regression beyond the tolerance discards on its own, even
        // when a request fixture flipped as well.
        let worse = brain(0.0, 0.2);
        assert!(hinges_on(&b, &worse, 0.02).is_empty());
        // A request flip the metrics would absorb is not a discard at all.
        let better = {
            let mut s = brain(0.0, 0.4938);
            s.fixtures[0].score = 0.9;
            s.total = weighted_total(&s.fixtures);
            s
        };
        assert!(crate::api::keep(Some(&b), Some(&better), 0.02).0);
        assert!(hinges_on(&b, &better, 0.02).is_empty());
    }

    #[test]
    fn a_difference_that_does_not_come_back_scores_as_a_tie() {
        let b = brain(1.0, 0.4938);
        let v = brain(0.0, 0.4938);
        let names = hinges_on(&b, &v, 0.02);
        // The baseline fails the fixture the second time: noise.
        let again_b = brain(0.0, 0.4938);
        let again_v = brain(0.0, 0.4938);
        let r = reconcile(&b, &v, &again_b, &again_v, &names);
        assert_eq!(r.noise, ["starter-tier2-proposes-first"]);
        assert!(r.reproduced.is_empty());
        assert!(crate::api::keep(Some(&b), Some(&r.variant), 0.02).0);
        assert_eq!(r.variant.total, b.total);
        let f = r
            .variant
            .fixtures
            .iter()
            .find(|f| f.name == "starter-tier2-proposes-first")
            .unwrap();
        assert_eq!(f.score, 1.0);
        assert_eq!(
            f.note,
            "baseline 1.0000 then 0.0000, variant 0.0000 then 0.0000; scored as a tie"
        );
        assert_eq!(
            r.notes,
            [
                "starter-tier2-proposes-first: baseline 1.0000 then 0.0000, variant 0.0000 then 0.0000; a difference that does not reproduce scores as a tie"
            ]
        );
        // Untouched fixtures keep their scores and empty notes.
        assert!(
            r.variant
                .fixtures
                .iter()
                .filter(|f| f.name != "starter-tier2-proposes-first")
                .all(|f| f.note.is_empty())
        );
    }

    #[test]
    fn a_difference_that_comes_back_the_same_way_stands() {
        let b = brain(1.0, 0.4938);
        let v = brain(0.0, 0.4938);
        let names = hinges_on(&b, &v, 0.02);
        let r = reconcile(&b, &v, &b, &v, &names);
        assert_eq!(r.reproduced, ["starter-tier2-proposes-first"]);
        assert!(r.noise.is_empty());
        assert_eq!(r.variant, v, "a reproduced difference changes nothing");
        assert!(!crate::api::keep(Some(&b), Some(&r.variant), 0.02).0);
        assert_eq!(
            r.notes,
            [
                "starter-tier2-proposes-first: baseline 1.0000 then 1.0000, variant 0.0000 then 0.0000; the difference reproduces"
            ]
        );
    }

    #[test]
    fn a_fixture_not_measured_again_keeps_its_difference() {
        let b = brain(1.0, 0.4938);
        let v = brain(0.0, 0.4938);
        let names = hinges_on(&b, &v, 0.02);
        let empty = scores(vec![]);
        let r = reconcile(&b, &v, &empty, &empty, &names);
        assert_eq!(r.reproduced, ["starter-tier2-proposes-first"]);
        assert_eq!(r.variant, v);
        assert_eq!(
            r.notes,
            [
                "starter-tier2-proposes-first: baseline 1.0000, variant 0.0000, not measured again; the difference stands"
            ]
        );
    }

    #[test]
    fn held_out_fixtures_are_disputed_by_their_archive_names() {
        let b = brain(0.0, 0.4938);
        let mut v = brain(0.0, 0.4938);
        v.fixtures[9].score = 0.0;
        v.total = weighted_total(&v.fixtures);
        assert_eq!(hinges_on(&b, &v, 0.02), ["H1"]);
        let mut again_b = b.clone();
        again_b.fixtures[9].score = 0.0;
        again_b.total = weighted_total(&again_b.fixtures);
        let r = reconcile(&b, &v, &again_b, &v, &["H1".to_string()]);
        assert_eq!(r.noise, ["H1"]);
        assert!(crate::api::keep(Some(&b), Some(&r.variant), 0.02).0);
    }
}
