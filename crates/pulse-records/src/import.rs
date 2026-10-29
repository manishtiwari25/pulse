use crate::frontmatter::parse_frontmatter;
use crate::lock::lock_writes;
use crate::records::{
    Frontmatter, apply_columns, input_error, numbered, scoped_id, slugify, text_value,
};
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub dry_run: bool,
    pub force_rev: bool,
    pub author: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct KindCount {
    pub imported: i64,
    pub skipped: i64,
    pub revised: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub kind: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub project: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    #[serde(skip)]
    pub short_ref: String,
    pub dry_run: bool,
    pub kinds: BTreeMap<String, KindCount>,
    pub imported: i64,
    pub revised: i64,
    pub skipped: Vec<SkippedFile>,
}

const IMPORT_PATHS: &[&str] = &[
    "docs/decisions",
    "docs/plans",
    "docs/features",
    "docs/memory",
    "docs/workflows",
    "docs/skills",
    "docs/guides",
    "docs/context",
    "docs/architecture",
    "docs/evolution",
    "docs/usage",
    "docs/prompts",
    "docs/README.md",
    "docs/versions.md",
    "AGENTS.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    ".pulse/agents",
];

fn folder_kind(folder: &str) -> &'static str {
    match folder {
        "decisions" => "adr",
        "plans" => "plan",
        "features" => "feature",
        "memory" => "memory",
        "workflows" => "workflow",
        "skills" => "skill",
        "guides" => "guide",
        "context" => "context",
        "architecture" => "architecture",
        "evolution" => "evolution",
        "usage" => "usage",
        "prompts" => "prompt",
        _ => "",
    }
}

#[derive(Clone)]
struct ImportItem {
    record: pulse_store::Record,
    front: Frontmatter,
    body: String,
    revise: bool,
}

