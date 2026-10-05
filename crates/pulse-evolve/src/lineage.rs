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
    let mut kept = usize::from(row.kept);
    if let Some(kids) = children.get(id) {
        for child in kids {
            let (child_total, child_kept) = subtree_counts(child, by_id, children);
            total += child_total;
            kept += child_kept;
        }
    }
    (total, kept)
}

pub fn cmp(row: &crate::archive::Archived, rows: &[crate::archive::Archived]) -> f64 {
    let by_id: BTreeMap<String, &crate::archive::Archived> =
        rows.iter().map(|r| (r.id.clone(), r)).collect();
    let children = children(rows);
    let (total, kept) = subtree_counts(&row.id, &by_id, &children);
    (kept as f64 + 1.0) / (total as f64 + 2.0)
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
        hypothesis: row.hypothesis.clone(),
        score_delta: row.variant_score - row.baseline_score,
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
    LineageSummary {
        rows: rows.len(),
        kept: rows.iter().filter(|r| r.kept).count(),
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

    fn row(id: &str, parent: &str, kept: bool, patch: &str) -> crate::archive::Archived {
        crate::archive::Archived {
            id: id.into(),
            parent: parent.into(),
            kept,
            patch: patch.into(),
            hypothesis: "change".into(),
            ..Default::default()
        }
    }

    #[test]
    fn cmp_counts_descendant_productivity_with_beta_prior() {
        let rows = vec![
            row("B-001", "", true, ""),
            row("B-002", "B-001", false, ""),
            row("B-003", "B-001", true, ""),
            row("B-004", "B-002", false, ""),
        ];
        assert!((cmp(&rows[2], &rows) - (2.0 / 3.0)).abs() < 0.0001);
        assert!((cmp(&rows[1], &rows) - 0.25).abs() < 0.0001);
        assert!((cmp(&rows[0], &rows) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn pick_parent_obeys_rate_candidates_and_weights() {
        let dir = tempfile::tempdir().unwrap();
        let high_patch = dir.path().join("high.patch");
        let low_patch = dir.path().join("low.patch");
        std::fs::write(&high_patch, "high").unwrap();
        std::fs::write(&low_patch, "low").unwrap();
        let rows = vec![
            row("B-001", "", false, low_patch.to_str().unwrap()),
            row("B-002", "", true, high_patch.to_str().unwrap()),
            row("B-003", "B-002", true, ""),
            row("B-004", "B-002", true, ""),
        ];
        assert_eq!(pick_parent(&rows, 0.0, "seed"), None);
        assert!(pick_parent(&rows, 1.0, "seed").is_some());
        let picks = (0..64)
            .filter_map(|i| pick_parent(&rows, 1.0, &format!("seed-{i}")))
            .collect::<Vec<_>>();
        let high = picks.iter().filter(|id| *id == "B-002").count();
        let low = picks.iter().filter(|id| *id == "B-001").count();
        assert!(high > low, "high={high} low={low} picks={picks:?}");
    }
}
