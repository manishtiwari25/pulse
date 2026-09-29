use crate::board::*;
use crate::projects::{is_inside, real_path};
use anyhow::Result;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub use crate::projects::import_registry;

pub fn export_store(
    s: &pulse_store::Store,
    sql_path: &str,
    db_path: &str,
    force: bool,
) -> Result<String> {
    let raw = if !sql_path.is_empty() {
        sql_path
    } else {
        db_path
    };
    if raw.is_empty() {
        return Err(refuse(2, "export --sql PATH or --db PATH"));
    }
    let mut target = PathBuf::from(raw);
    if raw.starts_with('~')
        && let Some(home) = home::home_dir()
    {
        target = home.join(raw.trim_start_matches('~').trim_start_matches('/'));
    }
    if target.is_relative() {
        target = std::env::current_dir()?.join(target);
    }
    for p in s.projects(false)? {
        if is_inside(&real_path(&target), &real_path(&p.path)) {
            return Err(refuse(
                2,
                format!(
                    "Refusing to export into the registered project {} ({}); pick a path outside every checkout",
                    p.slug, p.path
                ),
            ));
        }
    }
    if target.exists() {
        if !force {
            return Err(refuse(
                2,
                format!("{} exists; pass --force to overwrite", target.display()),
            ));
        }
        fs::remove_file(&target)?;
    }
    fs::create_dir_all(target.parent().unwrap_or_else(|| Path::new(".")))?;
    if !sql_path.is_empty() {
        dump_sql(s, &target)?;
    } else {
        s.conn()?
            .execute("VACUUM INTO ?", [target.display().to_string()])?;
    }
    Ok(target.display().to_string())
}

fn dump_sql(s: &pulse_store::Store, target: &Path) -> Result<()> {
    let conn = s.conn()?;
    let mut w = BufWriter::new(fs::File::create(target)?);
    writeln!(w, "BEGIN TRANSACTION;")?;
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'trigger' THEN 2 ELSE 3 END, name")?;
    let objs = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (typ, name, sql) in &objs {
        if typ == "table" {
            writeln!(w, "{};", sql.trim_end_matches(';'))?;
            dump_rows(&conn, &mut w, name)?;
        }
    }
    for (typ, _, sql) in &objs {
        if typ != "table" {
            writeln!(w, "{};", sql.trim_end_matches(';'))?;
        }
    }
    writeln!(w, "COMMIT;")?;
    Ok(())
}
fn dump_rows(conn: &rusqlite::Connection, w: &mut dyn Write, table: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!(r#"SELECT * FROM "{table}""#))?;
    let cols = stmt.column_count();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut lits = Vec::new();
        for i in 0..cols {
            let v: rusqlite::types::Value = row.get(i)?;
            lits.push(sql_literal(v));
        }
        writeln!(w, r#"INSERT INTO "{table}" VALUES({});"#, lits.join(","))?;
    }
    Ok(())
}
fn sql_literal(v: rusqlite::types::Value) -> String {
    match v {
        rusqlite::types::Value::Null => "NULL".into(),
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(f) => f.to_string(),
        rusqlite::types::Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        rusqlite::types::Value::Blob(b) => format!("X'{}'", hex::encode_upper(b)),
    }
}
#[cfg(test)]
mod tests {
    use crate::test_helpers::{count_rows, fixture, git_repo, new_home, what_any};

    #[test]
    fn literal_quotes() {
        assert_eq!(
            super::sql_literal(rusqlite::types::Value::Text("a'b".into())),
            "'a''b'"
        );
    }

    #[test]
    fn import_registry() {
        let (_home_dir, _, s) = new_home();
        let repo = git_repo();
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("projects.json");
        std::fs::write(
            &reg,
            format!(
                r#"{{"projects": [{{"slug": "one", "path": "{}", "brain": true}}, {{"path": "/nope/missing"}}], "commons": "/tmp/commons"}}"#,
                repo.path().display()
            ),
        )
        .unwrap();
        let got =
            crate::projects::import_registry(&s, &reg.display().to_string(), "migrate").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "one");
        assert!(got[0].created);
        let (v, ok) = s.meta("commons_path").unwrap();
        assert!(ok);
        assert_eq!(v, "/tmp/commons");
        assert_eq!(s.brain().unwrap().unwrap().slug, "one");
        let missing = dir.path().join("none.json");
        assert!(
            crate::projects::import_registry(&s, &missing.display().to_string(), "migrate")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn export_store() {
        let (_home_dir, _, s, p, _repo) = fixture();
        let err = super::export_store(&s, "", "", false).unwrap_err();
        assert_eq!(what_any(&err), "export --sql PATH or --db PATH");
        let inside = std::path::Path::new(&p.path).join("dump.sql");
        let err = super::export_store(&s, &inside.display().to_string(), "", false).unwrap_err();
        assert!(what_any(&err).starts_with("Refusing to export into the registered project demo"));
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("copy.sqlite");
        let got = super::export_store(&s, "", &db.display().to_string(), false).unwrap();
        assert_eq!(got, db.display().to_string());
        let err = super::export_store(&s, "", &db.display().to_string(), false).unwrap_err();
        assert_eq!(
            what_any(&err),
            format!("{} exists; pass --force to overwrite", db.display())
        );
        super::export_store(&s, "", &db.display().to_string(), true).unwrap();
        let copy_dir = dir.path().join("home");
        std::fs::create_dir_all(&copy_dir).unwrap();
        std::fs::copy(&db, copy_dir.join(pulse_store::STORE_NAME)).unwrap();
        let c = pulse_store::Store::open(pulse_store::Options {
            home: copy_dir.display().to_string(),
            skip_records: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(count_rows(&c, "agents", "", &[]), 3);
        let sql_path = dir.path().join("sub").join("dump.sql");
        super::export_store(&s, &sql_path.display().to_string(), "", false).unwrap();
        let text = std::fs::read_to_string(sql_path).unwrap();
        assert!(text.starts_with("BEGIN TRANSACTION;\n"));
        assert!(text.ends_with("COMMIT;\n"));
        assert!(text.contains("CREATE TABLE agents"));
        assert!(text.contains(r#"INSERT INTO "agents" VALUES('alice'"#));
        assert!(text.contains("CREATE TRIGGER posts_no_update"));
    }
}