pub fn import_repo(
    s: &pulse_store::Store,
    repo: &str,
    ref_: &str,
    project: &str,
    opts: ImportOptions,
) -> Result<Summary> {
    let mut summary = Summary {
        dry_run: opts.dry_run,
        ..Summary::default()
    };
    let _guard = if opts.dry_run {
        None
    } else {
        Some(lock_writes(s)?)
    };
    for kind in pulse_store::RECORD_KINDS {
        summary.kinds.insert((*kind).into(), KindCount::default());
    }
    let repo = if repo.is_empty() { "." } else { repo };
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| PathBuf::from(repo));
    let ref_ = if ref_.is_empty() { "HEAD" } else { ref_ };
    let commit = git_read(
        &repo,
        &[
            "log",
            "-1",
            "--format=%H%n%h",
            "--end-of-options",
            &format!("{ref_}^{{commit}}"),
            "--",
        ],
    )?;
    let (full, short) = commit
        .trim()
        .split_once('\n')
        .unwrap_or((commit.trim(), ""));
    summary.ref_ = full.into();
    summary.short_ref = short.into();
    let p = crate::records::resolve_project(s, project, repo.to_str().unwrap_or("."))?;
    summary.project = p.slug.clone();
    let paths: Vec<String> = if opts.paths.is_empty() {
        IMPORT_PATHS.iter().map(|s| (*s).into()).collect()
    } else {
        opts.paths.clone()
    };
    let mut args = vec![
        "ls-tree".to_string(),
        "-r".into(),
        "-z".into(),
        summary.ref_.clone(),
        "--".into(),
    ];
    args.extend(paths);
    let tree = git_read_owned(&repo, &args)?;
    let mut files = Vec::new();
    let mut oids = BTreeMap::new();
    for entry in tree.split('\0') {
        let Some((meta, file)) = entry.split_once('\t') else {
            continue;
        };
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() >= 3 && fields[1] == "blob" {
            files.push(file.to_string());
            oids.insert(file.to_string(), fields[2].to_string());
        }
    }
    let blobs = git_cat_blobs(&repo, &oids)?;
    let existing = s.list_records(pulse_store::RecordFilter::default())?;
    let mut by_id = BTreeMap::new();
    let mut by_path = BTreeMap::new();
    for rec in existing {
        by_id.insert(rec.id.clone(), rec.clone());
        if rec.project == p.slug && rec.origin.starts_with("markdown:") {
            let mut origin = rec.origin.trim_start_matches("markdown:").to_string();
            if let Some(at) = origin.rfind('@') {
                origin.truncate(at);
            }
            by_path.insert(origin, rec);
        }
    }
    let mut items = Vec::new();
    for file in files {
        let (kind, slug, reason) = classify_path(&file);
        if !reason.is_empty() {
            summary.skip(&file, &kind, &reason);
            continue;
        }
        let prev = by_path.get(&file).cloned();
        let imported = prev.is_some();
        if let Some(prev) = &prev
            && !opts.force_rev
        {
            summary.skip(
                &file,
                &kind,
                &format!("origin path already imported as {}", prev.id),
            );
            continue;
        }
        let oid = &oids[&file];
        let Some(text) = blobs.get(oid) else {
            return Err(input_error(
                "Cannot read repository history",
                &format!("git cat-file returned no object {oid} for {file}"),
                &format!("git -C {:?} fsck", repo.display().to_string()),
            ));
        };
        let (mut front, section) = match parse_frontmatter(text) {
            Ok(v) => v,
            Err(e) => {
                summary.skip(&file, &kind, &e.to_string());
                continue;
            }
        };
        // An agent record keeps the file whole, front matter included: the
        // harness reads the body the way it reads the file.
        let body = if kind == "agent" {
            if text_value(front.get("title")).is_empty()
                && let Some(name) = front.get("name").and_then(|v| v.as_str())
            {
                front.insert("title".into(), serde_json::Value::String(name.into()));
            }
            text.clone()
        } else {
            section.clone()
        };
        let mut rec = pulse_store::Record {
            project: p.slug.clone(),
            kind: kind.clone(),
            slug: slug.clone(),
            path: file.clone(),
            origin: format!("markdown:{file}@{}", summary.ref_),
            ..pulse_store::Record::default()
        };
        rec.id = scoped_id(&kind, &slug, &p);
        if numbered(&kind) {
            let mut id = text_value(front.get("id"));
            if id.is_empty() {
                let base = Path::new(&file)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let parts: Vec<_> = base.split('-').collect();
                if parts.len() > 1 && parts[0] == crate::records::record_prefix(&kind).unwrap_or("")
                {
                    id = format!("{}-{}", parts[0], parts[1]);
                }
            }
            let prefix = crate::records::record_prefix(&kind).unwrap_or("");
            if id.starts_with(&format!("{prefix}-")) {
                let n = id.trim_start_matches(&format!("{prefix}-")).parse::<i64>();
                match n {
                    Ok(n) if n > 0 => {
                        rec.id = id;
                        rec.number = n;
                    }
                    _ => {
                        summary.skip(&file, &kind, &format!("invalid numbered id {id}"));
                        continue;
                    }
                }
            } else if !id.is_empty() {
                summary.skip(&file, &kind, &format!("id {id} does not match kind {kind}"));
                continue;
            }
        }
        if let Err(e) = apply_columns(&mut rec, &front, &body) {
            summary.skip(&file, &kind, &e.to_string());
            continue;
        }
        if rec.date.is_empty() {
            rec.date = git_read(
                &repo,
                &[
                    "log",
                    "--diff-filter=A",
                    "--format=%cs",
                    "-1",
                    &summary.ref_,
                    "--",
                    &file,
                ],
            )?
            .trim()
            .into();
        }
        if let Some(prev) = prev {
            let (_, revision) = s.get_record(&prev.id, 0)?;
            let revision = revision.expect("current revision exists");
            if revision.body == body {
                summary.skip(&file, &kind, &format!("body unchanged from {}", prev.id));
                continue;
            }
            if prev.kind != kind || prev.id != rec.id {
                summary.skip(
                    &file,
                    &kind,
                    &format!(
                        "origin path changed identity; existing record is {}",
                        prev.id
                    ),
                );
                continue;
            }
            rec.created_at = prev.created_at;
            rec.created_by = prev.created_by;
            rec.origin = prev.origin;
        } else if let Some(collision) = by_id.get(&rec.id) {
            summary.skip(
                &file,
                &kind,
                &format!(
                    "id {} already belongs to project {}, path {}",
                    rec.id, collision.project, collision.path
                ),
            );
            continue;
        }
        let role = if kind == "agent" {
            role_from_agent(&front, &rec.title)
        } else {
            None
        };
        let date = rec.date.clone();
        by_id.insert(rec.id.clone(), rec.clone());
        items.push(ImportItem {
            record: rec,
            front,
            body,
            revise: imported,
        });
        let Some((role_slug, role_front)) = role else {
            continue;
        };
        // The role an agent file declares is a second record from the same
        // path; `#role` on the origin keeps the two apart.
        let key = format!("{file}#role");
        let prev = by_path.get(&key).cloned();
        let imported = prev.is_some();
        if let Some(prev) = &prev
            && !opts.force_rev
        {
            summary.skip(
                &file,
                "role",
                &format!("origin path already imported as {}", prev.id),
            );
            continue;
        }
        let mut rec = pulse_store::Record {
            id: scoped_id("role", &role_slug, &p),
            project: p.slug.clone(),
            kind: "role".into(),
            slug: role_slug,
            path: file.clone(),
            origin: format!("markdown:{key}@{}", summary.ref_),
            ..pulse_store::Record::default()
        };
        if let Err(e) = apply_columns(&mut rec, &role_front, &section) {
            summary.skip(&file, "role", &e.to_string());
            continue;
        }
        if rec.date.is_empty() {
            rec.date = date;
        }
        if let Some(prev) = prev {
            let (_, revision) = s.get_record(&prev.id, 0)?;
            let revision = revision.expect("current revision exists");
            if revision.body == section && revision.frontmatter == role_front {
                summary.skip(&file, "role", &format!("role unchanged from {}", prev.id));
                continue;
            }
            if prev.id != rec.id {
                summary.skip(
                    &file,
                    "role",
                    &format!(
                        "origin path changed identity; existing record is {}",
                        prev.id
                    ),
                );
                continue;
            }
            rec.created_at = prev.created_at;
            rec.created_by = prev.created_by;
            rec.origin = prev.origin;
        } else if let Some(collision) = by_id.get(&rec.id) {
            summary.skip(
                &file,
                "role",
                &format!(
                    "id {} already belongs to project {}, path {}",
                    rec.id, collision.project, collision.path
                ),
            );
            continue;
        }
        by_id.insert(rec.id.clone(), rec.clone());
        items.push(ImportItem {
            record: rec,
            front: role_front,
            body: section,
            revise: imported,
        });
    }
    for item in items {
        if !opts.dry_run {
            let note = format!("import {}@{}", item.record.path, summary.ref_);
            s.put_record(
                item.record.clone(),
                item.front,
                &item.body,
                &opts.author,
                &note,
            )
            .map_err(|e| anyhow!("import {}: {e}", item.record.path))?;
        }
        let count = summary.kinds.entry(item.record.kind).or_default();
        if item.revise {
            count.revised += 1;
            summary.revised += 1;
        } else {
            count.imported += 1;
            summary.imported += 1;
        }
    }
    if !opts.dry_run {
        s.event(
            &opts.author,
            "records.migrated",
            &p.slug,
            pulse_store::Detail::json(&summary)?,
        )?;
    }
    Ok(summary)
}

