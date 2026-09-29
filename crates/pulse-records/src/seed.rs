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

    const TEMPLATE_CONSTITUTION: &str = concat!(
        "# <Project> Evolution Constitution\n\nRules the loop can never change. <Project> ships the product.\n\n",
        "## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| <product hard-stop surfaces only> | Tier 2 surfaces (ADR-NNN) |\n"
    );
    const TEMPLATE_EVALUATORS: &str =
        "# <Project> Evaluators\n\nWhat better means for <Project>.\n";

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

    /// Go: namedProject
    fn named_project(s: &pulse_store::Store, slug: &str, name: &str, dir: &std::path::Path) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,0,?)",
                rusqlite::params![slug, name, dir.display().to_string(), s.now()],
            )
            .unwrap();
    }

    /// seedTemplates gives the store a brain and the two template-owned records
    /// the way pulse migrate imports them; the constitution has two revisions so
    /// tests see that the current one is copied.
    fn seed_templates(s: &pulse_store::Store) -> tempfile::TempDir {
        let brain = tempfile::tempdir().unwrap();
        add_project(s, "brain", "brain", brain.path(), true);
        let templates: [(&str, &str, crate::records::Frontmatter); 2] = [
            (
                "constitution",
                TEMPLATE_CONSTITUTION,
                [
                    ("id".to_string(), json!("CONSTITUTION")),
                    ("title".to_string(), json!("<Project> Evolution Constitution")),
                    ("status".to_string(), json!("Active")),
                    ("date".to_string(), json!("YYYY-MM-DD")),
                    ("controlling_adr".to_string(), json!("ADR-NNN")),
                    ("loop".to_string(), json!("off")),
                    ("limits".to_string(), json!("none")),
                ]
                .into(),
            ),
            (
                "evaluators",
                TEMPLATE_EVALUATORS,
                [
                    ("id".to_string(), json!("EVALUATORS")),
                    ("title".to_string(), json!("<Project> Evaluators")),
                    ("status".to_string(), json!("Active")),
                    ("date".to_string(), json!("YYYY-MM-DD")),
                    ("controlling_adr".to_string(), json!("ADR-NNN")),
                    ("baseline".to_string(), json!("pending")),
                ]
                .into(),
            ),
        ];
        for (slug, body, front) in templates {
            let path = format!("docs/evolution/_template-{slug}.md");
            let rec = pulse_store::Record {
                id: format!("TPL-evolution-{slug}"),
                project: "brain".into(),
                kind: "template".into(),
                slug: format!("evolution-{slug}"),
                title: crate::records::text_value(front.get("title")),
                status: "Active".into(),
                path: path.clone(),
                origin: format!("markdown:{path}@0000000"),
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
        brain
    }

    /// Go: eventCount (import_test.go)
    fn event_count(s: &pulse_store::Store, action: &str) -> i64 {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = ?",
                [action],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// Go: revisionCount
    fn revision_count(s: &pulse_store::Store, id: &str) -> usize {
        s.revisions(id).unwrap().len()
    }

    fn cli_code(err: &anyhow::Error) -> Option<i32> {
        err.downcast_ref::<pulse_cli::Error>().map(|e| e.code)
    }

    #[test]
    fn seed_loop_records_copies_the_template_once() {
        let s = temp_store();
        let _brain = seed_templates(&s);
        let project_dir = tempfile::tempdir().unwrap();
        named_project(&s, "lamplitlabs", "Lamplit Labs", project_dir.path());
        let summary = seed_loop_records(&s, "lamplitlabs", "alice").unwrap();
        let want = SeedSummary {
            project: "lamplitlabs".into(),
            constitution: SeedResult {
                id: "EVO-constitution-lamplitlabs".into(),
                action: "copied".into(),
                source: "TPL-evolution-constitution@r2".into(),
                rev: 1,
            },
            evaluators: SeedResult {
                id: "EVO-evaluators-lamplitlabs".into(),
                action: "copied".into(),
                source: "TPL-evolution-evaluators@r1".into(),
                rev: 1,
            },
        };
        assert_eq!(summary, want, "summary {summary:?}, want {want:?}");
        let (rec, rev) = s.get_record("EVO-constitution-lamplitlabs", 0).unwrap();
        let rev = rev.unwrap();
        assert!(
            rec.kind == "evolution"
                && rec.slug == "constitution"
                && rec.project == "lamplitlabs"
                && rec.origin == "store"
                && rec.title == "Lamplit Labs Evolution Constitution"
                && rec.status == "Active"
                && rec.date == "2026-09-06"
                && rec.created_by == "alice",
            "copied head row: {rec:?}"
        );
        assert!(
            rev.author == "alice"
                && rev.note == "seeded from TPL-evolution-constitution@r2 (ADR-030)",
            "copied revision: author {:?} note {:?}",
            rev.author,
            rev.note
        );
        for (key, value) in [
            ("id", "EVO-constitution-lamplitlabs"),
            ("title", "Lamplit Labs Evolution Constitution"),
            ("status", "Active"),
            ("date", "2026-09-06"),
            ("seeded_from", "brain/TPL-evolution-constitution@r2"),
            ("seeded_by", "alice"),
            ("loop", "off"),
            ("limits", "none"),
            ("controlling_adr", "ADR-NNN"),
        ] {
            assert_eq!(
                rev.frontmatter.get(key),
                Some(&json!(value)),
                "frontmatter {key} = {:?}, want {value:?} ({:?})",
                rev.frontmatter.get(key),
                rev.frontmatter
            );
        }
        assert_eq!(rev.frontmatter.len(), 9, "frontmatter keys: {:?}", rev.frontmatter);
        assert!(
            rev.body == TEMPLATE_CONSTITUTION.replace("<Project>", "Lamplit Labs")
                && !rev.body.contains("<Project>")
                && rev.body.contains("<product hard-stop surfaces only>")
                && rev.body.contains("ADR-NNN"),
            "copied body:\n{}",
            rev.body
        );
        let (_, evaluators) = s.get_record("EVO-evaluators-lamplitlabs", 0).unwrap();
        let evaluators = evaluators.unwrap();
        assert!(
            evaluators.frontmatter.get("baseline") == Some(&json!("pending"))
                && evaluators.frontmatter.get("seeded_from")
                    == Some(&json!("brain/TPL-evolution-evaluators@r1"))
                && evaluators.frontmatter.get("title") == Some(&json!("Lamplit Labs Evaluators"))
                && evaluators.body == TEMPLATE_EVALUATORS.replace("<Project>", "Lamplit Labs"),
            "copied evaluators: {evaluators:?}"
        );
        assert!(
            event_count(&s, "evolution.records.seeded") == 1 && event_count(&s, "record.created") == 4,
            "seeding did not record one summary event beside the two created records"
        );
        assert!(
            revision_count(&s, "TPL-evolution-constitution") == 2
                && revision_count(&s, "TPL-evolution-evaluators") == 1,
            "seeding wrote to the templates"
        );

        // Repeating keeps both; nothing is revised and no event is added.
        let again = seed_loop_records(&s, "lamplitlabs", "bob").unwrap();
        let kept = SeedSummary {
            project: "lamplitlabs".into(),
            constitution: SeedResult {
                id: "EVO-constitution-lamplitlabs".into(),
                action: "kept".into(),
                source: String::new(),
                rev: 1,
            },
            evaluators: SeedResult {
                id: "EVO-evaluators-lamplitlabs".into(),
                action: "kept".into(),
                source: String::new(),
                rev: 1,
            },
        };
        assert!(
            again == kept
                && revision_count(&s, "EVO-constitution-lamplitlabs") == 1
                && event_count(&s, "evolution.records.seeded") == 1,
            "second seeding: {again:?}"
        );

        // An existing record is never touched, whatever its text; the missing one is copied.
        let other_dir = tempfile::tempdir().unwrap();
        named_project(&s, "other", "", other_dir.path());
        let own = pulse_store::Record {
            id: "EVO-constitution-other".into(),
            project: "other".into(),
            kind: "evolution".into(),
            slug: "constitution".into(),
            title: "Own rules".into(),
            ..pulse_store::Record::default()
        };
        for body in ["first\n", "second\n"] {
            s.put_record(
                own.clone(),
                [("title".to_string(), json!("Own rules"))].into(),
                body,
                "human",
                "",
            )
            .unwrap();
        }
        let summary = seed_loop_records(&s, "other", "").unwrap();
        assert!(
            summary.constitution
                == SeedResult {
                    id: "EVO-constitution-other".into(),
                    action: "kept".into(),
                    source: String::new(),
                    rev: 2,
                }
                && summary.evaluators
                    == SeedResult {
                        id: "EVO-evaluators-other".into(),
                        action: "copied".into(),
                        source: "TPL-evolution-evaluators@r1".into(),
                        rev: 1,
                    },
            "mixed seeding: {summary:?}"
        );
        let (_, rev) = s.get_record("EVO-constitution-other", 0).unwrap();
        let rev = rev.unwrap();
        assert!(
            rev.rev == 2 && rev.body == "second\n" && !rev.frontmatter.contains_key("seeded_from"),
            "the owner's constitution changed: {rev:?}"
        );
        let (_, rev) = s.get_record("EVO-evaluators-other", 0).unwrap();
        let rev = rev.unwrap();
        assert!(
            rev.frontmatter.get("seeded_by") == Some(&json!(pulse_store::HUMAN_AGENT))
                && rev.frontmatter.get("title") == Some(&json!("other Evaluators"))
                && rev.author == pulse_store::HUMAN_AGENT,
            "an empty actor is the human and an empty name is the slug: {rev:?}"
        );

        // The commons and unknown projects get nothing.
        let err = seed_loop_records(&s, pulse_store::COMMONS_SLUG, "human").unwrap_err();
        assert_eq!(cli_code(&err), Some(2), "commons: {err}");
        let err = seed_loop_records(&s, "absent", "human").unwrap_err();
        assert_eq!(cli_code(&err), Some(2), "unknown project: {err}");
        let rows = s
            .list_records(pulse_store::RecordFilter {
                kind: "evolution".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 4, "evolution records: {}", rows.len());
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
        let _brain = seed_templates(&s);
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
