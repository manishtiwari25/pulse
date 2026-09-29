use crate::import::{ImportOptions, git_read};
use crate::records::{Frontmatter, apply_columns, input_error, scoped_id, text_value};
use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SeedResult {
    pub id: String,
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub rev: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SeedSummary {
    pub project: String,
    pub constitution: SeedResult,
    pub evaluators: SeedResult,
}

const LOOP_RECORDS: &[&str] = &["constitution", "evaluators"];

pub fn seed_loop_records(
    s: &pulse_store::Store,
    project: &str,
    actor: &str,
) -> Result<SeedSummary> {
    let mut summary = SeedSummary {
        project: project.into(),
        ..SeedSummary::default()
    };
    if project == pulse_store::COMMONS_SLUG {
        return Err(input_error(
            "Cannot seed the commons",
            "the loop's records belong to a project with a checkout",
            "pulse projects",
        ));
    }
    let p = s.project(project)?.ok_or_else(|| {
        input_error(
            &format!("Cannot seed {project}"),
            "no such project",
            "pulse projects",
        )
    })?;
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    summary.project = p.slug.clone();
    let committed = committed_blobs(&p.path, &loop_record_files());
    let mut created = false;
    for slug in LOOP_RECORDS {
        let result = seed_loop_record(s, &p, slug, actor, &committed)?;
        created |= matches!(result.action.as_str(), "copied" | "imported");
        if *slug == "constitution" {
            summary.constitution = result;
        } else {
            summary.evaluators = result;
        }
    }
    if created {
        s.event(
            actor,
            "evolution.records.seeded",
            &p.slug,
            pulse_store::Detail::json(&summary)?,
        )?;
    }
    Ok(summary)
}

fn seed_loop_record(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    slug: &str,
    actor: &str,
    committed: &BTreeMap<String, bool>,
) -> Result<SeedResult> {
    if let Some(existing) = loop_record(s, &p.slug, slug)? {
        return Ok(SeedResult {
            id: existing.id,
            action: "kept".into(),
            rev: existing.current_rev,
            source: String::new(),
        });
    }
    if p.brain {
        return Ok(SeedResult {
            id: scoped_id("evolution", slug, p),
            action: "absent".into(),
            ..SeedResult::default()
        });
    }
    let file = format!("docs/evolution/{slug}.md");
    if committed.get(&file).copied().unwrap_or(false) {
        import_loop_record(s, p, slug, &file, actor)
    } else {
        copy_loop_record(s, p, slug, actor)
    }
}

fn loop_record(
    s: &pulse_store::Store,
    project: &str,
    slug: &str,
) -> Result<Option<pulse_store::Record>> {
    let rows = s.list_records(pulse_store::RecordFilter {
        project: project.into(),
        kind: "evolution".into(),
        slug: slug.into(),
        ..pulse_store::RecordFilter::default()
    })?;
    Ok(rows.into_iter().next())
}

fn loop_record_files() -> Vec<String> {
    LOOP_RECORDS
        .iter()
        .map(|slug| format!("docs/evolution/{slug}.md"))
        .collect()
}

fn committed_blobs(repo: &str, paths: &[String]) -> BTreeMap<String, bool> {
    let mut found = BTreeMap::new();
    if repo.is_empty() || paths.is_empty() {
        return found;
    }
    let mut args = vec![
        "ls-tree".to_string(),
        "-z".into(),
        "HEAD".into(),
        "--".into(),
    ];
    args.extend(paths.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Ok(out) = git_read(std::path::Path::new(repo), &refs) else {
        return found;
    };
    for line in out.split('\0') {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() >= 2 && fields[1] == "blob" {
            found.insert(path.into(), true);
        }
    }
    found
}

fn import_loop_record(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    slug: &str,
    file: &str,
    actor: &str,
) -> Result<SeedResult> {
    let summary = crate::import::import_repo(
        s,
        &p.path,
        "HEAD",
        &p.slug,
        ImportOptions {
            author: actor.into(),
            paths: vec![file.into()],
            ..ImportOptions::default()
        },
    )?;
    if let Some(skipped) = summary.skipped.first() {
        return Err(input_error(
            &format!("Cannot index {file} for {}", p.slug),
            &skipped.reason,
            &format!(
                "fix the committed file, then pulse evolve seed --project {}",
                p.slug
            ),
        ));
    }
    let Some(rec) = loop_record(s, &p.slug, slug)? else {
        return Err(anyhow!("{}: {file} was not indexed", p.slug));
    };
    Ok(SeedResult {
        id: rec.id,
        action: "imported".into(),
        source: format!("{file}@{}", summary.short_ref),
        rev: rec.current_rev,
    })
}

fn copy_loop_record(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    slug: &str,
    actor: &str,
) -> Result<SeedResult> {
    let (source, revision) = seed_source(s, slug)?;
    let _unlock = crate::lock::lock_writes(s)?;
    let id = scoped_id("evolution", slug, p);
    match s.get_record(&id, 0) {
        Ok((rec, _)) => {
            return Ok(SeedResult {
                id,
                action: "kept".into(),
                rev: rec.current_rev,
                source: String::new(),
            });
        }
        Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {}
        Err(e) => return Err(e),
    }
    let name = if p.name.trim().is_empty() {
        p.slug.clone()
    } else {
        p.name.trim().to_string()
    };
    let from = format!("{}@r{}", source.id, revision.rev);
    let body = revision.body.replace("<Project>", &name);
    let mut front: Frontmatter = revision.frontmatter.clone();
    front.insert("id".into(), Value::String(id.clone()));
    front.insert(
        "title".into(),
        Value::String(text_value(revision.frontmatter.get("title")).replace("<Project>", &name)),
    );
    front.insert("status".into(), Value::String("Active".into()));
    front.insert("date".into(), Value::String(s.now()[..10].into()));
    front.insert(
        "seeded_from".into(),
        Value::String(format!("{}/{}", source.project, from)),
    );
    front.insert("seeded_by".into(), Value::String(actor.into()));
    let mut rec = pulse_store::Record {
        id: id.clone(),
        project: p.slug.clone(),
        kind: "evolution".into(),
        slug: slug.into(),
        origin: "store".into(),
        ..pulse_store::Record::default()
    };
    apply_columns(&mut rec, &front, &body)?;
    front.insert("title".into(), Value::String(rec.title.clone()));
    let rev = s.put_record(
        rec,
        front,
        &body,
        actor,
        &format!("seeded from {from} (ADR-030)"),
    )?;
    Ok(SeedResult {
        id,
        action: "copied".into(),
        source: from,
        rev,
    })
}

fn seed_source(
    s: &pulse_store::Store,
    slug: &str,
) -> Result<(pulse_store::Record, pulse_store::Revision)> {
    let candidates = [
        (format!("TPL-evolution-{slug}"), "template", 0),
        (format!("EVO-{slug}"), "evolution", 1),
    ];
    for (id, kind, revno) in candidates {
        match s.get_record(&id, revno) {
            Ok((rec, Some(rev))) if rec.kind == kind => return Ok((rec, rev)),
            Ok(_) => {}
            Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {}
            Err(e) => return Err(e),
        }
    }
    Err(input_error(
        &format!("Cannot seed the {slug} record"),
        &format!("the store has neither TPL-evolution-{slug} nor EVO-{slug} to copy from"),
        &format!(
            "pulse migrate --records --repo BRAIN_CHECKOUT  (imports the templates), or pulse record new evolution --project P --title {}{} --body-file FILE",
            &slug[..1].to_ascii_uppercase(),
            &slug[1..]
        ),
    ))
}

pub use seed_loop_records as SeedLoopRecords;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONSTITUTION: &str = "# <Project> Evolution Constitution\n\nRules the loop can never change. <Project> ships the product.\n";
    const EVALUATORS: &str = "# <Project> Evaluators\n\nWhat better means for <Project>.\n";

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

    fn add_project(
        s: &pulse_store::Store,
        slug: &str,
        name: &str,
        path: &std::path::Path,
        brain: bool,
    ) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, name, path.display().to_string(), brain, s.now()],
            )
            .unwrap();
    }

    fn seed_templates(s: &pulse_store::Store) {
        let brain = tempfile::tempdir().unwrap();
        add_project(s, "brain", "brain", brain.path(), true);
        for (slug, body, extra) in [
            ("constitution", CONSTITUTION, ("loop", "off")),
            ("evaluators", EVALUATORS, ("baseline", "pending")),
        ] {
            let front: crate::records::Frontmatter = [
                ("id".into(), json!(slug.to_ascii_uppercase())),
                (
                    "title".into(),
                    json!(format!(
                        "<Project> {}",
                        if slug == "constitution" {
                            "Evolution Constitution"
                        } else {
                            "Evaluators"
                        }
                    )),
                ),
                ("status".into(), json!("Active")),
                ("date".into(), json!("YYYY-MM-DD")),
                (extra.0.into(), json!(extra.1)),
            ]
            .into();
            let rec = pulse_store::Record {
                id: format!("TPL-evolution-{slug}"),
                project: "brain".into(),
                kind: "template".into(),
                slug: format!("evolution-{slug}"),
                title: format!("<Project> {slug}"),
                status: "Active".into(),
                path: format!("docs/evolution/_template-{slug}.md"),
                origin: "markdown:x@0000000".into(),
                ..pulse_store::Record::default()
            };
            if slug == "constitution" {
                s.put_record(
                    rec.clone(),
                    front.clone(),
                    &format!("draft {body}"),
                    "tester",
                    "import",
                )
                .unwrap();
            }
            s.put_record(rec, front, body, "tester", "import").unwrap();
        }
    }

    #[test]
    fn seed_loop_records_copies_the_template_once() {
        let s = temp_store();
        seed_templates(&s);
        let project_dir = tempfile::tempdir().unwrap();
        add_project(&s, "lamplitlabs", "Lamplit Labs", project_dir.path(), false);
        let summary = seed_loop_records(&s, "lamplitlabs", "alice").unwrap();
        assert_eq!(summary.constitution.action, "copied");
        assert_eq!(summary.constitution.source, "TPL-evolution-constitution@r2");
        assert_eq!(summary.evaluators.source, "TPL-evolution-evaluators@r1");
        let (rec, rev) = crate::records::get(&s, "EVO-constitution-lamplitlabs", 0).unwrap();
        assert_eq!(rec.project, "lamplitlabs");
        assert_eq!(rec.title, "Lamplit Labs Evolution Constitution");
        assert!(rev.body.contains("Lamplit Labs ships the product"));
        assert_eq!(
            rev.frontmatter["seeded_from"],
            json!("brain/TPL-evolution-constitution@r2")
        );
        let again = seed_loop_records(&s, "lamplitlabs", "bob").unwrap();
        assert_eq!(again.constitution.action, "kept");
        assert_eq!(again.evaluators.action, "kept");
        assert!(seed_loop_records(&s, pulse_store::COMMONS_SLUG, "human").is_err());
        assert!(seed_loop_records(&s, "absent", "human").is_err());
    }

    #[test]
    fn seed_loop_records_falls_back_to_the_brains_first_revision() {
        let s = temp_store();
        let brain = tempfile::tempdir().unwrap();
        add_project(&s, "brain", "brain", brain.path(), true);
        for slug in ["constitution", "evaluators"] {
            let rec = pulse_store::Record {
                id: format!("EVO-{slug}"),
                project: "brain".into(),
                kind: "evolution".into(),
                slug: slug.into(),
                ..pulse_store::Record::default()
            };
            let mut front: crate::records::Frontmatter = [
                ("title".into(), json!(format!("Brain {slug}"))),
                ("loop".into(), json!("off")),
            ]
            .into();
            s.put_record(
                rec.clone(),
                front.clone(),
                &format!("# Brain {slug}\n\nAs first written for <Project>.\n"),
                "human",
                "import",
            )
            .unwrap();
            front.insert("loop".into(), json!("on"));
            s.put_record(
                rec,
                front,
                &format!("# Brain {slug}\n\nShaped by its owner.\n"),
                "human",
                "amend",
            )
            .unwrap();
        }
        let app = tempfile::tempdir().unwrap();
        add_project(&s, "app", "App", app.path(), false);
        let summary = seed_loop_records(&s, "app", "alice").unwrap();
        assert_eq!(summary.constitution.action, "copied");
        assert_eq!(summary.constitution.source, "EVO-constitution@r1");
        assert_eq!(summary.evaluators.source, "EVO-evaluators@r1");
        let (_, rev) = crate::records::get(&s, "EVO-constitution-app", 0).unwrap();
        assert_eq!(
            rev.body,
            "# Brain constitution\n\nAs first written for App.\n"
        );
        assert_eq!(
            rev.frontmatter["seeded_from"],
            json!("brain/EVO-constitution@r1")
        );
        assert_eq!(rev.frontmatter["loop"], json!("off"));
        assert_eq!(rev.frontmatter["title"], json!("Brain constitution"));
        assert_eq!(rev.note, "seeded from EVO-constitution@r1 (ADR-030)");
        let brain_summary = seed_loop_records(&s, "brain", "alice").unwrap();
        assert_eq!(brain_summary.constitution.action, "kept");
        assert_eq!(brain_summary.evaluators.action, "kept");
        let empty = temp_store();
        let lonely = tempfile::tempdir().unwrap();
        add_project(&empty, "lonely", "Lonely", lonely.path(), false);
        let err = seed_loop_records(&empty, "lonely", "alice").unwrap_err();
        assert!(err.to_string().contains("TPL-evolution-constitution"));
        assert!(
            empty
                .list_records(pulse_store::RecordFilter::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn seed_loop_records_indexes_the_checkouts_committed_file() {
        let s = temp_store();
        seed_templates(&s);
        let repo = tempfile::tempdir().unwrap();
        pulse_testutil::git(&[
            "-C",
            repo.path().to_str().unwrap(),
            "init",
            "-q",
            "-b",
            "main",
        ])
        .unwrap();
        std::fs::create_dir_all(repo.path().join("docs/evolution")).unwrap();
        std::fs::write(repo.path().join("docs/evolution/constitution.md"), "---\ntitle: Amistio Constitution\nstatus: Active\nloop: on\n---\n\n# Amistio Constitution\n").unwrap();
        pulse_testutil::git(&["-C", repo.path().to_str().unwrap(), "add", "."]).unwrap();
        pulse_testutil::git(&[
            "-C",
            repo.path().to_str().unwrap(),
            "commit",
            "-q",
            "-m",
            "rules",
        ])
        .unwrap();
        let short = pulse_testutil::git(&[
            "-C",
            repo.path().to_str().unwrap(),
            "rev-parse",
            "--short",
            "HEAD",
        ])
        .unwrap()
        .trim()
        .to_string();
        add_project(&s, "amistio", "Amistio", repo.path(), false);
        let summary = seed_loop_records(&s, "amistio", "alice").unwrap();
        assert_eq!(summary.constitution.action, "imported");
        assert_eq!(
            summary.constitution.source,
            format!("docs/evolution/constitution.md@{short}")
        );
        assert_eq!(summary.evaluators.action, "copied");
        let (rec, rev) = crate::records::get(&s, "EVO-constitution-amistio", 0).unwrap();
        assert_eq!(
            rec.origin,
            format!(
                "markdown:docs/evolution/constitution.md@{}",
                crate::import::git_read(repo.path(), &["rev-parse", "HEAD"])
                    .unwrap()
                    .trim()
            )
        );
        assert_eq!(rev.frontmatter["loop"], json!("on"));
    }
}