impl Summary {
    fn skip(&mut self, file: &str, kind: &str, reason: &str) {
        self.skipped.push(SkippedFile {
            path: file.into(),
            kind: kind.into(),
            reason: reason.into(),
        });
        self.kinds.entry(kind.into()).or_default().skipped += 1;
    }
}

/// The role an agent file declares in its front matter's `pulse:` block
/// (`pulse: {role: content, every: 1w, about: ...}`): the role word, and the
/// block's other keys as the role record's front matter, titled after the
/// agent when the block names no title. The agent's body is the role's
/// section text.
fn role_from_agent(front: &Frontmatter, title: &str) -> Option<(String, Frontmatter)> {
    let block = front.get("pulse")?.as_object()?;
    let role = slugify(block.get("role")?.as_str()?);
    if role.is_empty() {
        return None;
    }
    let mut out: Frontmatter = block
        .iter()
        .filter(|(k, _)| k.as_str() != "role")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if text_value(out.get("title")).is_empty() && !title.is_empty() {
        out.insert("title".into(), serde_json::Value::String(title.into()));
    }
    Some((role, out))
}

pub fn classify_path(file: &str) -> (String, String, String) {
    match file {
        "AGENTS.md" => return ("instruction".into(), "agents".into(), String::new()),
        "CLAUDE.md" => return ("instruction".into(), "claude".into(), String::new()),
        ".github/copilot-instructions.md" => {
            return (
                "instruction".into(),
                "copilot-instructions".into(),
                String::new(),
            );
        }
        "docs/README.md" => return ("guide".into(), "docs".into(), String::new()),
        "docs/versions.md" => return ("usage".into(), "versions".into(), String::new()),
        _ => {}
    }
    let parts: Vec<_> = file.split('/').collect();
    if parts.len() == 3 && parts[0] == ".pulse" && parts[1] == "agents" {
        // An agent file the harness reads from `.pulse/agents/<name>.md`
        // (or `<name>.agent.md`); the file is the record, front matter and
        // all.
        let base = parts[2];
        let Some(stem) = base.strip_suffix(".md") else {
            return (
                "agent".into(),
                String::new(),
                "not a Markdown record".into(),
            );
        };
        let stem = stem.strip_suffix(".agent").unwrap_or(stem);
        return ("agent".into(), slugify(stem), String::new());
    }
    if parts.len() < 3 || parts[0] != "docs" {
        return (
            String::new(),
            String::new(),
            "outside record folders".into(),
        );
    }
    let mut kind = folder_kind(parts[1]).to_string();
    if kind.is_empty() {
        return (String::new(), String::new(), "not a record folder".into());
    }
    let base = parts.last().copied().unwrap_or("");
    if base == "README.md" {
        return (
            kind,
            String::new(),
            "folder index; use pulse record list".into(),
        );
    }
    if kind == "skill" && base != "SKILL.md" {
        return (
            kind,
            String::new(),
            "skill companion file; only SKILL.md is a record".into(),
        );
    }
    if !base.ends_with(".md") {
        return (kind, String::new(), "not a Markdown record".into());
    }
    if base.starts_with("_template") {
        let mut name = base.trim_end_matches(".md").trim_start_matches("_template");
        name = name.trim_start_matches('-');
        if kind == "plan" && name == "work-unit" {
            return ("template".into(), "wu".into(), String::new());
        }
        let mut slug = kind.clone();
        if !name.is_empty() {
            slug.push('-');
            slug.push_str(name);
        }
        return ("template".into(), slug, String::new());
    }
    if kind == "skill" {
        return (
            kind,
            slugify(
                parts
                    .get(parts.len().saturating_sub(2))
                    .copied()
                    .unwrap_or(""),
            ),
            String::new(),
        );
    }
    if kind == "plan" && base.starts_with("WU-") {
        kind = "wu".into();
    }
    let mut slug = base
        .trim_end_matches(".md")
        .trim_end_matches(".prompt")
        .to_string();
    if numbered(&kind) {
        let prefix = crate::records::record_prefix(&kind).unwrap_or("");
        let p: Vec<_> = slug.splitn(3, '-').collect();
        if p.len() == 3 && p[0] == prefix {
            slug = p[2].into();
        } else if kind == "feature"
            && let Some((_, after)) = slug.split_once('-')
        {
            slug = after.into();
        }
    }
    (kind, slugify(&slug), String::new())
}

