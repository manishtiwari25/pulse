use crate::pyjson::py_json;
use crate::store::{InputError, Store};
use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Record {
    pub id: String,
    pub project: String,
    pub kind: String,
    pub number: i64,
    pub slug: String,
    pub title: String,
    pub status: String,
    pub tier: i64,
    pub areas: Vec<String>,
    pub tags: Vec<String>,
    pub delivery: String,
    pub delivered: String,
    pub date: String,
    pub path: String,
    pub created_at: String,
    pub created_by: String,
    pub updated_at: String,
    pub current_rev: i64,
    pub origin: String,
}
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Revision {
    pub record_id: String,
    pub rev: i64,
    pub frontmatter: BTreeMap<String, Value>,
    pub body: String,
    pub author: String,
    pub at: String,
    pub note: String,
    #[serde(skip)]
    pub raw_front: String,
}
pub const RECORD_KINDS: &[&str] = &[
    "adr",
    "plan",
    "wu",
    "feature",
    "memory",
    "workflow",
    "skill",
    "guide",
    "context",
    "architecture",
    "evolution",
    "instruction",
    "template",
    "usage",
    "prompt",
];
pub fn record_prefix() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("adr", "ADR"),
        ("plan", "PLAN"),
        ("wu", "WU"),
        ("feature", "FEAT"),
        ("memory", "MEM"),
        ("workflow", "WF"),
        ("skill", "SKILL"),
        ("guide", "GUIDE"),
        ("context", "CTX"),
        ("architecture", "ARCH"),
        ("evolution", "EVO"),
        ("instruction", "INSTR"),
        ("template", "TPL"),
        ("usage", "USAGE"),
        ("prompt", "PROMPT"),
    ])
}
#[derive(Debug, thiserror::Error)]
#[error("no such record")]
pub struct ErrNoRecord;
const RECORD_COLS: &str = "id, project, kind, COALESCE(number, 0), slug, title, status, COALESCE(tier, 0), areas, tags, delivery, delivered, date, path, created_at, created_by, updated_at, current_rev, origin";
fn json_list(text: String) -> Vec<String> {
    serde_json::from_str(&text).unwrap_or_default()
}
fn list_json(items: &[String]) -> String {
    crate::gojson::to_string(items).unwrap_or_else(|_| "[]".into())
}
fn scan_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: r.get(0)?,
        project: r.get(1)?,
        kind: r.get(2)?,
        number: r.get(3)?,
        slug: r.get(4)?,
        title: r.get(5)?,
        status: r.get(6)?,
        tier: r.get(7)?,
        areas: json_list(r.get(8)?),
        tags: json_list(r.get(9)?),
        delivery: r.get(10)?,
        delivered: r.get(11)?,
        date: r.get(12)?,
        path: r.get(13)?,
        created_at: r.get(14)?,
        created_by: r.get(15)?,
        updated_at: r.get(16)?,
        current_rev: r.get(17)?,
        origin: r.get(18)?,
    })
}
#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    pub project: String,
    pub kind: String,
    pub status: String,
    pub area: String,
    pub slug: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecordDraft {
    pub record_id: String,
    pub base_rev: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub author: String,
    pub at: String,
    pub note: String,
    pub job_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
}
const DRAFT_COLS: &str =
    "d.record_id, d.base_rev, d.body, d.author, d.at, d.note, d.job_id, r.project, r.kind, r.title";
