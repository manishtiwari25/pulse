use crate::additions::{addition_available, addition_failure, addition_method, addition_not_found};
use crate::server::{Handler, write_error, write_json};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub(crate) const FOLDER_LIMIT: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FolderEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) checkout: bool,
    pub(crate) registered: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FolderRoot {
    pub(crate) label: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FolderListing {
    pub(crate) path: String,
    pub(crate) parent: String,
    pub(crate) home: String,
    pub(crate) roots: Vec<FolderRoot>,
    pub(crate) checkout: bool,
    pub(crate) registered: String,
    pub(crate) inside: String,
    pub(crate) entries: Vec<FolderEntry>,
    pub(crate) truncated: bool,
}

impl Handler {
    pub(crate) fn addition_folders(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) {
        if parts.len() != 1 {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string()])
            || !addition_available(self, w, r, vec!["projects".to_string()])
        {
            return;
        }
        if !loopback_host(r) {
            write_error(
                w,
                403,
                "Folders are listed only on loopback".to_string(),
                "The request named a host other than this machine.".to_string(),
                "Open the hub on 127.0.0.1.".to_string(),
            );
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let projects = match store.projects(false) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let mut registered = BTreeMap::new();
        let mut roots = Vec::new();
        if !home.is_empty() {
            roots.push(FolderRoot {
                label: "Home".to_string(),
                path: home.clone(),
            });
        }
        let mut seen = BTreeMap::<String, bool>::new();
        let mut start = home.clone();
        for p in projects {
            if p.path.is_empty() || p.slug == pulse_store::COMMONS_SLUG {
                continue;
            }
            let clean = clean_path(&p.path);
            registered.insert(clean.clone(), p.slug);
            let parent = parent_path(&clean);
            if parent == parent_path(&parent) {
                continue;
            }
            if start == home {
                start = parent.clone();
            }
            if parent != home && !seen.contains_key(&parent) {
                seen.insert(parent.clone(), true);
                roots.push(FolderRoot {
                    label: Path::new(&parent)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string(),
                    path: parent,
                });
            }
        }

        let mut path = r.query("path").trim().to_string();
        if path.is_empty() {
            path = start;
        } else if path == "~" || path.starts_with("~/") {
            path = format!("{}{}", home, &path[1..]);
        }
        if !Path::new(&path).is_absolute() {
            write_error(
                w,
                400,
                "Not an absolute path".to_string(),
                "Folders are listed from an absolute path.".to_string(),
                "Start the path with / or ~.".to_string(),
            );
            return;
        }
        path = clean_path(&path);
        let st = match fs::metadata(&path) {
            Ok(st) => st,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                write_error(
                    w,
                    404,
                    format!("No folder {path}"),
                    "Nothing exists at that path on this machine.".to_string(),
                    "Pick a folder that exists or go up one level.".to_string(),
                );
                return;
            }
            Err(e) => {
                addition_failure(
                    w,
                    pulse_cli::errorf(
                        2,
                        &format!("Cannot read folder {path}"),
                        &e.to_string(),
                        "Pick a folder this hub may read.",
                    ),
                );
                return;
            }
        };
        if !st.is_dir() {
            write_error(
                w,
                400,
                format!("Not a folder: {path}"),
                "That path is a file.".to_string(),
                "Pick the folder that contains it.".to_string(),
            );
            return;
        }
        let dirents = match fs::read_dir(&path) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(
                    w,
                    pulse_cli::errorf(
                        2,
                        &format!("Cannot read folder {path}"),
                        &e.to_string(),
                        "Pick a folder this hub may read.",
                    ),
                );
                return;
            }
        };
        let mut listing = FolderListing {
            parent: {
                let p = parent_path(&path);
                if p == path { String::new() } else { p }
            },
            home,
            roots,
            checkout: is_checkout(&path),
            registered: registered.get(&path).cloned().unwrap_or_default(),
            inside: inside_project(&registered, &path),
            entries: Vec::new(),
            truncated: false,
            path: path.clone(),
        };
        for entry in dirents.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let full = Path::new(&path).join(&name);
            let file_type = match entry.file_type() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                if !file_type.is_symlink() {
                    continue;
                }
                if fs::metadata(&full).map(|st| !st.is_dir()).unwrap_or(true) {
                    continue;
                }
            }
            let full_text = full.to_string_lossy().to_string();
            listing.entries.push(FolderEntry {
                name,
                path: full_text.clone(),
                checkout: is_checkout(&full_text),
                registered: registered.get(&full_text).cloned().unwrap_or_default(),
            });
        }
        listing.entries.sort_by(|a, b| {
            let al = a.name.to_lowercase();
            let bl = b.name.to_lowercase();
            al.cmp(&bl).then_with(|| a.name.cmp(&b.name))
        });
        if listing.entries.len() > FOLDER_LIMIT {
            listing.entries.truncate(FOLDER_LIMIT);
            listing.truncated = true;
        }
        write_json(w, 200, value_from(&listing));
    }
}

