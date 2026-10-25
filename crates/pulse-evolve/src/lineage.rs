use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub type ArchiveId = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LineageNode {
    pub id: ArchiveId,
    pub parent: String,
    pub depth: usize,
    pub kept: bool,
    /// `improved`, `neutral` or `discarded` ([`crate::archive::ArchiveOutcome`]).
    #[serde(default)]
    pub outcome: String,
    pub hypothesis: String,
    pub score_delta: f64,
    pub cmp: f64,
    pub children: usize,
    pub patch: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LineageSummary {
    pub rows: usize,
    pub kept: usize,
    /// Kept rows whose delta reached the tolerance; the only rows that earn
    /// clade productivity. `kept - improved` is the kept ties (`neutral`).
    #[serde(default)]
    pub improved: usize,
    #[serde(default)]
    pub neutral: usize,
    pub built_on_parent: usize,
    pub candidates: usize,
    pub best_clade: String,
    pub best_cmp: f64,
}

fn children(rows: &[crate::archive::Archived]) -> BTreeMap<String, Vec<String>> {
    let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        if !row.parent.is_empty() && ids.contains(row.parent.as_str()) {
            children
                .entry(row.parent.clone())
                .or_default()
                .push(row.id.clone());
        }
    }
    children
}

fn subtree_counts(
    id: &str,
    by_id: &BTreeMap<String, &crate::archive::Archived>,
    children: &BTreeMap<String, Vec<String>>,
) -> (usize, usize) {
    let Some(row) = by_id.get(id) else {
        return (0, 0);
    };
    let mut total = 1usize;
    let mut improved = usize::from(row.outcome() == crate::archive::ArchiveOutcome::Improved);
    if let Some(kids) = children.get(id) {
        for child in kids {
            let (child_total, child_improved) = subtree_counts(child, by_id, children);
            total += child_total;
            improved += child_improved;
        }
    }
    (total, improved)
}

/// Clade metaproductivity (Huxley-Gödel Machine): `(improved + 1) / (total + 2)`
/// over the row and its descendants, where productivity is a kept improvement
/// (`delta >= tolerance`), not a kept tie. Pass/fail held-out fixtures make a
/// green variant measure as a tie, so most kept rows say nothing about their
/// clade; they count as descendants but earn no credit. The Beta prior keeps
/// unexplored nodes alive.
pub fn cmp(row: &crate::archive::Archived, rows: &[crate::archive::Archived]) -> f64 {
    let by_id: BTreeMap<String, &crate::archive::Archived> =
        rows.iter().map(|r| (r.id.clone(), r)).collect();
    let children = children(rows);
    let (total, improved) = subtree_counts(&row.id, &by_id, &children);
    (improved as f64 + 1.0) / (total as f64 + 2.0)
}

fn path_exists(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home::home_dir()
    {
        return home.join(rest).is_file();
    }
    Path::new(path).is_file()
}

