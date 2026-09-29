//! Port of internal/evolve/readonly.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, FileType, Metadata};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TreeEntry {
    pub(crate) name: String,
    pub(crate) mode: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct TreeRoot {
    root: PathBuf,
}

impl TreeRoot {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            root: path.as_ref().to_path_buf(),
        })
    }

    fn join(&self, name: &str) -> PathBuf {
        if name == "." {
            self.root.clone()
        } else {
            self.root.join(name)
        }
    }
}

fn walk_names(root: &Path) -> Result<Vec<String>> {
    fn rec(root: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
        out.push(if rel.is_empty() {
            ".".to_string()
        } else {
            rel.to_string()
        });
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let meta = fs::symlink_metadata(&dir)?;
        if !meta.is_dir() {
            return Ok(());
        }
        let mut entries = fs::read_dir(&dir)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().to_string()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for name in entries {
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            rec(root, &child, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    rec(root, "", &mut out)?;
    Ok(out)
}

fn escaping_symlink(_root: &TreeRoot, name: &str, target: &Path) -> bool {
    if target.is_absolute() || target.to_string_lossy().contains('\\') {
        return true;
    }
    let base = if name == "." {
        Path::new("")
    } else {
        Path::new(name).parent().unwrap_or(Path::new(""))
    };
    let joined = base.join(target);
    let mut depth = 0i64;
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return true,
        }
    }
    false
}

fn violation(reason: &str) -> anyhow::Error {
    anyhow!(crate::clones::ViolationError {
        reason: reason.to_string()
    })
}

fn mode(meta: &Metadata) -> u32 {
    meta.permissions().mode()
}

fn file_type(meta: &Metadata) -> FileType {
    meta.file_type()
}

/// Go: func freezeTree(root *os.Root) (func() error, error) {
pub(crate) fn freeze_tree(
    root: &TreeRoot,
) -> Result<Box<dyn FnOnce() -> Result<()> + Send + 'static>> {
    let mut entries = Vec::new();
    for name in walk_names(&root.root)? {
        let path = root.join(&name);
        let info = fs::symlink_metadata(&path)?;
        let ft = file_type(&info);
        if ft.is_symlink() {
            let target = fs::read_link(&path)
                .map_err(|_| violation("scored tree has an escaping symlink"))?;
            if escaping_symlink(root, &name, &target) {
                return Err(violation("scored tree has an escaping symlink"));
            }
            if let Err(e) = fs::metadata(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(violation("scored tree has an unresolvable symlink"));
            }
            continue;
        }
        if !info.is_dir() && (!info.is_file() || crate::process::multiple_links(&info)) {
            return Err(violation(
                "scored tree has a special file or shared hardlink",
            ));
        }
        entries.push(TreeEntry {
            name,
            mode: mode(&info),
        });
    }
    let restore_root = root.root.clone();
    let restore_entries = entries.clone();
    let restore = move || {
        let mut first: Option<anyhow::Error> = None;
        for entry in restore_entries {
            let path = if entry.name == "." {
                restore_root.clone()
            } else {
                restore_root.join(&entry.name)
            };
            let Ok(info) = fs::symlink_metadata(&path) else {
                continue;
            };
            if info.file_type().is_symlink() || crate::process::multiple_links(&info) {
                continue;
            }
            if let Err(e) =
                fs::set_permissions(&path, fs::Permissions::from_mode(entry.mode & 0o777))
            {
                first.get_or_insert_with(|| e.into());
            }
        }
        if let Some(e) = first { Err(e) } else { Ok(()) }
    };
    for entry in entries.iter().rev() {
        let path = root.join(&entry.name);
        if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(entry.mode & !0o222))
        {
            let _ = restore();
            return Err(e.into());
        }
    }
    Ok(Box::new(restore))
}

/// Go: func treeFingerprint(root *os.Root) (string, error) {
pub(crate) fn tree_fingerprint(root: &TreeRoot) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    for name in walk_names(&root.root)? {
        let path = root.join(&name);
        let info = fs::symlink_metadata(&path)?;
        name.hash(&mut hasher);
        mode(&info).hash(&mut hasher);
        if info.is_file() {
            if crate::process::multiple_links(&info) {
                return Err(violation("scorer created a shared hardlink"));
            }
            let mut f = fs::File::open(path)?;
            let mut buf = [0u8; 8192];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                buf[..n].hash(&mut hasher);
            }
        } else if info.file_type().is_symlink() {
            fs::read_link(path)?.to_string_lossy().hash(&mut hasher);
        } else if !info.is_dir() {
            return Err(violation("scored tree contains a special file"));
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}
