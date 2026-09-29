use crate::records::{Frontmatter, apply_columns, input_error, text_value};
use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const BUNDLE_JSON: &str = include_str!("../seed/bundle.json");
pub const FRAMEWORK_KINDS: &[&str] = &[
    "instruction",
    "guide",
    "skill",
    "workflow",
    "prompt",
    "template",
    "architecture",
];
pub const BUNDLE_FILE: &str = "internal/records/seed/bundle.json";
pub const BUNDLE_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleRecord {
    pub id: String,
    pub kind: String,
    pub slug: String,
    pub frontmatter: Frontmatter,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    pub version: i64,
    pub records: Vec<BundleRecord>,
}

pub fn framework_bundle() -> Result<Bundle> {
    decode_bundle(BUNDLE_JSON.as_bytes())
}

pub fn decode_bundle(data: &[u8]) -> Result<Bundle> {
    let b: Bundle = serde_json::from_slice(data).map_err(|e| anyhow!("framework bundle: {e}"))?;
    if b.version != BUNDLE_VERSION {
        return Err(anyhow!(
            "framework bundle: version {}, this pulse reads {}",
            b.version,
            BUNDLE_VERSION
        ));
    }
    for r in &b.records {
        if r.id.is_empty() || r.kind.is_empty() || r.slug.is_empty() {
            return Err(anyhow!("framework bundle: a record lacks id, kind or slug"));
        }
        if !framework_kind(&r.kind) {
            return Err(anyhow!(
                "framework bundle: {} has kind {:?}, not one of {}",
                r.id,
                r.kind,
                FRAMEWORK_KINDS.join(", ")
            ));
        }
    }
    Ok(b)
}

pub fn framework_kind(kind: &str) -> bool {
    FRAMEWORK_KINDS.contains(&kind)
}

pub fn seeded_from() -> String {
    let mut rev = pulse_cli::revision();
    if rev.is_empty() {
        return "pulse@dev".into();
    }
    if rev.len() > 12 {
        rev.truncate(12);
    }
    format!("pulse@{rev}")
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EnsureResult {
    pub written: Vec<String>,
    pub kept: i64,
    pub from: String,
}

pub fn ensure_framework(s: &pulse_store::Store, actor: &str) -> Result<EnsureResult> {
    let mut result = EnsureResult {
        written: vec![],
        kept: 0,
        from: seeded_from(),
    };
    let b = framework_bundle()?;
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let mut missing = Vec::new();
    for r in b.records {
        match s.get_record(&r.id, 0) {
            Ok(_) => result.kept += 1,
            Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => missing.push(r),
            Err(e) => return Err(e),
        }
    }
    if missing.is_empty() {
        return Ok(result);
    }
    let _unlock = crate::lock::lock_writes(s)?;
    for r in missing {
        match s.get_record(&r.id, 0) {
            Ok(_) => {
                result.kept += 1;
                continue;
            }
            Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {}
            Err(e) => return Err(e),
        }
        let mut rec = pulse_store::Record {
            id: r.id.clone(),
            project: pulse_store::COMMONS_SLUG.into(),
            kind: r.kind.clone(),
            slug: r.slug.clone(),
            origin: format!("seed:{}", result.from),
            ..pulse_store::Record::default()
        };
        apply_columns(&mut rec, &r.frontmatter, &r.body).map_err(|e| anyhow!("{}: {e}", r.id))?;
        s.put_record(
            rec,
            r.frontmatter,
            &r.body,
            actor,
            "seeded from the framework bundle (ADR-038)",
        )?;
        result.written.push(r.id);
    }
    if !result.written.is_empty() {
        s.event(
            actor,
            "records.framework.seeded",
            pulse_store::COMMONS_SLUG,
            pulse_store::Detail::json(
                serde_json::json!({"written": result.written.len(), "from": result.from}),
            )?,
        )?;
    }
    Ok(result)
}

pub fn secret_shaped(text: &str) -> String {
    let patterns = [
        r"\bgh[pousr]_[A-Za-z0-9]{20,}",
        r"\bgithub_pat_[A-Za-z0-9_]{20,}",
        r"\bsk-[A-Za-z0-9_-]{20,}",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        r"\bAIza[0-9A-Za-z_-]{35}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"\beyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}",
    ];
    for pat in patterns {
        if let Some(m) = Regex::new(pat).unwrap().find(text) {
            let mut s = m.as_str().to_string();
            if s.len() > 12 {
                s.truncate(12);
                s.push('…');
            }
            return s;
        }
    }
    String::new()
}

