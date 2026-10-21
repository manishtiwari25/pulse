//! Skills: folders holding a `SKILL.md` the model loads on demand through
//! the `skill` tool. The layout is the one Claude Code, Copilot and the
//! agents.md convention share, so a repository's existing skills work as
//! they are.

use pulse_provider::Tool;
use serde_json::json;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Where a clone's skills live, relative to its root, in search order.
pub const SKILL_DIRS: &[&str] = &[
    ".pulse/skills",
    ".claude/skills",
    ".github/skills",
    ".agents/skills",
];

/// The name of the tool that loads a skill.
pub const SKILL_TOOL: &str = "skill";

/// Bytes of one SKILL.md the model gets before it is cut.
pub const SKILL_BODY_LIMIT: usize = 48 * 1024;

/// Bytes of a skill's support file the model gets before it is cut.
pub const SKILL_FILE_LIMIT: usize = 64 * 1024;

/// Skills named in the tool description; the rest are counted.
pub const SKILL_LIST_LIMIT: usize = 60;

/// Support files listed under a loaded skill; the rest are counted.
pub const SKILL_FILES_LIMIT: usize = 100;

const DESCRIPTION_LIMIT: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The folder holding SKILL.md and its support files; empty for a
    /// skill whose body is inline.
    pub dir: PathBuf,
    /// Where the skill came from: `.claude/skills`, `~/.pulse/skills`,
    /// `pulse skill`.
    pub source: String,
    /// The instructions of a skill that has no folder (one from the
    /// `pulse skill` registry); None reads `<dir>/SKILL.md`.
    pub body: Option<String>,
}

impl Skill {
    /// A skill whose instructions are given inline, as the `pulse skill`
    /// registry keeps them.
    pub fn inline(name: &str, description: &str, body: &str, source: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            dir: PathBuf::new(),
            source: source.to_string(),
            body: Some(body.to_string()),
        }
    }
}

/// Reads the skills of the clone at `cwd` (every [`SKILL_DIRS`] entry) and
/// the owner's own under `<home>/skills`. The first skill of a name wins,
/// so a repository's skill shadows the owner's.
pub fn load_skills(cwd: &str, home: &str) -> Vec<Skill> {
    let root = if cwd.is_empty() {
        Path::new(".")
    } else {
        Path::new(cwd)
    };
    let mut roots: Vec<(PathBuf, String)> = SKILL_DIRS
        .iter()
        .map(|d| (root.join(d), (*d).to_string()))
        .collect();
    if !home.is_empty() {
        roots.push((
            Path::new(home).join("skills"),
            "~/.pulse/skills".to_string(),
        ));
    }
    let mut out: Vec<Skill> = Vec::new();
    for (dir, source) in roots {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let Some(skill) = read_skill(&folder, &source) else {
                continue;
            };
            if out.iter().any(|s| s.name == skill.name) {
                continue;
            }
            out.push(skill);
        }
    }
    out
}

fn read_skill(folder: &Path, source: &str) -> Option<Skill> {
    let raw = fs::read_to_string(folder.join("SKILL.md")).ok()?;
    let (front, body) = split_front_matter(&raw);
    let folder_name = folder.file_name()?.to_string_lossy().to_string();
    let mut name = front_value(front, "name").unwrap_or_default();
    if name.is_empty() {
        name = folder_name;
    }
    let mut description = front_value(front, "description").unwrap_or_default();
    if description.is_empty() {
        description = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("")
            .to_string();
    }
    if description.chars().count() > DESCRIPTION_LIMIT {
        description = format!(
            "{}...",
            description
                .chars()
                .take(DESCRIPTION_LIMIT - 3)
                .collect::<String>()
                .trim_end()
        );
    }
    Some(Skill {
        name,
        description,
        dir: folder.to_path_buf(),
        source: source.to_string(),
        body: None,
    })
}

/// Splits a leading `---` front-matter block off a Markdown file; returns
/// (front matter without the fences, body).
pub fn split_front_matter(raw: &str) -> (&str, &str) {
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = text.strip_prefix("---") else {
        return ("", text);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return ("", text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let front = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (front, body);
        }
        offset += line.len();
    }
    ("", text)
}