pub fn git_repo_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "GIT_DIR" | "GIT_WORK_TREE" | "GIT_INDEX_FILE" | "GIT_PREFIX"
            )
        })
        .collect()
}

pub fn git_read(repo: &Path, args: &[&str]) -> Result<String> {
    let args: Vec<String> = args.iter().map(|s| (*s).into()).collect();
    git_read_owned(repo, &args)
}

fn git_read_owned(repo: &Path, args: &[String]) -> Result<String> {
    let output = Command::new(pulse_gitenv::git_program())
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_clear()
        .envs(git_repo_env())
        .output();
    match output {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(input_error(
            "Cannot read repository history",
            "git is not installed",
            "git --version",
        )),
        Err(e) => Err(e.into()),
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).to_string()),
        Ok(out) => Err(input_error(
            "Cannot read repository history",
            &format!(
                "{} ({})",
                String::from_utf8_lossy(&out.stderr).trim(),
                out.status
            ),
            &format!("git -C {:?} status", repo.display().to_string()),
        )),
    }
}

pub fn git_cat_blobs(
    repo: &Path,
    oids: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut blobs = BTreeMap::new();
    if oids.is_empty() {
        return Ok(blobs);
    }
    let mut input = String::new();
    for oid in oids.values() {
        if !blobs.contains_key(oid) {
            blobs.insert(oid.clone(), String::new());
            input.push_str(oid);
            input.push('\n');
        }
    }
    let mut child = Command::new(pulse_gitenv::git_program())
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .env_clear()
        .envs(git_repo_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                input_error(
                    "Cannot read repository history",
                    "git is not installed",
                    "git --version",
                )
            } else {
                e.into()
            }
        })?;
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(input_error(
            "Cannot read repository history",
            &format!(
                "{} ({})",
                String::from_utf8_lossy(&out.stderr).trim(),
                out.status
            ),
            &format!("git -C {:?} status", repo.display().to_string()),
        ));
    }
    let mut rest = String::from_utf8_lossy(&out.stdout).to_string();
    while !rest.is_empty() {
        let Some((header, body)) = rest.split_once('\n') else {
            break;
        };
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() == 2 && fields[1] == "missing" {
            blobs.remove(fields[0]);
            rest = body.to_string();
            continue;
        }
        if fields.len() != 3 {
            return Err(input_error(
                "Cannot read repository history",
                &format!("unexpected git cat-file --batch header {header:?}"),
                &format!("git -C {:?} fsck", repo.display().to_string()),
            ));
        }
        let size = fields[2].parse::<usize>().map_err(|_| {
            input_error(
                "Cannot read repository history",
                &format!("unexpected git cat-file --batch header {header:?}"),
                &format!("git -C {:?} fsck", repo.display().to_string()),
            )
        })?;
        if size > body.len() {
            return Err(input_error(
                "Cannot read repository history",
                &format!("unexpected git cat-file --batch header {header:?}"),
                &format!("git -C {:?} fsck", repo.display().to_string()),
            ));
        }
        blobs.insert(fields[0].into(), body[..size].into());
        rest = body[size..]
            .strip_prefix('\n')
            .unwrap_or(&body[size..])
            .to_string();
    }
    Ok(blobs)
}