pub fn retired_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "deprecated" | "retired" | "superseded"
    )
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BundleSummary {
    pub path: String,
    pub records: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub unchanged: usize,
    pub skipped: Vec<String>,
}

impl BundleSummary {
    pub fn same(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

pub fn build_bundle(s: &pulse_store::Store) -> Result<(Bundle, Vec<String>)> {
    let brain = s.brain()?.ok_or_else(|| {
        input_error(
            "Cannot build the seed bundle",
            "no active project is the brain; the framework's records are the brain's",
            "pulse init PATH --brain, then pulse seed export",
        )
    })?;
    let mut rows = s.list_records(pulse_store::RecordFilter {
        project: brain.slug,
        ..pulse_store::RecordFilter::default()
    })?;
    rows.extend(s.list_records(pulse_store::RecordFilter {
        project: pulse_store::COMMONS_SLUG.into(),
        ..pulse_store::RecordFilter::default()
    })?);
    let mut b = Bundle {
        version: BUNDLE_VERSION,
        records: vec![],
    };
    let mut skipped = Vec::new();
    for row in rows {
        if !framework_kind(&row.kind) {
            continue;
        }
        if retired_status(&row.status) {
            skipped.push(format!("{} ({})", row.id, row.status));
            continue;
        }
        let (_, rev) = s.get_record(&row.id, 0)?;
        let rev = rev.expect("current revision exists");
        let mut front = Frontmatter::new();
        for (k, v) in rev.frontmatter {
            if k != "seeded_from" && k != "seeded_by" {
                front.insert(k, v);
            }
        }
        let shape = secret_shaped(&rev.body);
        if !shape.is_empty() {
            return Err(input_error(
                &format!("Refusing to bundle {}", row.id),
                &format!("its body carries a secret-shaped value ({shape})"),
                &format!(
                    "remove the value with pulse record amend {}, then pulse seed export",
                    row.id
                ),
            ));
        }
        for (key, value) in &front {
            let shape = secret_shaped(&text_value(Some(value)));
            if !shape.is_empty() {
                return Err(input_error(
                    &format!("Refusing to bundle {}", row.id),
                    &format!("frontmatter {key} carries a secret-shaped value ({shape})"),
                    &format!(
                        "remove the value with pulse record amend {}, then pulse seed export",
                        row.id
                    ),
                ));
            }
        }
        b.records.push(BundleRecord {
            id: row.id,
            kind: row.kind,
            slug: row.slug,
            frontmatter: front,
            body: rev.body,
        });
    }
    sort_bundle(&mut b);
    Ok((b, skipped))
}

pub fn sort_bundle(b: &mut Bundle) {
    let order: BTreeMap<&str, usize> = FRAMEWORK_KINDS
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i))
        .collect();
    b.records.sort_by(|a, b| {
        order
            .get(a.kind.as_str())
            .cmp(&order.get(b.kind.as_str()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

pub fn encode_bundle(b: &Bundle) -> Result<Vec<u8>> {
    let mut data = pulse_store::gojson::to_vec_pretty_no_html(b)?;
    data.push(b'\n');
    Ok(data)
}

pub fn bundle_path(s: &pulse_store::Store, out: &str) -> Result<PathBuf> {
    if !out.trim().is_empty() {
        return Ok(PathBuf::from(out));
    }
    let brain = s.brain()?;
    let Some(brain) = brain else {
        return Err(input_error(
            "Cannot place the seed bundle",
            "no brain checkout to write it into",
            "pulse seed export --out PATH",
        ));
    };
    if brain.path.is_empty() {
        return Err(input_error(
            "Cannot place the seed bundle",
            "no brain checkout to write it into",
            "pulse seed export --out PATH",
        ));
    }
    Ok(Path::new(&brain.path).join(BUNDLE_FILE))
}

pub fn export_bundle(s: &pulse_store::Store, path: impl AsRef<Path>) -> Result<BundleSummary> {
    let path = path.as_ref();
    let mut summary = BundleSummary {
        path: path.display().to_string(),
        added: vec![],
        removed: vec![],
        changed: vec![],
        skipped: vec![],
        ..BundleSummary::default()
    };
    let (b, skipped) = build_bundle(s)?;
    summary.records = b.records.len();
    summary.skipped = skipped;
    let data = encode_bundle(&b)?;
    match fs::read(path) {
        Ok(old) => match decode_bundle(&old) {
            Ok(prev) => diff_bundles(&prev, &b, &mut summary),
            Err(_) => summary.added.extend(b.records.iter().map(|r| r.id.clone())),
        },
        Err(_) => summary.added.extend(b.records.iter().map(|r| r.id.clone())),
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, data)?;
    Ok(summary)
}

pub fn diff_bundles(prev: &Bundle, next: &Bundle, summary: &mut BundleSummary) {
    let before: BTreeMap<_, _> = prev.records.iter().map(|r| (&r.id, r)).collect();
    let mut after = BTreeSet::new();
    for r in &next.records {
        after.insert(&r.id);
        match before.get(&r.id) {
            None => summary.added.push(r.id.clone()),
            Some(old) if same_bundle_record(old, r) => summary.unchanged += 1,
            Some(_) => summary.changed.push(r.id.clone()),
        }
    }
    for r in &prev.records {
        if !after.contains(&r.id) {
            summary.removed.push(r.id.clone());
        }
    }
    summary.removed.sort();
}

fn same_bundle_record(a: &BundleRecord, b: &BundleRecord) -> bool {
    a.kind == b.kind
        && a.slug == b.slug
        && a.body == b.body
        && pulse_store::gojson::to_vec(&a.frontmatter).ok()
            == pulse_store::gojson::to_vec(&b.frontmatter).ok()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BundleStatus {
    pub records: usize,
    pub present: usize,
    pub missing: Vec<String>,
    pub stale: Vec<String>,
    pub unshipped: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub brain: String,
}

pub fn framework_status(s: &pulse_store::Store) -> Result<BundleStatus> {
    let mut status = BundleStatus {
        missing: vec![],
        stale: vec![],
        unshipped: vec![],
        ..BundleStatus::default()
    };
    let b = framework_bundle()?;
    status.records = b.records.len();
    let mut shipped = BTreeMap::new();
    for r in b.records {
        shipped.insert(r.id.clone(), r.clone());
        match s.get_record(&r.id, 0) {
            Ok(_) => status.present += 1,
            Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {
                status.missing.push(r.id)
            }
            Err(e) => return Err(e),
        }
    }
    let Some(brain) = s.brain()? else {
        return Ok(status);
    };
    status.brain = brain.slug;
    let (current, _) = build_bundle(s)?;
    for r in current.records {
        match shipped.get(&r.id) {
            None => status.unshipped.push(r.id),
            Some(old) if !same_bundle_record(old, &r) => status.stale.push(r.id),
            Some(_) => {}
        }
    }
    Ok(status)
}

pub use build_bundle as BuildBundle;
pub use bundle_path as BundlePath;
pub use encode_bundle as encodeBundle;
pub use ensure_framework as EnsureFramework;
pub use export_bundle as ExportBundle;
pub use framework_bundle as FrameworkBundle;
pub use framework_status as FrameworkStatus;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store() -> pulse_store::Store {
        let home = tempfile::tempdir().unwrap().keep();
        pulse_store::Store::open(pulse_store::Options {
            home: home.display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap()
    }

    fn add_project(s: &pulse_store::Store, slug: &str, path: &std::path::Path, brain: bool) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, slug, path.display().to_string(), brain, s.now()],
            )
            .unwrap();
    }

    #[test]
    fn framework_bundle_carries_the_framework() {
        let b = framework_bundle().unwrap();
        assert!(
            b.version == BUNDLE_VERSION && b.records.len() >= 60,
            "bundle version {} with {} records",
            b.version,
            b.records.len()
        );
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        let mut ids: BTreeMap<String, BundleRecord> = BTreeMap::new();
        for (i, r) in b.records.iter().enumerate() {
            *kinds.entry(r.kind.clone()).or_default() += 1;
            assert!(!ids.contains_key(&r.id), "{} occurs twice", r.id);
            ids.insert(r.id.clone(), r.clone());
            assert!(
                framework_kind(&r.kind) && !r.slug.is_empty() && !r.body.trim().is_empty(),
                "record {i} is not a framework record: {}",
                r.id
            );
            for key in ["seeded_from", "seeded_by"] {
                assert!(
                    !r.frontmatter.contains_key(key),
                    "{} carries {key}; the bundle is the source, not a copy",
                    r.id
                );
            }
            let shape = secret_shaped(&r.body);
            assert!(shape.is_empty(), "{} carries a {shape}", r.id);
            assert!(
                !retired_status(&crate::records::text_value(r.frontmatter.get("status"))),
                "{} is {:?} and still bundled",
                r.id,
                r.frontmatter.get("status")
            );
            assert!(
                !r.body.contains("/Users/") && !r.body.contains("/home/"),
                "{} names a home directory",
                r.id
            );
        }
        for kind in FRAMEWORK_KINDS {
            assert!(
                kinds.get(*kind).copied().unwrap_or(0) > 0,
                "no {kind} record in the bundle"
            );
        }
        for id in [
            "INSTR-agents",
            "GUIDE-new-repo",
            "TPL-evolution-constitution",
            "TPL-evolution-evaluators",
            "TPL-adr",
            "TPL-plan",
            "WF-pulse",
            "ARCH-overview",
        ] {
            assert!(ids.contains_key(id), "bundle lacks {id}");
        }
        let mut sorted = b.clone();
        sort_bundle(&mut sorted);
        for i in 0..b.records.len() {
            assert_eq!(
                b.records[i].id, sorted.records[i].id,
                "bundle is not in export order at {i}: {} vs {}",
                b.records[i].id, sorted.records[i].id
            );
        }
        // The file on disk is the bundle re-encoded: an export that changes nothing rewrites nothing.
        let encoded = encode_bundle(&b).unwrap();
        assert!(
            encoded == BUNDLE_JSON.as_bytes(),
            "seed/bundle.json is not in encodeBundle's form; run pulse seed export"
        );
    }

    #[test]
    fn ensure_framework_seeds_a_fresh_store_once_and_keeps_what_is_there() {
        let b = framework_bundle().unwrap();
        let s = temp_store();
        let mine = pulse_store::Record {
            id: "TPL-adr".into(),
            project: "app".into(),
            kind: "template".into(),
            slug: "adr".into(),
            title: "My ADR template".into(),
            origin: "store".into(),
            ..pulse_store::Record::default()
        };
        s.put_record(
            mine,
            [
                ("id".into(), json!("TPL-adr")),
                ("title".into(), json!("My ADR template")),
            ]
            .into(),
            "# Mine\n",
            "alice",
            "",
        )
        .unwrap();
        let result = ensure_framework(&s, "alice").unwrap();
        assert_eq!(result.written.len(), b.records.len() - 1);
        assert_eq!(result.kept, 1);
        assert!(result.from.starts_with("pulse@"));
        let (rec, rev) = crate::records::get(&s, "INSTR-agents", 0).unwrap();
        assert_eq!(rec.project, pulse_store::COMMONS_SLUG);
        assert_eq!(rec.origin, format!("seed:{}", result.from));
        assert_eq!(rev.note, "seeded from the framework bundle (ADR-038)");
        let again = ensure_framework(&s, "").unwrap();
        assert!(again.written.is_empty());
        assert_eq!(again.kept as usize, b.records.len());
        let status = framework_status(&s).unwrap();
        assert_eq!(status.present, b.records.len());
        assert!(status.missing.is_empty());
    }

    #[test]
    fn framework_records_serve_the_loop_and_the_guide_without_a_brain() {
        let s = temp_store();
        let project_dir = tempfile::tempdir().unwrap();
        add_project(&s, "lamplitlabs", project_dir.path(), false);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET name = 'Lamplit Labs' WHERE slug = 'lamplitlabs'",
                [],
            )
            .unwrap();
        assert!(crate::seed::seed_loop_records(&s, "lamplitlabs", "alice").is_err());
        ensure_framework(&s, "alice").unwrap();
        let summary = crate::seed::seed_loop_records(&s, "lamplitlabs", "alice").unwrap();
        assert_eq!(summary.constitution.action, "copied");
        assert_eq!(summary.constitution.source, "TPL-evolution-constitution@r1");
        assert_eq!(summary.evaluators.action, "copied");
        assert_eq!(summary.evaluators.source, "TPL-evolution-evaluators@r1");
        let (_, rev) = crate::records::get(&s, "EVO-constitution-lamplitlabs", 0).unwrap();
        assert!(!rev.body.contains("<Project>"));
        assert!(rev.body.contains("Lamplit Labs"));
        assert_eq!(
            rev.frontmatter["seeded_from"],
            serde_json::json!("commons/TPL-evolution-constitution@r1")
        );
        let out = pulse_cli::Buffer::new();
        let err = pulse_cli::Buffer::new();
        let mut c = pulse_cli::Context {
            store: Some(s.clone()),
            home: s.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(err),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: "lamplitlabs".into(),
            session: String::new(),
            now: String::new(),
            args: vec![],
        };
        crate::register::run_guide(&mut c, &pulse_cli::FlagSet::new(""), &[]).unwrap();
        assert!(out.string().starts_with("# PULSE"));
        assert!(!out.string().contains("No instruction records"));
    }

    #[test]
    fn export_bundle_writes_the_framework_records_and_refuses_secrets() {
        let s = temp_store();
        let brain_dir = tempfile::tempdir().unwrap();
        add_project(&s, "brain", brain_dir.path(), true);
        let put = |id: &str, project: &str, kind: &str, slug: &str, status: &str, body: &str| {
            let front = [
                ("id".into(), json!(id)),
                ("title".into(), json!(slug.to_ascii_uppercase())),
                ("status".into(), json!(status)),
            ]
            .into();
            let rec = pulse_store::Record {
                id: id.into(),
                project: project.into(),
                kind: kind.into(),
                slug: slug.into(),
                title: slug.to_ascii_uppercase(),
                status: status.into(),
                origin: "store".into(),
                ..pulse_store::Record::default()
            };
            s.put_record(rec, front, body, "tester", "").unwrap();
        };
        put(
            "INSTR-agents",
            "brain",
            "instruction",
            "agents",
            "",
            "# Agents\n",
        );
        put("TPL-adr", "brain", "template", "adr", "Proposed", "# ADR\n");
        put(
            "PROMPT-old",
            "brain",
            "prompt",
            "old",
            "Deprecated",
            "# Old\n",
        );
        put(
            "ADR-001",
            "brain",
            "adr",
            "001-x",
            "Accepted",
            "# Not framework\n",
        );
        put(
            "WF-shared",
            pulse_store::COMMONS_SLUG,
            "workflow",
            "shared",
            "",
            "# Seeded first\n",
        );
        let (b, skipped) = build_bundle(&s).unwrap();
        assert_eq!(
            b.records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["INSTR-agents", "WF-shared", "TPL-adr"]
        );
        assert!(skipped[0].starts_with("PROMPT-old"));
        let path = bundle_path(&s, "").unwrap();
        assert_eq!(path, brain_dir.path().join(BUNDLE_FILE));
        let summary = export_bundle(&s, &path).unwrap();
        assert_eq!(summary.records, 3);
        assert_eq!(summary.added.len(), 3);
        assert!(!summary.same());
        let again = export_bundle(&s, &path).unwrap();
        assert!(again.same());
        assert_eq!(again.unchanged, 3);
        put(
            "TPL-adr",
            "brain",
            "template",
            "adr",
            "Proposed",
            "# ADR\n\nToken: AKIAABCDEFGHIJKLMNOP\n",
        );
        assert!(
            build_bundle(&s)
                .unwrap_err()
                .to_string()
                .contains("secret-shaped")
        );
        assert!(retired_status("Deprecated"));
        assert!(!retired_status("Accepted"));
    }
}