fn scan_draft(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecordDraft> {
    Ok(RecordDraft {
        record_id: r.get(0)?,
        base_rev: r.get(1)?,
        body: r.get(2)?,
        author: r.get(3)?,
        at: r.get(4)?,
        note: r.get(5)?,
        job_id: r.get(6)?,
        project: r.get(7)?,
        kind: r.get(8)?,
        title: r.get(9)?,
    })
}
fn null_int(n: i64) -> SqlValue {
    if n == 0 {
        SqlValue::Null
    } else {
        SqlValue::Integer(n)
    }
}

impl Store {
    pub fn get_record(&self, id: &str, rev: i64) -> Result<(Record, Option<Revision>)> {
        let conn = self.conn()?;
        let r = conn
            .query_row(
                &format!("SELECT {RECORD_COLS} FROM records WHERE id = ?"),
                [id],
                scan_record,
            )
            .optional()?
            .ok_or_else(|| anyhow!(ErrNoRecord))?;
        let want = if rev <= 0 { r.current_rev } else { rev };
        let row: Option<(String, String, String, String, String)> = conn
            .query_row(
                "SELECT frontmatter, body, author, at, note FROM record_revisions WHERE record_id = ? AND rev = ?",
                params![id, want],
                |rr| Ok((rr.get(0)?, rr.get(1)?, rr.get(2)?, rr.get(3)?, rr.get(4)?)),
            )
            .optional()?;
        let Some((raw, body, author, at, note)) = row else {
            return Ok((r, None));
        };
        let frontmatter = serde_json::from_str(&raw).unwrap_or_default();
        Ok((
            r,
            Some(Revision {
                record_id: id.into(),
                rev: want,
                frontmatter,
                body,
                author,
                at,
                note,
                raw_front: raw,
            }),
        ))
    }
    pub fn revisions(&self, id: &str) -> Result<Vec<Revision>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT rev, author, at, note FROM record_revisions WHERE record_id = ? ORDER BY rev",
        )?;
        Ok(stmt
            .query_map([id], |r| {
                Ok(Revision {
                    record_id: id.into(),
                    rev: r.get(0)?,
                    author: r.get(1)?,
                    at: r.get(2)?,
                    note: r.get(3)?,
                    ..Revision::default()
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn list_records(&self, f: RecordFilter) -> Result<Vec<Record>> {
        let conn = self.conn()?;
        let mut q = format!("SELECT {RECORD_COLS} FROM records WHERE 1=1");
        let mut vals: Vec<String> = Vec::new();
        if !f.project.is_empty() {
            q.push_str(" AND project = ?");
            vals.push(f.project);
        }
        if !f.kind.is_empty() {
            q.push_str(" AND kind = ?");
            vals.push(f.kind);
        }
        if !f.status.is_empty() {
            q.push_str(" AND status = ? COLLATE NOCASE");
            vals.push(f.status);
        }
        if !f.slug.is_empty() {
            q.push_str(" AND slug = ?");
            vals.push(f.slug);
        }
        q.push_str(" ORDER BY kind, number, slug");
        let mut stmt = conn.prepare(&q)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            vals.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), scan_record)?;
        let mut out = Vec::new();
        for row in rows {
            let r = row?;
            if f.area.is_empty() || r.areas.iter().any(|a| a.eq_ignore_ascii_case(&f.area)) {
                out.push(r)
            }
        }
        Ok(out)
    }
    pub fn next_record_number(&self, project: &str, kind: &str) -> Result<i64> {
        let conn = self.conn()?;
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM records WHERE project = ? AND kind = ?",
            params![project, kind],
            |r| r.get(0),
        )?)
    }
    pub fn put_record(
        &self,
        mut r: Record,
        frontmatter: BTreeMap<String, Value>,
        body: &str,
        author: &str,
        note: &str,
    ) -> Result<i64> {
        if r.id.is_empty() || r.kind.is_empty() || r.project.is_empty() {
            return Err(InputError {
                msg: "a record needs an id, a kind and a project".into(),
            }
            .into());
        }
        if !record_prefix().contains_key(r.kind.as_str()) {
            return Err(InputError {
                msg: format!(
                    "unknown record kind {:?} (one of {})",
                    r.kind,
                    RECORD_KINDS.join(", ")
                ),
            }
            .into());
        }
        let front = crate::gojson::to_string(&frontmatter)?;
        if r.slug.is_empty() {
            r.slug = r.id.to_ascii_lowercase();
        }
        if r.origin.is_empty() {
            r.origin = "store".into();
        }
        let now = self.now();
        let tx = self.begin()?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT current_rev FROM records WHERE id = ?",
                [&r.id],
                |rr| rr.get(0),
            )
            .optional()?;
        let (rev, action) = if let Some(cur) = current {
            tx.execute("UPDATE records SET title = ?, status = ?, tier = ?, areas = ?, tags = ?, delivery = ?, delivered = ?, date = ?, path = ?, updated_at = ?, current_rev = ? WHERE id = ?",params![r.title,r.status,null_int(r.tier),list_json(&r.areas),list_json(&r.tags),r.delivery,r.delivered,r.date,r.path,now,cur+1,r.id])?;
            (cur + 1, "record.revised")
        } else {
            if r.created_at.is_empty() {
                r.created_at = now.clone();
            }
            if r.created_by.is_empty() {
                r.created_by = author.into();
            }
            tx.execute("INSERT INTO records (id, project, kind, number, slug, title, status, tier, areas, tags, delivery, delivered, date, path, created_at, created_by, updated_at, current_rev, origin) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",params![r.id,r.project,r.kind,null_int(r.number),r.slug,r.title,r.status,null_int(r.tier),list_json(&r.areas),list_json(&r.tags),r.delivery,r.delivered,r.date,r.path,r.created_at,r.created_by,now,r.origin])?;
            (1, "record.created")
        };
        tx.execute("INSERT INTO record_revisions (record_id, rev, frontmatter, body, author, at, note) VALUES (?, ?, ?, ?, ?, ?, ?)",params![r.id,rev,front,body,author,now,note])?;
        let detail =
            py_json(serde_json::json!({"kind":r.kind,"project":r.project,"rev":rev,"note":note}))?;
        tx.event(author, action, &r.id, detail)?;
        tx.commit()?;
        Ok(rev)
    }
    pub fn set_record_status(&self, id: &str, status: &str, actor: &str) -> Result<()> {
        let tx = self.begin()?;
        let n = tx.execute(
            "UPDATE records SET status = ?, updated_at = ? WHERE id = ?",
            params![status, self.now(), id],
        )?;
        if n == 0 {
            return Err(anyhow!(ErrNoRecord));
        }
        tx.event(actor, "record.status", id, status)?;
        tx.commit()
    }
    pub fn get_draft(&self, id: &str) -> Result<Option<RecordDraft>> {
        let conn = self.conn()?;
        Ok(conn.query_row(&format!("SELECT {DRAFT_COLS} FROM record_drafts d JOIN records r ON r.id = d.record_id WHERE d.record_id = ?"),[id],scan_draft).optional()?)
    }
    pub fn list_drafts(&self, project: &str) -> Result<Vec<RecordDraft>> {
        let conn = self.conn()?;
        if project.is_empty() {
            let mut stmt=conn.prepare(&format!("SELECT {DRAFT_COLS} FROM record_drafts d JOIN records r ON r.id = d.record_id ORDER BY d.at, d.record_id"))?;
            Ok(stmt
                .query_map([], scan_draft)?
                .collect::<rusqlite::Result<_>>()?)
        } else {
            let mut stmt=conn.prepare(&format!("SELECT {DRAFT_COLS} FROM record_drafts d JOIN records r ON r.id = d.record_id WHERE r.project = ? ORDER BY d.at, d.record_id"))?;
            Ok(stmt
                .query_map([project], scan_draft)?
                .collect::<rusqlite::Result<_>>()?)
        }
    }
    pub fn put_draft(
        &self,
        id: &str,
        body: &str,
        author: &str,
        note: &str,
        job_id: &str,
    ) -> Result<Option<RecordDraft>> {
        let tx = self.begin()?;
        let base: Option<i64> = tx
            .query_row("SELECT current_rev FROM records WHERE id = ?", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(base) = base else {
            return Err(anyhow!(ErrNoRecord));
        };
        tx.execute("INSERT INTO record_drafts (record_id, base_rev, body, author, at, note, job_id) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(record_id) DO UPDATE SET base_rev = excluded.base_rev, body = excluded.body, author = excluded.author, at = excluded.at, note = excluded.note, job_id = excluded.job_id",params![id,base,body,author,self.now(),note,job_id])?;
        tx.event(
            author,
            "record.draft",
            id,
            py_json(serde_json::json!({"base_rev":base,"job":job_id,"bytes":body.len()}))?,
        )?;
        tx.commit()?;
        self.get_draft(id)
    }
    pub fn delete_draft<D: Into<crate::store::Detail>>(
        &self,
        id: &str,
        actor: &str,
        action: &str,
        detail: D,
    ) -> Result<bool> {
        let tx = self.begin()?;
        let n = tx.execute("DELETE FROM record_drafts WHERE record_id = ?", [id])?;
        if n == 0 {
            return Ok(false);
        }
        tx.event(actor, action, id, detail)?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Store};

    #[test]
    fn put_get_record() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(s.next_record_number("proj", "adr").unwrap(), 1);
        let rec = Record {
            id: "ADR-001".into(),
            project: "proj".into(),
            kind: "adr".into(),
            number: 1,
            slug: "first".into(),
            title: "First".into(),
            status: "Proposed".into(),
            areas: vec!["runner".into()],
            ..Record::default()
        };
        assert_eq!(
            s.put_record(
                rec.clone(),
                BTreeMap::from([("title".into(), Value::String("First".into()))]),
                "# First\n",
                "tester",
                ""
            )
            .unwrap(),
            1
        );
        let mut rec2 = rec;
        rec2.status = "Accepted".into();
        assert_eq!(
            s.put_record(
                rec2,
                BTreeMap::new(),
                "# First\n\nMore.\n",
                "tester",
                "amended"
            )
            .unwrap(),
            2
        );
        let (head, rev) = s.get_record("ADR-001", 0).unwrap();
        let rev = rev.unwrap();
        assert_eq!(head.current_rev, 2);
        assert_eq!(rev.body, "# First\n\nMore.\n");
        assert_eq!(s.revisions("ADR-001").unwrap().len(), 2);
        assert_eq!(
            s.list_records(RecordFilter {
                project: "proj".into(),
                kind: "adr".into(),
                area: "runner".into(),
                ..RecordFilter::default()
            })
            .unwrap()
            .len(),
            1
        );
        s.set_record_status("ADR-001", "Retired", "human").unwrap();
    }
}