/// The value of a top-level `key: value` line of a front-matter block,
/// quotes stripped; a `key: >` or `key: |` block folds its indented lines.
pub fn front_value(front: &str, key: &str) -> Option<String> {
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        if v == ">" || v == "|" || v == ">-" || v == "|-" {
            let mut parts = Vec::new();
            while let Some(next) = lines.peek() {
                if next.starts_with(' ') || next.starts_with('\t') {
                    parts.push(next.trim().to_string());
                    lines.next();
                } else {
                    break;
                }
            }
            return Some(parts.join(" "));
        }
        let unquoted = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v);
        return Some(unquoted.to_string());
    }
    None
}

/// The `skill` tool, its description naming every skill the run offers.
pub fn skill_tool(skills: &[Skill]) -> Tool {
    let mut listing: Vec<String> = skills
        .iter()
        .take(SKILL_LIST_LIMIT)
        .map(|s| {
            if s.description.is_empty() {
                s.name.clone()
            } else {
                format!("{}: {}", s.name, s.description)
            }
        })
        .collect();
    if skills.len() > SKILL_LIST_LIMIT {
        listing.push(format!(
            "... and {} more (call skill with any name to try it)",
            skills.len() - SKILL_LIST_LIMIT
        ));
    }
    Tool {
        name: SKILL_TOOL.to_string(),
        description: format!(
            "Load a skill before doing work it covers: its SKILL.md instructions come back with the list of files beside it; pass file to read one of those files. Skills: {}",
            listing.join("; ")
        ),
        parameters: json!({"type":"object","properties":{
            "name":{"type":"string","description":"The skill to load."},
            "file":{"type":"string","description":"A support file of the skill to read instead, relative to the skill folder."}
        },"required":["name"]}),
    }
}

/// What the model gets for `skill` with `name` (and `file`): the SKILL.md
/// body and the files beside it, or one support file. `Err` carries the
/// text of a semantic failure (unknown skill, file outside the folder).
pub fn load_skill(skills: &[Skill], name: &str, file: &str) -> Result<String, String> {
    let wanted = name.trim();
    let Some(skill) = skills
        .iter()
        .find(|s| s.name == wanted)
        .or_else(|| skills.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)))
    else {
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        return Err(format!(
            "unknown skill {wanted:?}; available: {}",
            names.join(", ")
        ));
    };
    if let Some(body) = &skill.body {
        if !file.trim().is_empty() {
            return Err(format!(
                "skill {}: a {} skill has no files beside it",
                skill.name, skill.source
            ));
        }
        return Ok(format!(
            "skill {} ({})\n{}\n",
            skill.name,
            skill.source,
            cut(body.trim(), SKILL_BODY_LIMIT, "the skill")
        ));
    }
    if !file.trim().is_empty() {
        return skill_file(skill, file.trim());
    }
    let raw = fs::read_to_string(skill.dir.join("SKILL.md"))
        .map_err(|e| format!("skill {}: read SKILL.md: {e}", skill.name))?;
    let (_, body) = split_front_matter(&raw);
    let mut out = format!(
        "skill {} ({}/{})\n{}\n",
        skill.name,
        skill.source,
        skill
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        cut(body.trim(), SKILL_BODY_LIMIT, "SKILL.md")
    );
    let files = support_files(&skill.dir);
    if !files.is_empty() {
        out.push_str("\nFiles in this skill (pass file to read one): ");
        out.push_str(&files.join(", "));
        out.push('\n');
    }
    Ok(out)
}