pub(crate) fn loopback_host(r: &crate::httpx::Request) -> bool {
    let mut host = r.host();
    if let Some(stripped) = host.strip_prefix('[')
        && let Some(end) = stripped.find(']')
    {
        host = stripped[..end].to_string();
    } else if let Some((h, p)) = host.rsplit_once(':')
        && p.chars().all(|c| c.is_ascii_digit())
        && !h.contains(':')
    {
        host = h.to_string();
    }
    let host = host.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

pub(crate) fn inside_project(registered: &BTreeMap<String, String>, path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let mut inside = String::new();
    let mut depth = 0usize;
    for (root, slug) in registered {
        let prefix = format!("{root}{sep}");
        if root != path && path.starts_with(&prefix) && root.len() > depth {
            inside = slug.clone();
            depth = root.len();
        }
    }
    inside
}

pub(crate) fn is_checkout(dir: &str) -> bool {
    fs::symlink_metadata(Path::new(dir).join(".git")).is_ok()
}

fn clean_path(path: &str) -> String {
    let mut out = PathBuf::new();
    for c in Path::new(path).components() {
        out.push(c.as_os_str());
    }
    out.to_string_lossy().to_string()
}

fn parent_path(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn value_from<T: Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize folder listing");
    serde_json::from_str(&text).expect("folder listing JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http as http;
    use serde_json::Value;
    use std::fs;
    use std::process::Command;
    use std::sync::Arc;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn folders_request(
        handler: Handler,
        method: &str,
        path: &str,
        host: &str,
    ) -> (u16, serde_json::Map<String, Value>) {
        let h = Arc::new(handler);
        let server =
            crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
        let client = crate::test_helpers::client();
        let url = format!("{}{}", server.url, path);
        let req = if method == "POST" {
            client.post(&url).header("Content-Type", "application/json")
        } else {
            client.get(&url)
        }
        .header("Host", host);
        let response = req.send().unwrap();
        let status = response.status().as_u16();
        let text = response.text().unwrap();
        let result = serde_json::from_str::<Value>(&text).unwrap_or_else(|err| {
            panic!("{method} {path}: status {status}, non-JSON response {text:?}: {err}")
        });
        (status, result.as_object().unwrap().clone())
    }

    fn folder_names(result: &serde_json::Map<String, Value>) -> Vec<String> {
        result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn folders_listing() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let repo = repo();
        pulse_board::add_project(
            &s,
            repo.path().to_str().unwrap(),
            "My App",
            "",
            false,
            "",
            "human",
            "my-app",
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        for d in ["Zeta", "alpha", ".hidden", "beta/.git"] {
            fs::create_dir_all(root.path().join(d)).unwrap();
        }
        fs::write(root.path().join("notes.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(repo.path(), root.path().join("linked")).unwrap();
        let root_text = root.path().to_string_lossy().to_string();
        let (status, result) = folders_request(
            http::handler(&s),
            "GET",
            &format!(
                "/api/v1/folders?path={}",
                url::form_urlencoded::byte_serialize(root_text.as_bytes()).collect::<String>()
            ),
            "127.0.0.1:8765",
        );
        assert_eq!(status, 200);
        assert_eq!(
            folder_names(&result),
            vec!["alpha", "beta", "linked", "Zeta"]
        );
        assert_eq!(result["path"], root_text);
        assert_eq!(result["checkout"], false);
        assert_eq!(result["registered"], "");
        assert_eq!(result["inside"], "");
        for entry in result["entries"].as_array().unwrap() {
            match entry["name"].as_str().unwrap() {
                "beta" => {
                    assert_eq!(entry["checkout"], true);
                    assert_eq!(entry["registered"], "");
                }
                "linked" => {
                    assert_eq!(entry["checkout"], true);
                    assert_eq!(entry["registered"], "");
                }
                "alpha" | "Zeta" => assert_eq!(entry["checkout"], false),
                _ => {}
            }
        }
        let repo_text: String = s
            .conn()
            .unwrap()
            .query_row("SELECT path FROM projects WHERE slug = 'my-app'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let (status, result) = folders_request(
            http::handler(&s),
            "GET",
            &format!(
                "/api/v1/folders?path={}",
                url::form_urlencoded::byte_serialize(repo_text.as_bytes()).collect::<String>()
            ),
            "127.0.0.1:8765",
        );
        assert_eq!(status, 200);
        assert_eq!(result["checkout"], true);
        assert_eq!(result["registered"], "my-app");
        assert_eq!(result["inside"], "");
        let inside_path = std::path::Path::new(&repo_text).join("docs/private");
        fs::create_dir_all(&inside_path).unwrap();
        let inside = inside_path.to_string_lossy().to_string();
        let (status, result) = folders_request(
            http::handler(&s),
            "GET",
            &format!(
                "/api/v1/folders?path={}",
                url::form_urlencoded::byte_serialize(inside.as_bytes()).collect::<String>()
            ),
            "127.0.0.1:8765",
        );
        assert_eq!(status, 200);
        assert_eq!(result["inside"], "my-app");
    }

    #[test]
    fn folders_refusals() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file.txt"), b"x").unwrap();
        for (path, code, what) in [
            (
                format!("?path={}", root.path().join("missing").to_string_lossy()),
                404,
                format!(
                    "No folder {}",
                    root.path().join("missing").to_string_lossy()
                ),
            ),
            (
                format!("?path={}", root.path().join("file.txt").to_string_lossy()),
                400,
                format!(
                    "Not a folder: {}",
                    root.path().join("file.txt").to_string_lossy()
                ),
            ),
            (
                "?path=relative/dir".to_string(),
                400,
                "Not an absolute path".to_string(),
            ),
        ] {
            let (status, result) = folders_request(
                http::handler(&s),
                "GET",
                &format!("/api/v1/folders{path}"),
                "127.0.0.1:8765",
            );
            assert_eq!(status, code, "{path}");
            assert_eq!(result["error"], what);
        }
        let (status, _) = folders_request(
            http::handler(&s),
            "POST",
            "/api/v1/folders",
            "127.0.0.1:8765",
        );
        assert_eq!(status, 405);
        let (status, _) = folders_request(
            http::handler(&s),
            "GET",
            "/api/v1/folders/x",
            "127.0.0.1:8765",
        );
        assert_eq!(status, 404);
        let root_q = root.path().to_string_lossy();
        let (status, _) = folders_request(
            http::handler(&s),
            "GET",
            &format!("/api/v1/folders?path={root_q}"),
            "evil.example:8765",
        );
        assert_eq!(status, 403);
        for host in [
            "127.0.0.1:8765",
            "localhost:8765",
            "[::1]:8765",
            "LOCALHOST",
        ] {
            let (status, _) = folders_request(
                http::handler(&s),
                "GET",
                &format!("/api/v1/folders?path={root_q}"),
                host,
            );
            assert_eq!(status, 200, "{host}");
        }
    }
}