fn xorshift(mut x: u64) -> u64 {
    if x == 0 {
        x = 0x9e37_79b9_7f4a_7c15;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

fn seed_u64(seed: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in seed.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn unit(seed: u64) -> f64 {
    let v = xorshift(seed) >> 11;
    (v as f64) / ((1u64 << 53) as f64)
}

pub fn pick_parent(
    rows: &[crate::archive::Archived],
    rate: f64,
    rng_seed: &str,
) -> Option<ArchiveId> {
    let rate = if rate.is_finite() {
        rate.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if rate <= 0.0 || unit(seed_u64(rng_seed)) >= rate {
        return None;
    }
    let children = children(rows);
    let mut weighted = Vec::new();
    for row in rows {
        if row.hypothesis == crate::api::BASELINE_HYPOTHESIS
            || row.patch.is_empty()
            || !path_exists(&row.patch)
        {
            continue;
        }
        let novelty = 1.0 / (1.0 + children.get(&row.id).map_or(0, Vec::len) as f64);
        weighted.push((row.id.clone(), cmp(row, rows) * novelty));
    }
    let total: f64 = weighted.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        return None;
    }
    let mut pick = unit(seed_u64(&format!("{rng_seed}:parent"))) * total;
    for (id, weight) in weighted {
        if pick < weight {
            return Some(id);
        }
        pick -= weight;
    }
    None
}

fn visit(
    id: &str,
    depth: usize,
    rows: &[crate::archive::Archived],
    by_id: &BTreeMap<String, &crate::archive::Archived>,
    children: &BTreeMap<String, Vec<String>>,
    out: &mut Vec<LineageNode>,
) {
    let Some(row) = by_id.get(id) else {
        return;
    };
    out.push(LineageNode {
        id: row.id.clone(),
        parent: row.parent.clone(),
        depth,
        kept: row.kept,
        outcome: row.outcome().label().to_string(),
        hypothesis: row.hypothesis.clone(),
        score_delta: row.delta(),
        cmp: cmp(row, rows),
        children: children.get(&row.id).map_or(0, Vec::len),
        patch: row.patch.clone(),
        base_commit: row.base_commit.clone(),
    });
    if let Some(kids) = children.get(id) {
        for child in kids {
            visit(child, depth + 1, rows, by_id, children, out);
        }
    }
}

pub fn tree(rows: &[crate::archive::Archived]) -> Vec<LineageNode> {
    let by_id: BTreeMap<String, &crate::archive::Archived> =
        rows.iter().map(|r| (r.id.clone(), r)).collect();
    let children = children(rows);
    let mut roots: Vec<String> = rows
        .iter()
        .filter(|r| r.parent.is_empty() || !by_id.contains_key(&r.parent))
        .map(|r| r.id.clone())
        .collect();
    roots.sort();
    let mut out = Vec::new();
    for root in roots {
        visit(&root, 0, rows, &by_id, &children, &mut out);
    }
    out
}

pub fn summary(rows: &[crate::archive::Archived]) -> LineageSummary {
    let nodes = tree(rows);
    let mut best = nodes.iter().max_by(|a, b| {
        a.cmp
            .partial_cmp(&b.cmp)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    let best_node = best.take();
    let outcomes =
        |want: crate::archive::ArchiveOutcome| rows.iter().filter(|r| r.outcome() == want).count();
    LineageSummary {
        rows: rows.len(),
        kept: rows.iter().filter(|r| r.kept).count(),
        improved: outcomes(crate::archive::ArchiveOutcome::Improved),
        neutral: outcomes(crate::archive::ArchiveOutcome::Neutral),
        built_on_parent: rows.iter().filter(|r| !r.parent.is_empty()).count(),
        candidates: rows
            .iter()
            .filter(|r| {
                r.hypothesis != crate::api::BASELINE_HYPOTHESIS
                    && !r.patch.is_empty()
                    && path_exists(&r.patch)
            })
            .count(),
        best_clade: best_node.map(|n| n.id.clone()).unwrap_or_default(),
        best_cmp: best_node.map(|n| n.cmp).unwrap_or(0.0),
    }
}

pub fn parent_rate(s: &pulse_store::Store) -> f64 {
    s.setting("evolve.parent_rate", "0.5")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{ArchiveOutcome, Archived};

    /// A row the Keep rule classifies as `outcome` at tolerance 0.02.
    fn row(id: &str, parent: &str, outcome: ArchiveOutcome, patch: &str) -> Archived {
        let (variant_score, kept) = match outcome {
            ArchiveOutcome::Improved => (0.6, true),
            ArchiveOutcome::Neutral => (0.5, true),
            ArchiveOutcome::Discarded => (0.4, false),
        };
        let row = Archived {
            id: id.into(),
            parent: parent.into(),
            kept,
            baseline_score: 0.5,
            variant_score,
            tolerance: 0.02,
            patch: patch.into(),
            hypothesis: "change".into(),
            ..Default::default()
        };
        assert_eq!(row.outcome(), outcome);
        row
    }

    #[test]
    fn cmp_counts_descendant_productivity_with_beta_prior() {
        use ArchiveOutcome::*;
        let rows = vec![
            row("B-001", "", Improved, ""),
            row("B-002", "B-001", Discarded, ""),
            row("B-003", "B-001", Improved, ""),
            row("B-004", "B-002", Discarded, ""),
        ];
        assert!((cmp(&rows[2], &rows) - (2.0 / 3.0)).abs() < 0.0001);
        assert!((cmp(&rows[1], &rows) - 0.25).abs() < 0.0001);
        assert!((cmp(&rows[0], &rows) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn cmp_credits_kept_improvements_not_kept_ties() {
        use ArchiveOutcome::*;
        let rows = vec![
            row("B-001", "", Neutral, ""),
            row("B-002", "B-001", Neutral, ""),
            row("B-003", "B-001", Neutral, ""),
            row("B-004", "B-001", Neutral, ""),
            row("B-005", "", Neutral, ""),
            row("B-006", "B-005", Improved, ""),
            row("B-007", "B-005", Improved, ""),
            row("B-008", "B-005", Improved, ""),
        ];
        // Three kept ties: four descendants, none of them productive.
        assert!((cmp(&rows[0], &rows) - 1.0 / 6.0).abs() < 0.0001);
        // Three kept improvements under an identical tied parent.
        assert!((cmp(&rows[4], &rows) - 4.0 / 6.0).abs() < 0.0001);
        assert!((cmp(&rows[1], &rows) - 1.0 / 3.0).abs() < 0.0001);
        assert!((cmp(&rows[5], &rows) - 2.0 / 3.0).abs() < 0.0001);
        let nodes = tree(&rows);
        assert_eq!(nodes[0].outcome, "neutral");
        assert!(nodes[0].kept);
        assert_eq!(nodes[5].outcome, "improved");
        let summary = summary(&rows);
        assert_eq!(
            (summary.kept, summary.improved, summary.neutral),
            (8, 3, 5),
            "{summary:?}"
        );
        // B-005's clade and each improved leaf tie at 2/3; the tied clade is behind.
        assert!((summary.best_cmp - 2.0 / 3.0).abs() < 0.0001);
        assert_ne!(summary.best_clade, "B-001");
    }

    #[test]
    fn pick_parent_obeys_rate_candidates_and_prefers_the_improved_clade() {
        use ArchiveOutcome::*;
        let dir = tempfile::tempdir().unwrap();
        let high_patch = dir.path().join("high.patch");
        let low_patch = dir.path().join("low.patch");
        std::fs::write(&high_patch, "high").unwrap();
        std::fs::write(&low_patch, "low").unwrap();
        // Two tied roots with three kept children each; only the children's
        // outcomes differ, so the old "count kept" rule weighted them equally.
        let rows = vec![
            row("B-001", "", Neutral, low_patch.to_str().unwrap()),
            row("B-002", "B-001", Neutral, ""),
            row("B-003", "B-001", Neutral, ""),
            row("B-004", "B-001", Neutral, ""),
            row("B-005", "", Neutral, high_patch.to_str().unwrap()),
            row("B-006", "B-005", Improved, ""),
            row("B-007", "B-005", Improved, ""),
            row("B-008", "B-005", Improved, ""),
        ];
        assert_eq!(pick_parent(&rows, 0.0, "seed"), None);
        assert!(pick_parent(&rows, 1.0, "seed").is_some());
        let picks = (0..64)
            .filter_map(|i| pick_parent(&rows, 1.0, &format!("seed-{i}")))
            .collect::<Vec<_>>();
        assert!(
            picks.iter().all(|id| id == "B-001" || id == "B-005"),
            "a row without a saved patch is not a candidate: {picks:?}"
        );
        let high = picks.iter().filter(|id| *id == "B-005").count();
        let low = picks.iter().filter(|id| *id == "B-001").count();
        // Weights are cmp x novelty: 4/6 x 1/4 against 1/6 x 1/4, so the
        // improved clade should take about four picks in five.
        assert!(high > 2 * low, "high={high} low={low} picks={picks:?}");
    }
}