fn skill_file(skill: &Skill, file: &str) -> Result<String, String> {
    let rel = Path::new(file);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "skill {}: file must be a path inside the skill folder, not {file:?}",
            skill.name
        ));
    }
    let path = skill.dir.join(rel);
    let root = fs::canonicalize(&skill.dir).unwrap_or_else(|_| skill.dir.clone());
    let real = fs::canonicalize(&path).map_err(|e| format!("skill {}: {file}: {e}", skill.name))?;
    if !real.starts_with(&root) {
        return Err(format!(
            "skill {}: {file} points outside the skill folder",
            skill.name
        ));
    }
    if real.is_dir() {
        let mut names: Vec<String> = fs::read_dir(&real)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        names.sort();
        return Ok(format!("{file}/ holds: {}", names.join(", ")));
    }
    let bytes = fs::read(&real).map_err(|e| format!("skill {}: {file}: {e}", skill.name))?;
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(format!(
            "{} of skill {}\n{}",
            file,
            skill.name,
            cut(&text, SKILL_FILE_LIMIT, file)
        )),
        Ok(text) => Ok(format!("{file}: binary file, {} bytes", text.len())),
        Err(e) => Ok(format!(
            "{file}: binary file, {} bytes",
            e.into_bytes().len()
        )),
    }
}

/// Every file under a skill folder except SKILL.md, relative to the folder.
fn support_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n != "SKILL.md")
                && let Ok(rel) = p.strip_prefix(dir)
            {
                out.push(rel.display().to_string());
            }
        }
    }
    out.sort();
    if out.len() > SKILL_FILES_LIMIT {
        let more = out.len() - SKILL_FILES_LIMIT;
        out.truncate(SKILL_FILES_LIMIT);
        out.push(format!("... and {more} more"));
    }
    out
}