pub use import_repo as ImportRepo;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportOptions, export};
    use crate::frontmatter::parse_frontmatter;
    use std::path::PathBuf;
    use std::process::Command;

    const FIXTURE_REF: &str = "02691f8";

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
        pulse_testutil::neutralize_runner_environment();
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

    fn source_repo() -> Option<PathBuf> {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .ok()?;
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["cat-file", "-e", &format!("{FIXTURE_REF}^{{commit}}")])
            .status()
            .ok()?
            .success()
            .then_some(repo)
    }

    fn git_fixture(repo: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new(pulse_gitenv::git_program())
            .arg("-C")
            .arg(repo)
            .args(args)
            .env_clear()
            .envs(git_repo_env())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@x")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@x")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}\n{}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn classify_paths_match_record_layout() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            classify_path("AGENTS.md"),
            ("instruction".into(), "agents".into(), String::new())
        );
        assert_eq!(
            classify_path("docs/README.md"),
            ("guide".into(), "docs".into(), String::new())
        );
        assert_eq!(
            classify_path("docs/plans/WU-022-decision-hook.md"),
            ("wu".into(), "decision-hook".into(), String::new())
        );
        assert_eq!(
            classify_path("docs/features/016-project-environment-recipes.md"),
            (
                "feature".into(),
                "project-environment-recipes".into(),
                String::new()
            )
        );
        assert_eq!(
            classify_path("docs/plans/_template-work-unit.md"),
            ("template".into(), "wu".into(), String::new())
        );
        assert_eq!(
            classify_path("docs/skills/pulse-plan/README.md").2,
            "folder index; use pulse record list"
        );
        assert_eq!(classify_path("README.md").2, "outside record folders");
        assert_eq!(
            classify_path(".pulse/agents/reviewer.agent.md"),
            ("agent".into(), "reviewer".into(), String::new())
        );
        assert_eq!(
            classify_path(".pulse/agents/notes.txt").2,
            "not a Markdown record"
        );
        assert_eq!(
            classify_path(".pulse/agents/deep/x.md").2,
            "outside record folders"
        );
    }

    #[test]
    fn agent_files_import_whole_and_their_pulse_block_makes_a_role() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path().to_str().unwrap();
        pulse_testutil::git(&["-C", root, "init", "-q", "-b", "main"]).unwrap();
        std::fs::create_dir_all(repo.path().join(".pulse/agents")).unwrap();
        let agent = "---\nname: content\ndescription: keeps the blog fed\npulse:\n  role: content\n  every: 1w\n  about: keeps the blog fed and the archive tidy\n  gaps: [stale-lockfile]\n---\n## Your concern: content for {slug}\n\n1. Read the drafts.\n";
        std::fs::write(repo.path().join(".pulse/agents/content.md"), agent).unwrap();
        std::fs::write(
            repo.path().join(".pulse/agents/reviewer.agent.md"),
            "---\nname: reviewer\n---\nRead the diff.\n",
        )
        .unwrap();
        pulse_testutil::git(&["-C", root, "add", "."]).unwrap();
        pulse_testutil::git(&["-C", root, "commit", "-q", "-m", "agents"]).unwrap();
        add_project(&s, "app", repo.path(), false);
        let opts = || ImportOptions {
            author: "tester".into(),
            ..ImportOptions::default()
        };
        let summary = import_repo(&s, root, "HEAD", "app", opts()).unwrap();
        assert_eq!(summary.imported, 3, "{:?}", summary.skipped);
        assert_eq!(summary.kinds["agent"].imported, 2);
        assert_eq!(summary.kinds["role"].imported, 1);
        let (rec, rev) = s.get_record("AGENT-content-app", 0).unwrap();
        let rev = rev.unwrap();
        assert_eq!(rec.title, "content");
        assert_eq!(rev.body, agent, "the agent record is the file, whole");
        assert!(rec.origin.starts_with("markdown:.pulse/agents/content.md@"));
        let (role, role_rev) = s.get_record("ROLE-content-app", 0).unwrap();
        let role_rev = role_rev.unwrap();
        assert_eq!(role.kind, "role");
        assert_eq!(role.slug, "content");
        assert_eq!(role.title, "content");
        assert!(
            role.origin
                .starts_with("markdown:.pulse/agents/content.md#role@")
        );
        assert_eq!(role_rev.frontmatter["every"], "1w");
        assert_eq!(
            role_rev.frontmatter["gaps"],
            serde_json::json!(["stale-lockfile"])
        );
        assert!(!role_rev.frontmatter.contains_key("role"));
        assert_eq!(
            role_rev.body,
            "## Your concern: content for {slug}\n\n1. Read the drafts.\n"
        );
        assert!(s.get_record("ROLE-reviewer-app", 0).is_err());
        let again = import_repo(&s, root, "HEAD", "app", opts()).unwrap();
        assert_eq!(again.imported, 0);
        assert_eq!(again.skipped.len(), 2, "{:?}", again.skipped);
        assert!(
            again
                .skipped
                .iter()
                .all(|k| k.reason.contains("origin path already imported"))
        );
        let forced = |s: &pulse_store::Store| {
            import_repo(
                s,
                root,
                "HEAD",
                "app",
                ImportOptions {
                    force_rev: true,
                    ..opts()
                },
            )
            .unwrap()
        };
        let same = forced(&s);
        assert_eq!(same.revised, 0);
        assert!(same.skipped.iter().all(|k| k.reason.contains("unchanged")));
        std::fs::write(
            repo.path().join(".pulse/agents/content.md"),
            agent.replace("every: 1w", "every: 2w"),
        )
        .unwrap();
        pulse_testutil::git(&["-C", root, "commit", "-qam", "slower"]).unwrap();
        let changed = forced(&s);
        assert_eq!(changed.revised, 2, "{:?}", changed.skipped);
        let (role2, rev2) = s.get_record("ROLE-content-app", 0).unwrap();
        assert_eq!(rev2.unwrap().frontmatter["every"], "2w");
        assert_eq!(role2.created_at, role.created_at);
        assert_eq!(role2.current_rev, 2);
    }

    #[test]
    fn import_repo_paths_restrict_the_committed_tree() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
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
        std::fs::create_dir_all(repo.path().join("docs/context")).unwrap();
        std::fs::write(
            repo.path().join("docs/evolution/constitution.md"),
            "---\ntitle: Constitution\nstatus: Active\n---\n\n# Constitution\n",
        )
        .unwrap();
        std::fs::write(
            repo.path().join("docs/context/environment.md"),
            "---\ntitle: Environment\n---\n\n# Environment\n",
        )
        .unwrap();
        pulse_testutil::git(&["-C", repo.path().to_str().unwrap(), "add", "."]).unwrap();
        pulse_testutil::git(&[
            "-C",
            repo.path().to_str().unwrap(),
            "commit",
            "-q",
            "-m",
            "docs",
        ])
        .unwrap();
        add_project(&s, "brain", repo.path(), true);
        let dry = import_repo(
            &s,
            repo.path().to_str().unwrap(),
            "HEAD",
            "",
            ImportOptions {
                dry_run: true,
                author: "tester".into(),
                paths: vec!["docs/evolution/constitution.md".into()],
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(dry.imported, 1);
        assert!(
            s.list_records(pulse_store::RecordFilter::default())
                .unwrap()
                .is_empty()
        );
        let summary = import_repo(
            &s,
            repo.path().to_str().unwrap(),
            "HEAD",
            "",
            ImportOptions {
                author: "tester".into(),
                paths: vec!["docs/evolution/constitution.md".into()],
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(
            s.list_records(pulse_store::RecordFilter::default())
                .unwrap()
                .len(),
            1
        );
        let second = import_repo(
            &s,
            repo.path().to_str().unwrap(),
            "HEAD",
            "",
            ImportOptions {
                author: "tester".into(),
                paths: vec!["docs/evolution/constitution.md".into()],
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(second.imported, 0);
        assert!(
            second.skipped[0]
                .reason
                .contains("origin path already imported")
        );
    }

    #[test]
    fn import_real_repo_and_export_round_trip() {
        let _serial = crate::test_helpers::serial();
        let Some(repo) = source_repo() else {
            return;
        };
        let s = temp_store();
        add_project(&s, "brain", &repo, true);
        let dry = import_repo(
            &s,
            repo.to_str().unwrap(),
            FIXTURE_REF,
            "",
            ImportOptions {
                dry_run: true,
                author: "tester".into(),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert!(
            crate::records::list(&s, pulse_store::RecordFilter::default())
                .unwrap()
                .is_empty()
        );
        let first = import_repo(
            &s,
            repo.to_str().unwrap(),
            FIXTURE_REF,
            "",
            ImportOptions {
                author: "tester".into(),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(first.imported, dry.imported);
        assert_eq!(first.revised, 0);
        assert_eq!(first.imported, 159);
        assert_eq!(first.skipped.len(), 15);
        for (kind, want) in [
            ("adr", 26),
            ("plan", 20),
            ("wu", 21),
            ("feature", 16),
            ("memory", 3),
            ("workflow", 23),
            ("skill", 13),
            ("guide", 5),
            ("context", 4),
            ("architecture", 1),
            ("evolution", 3),
            ("instruction", 3),
            ("template", 9),
            ("usage", 2),
            ("prompt", 10),
        ] {
            assert_eq!(first.kinds[kind].imported, want, "{kind}");
        }
        for id in [
            "ADR-019",
            "PLAN-026",
            "WU-022",
            "FEAT-016",
            "EVO-constitution",
            "EVO-archive",
            "CTX-environment",
            "GUIDE-docs",
            "USAGE-versions",
            "PROMPT-eli5",
            "TPL-adr",
            "TPL-wu",
        ] {
            crate::records::get(&s, id, 0).unwrap();
        }
        let (rec, rev) = crate::records::get(&s, "INSTR-agents", 0).unwrap();
        assert!(!rec.date.is_empty());
        assert!(rev.frontmatter.is_empty());
        assert_eq!(rec.title, "PULSE - Agent Operating Guide");
        let output = tempfile::tempdir().unwrap();
        let n = export(
            &s,
            output.path().to_str().unwrap(),
            ExportOptions::default(),
        )
        .unwrap();
        assert_eq!(n as i64, first.imported);
        for rec in crate::records::list(&s, pulse_store::RecordFilter::default()).unwrap() {
            let original = git_fixture(&repo, &["show", &format!("{FIXTURE_REF}:{}", rec.path)]);
            let exported = std::fs::read_to_string(
                output
                    .path()
                    .join(rec.path.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )
            .unwrap();
            let (f1, b1) = parse_frontmatter(&original).unwrap();
            let (f2, b2) = parse_frontmatter(&exported).unwrap();
            assert_eq!(f2, f1, "{}", rec.path);
            assert_eq!(b2, b1, "{}", rec.path);
        }
        assert!(!output.path().join("README.md").exists());
        assert!(
            export(
                &s,
                output.path().to_str().unwrap(),
                ExportOptions::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn import_force_rev_and_project_isolation() {
        let _serial = crate::test_helpers::serial();
        let Some(repo) = source_repo() else {
            return;
        };
        let s = temp_store();
        add_project(&s, "brain", &repo, true);
        let app = tempfile::tempdir().unwrap();
        add_project(&s, "app", app.path(), false);
        let file = "docs/context/environment.md";
        s.put_record(
            pulse_store::Record {
                id: "CTX-environment".into(),
                project: "brain".into(),
                kind: "context".into(),
                slug: "environment".into(),
                path: file.into(),
                origin: format!("markdown:{file}@old"),
                ..pulse_store::Record::default()
            },
            crate::records::Frontmatter::new(),
            "Old recipe\n",
            "tester",
            "",
        )
        .unwrap();
        let summary = import_repo(
            &s,
            repo.to_str().unwrap(),
            FIXTURE_REF,
            "brain",
            ImportOptions {
                force_rev: true,
                author: "tester".into(),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(summary.revised, 1);
        let (head, rev) = crate::records::get(&s, "CTX-environment", 0).unwrap();
        assert_eq!(rev.rev, 2);
        assert!(rev.body.contains("# Environment"));
        assert_eq!(head.origin, format!("markdown:{file}@old"));
        let (_, old) = crate::records::get(&s, "CTX-environment", 1).unwrap();
        assert_eq!(old.body, "Old recipe\n");
        let other = import_repo(
            &s,
            repo.to_str().unwrap(),
            FIXTURE_REF,
            "app",
            ImportOptions {
                author: "tester".into(),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        for id in [
            "EVO-constitution-app",
            "EVO-archive-app",
            "CTX-environment-app",
            "INSTR-agents-app",
            "PROMPT-eli5-app",
            "TPL-adr-app",
        ] {
            assert_eq!(crate::records::get(&s, id, 0).unwrap().0.project, "app");
        }
        assert_eq!(other.kinds["adr"].imported, 0);
        assert!(
            import_repo(
                &s,
                repo.to_str().unwrap(),
                FIXTURE_REF,
                "missing",
                ImportOptions::default()
            )
            .is_err()
        );
    }
}
