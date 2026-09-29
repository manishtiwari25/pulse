use crate::lock::lock_writes;
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use pulse_cli as cli;
use pulse_store as store;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::BTreeMap;

pub type Filter = store::RecordFilter;
pub type Frontmatter = BTreeMap<String, Value>;

#[derive(Debug, Clone, Default)]
pub struct NewOptions {
    pub areas: Vec<String>,
    pub tags: Vec<String>,
    pub tier: i64,
    pub tier_set: bool,
    pub status: String,
    pub body: String,
    pub body_set: bool,
    pub author: String,
}

pub fn get(s: &store::Store, id: &str, rev: i64) -> Result<(store::Record, store::Revision)> {
    let (rec, rev) = s.get_record(id, rev)?;
    let rev = rev.ok_or_else(|| anyhow!(store::ErrNoRecord))?;
    Ok((rec, rev))
}

pub fn list(s: &store::Store, mut filter: store::RecordFilter) -> Result<Vec<store::Record>> {
    if !filter.kind.is_empty() {
        filter.kind = record_kind(&filter.kind)?;
    }
    s.list_records(filter)
}

pub fn new_record(
    s: &store::Store,
    kind: &str,
    project: &str,
    title: &str,
    mut opts: NewOptions,
) -> Result<store::Record> {
    let _unlock = lock_writes(s)?;
    let kind = record_kind(kind)?;
    let p = resolve_project(s, project, "")?;
    if title.trim().is_empty() {
        return Err(input_error(
            "Cannot create record",
            "--title must not be empty",
            &format!("pulse record new {kind} --title 'Title'"),
        ));
    }
    if opts.tier < 0 || opts.tier > 2 {
        return Err(input_error(
            "Cannot create record",
            "tier must be 0, 1, or 2",
            &format!("pulse record new {kind} --title 'Title' --tier 1"),
        ));
    }
    if !opts.tier_set && opts.tier == 0 && numbered(&kind) {
        opts.tier = if kind == "wu" { 1 } else { 2 };
    }
    opts.status = opts.status.trim().to_string();
    if opts.status.is_empty() {
        opts.status = if kind == "adr" { "Proposed" } else { "Draft" }.into();
    }
    if kind == "adr" && opts.status.trim().eq_ignore_ascii_case("Accepted") {
        return Err(accepted_error("ADR-NNN"));
    }
    if opts.areas.is_empty() && kind == "adr" {
        opts.areas = vec!["all".into()];
    }
    if kind == "adr" {
        check_wip(s, &p.slug, &opts.areas, "")?;
    }
    let slug = slugify(title);
    if slug.is_empty() {
        return Err(input_error(
            "Cannot create record",
            "title needs at least one letter or number",
            &format!("pulse record new {kind} --title 'Title'"),
        ));
    }
    let mut number = 0;
    let mut id = scoped_id(&kind, &slug, &p);
    if numbered(&kind) {
        number = s.next_record_number(&p.slug, &kind)?;
        loop {
            id = format!("{}-{number:03}", record_prefix(&kind).unwrap_or(""));
            match s.get_record(&id, 0) {
                Ok(_) => number += 1,
                Err(e) if e.downcast_ref::<store::ErrNoRecord>().is_some() => break,
                Err(e) => return Err(e),
            }
        }
    }
    match s.get_record(&id, 0) {
        Ok(_) => {
            return Err(input_error(
                &format!("Cannot create {id}"),
                "a record with this id already exists",
                &format!("pulse record amend {id} --body-file stdin"),
            ));
        }
        Err(e) if e.downcast_ref::<store::ErrNoRecord>().is_some() => {}
        Err(e) => return Err(e),
    }
    let mut body = opts.body;
    if !opts.body_set && body.is_empty() {
        body = template_body(s, &p, &kind)?;
    }
    let mut front = Frontmatter::new();
    front.insert("id".into(), Value::String(id.clone()));
    front.insert("title".into(), Value::String(title.to_string()));
    front.insert("status".into(), Value::String(opts.status.clone()));
    front.insert("tier".into(), Value::Number(Number::from(opts.tier)));
    front.insert("areas".into(), string_vec_value(&opts.areas));
    front.insert("tags".into(), string_vec_value(&opts.tags));
    front.insert("date".into(), Value::String(s.now()[..10].to_string()));
    if kind == "adr" {
        front.insert("delivery".into(), Value::String("not started".into()));
    }
    let mut rec = store::Record {
        id: id.clone(),
        project: p.slug,
        kind,
        slug,
        number,
        origin: "store".into(),
        ..store::Record::default()
    };
    apply_columns(&mut rec, &front, &body)?;
    s.put_record(rec, front, &body, &opts.author, "")?;
    Ok(s.get_record(&id, 0)?.0)
}