fn cut(text: &str, limit: usize, what: &str) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let at = text.floor_char_boundary(limit);
    format!(
        "{}\n[... {what} continues for {} more bytes ...]",
        &text[..at],
        text.len() - at
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn front_matter_split_and_values() {
        let raw = "---\nname: deploy\ndescription: \"Ship a release\"\nnotes: >\n  folded\n  text\n---\n# Deploy\nBody\n";
        let (front, body) = split_front_matter(raw);
        assert_eq!(front_value(front, "name").as_deref(), Some("deploy"));
        assert_eq!(
            front_value(front, "description").as_deref(),
            Some("Ship a release")
        );
        assert_eq!(front_value(front, "notes").as_deref(), Some("folded text"));
        assert_eq!(front_value(front, "missing"), None);
        assert_eq!(body, "# Deploy\nBody\n");
        // No front matter: the whole file is the body.
        assert_eq!(split_front_matter("# Plain\n"), ("", "# Plain\n"));
        // An unterminated block is left alone.
        assert_eq!(split_front_matter("---\nname: x\n"), ("", "---\nname: x\n"));
    }

    #[test]
    fn loads_clone_skills_before_the_owners_and_dedupes_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        let home = tmp.path().join("home");
        write(
            &clone,
            ".claude/skills/release/SKILL.md",
            "---\nname: release\ndescription: Cut a release.\n---\nSteps here.\n",
        );
        write(
            &clone,
            ".pulse/skills/review/SKILL.md",
            "# Review\n\nReview a change carefully.\n",
        );
        write(
            &clone,
            ".pulse/skills/empty/README.md",
            "no SKILL.md here\n",
        );
        write(
            &home,
            "skills/release/SKILL.md",
            "---\nname: release\ndescription: The owner's copy.\n---\n",
        );
        write(
            &home,
            "skills/notes/SKILL.md",
            "---\nname: notes\ndescription: Keep notes.\n---\nNote well.\n",
        );
        let skills = load_skills(clone.to_str().unwrap(), home.to_str().unwrap());
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["review", "release", "notes"]);
        let release = skills.iter().find(|s| s.name == "release").unwrap();
        assert_eq!(release.description, "Cut a release.");
        assert_eq!(release.source, ".claude/skills");
        let review = skills.iter().find(|s| s.name == "review").unwrap();
        assert_eq!(review.description, "Review a change carefully.");
        let notes = skills.iter().find(|s| s.name == "notes").unwrap();
        assert_eq!(notes.source, "~/.pulse/skills");
        let tool = skill_tool(&skills);
        assert_eq!(tool.name, "skill");
        assert!(
            tool.description
                .contains("review: Review a change carefully.")
        );
        assert!(tool.description.contains("release: Cut a release."));
    }

    #[test]
    fn load_skill_returns_body_files_and_support_files_inside_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        write(
            &clone,
            ".pulse/skills/release/SKILL.md",
            "---\nname: release\n---\n# Release\nRun scripts/ship.sh.\n",
        );
        write(
            &clone,
            ".pulse/skills/release/scripts/ship.sh",
            "#!/bin/sh\necho ship\n",
        );
        write(&clone, ".pulse/skills/release/ref/notes.md", "notes\n");
        write(&clone, "secret.txt", "top secret\n");
        let skills = load_skills(clone.to_str().unwrap(), "");
        let body = load_skill(&skills, "release", "").unwrap();
        assert!(body.starts_with(
            "skill release (.pulse/skills/release)\n# Release\nRun scripts/ship.sh.\n"
        ));
        assert!(body.contains(
            "Files in this skill (pass file to read one): ref/notes.md, scripts/ship.sh"
        ));
        let file = load_skill(&skills, "Release", "scripts/ship.sh").unwrap();
        assert_eq!(
            file,
            "scripts/ship.sh of skill release\n#!/bin/sh\necho ship\n"
        );
        assert_eq!(
            load_skill(&skills, "release", "scripts").unwrap(),
            "scripts/ holds: ship.sh"
        );
        let err = load_skill(&skills, "release", "../../secret.txt").unwrap_err();
        assert!(err.contains("inside the skill folder"), "{err}");
        let err = load_skill(&skills, "release", "/etc/passwd").unwrap_err();
        assert!(err.contains("inside the skill folder"), "{err}");
        let err = load_skill(&skills, "nope", "").unwrap_err();
        assert_eq!(err, "unknown skill \"nope\"; available: release");
        let err = load_skill(&skills, "release", "missing.txt").unwrap_err();
        assert!(err.contains("missing.txt"), "{err}");
    }

    #[test]
    fn long_bodies_are_cut_with_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        let big = "x".repeat(SKILL_BODY_LIMIT + 500);
        write(
            &clone,
            ".pulse/skills/big/SKILL.md",
            &format!(
                "---\nname: big\ndescription: {}\n---\n{big}\n",
                "d".repeat(400)
            ),
        );
        let skills = load_skills(clone.to_str().unwrap(), "");
        assert_eq!(skills[0].description.chars().count(), DESCRIPTION_LIMIT);
        assert!(skills[0].description.ends_with("..."));
        let body = load_skill(&skills, "big", "").unwrap();
        assert!(body.contains("[... SKILL.md continues for 500 more bytes ...]"));
    }

    #[test]
    fn inline_skills_serve_their_body_and_have_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        write(
            &clone,
            ".pulse/skills/release/SKILL.md",
            "---\nname: release\ndescription: folder one\n---\nfrom the folder\n",
        );
        let mut skills = load_skills(clone.to_str().unwrap(), "");
        for k in [
            Skill::inline("release", "registry copy", "shadowed", "pulse skill"),
            Skill::inline(
                "triage",
                "sort the inbox",
                "  Read every FINDING first.\n",
                "pulse skill",
            ),
        ] {
            if !skills.iter().any(|s| s.name == k.name) {
                skills.push(k);
            }
        }
        assert_eq!(skills.len(), 2);
        assert_eq!(
            load_skill(&skills, "triage", "").unwrap(),
            "skill triage (pulse skill)\nRead every FINDING first.\n"
        );
        assert!(
            load_skill(&skills, "release", "")
                .unwrap()
                .contains("from the folder")
        );
        let err = load_skill(&skills, "triage", "notes.md").unwrap_err();
        assert_eq!(
            err,
            "skill triage: a pulse skill skill has no files beside it"
        );
        let tool = skill_tool(&skills);
        assert!(
            tool.description.contains("triage: sort the inbox"),
            "{}",
            tool.description
        );
    }
}
