use crate::frontmatter::render_markdown;
use crate::records::{input_error, list};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub kind: String,
    pub project: String,
    pub force: bool,
}

pub fn export(s: &pulse_store::Store, dir: &str, opts: ExportOptions) -> Result<usize> {
    if dir.trim().is_empty() {
        return Err(input_error(
            "Cannot export records",
            "an output directory is required",
            "pulse record export /tmp/pulse-records",
        ));
    }
    let target = resolved_target(dir)?;
    if !opts.force && inside_checkout(&target)? {
        return Err(input_error(
            "Refusing Markdown export inside a git checkout",
            "SQL is the system of record; export outside the repository or explicitly use --force",
            "pulse record export /tmp/pulse-records",
        ));
    }
    let rows = list(
        s,
        pulse_store::RecordFilter {
            project: opts.project.clone(),
            kind: opts.kind.clone(),
            ..pulse_store::RecordFilter::default()
        },
    )?;
    let mut outputs = Vec::<(PathBuf, String)>::new();
    let mut seen = BTreeMap::<PathBuf, String>::new();
    for rec in rows {
        let mut name = if rec.path.is_empty() {
            PathBuf::from(format!("{}/{}.md", rec.kind, rec.id))
        } else {
            PathBuf::from(rec.path.replace('/', std::path::MAIN_SEPARATOR_STR))
        };
        if !is_local(&name) || name.to_string_lossy().contains('\\') {
            return Err(anyhow!(
                "cannot export {}: unsafe record path {:?}",
                rec.id,
                name
            ));
        }
        name = clean_relative(&name);
        if let Some(first) = seen.get(&name) {
            return Err(input_error(
                "Cannot export colliding paths",
                &format!("{} and {} both use {}", first, rec.id, name.display()),
                &format!("pulse record export {dir} --project {}", rec.project),
            ));
        }
        seen.insert(name.clone(), rec.id.clone());
        let (_, rev) = s.get_record(&rec.id, 0)?;
        let rev = rev.expect("current revision exists");
        if !opts.force
            && inside_checkout(target.join(name.parent().unwrap_or_else(|| Path::new(""))))?
        {
            return Err(input_error(
                "Refusing Markdown export inside a git checkout",
                &format!("a nested checkout contains {}", name.display()),
                "pulse record export /tmp/pulse-records",
            ));
        }
        outputs.push((name, render_markdown(&rec, &rev)));
    }
    fs::create_dir_all(&target)?;
    let mut written = 0;
    for (name, text) in outputs {
        ensure_no_symlink_escape(&target, name.parent().unwrap_or_else(|| Path::new("")))?;
        let full = target.join(&name);
        fs::create_dir_all(full.parent().unwrap_or(&target))?;
        ensure_no_symlink_escape(&target, name.parent().unwrap_or_else(|| Path::new("")))?;
        fs::write(&full, text).map_err(|e| anyhow!("export {}: {e}", name.display()))?;
        written += 1;
    }
    Ok(written)
}

fn is_local(path: &Path) -> bool {
    path.is_relative()
        && !path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn clean_relative(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        if let Component::Normal(s) = c {
            out.push(s);
        }
    }
    out
}

pub fn resolved_target(dir: impl AsRef<Path>) -> Result<PathBuf> {
    let abs = if dir.as_ref().is_absolute() {
        dir.as_ref().to_path_buf()
    } else {
        std::env::current_dir()?.join(dir)
    };
    let mut existing = abs.clone();
    let mut tail = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(mut resolved) => {
                for p in tail.iter().rev() {
                    resolved.push(p);
                }
                return Ok(resolved);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = existing.parent() else {
                    return Err(e.into());
                };
                tail.push(existing.file_name().unwrap_or_default().to_os_string());
                existing = parent.to_path_buf();
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn inside_checkout(target: impl AsRef<Path>) -> Result<bool> {
    let resolved = resolved_target(target)?;
    let mut dir = resolved.as_path();
    loop {
        match fs::metadata(dir.join(".git")) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            Err(_) => {}
        }
        let Some(parent) = dir.parent() else {
            return Ok(false);
        };
        if parent == dir {
            return Ok(false);
        }
        dir = parent;
    }
}

fn ensure_no_symlink_escape(root: &Path, rel_dir: &Path) -> Result<()> {
    let mut cur = root.to_path_buf();
    for comp in rel_dir.components() {
        if let Component::Normal(c) = comp {
            cur.push(c);
            if let Ok(meta) = fs::symlink_metadata(&cur)
                && meta.file_type().is_symlink()
            {
                anyhow::bail!("export {}: symlink escapes output root", rel_dir.display());
            }
        }
    }
    Ok(())
}

pub use export as Export;

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
    fn export_checkout_and_path_guards() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
        add_project(&s, "brain", project.path(), true);
        let rec = crate::records::new_record(
            &s,
            "memory",
            "brain",
            "Pattern",
            crate::records::NewOptions {
                body: "Keep me\n".into(),
                body_set: true,
                ..crate::records::NewOptions::default()
            },
        )
        .unwrap();
        let inside = project.path().join("docs");
        assert!(export(&s, inside.to_str().unwrap(), ExportOptions::default()).is_err());
        assert!(!inside.exists());
        assert_eq!(
            export(
                &s,
                inside.to_str().unwrap(),
                ExportOptions {
                    force: true,
                    ..ExportOptions::default()
                }
            )
            .unwrap(),
            1
        );
        let out = tempfile::tempdir().unwrap();
        assert_eq!(
            export(
                &s,
                out.path().to_str().unwrap(),
                ExportOptions {
                    force: true,
                    ..ExportOptions::default()
                }
            )
            .unwrap(),
            1
        );
        assert!(out.path().join(format!("memory/{}.md", rec.id)).exists());
        let mut bad = rec;
        bad.path = "../escape.md".into();
        s.put_record(
            bad,
            [("title".into(), json!("bad"))].into(),
            "No\n",
            "tester",
            "",
        )
        .unwrap();
        assert!(
            export(
                &s,
                tempfile::tempdir().unwrap().path().to_str().unwrap(),
                ExportOptions {
                    force: true,
                    ..ExportOptions::default()
                }
            )
            .is_err()
        );
    }
}