pub fn template_body(s: &store::Store, p: &store::Project, kind: &str) -> Result<String> {
    let mut ids = vec![scoped_id("template", kind, p)];
    if !p.brain {
        ids.push(format!("TPL-{kind}"));
    }
    for id in ids {
        match s.get_record(&id, 0) {
            Ok((rec, Some(rev))) if rec.kind == "template" => return Ok(rev.body),
            Ok(_) => {}
            Err(e) if e.downcast_ref::<store::ErrNoRecord>().is_some() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(String::new())
}

pub fn amend(
    s: &store::Store,
    id: &str,
    body: &str,
    author: &str,
    note: &str,
) -> Result<store::Record> {
    let _unlock = lock_writes(s)?;
    let (rec, rev) = get(s, id, 0)?;
    s.put_record(rec, rev.frontmatter, body, author, note)?;
    Ok(s.get_record(id, 0)?.0)
}

pub fn status(s: &store::Store, id: &str, status: &str, author: &str) -> Result<()> {
    status_note(s, id, status, author, "")
}

pub fn status_note(
    s: &store::Store,
    id: &str,
    status: &str,
    author: &str,
    note: &str,
) -> Result<()> {
    let status = status.trim();
    if status.is_empty() {
        return Err(input_error(
            "Cannot set record status",
            "status is empty",
            &format!("pulse record status {id} Draft"),
        ));
    }
    let _unlock = lock_writes(s)?;
    let (mut rec, mut rev) = get(s, id, 0)?;
    if rec.kind == "adr" && status.eq_ignore_ascii_case("Accepted") {
        return Err(accepted_error(id));
    }
    if rec.kind == "adr" && status.eq_ignore_ascii_case("Proposed") {
        check_wip(s, &rec.project, &rec.areas, id)?;
    }
    rec.status = status.into();
    rev.frontmatter
        .insert("status".into(), Value::String(status.into()));
    let mut message = format!("status: {status}");
    let note = note.trim();
    if !note.is_empty() {
        message.push_str(" - ");
        message.push_str(note);
    }
    s.put_record(rec, rev.frontmatter, &rev.body, author, &message)?;
    Ok(())
}

pub fn accept(s: &store::Store, id: &str, words: &str) -> Result<()> {
    if words.trim().is_empty() {
        return Err(input_error(
            "Cannot accept record",
            "the human's --words are required",
            &format!("pulse record accept {id} --words 'Approved because ...'"),
        ));
    }
    let _unlock = lock_writes(s)?;
    let (mut rec, mut rev) = get(s, id, 0)?;
    if rec.kind != "adr" {
        return Err(input_error(
            &format!("Cannot accept {id}"),
            "only ADRs use human acceptance",
            &format!("pulse record status {id} Accepted"),
        ));
    }
    rec.status = "Accepted".into();
    rev.frontmatter
        .insert("status".into(), Value::String("Accepted".into()));
    s.put_record(rec, rev.frontmatter, &rev.body, store::HUMAN_AGENT, words)?;
    Ok(())
}

pub fn deliver(s: &store::Store, id: &str, delivery: &str, date: &str, author: &str) -> Result<()> {
    match delivery {
        "not started" | "partially delivered" | "delivered" | "retired" => {}
        _ => {
            return Err(input_error(
                "Cannot mark delivery",
                "expected not started, partially delivered, delivered, or retired",
                &format!("pulse record deliver {id} 'delivered'"),
            ));
        }
    }
    if !date.is_empty() && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(input_error(
            "Cannot mark delivery",
            "date must be YYYY-MM-DD",
            &format!(
                "pulse record deliver {id} '{delivery}' --date {}",
                &s.now()[..10]
            ),
        ));
    }
    let _unlock = lock_writes(s)?;
    let (mut rec, mut rev) = get(s, id, 0)?;
    rec.delivery = delivery.into();
    rev.frontmatter
        .insert("delivery".into(), Value::String(delivery.into()));
    let mut date = date.to_string();
    if date.is_empty() && matches!(delivery, "delivered" | "partially delivered") {
        date = s.now()[..10].to_string();
    }
    if !date.is_empty() {
        rec.delivered = date.clone();
        rev.frontmatter
            .insert("delivered".into(), Value::String(date));
    } else if delivery == "not started" {
        rec.delivered.clear();
        rev.frontmatter
            .insert("delivered".into(), Value::String(String::new()));
    }
    s.put_record(
        rec,
        rev.frontmatter,
        &rev.body,
        author,
        &format!("delivery: {delivery}"),
    )?;
    Ok(())
}

pub fn check_wip(s: &store::Store, project: &str, areas: &[String], exclude: &str) -> Result<()> {
    let areas_buf;
    let areas = if areas.is_empty() {
        areas_buf = vec!["all".to_string()];
        &areas_buf
    } else {
        areas
    };
    let rows = s.list_records(store::RecordFilter {
        project: project.into(),
        kind: "adr".into(),
        status: "Proposed".into(),
        ..store::RecordFilter::default()
    })?;
    for r in rows {
        if r.id == exclude {
            continue;
        }
        let existing_buf;
        let existing = if r.areas.is_empty() {
            existing_buf = vec!["all".to_string()];
            &existing_buf
        } else {
            &r.areas
        };
        for a in areas {
            for b in existing {
                let aa = a.trim();
                let bb = b.trim();
                if aa.eq_ignore_ascii_case(bb)
                    || aa.eq_ignore_ascii_case("all")
                    || bb.eq_ignore_ascii_case("all")
                {
                    return Err(input_error(
                        "ADR work-in-progress limit",
                        &format!("{} is Proposed and shares area {aa:?}", r.id),
                        &format!("pulse record show {}", r.id),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn apply_columns(rec: &mut store::Record, front: &Frontmatter, body: &str) -> Result<()> {
    rec.title = text_value(front.get("title"));
    if rec.title.is_empty() {
        for line in body.split('\n') {
            if let Some(rest) = line.strip_prefix("# ") {
                rec.title = rest.trim().into();
                break;
            }
        }
    }
    if rec.title.is_empty() {
        rec.title = rec.slug.clone();
    }
    rec.status = text_value(front.get("status"));
    rec.delivery = text_value(front.get("delivery"));
    rec.delivered = text_value(front.get("delivered"));
    rec.date = text_value(front.get("date"));
    rec.areas = list_value(front.get("areas")).map_err(|e| anyhow!("areas: {e}"))?;
    rec.tags = list_value(front.get("tags")).map_err(|e| anyhow!("tags: {e}"))?;
    if let Some(value) = front.get("tier") {
        if !value.is_null() && !text_value(Some(value)).is_empty() {
            let n = text_value(Some(value)).parse::<i64>().map_err(|_| {
                anyhow!("tier must be 0, 1, or 2 (got {})", text_value(Some(value)))
            })?;
            if !(0..=2).contains(&n) {
                return Err(anyhow!(
                    "tier must be 0, 1, or 2 (got {})",
                    text_value(Some(value))
                ));
            }
            rec.tier = n;
        } else if rec.kind == "adr" {
            rec.tier = 2;
        }
    } else if rec.kind == "adr" {
        rec.tier = 2;
    }
    Ok(())
}

pub fn text_value(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => {
            if let Some(f) = n.as_f64()
                && f.fract() == 0.0
            {
                (f as i64).to_string()
            } else {
                n.to_string()
            }
        }
        Some(other) => other.to_string(),
    }
}

pub fn list_value(v: Option<&Value>) -> Result<Vec<String>> {
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::String(s)) if s.is_empty() => Ok(vec![]),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => Ok(s.clone()),
                _ => Err(anyhow!("list items must be strings")),
            })
            .collect(),
        _ => Err(anyhow!("expected a list of strings")),
    }
}

pub fn string_vec_value(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

pub fn numbered(kind: &str) -> bool {
    matches!(kind, "adr" | "plan" | "wu" | "feature")
}

pub fn record_kind(kind: &str) -> Result<String> {
    let kind = kind.to_ascii_lowercase();
    if store::record_prefix().contains_key(kind.as_str()) {
        return Ok(kind);
    }
    for (k, prefix) in store::record_prefix() {
        if prefix.eq_ignore_ascii_case(&kind) {
            return Ok(k.into());
        }
    }
    Err(input_error(
        &format!("Unknown record kind {kind}"),
        &format!("choose {}", store::RECORD_KINDS.join(", ")),
        "pulse record list",
    ))
}

pub fn record_prefix(kind: &str) -> Option<&'static str> {
    store::record_prefix().get(kind).copied()
}

pub fn resolve_project(s: &store::Store, project: &str, dir: &str) -> Result<store::Project> {
    let p = if !project.is_empty() {
        s.project(project)?
    } else {
        let dir = if dir.is_empty() {
            store::cwd()
        } else {
            dir.to_string()
        };
        s.project_for_dir(&dir)?
    };
    p.ok_or_else(|| {
        input_error(
            "Cannot resolve project",
            "register this repository or pass a registered --project",
            "pulse projects",
        )
    })
}

pub fn scoped_id(kind: &str, slug: &str, p: &store::Project) -> String {
    let mut id = format!("{}-{slug}", record_prefix(kind).unwrap_or(""));
    if !p.brain {
        id.push('-');
        id.push_str(&p.slug);
    }
    id
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in s.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            out.push(ch);
            dash = false;
        } else {
            dash = true;
        }
    }
    out
}

pub fn input_error(what: &str, why: &str, fix: &str) -> anyhow::Error {
    cli::errorf(2, what, why, fix).into()
}

pub fn accepted_error(id: &str) -> anyhow::Error {
    input_error(
        "ADR acceptance is human only",
        "agents may not set an ADR to Accepted",
        &format!("pulse record accept {id} --words '...'"),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRecords {
    pub records: Vec<store::Record>,
}

// Go-recognizable aliases.
pub use accept as Accept;
pub use amend as Amend;
pub use get as Get;
pub use list as List;
pub use new_record as New;
pub use status as Status;
pub use status_note as StatusNote;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn temp_store() -> TestStore {
        let home = tempfile::tempdir().unwrap();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        TestStore { _home: home, store }
    }

    fn add_project(s: &pulse_store::Store, slug: &str, brain: bool) {
        let dir = std::path::Path::new(&s.home).join(format!("project-{slug}"));
        std::fs::create_dir_all(&dir).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, slug, dir.display().to_string(), brain, s.now()],
            )
            .unwrap();
    }

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

    #[test]
    fn new_numbering_templates_and_wip() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        add_project(&s, "brain", true);
        add_project(&s, "app", false);
        s.put_record(
            pulse_store::Record {
                id: "TPL-adr".into(),
                kind: "template".into(),
                project: "brain".into(),
                slug: "adr".into(),
                ..pulse_store::Record::default()
            },
            Frontmatter::new(),
            "# ADR template\n",
            "tester",
            "",
        )
        .unwrap();
        let first = new_record(
            &s,
            "adr",
            "brain",
            "One",
            NewOptions {
                areas: vec!["runner".into()],
                author: "agent".into(),
                ..NewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(first.id, "ADR-001");
        assert_eq!(first.status, "Proposed");
        assert_eq!(first.tier, 2);
        assert_eq!(first.delivery, "not started");
        let (_, rev) = get(&s, &first.id, 0).unwrap();
        assert_eq!(rev.body, "# ADR template\n");
        assert!(
            new_record(
                &s,
                "adr",
                "brain",
                "Blocked",
                NewOptions {
                    areas: vec!["RUNNER".into()],
                    ..NewOptions::default()
                },
            )
            .unwrap_err()
            .to_string()
            .contains("ADR-001")
        );
        let second = new_record(
            &s,
            "adr",
            "brain",
            "Other area",
            NewOptions {
                areas: vec!["data".into()],
                body_set: true,
                ..NewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(second.id, "ADR-002");
        let (_, rev) = get(&s, &second.id, 0).unwrap();
        assert_eq!(rev.body, "");
        let app = new_record(
            &s,
            "adr",
            "app",
            "App decision",
            NewOptions {
                areas: vec!["runner".into()],
                ..NewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(app.id, "ADR-003");
        let memory = new_record(
            &s,
            "memory",
            "app",
            "Useful Pattern",
            NewOptions {
                author: "agent".into(),
                ..NewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(memory.id, "MEM-useful-pattern-app");
        assert!(new_record(&s, "memory", "app", "Useful Pattern", NewOptions::default()).is_err());
        let wu = new_record(&s, "wu", "brain", "Small unit", NewOptions::default()).unwrap();
        assert_eq!(wu.id, "WU-001");
        assert_eq!(wu.tier, 1);
        assert!(
            new_record(
                &s,
                "adr",
                "brain",
                "Invalid",
                NewOptions {
                    status: "Accepted".into(),
                    ..NewOptions::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn status_acceptance_amendment_and_delivery() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        add_project(&s, "brain", true);
        let rec = new_record(
            &s,
            "adr",
            "brain",
            "Decision",
            NewOptions {
                author: "agent".into(),
                body: "Original body\n".into(),
                body_set: true,
                ..NewOptions::default()
            },
        )
        .unwrap();
        for actor in ["agent", pulse_store::HUMAN_AGENT] {
            let err = status(&s, &rec.id, "Accepted", actor).unwrap_err();
            assert!(err.to_string().contains("pulse record accept"));
        }
        status(&s, &rec.id, "Draft", "agent").unwrap();
        assert!(accept(&s, &rec.id, "").is_err());
        accept(&s, &rec.id, "Accept the tradeoff").unwrap();
        let (head, rev) = get(&s, &rec.id, 0).unwrap();
        assert_eq!(rev.author, pulse_store::HUMAN_AGENT);
        assert_eq!(rev.note, "Accept the tradeoff");
        assert_eq!(head.status, "Accepted");
        assert_eq!(rev.frontmatter["status"], json!("Accepted"));
        assert_eq!(rev.rev, 3);
        deliver(&s, &rec.id, "delivered", "2026-09-09", "agent").unwrap();
        let amended = amend(&s, &rec.id, "Revised body\n", "agent", "Clarify rationale").unwrap();
        assert_eq!(amended.current_rev, 5);
        let (head, rev) = get(&s, &rec.id, 0).unwrap();
        assert_eq!(head.delivery, "delivered");
        assert_eq!(head.delivered, "2026-09-09");
        assert_eq!(rev.frontmatter["delivered"], json!("2026-09-09"));
        assert_eq!(rev.body, "Revised body\n");
        let (_, old) = get(&s, &rec.id, 1).unwrap();
        assert_eq!(old.body, "Original body\n");
        assert_eq!(old.frontmatter["status"], json!("Proposed"));
        assert_eq!(old.frontmatter["delivery"], json!("not started"));
        let rendered = crate::frontmatter::render_markdown(&head, &old);
        assert!(rendered.contains("\"Proposed\""));
        assert!(!rendered.contains("\"Accepted\""));
        assert!(rendered.ends_with("Original body\n"));
        assert_eq!(event_count(&s, "record.created"), 1);
        assert_eq!(event_count(&s, "record.revised"), 4);
        for delivery in ["unknown", "Delivered"] {
            assert!(deliver(&s, &rec.id, delivery, "", "agent").is_err());
        }
        assert!(deliver(&s, &rec.id, "delivered", "2026-02-30", "agent").is_err());
        deliver(&s, &rec.id, "not started", "", "agent").unwrap();
        let (head, rev) = get(&s, &rec.id, 0).unwrap();
        assert_eq!(head.delivered, "");
        assert_eq!(rev.frontmatter["delivered"], json!(""));
    }

    #[test]
    fn concurrent_new_does_not_revise_existing_ids() {
        let _serial = crate::test_helpers::serial();
        let s = std::sync::Arc::new(temp_store());
        add_project(&s, "brain", true);
        let n = 12;
        let mut handles = Vec::new();
        for i in 0..n {
            let s = s.clone();
            handles.push(std::thread::spawn(move || {
                let rec = new_record(
                    &s,
                    "wu",
                    "brain",
                    &format!("Unit {i}"),
                    NewOptions {
                        author: "agent".into(),
                        ..NewOptions::default()
                    },
                )?;
                if rec.current_rev != 1 {
                    anyhow::bail!("{} got revision {}", rec.id, rec.current_rev);
                }
                Ok::<(), anyhow::Error>(())
            }));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let rows = list(
            &s,
            Filter {
                kind: "wu".into(),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), n);
    }

    #[test]
    fn concurrent_processes_reserve_new_ids() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        add_project(&s, "brain", true);
        let n = 4;
        let exe = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..n {
            children.push(
                std::process::Command::new(&exe)
                    .args([
                        "--exact",
                        "records::tests::record_writer_process",
                        "--nocapture",
                    ])
                    .env("PULSE_RECORDS_HELPER", "1")
                    .env("PULSE_RECORD_TEST_HOME", &s.home)
                    .spawn()
                    .unwrap(),
            );
        }
        for (i, child) in children.into_iter().enumerate() {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "writer {i}: {}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let rows = list(
            &s,
            Filter {
                kind: "wu".into(),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), n);
        for rec in rows {
            assert_eq!(rec.current_rev, 1, "{} got revised", rec.id);
        }
    }

    #[test]
    fn record_writer_process() {
        let _serial = crate::test_helpers::serial();
        if std::env::var("PULSE_RECORDS_HELPER").ok().as_deref() != Some("1") {
            return;
        }
        let home = std::env::var("PULSE_RECORD_TEST_HOME").unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home,
            ..pulse_store::Options::default()
        })
        .unwrap();
        new_record(
            &s,
            "wu",
            "brain",
            "Concurrent unit",
            NewOptions {
                author: "tester".into(),
                ..NewOptions::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn apply_columns_and_record_kind_validation() {
        let _serial = crate::test_helpers::serial();
        let mut rec = pulse_store::Record {
            kind: "adr".into(),
            slug: "fallback".into(),
            ..pulse_store::Record::default()
        };
        let front = Frontmatter::from([
            ("areas".into(), json!(["a", "b"])),
            ("tags".into(), json!("solo")),
            ("tier".into(), json!(2)),
        ]);
        apply_columns(&mut rec, &front, "# Heading\n\nBody").unwrap();
        assert_eq!(rec.title, "Heading");
        assert_eq!(rec.areas, vec!["a", "b"]);
        assert_eq!(rec.tags, vec!["solo"]);
        assert_eq!(record_kind("ADR").unwrap(), "adr");
        assert_eq!(record_kind("FEAT").unwrap(), "feature");
        assert!(record_kind("no-kind").is_err());
    }
}