#[cfg(test)]
mod go_record_tests {
    use crate::store::{has_column, init_schema};
    use crate::{
        COMPUTE_PROVIDERS, COMPUTE_RULES, RUNNER_COLUMNS, STORE_NAME, compute_size_setting,
        secret_shaped, validate_setting,
    };
    use crate::{Options, Store};
    use rusqlite::Connection;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn compute_settings() {
        assert!(validate_setting("compute.max_vms", "-1").is_err());
        assert_eq!(validate_setting("compute.max_vms", "3").unwrap(), "3");
        assert!(validate_setting("compute.azure.subscription", "ghp_abcdefghijklmnop").is_err());
        assert_eq!(
            validate_setting("compute.azure.location", "westeurope").unwrap(),
            "westeurope"
        );
        assert!(validate_setting("compute.provider", "moon").is_err());
        for p in COMPUTE_PROVIDERS {
            assert_eq!(validate_setting("compute.provider", p).unwrap(), *p);
            assert!(!compute_size_setting(p).is_empty());
            assert!(!COMPUTE_RULES[&compute_size_setting(p)].kind.is_empty());
        }
        assert_eq!(compute_size_setting("moon"), "compute.azure.vm_size");
        assert_eq!(compute_size_setting("gcp"), "compute.gcp.machine_type");
        assert!(
            validate_setting(
                "compute.aws.image",
                "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
            )
            .unwrap()
            .starts_with("/aws/")
        );
        assert!(validate_setting("compute.gcp.project", "AKIAIOSFODNN7EXAMPLE1234").is_err());
        assert!(secret_shaped("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(!secret_shaped(
            "/usr/local/bin:/opt/homebrew/bin/some/very/long/path/that/keeps/going/on"
        ));
        let _guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        crate::create(home.path().join(STORE_NAME)).unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(s.setting("compute.provider", ""), "azure");
        assert_eq!(s.setting_int("compute.max_vms"), 0);
        s.set_setting("human", "compute.auto_approve", "1").unwrap();
        assert_eq!(s.setting("compute.auto_approve", ""), "1");
        assert_eq!(s.setting("evolve.limits", ""), "none");
        assert_eq!(s.setting("evolve.tolerance", ""), "0.02");
        assert!(validate_setting("evolve.tolerance", "1.5").is_err());
        assert_eq!(s.setting("compute.self_heal", ""), "1");
        assert_eq!(s.setting("compute.self_retire", ""), "1");
        assert_eq!(s.setting_int("compute.retire_grace_hours"), 24);
        for name in ["compute.self_heal", "compute.self_retire"] {
            assert_eq!(validate_setting(name, "0").unwrap(), "0");
            assert!(validate_setting(name, "yes").is_err());
        }
        assert!(validate_setting("compute.retire_grace_hours", "0").is_err());
        assert_eq!(
            validate_setting("compute.retire_grace_hours", "48").unwrap(),
            "48"
        );
    }

    #[test]
    fn additive_columns() {
        let _guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let db = Connection::open(home.path().join(STORE_NAME)).unwrap();
        crate::store::configure_conn(&db, false).unwrap();
        init_schema(&db, "2026-09-08T00:00:00Z").unwrap();
        assert!(!has_column(&db, "executors", "admin").unwrap());
        drop(db);
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        for c in RUNNER_COLUMNS {
            assert!(
                has_column(&s.conn().unwrap(), c.table, c.name).unwrap(),
                "{}.{}",
                c.table,
                c.name
            );
        }
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO executors (name, created_at) VALUES ('x', '2026-09-08T00:00:00Z')",
                [],
            )
            .unwrap();
        let row: (String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT admin, healed_at FROM executors WHERE name = 'x'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("".into(), "".into()));
        s.conn().unwrap().execute("INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after) VALUES ('x', 'azure', 'rg', 'vm', 'a', 'b')", []).unwrap();
        assert!(s.additive_complete());
        s.ensure_records().unwrap();
        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'schema.records'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
