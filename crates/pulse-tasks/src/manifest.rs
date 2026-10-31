//! The project's own PULSE file. `.pulse/pulse.yaml` on the default branch
//! names the schedules the daemon keeps for the project - standing jobs,
//! skill runs, check and watch scripts - and the `SKILL.md` folders beside
//! it (`.pulse/skills`, `.claude/skills`, `.github/skills`, `.agents/skills`)
//! join the project's `pulse skill` registry. Both travel with the code: a
//! commit changes them, a pull request reviews them, a clone has them.
//!
//! The daemon reads the committed tree of the default branch, never a
//! working copy, and re-applies the file only when a commit changed it or
//! the skill folders (one `git ls-tree` tells). A schedule the file names
//! is added, updated in place, paused, resumed or ended as the file says;
//! one it no longer names ends. What the file gets wrong is a problem the
//! daemon reports - on its own line, in a `manifest.synced` event and on the
//! project board when it appears or clears - never a stopped schedule: an
//! entry with a problem leaves the row it had as it was.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::schedules::{self, ScheduleInput};

/// Where the file lives, relative to the repository root. The first is the
/// one to use; the second is read when it is the only one present.
pub const MANIFEST_PATHS: &[&str] = &[".pulse/pulse.yaml", ".pulse/pulse.yml"];
/// The skill folders the daemon reads, in the order the native harness
/// searches them: the first `SKILL.md` of a name wins.
pub const SKILL_DIRS: &[&str] = &[
    ".pulse/skills",
    ".claude/skills",
    ".github/skills",
    ".agents/skills",
];
/// The note a schedule paused by the file carries; the file resumes only a
/// schedule whose pause it made, so a pause by hand survives.
pub const PAUSED_NOTE: &str = "paused in the project's PULSE file";
/// The note a schedule the file no longer names ends with.
pub const ENDED_NOTE: &str = "removed from the project's PULSE file";
/// The board area the daemon posts the file's problems under; the thread is
/// the project slug.
pub const BOARD_AREA: &str = "manifest";
/// Bytes of the file, or of one `SKILL.md`, past which it is refused.
pub const FILE_LIMIT: usize = 256 * 1024;
/// Support files listed under an imported skill; the rest are counted.
const SUPPORT_FILES_LIMIT: usize = 30;
/// Characters of a skill description the registry takes.
const DESCRIPTION_LIMIT: usize = 200;
/// Where the daemon keeps what it last applied for a project.
const META_PREFIX: &str = "manifest:";

/// The file: a list of schedules and, when the project's work must run on a
/// certain kind of machine, its needs. Every key is checked, so a misspelt
/// one is a problem, not a silent default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What kind of machine the project's jobs run on, as words every
    /// machine either offers or not: `macos`, `linux`, `arm64`, `xcode`,
    /// `docker`, or a label the owner gave a machine. When the key is
    /// present it sets the project's needs (an empty list clears them);
    /// when absent the project keeps what `pulse init --needs` set.
    #[serde(default, deserialize_with = "needs_list")]
    pub needs: Option<Vec<String>>,
    #[serde(default)]
    pub schedules: Vec<Entry>,
}

/// `needs:` as a list (`[macos, xcode]`) or one string (`macos, xcode`).
fn needs_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Words {
        List(Vec<String>),
        One(String),
    }
    Ok(match Option::<Words>::deserialize(d)? {
        None => None,
        Some(Words::List(v)) => Some(v),
        Some(Words::One(s)) => Some(vec![s]),
    })
}

/// One schedule of the file. `name` is its identity: the row keeps the
/// name's changes in place and a renamed entry is a new schedule.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// A slug: lowercase letters, digits and dashes.
    pub name: String,
    /// The title of every job; the name when empty.
    #[serde(default)]
    pub title: String,
    /// The cadence: `30m`, `6h`, `1d`, `2w`, `hourly`, `daily`, `weekly`.
    #[serde(default)]
    pub every: String,
    /// `HH:MM` UTC; anchors a cadence of whole days. Quote it (`"09:00"`).
    #[serde(default)]
    pub at: String,
    /// `task` (the default), `improve`, `clean`, `reflect`; or `check` and
    /// `watch`, which run `script`.
    #[serde(default)]
    pub kind: String,
    /// The brief of every job; a job kind needs it unless `skill` is set.
    #[serde(default)]
    pub brief: String,
    /// A `pulse skill` name every job loads first and follows: one of the
    /// repository's own `SKILL.md` folders or any enabled registry skill.
    #[serde(default)]
    pub skill: String,
    /// `check`/`watch`: the script, relative to the repository root.
    #[serde(default)]
    pub script: String,
    /// `check`/`watch`: the kind of job a red check or a firing watch
    /// queues, `task` (the default) or `improve`.
    #[serde(default)]
    pub job_kind: String,
    /// 0 or 1; 1 when absent.
    #[serde(default = "default_tier")]
    pub tier: i64,
    #[serde(default)]
    pub priority: i64,
    /// The custom agent the jobs run as (`pulse agents list`).
    #[serde(default)]
    pub run_as: String,
    /// End after this many jobs; 0 keeps going.
    #[serde(default)]
    pub times: i64,
    /// `true` keeps the schedule in the store but asleep.
    #[serde(default)]
    pub paused: bool,
}

fn default_tier() -> i64 {
    1
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            every: String::new(),
            at: String::new(),
            kind: String::new(),
            brief: String::new(),
            skill: String::new(),
            script: String::new(),
            job_kind: String::new(),
            tier: 1,
            priority: 0,
            run_as: String::new(),
            times: 0,
            paused: false,
        }
    }
}

impl Entry {
    /// The schedule this entry asks for, for `project`, set by `author`.
    pub fn input(&self, project: &str, path: &str, author: &str) -> ScheduleInput {
        ScheduleInput {
            project: project.to_string(),
            title: if self.title.trim().is_empty() {
                self.name.clone()
            } else {
                self.title.clone()
            },
            kind: self.kind.trim().to_string(),
            brief: self.brief.trim_end().to_string(),
            tier: clamp_i32(self.tier),
            priority: clamp_i32(self.priority),
            every: self.every.trim().to_string(),
            at: self.at.trim().to_string(),
            times: clamp_i32(self.times),
            author: author.to_string(),
            script: self.script.trim().to_string(),
            job_kind: self.job_kind.trim().to_string(),
            agent: self.run_as.trim().to_string(),
            skill: self.skill.trim().to_string(),
            origin: origin_of(path, &self.name),
        }
    }
}

fn clamp_i32(n: i64) -> i32 {
    n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The `origin` of a schedule the file keeps: `repo:<path>#<name>`.
pub fn origin_of(path: &str, name: &str) -> String {
    format!("{}{path}#{name}", pulse_store::REPO_ORIGIN)
}

/// The entry name in a schedule's `origin`, when the file keeps it.
pub fn origin_name(origin: &str) -> Option<&str> {
    origin
        .strip_prefix(pulse_store::REPO_ORIGIN)?
        .split_once('#')
        .map(|(_, name)| name)
}

/// Reads the file. The error names the line when the YAML is at fault and
/// the key when a field is unknown or of the wrong shape.
pub fn parse(text: &str) -> Result<Manifest, String> {
    if text.trim().is_empty() {
        return Ok(Manifest::default());
    }
    serde_saphyr::from_str::<Manifest>(text).map_err(|e| e.to_string().trim().to_string())
}

/// What is wrong with a file before any store is asked: names, kinds,
/// cadences, the fields each kind needs. Empty when the file is sound.
pub fn static_problems(m: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(needs) = &m.needs
        && let Err(e) = pulse_store::needs::parse(&needs.join(","))
    {
        out.push(format!("needs: {e}"));
    }
    let mut seen = BTreeSet::new();
    for (i, e) in m.schedules.iter().enumerate() {
        let name = e.name.trim();
        let label = if name.is_empty() {
            format!("schedule {}", i + 1)
        } else {
            format!("schedule `{name}`")
        };
        if name.is_empty() {
            out.push(format!(
                "{label}: needs a name (a slug: lowercase letters, digits, dashes)"
            ));
        } else if !pulse_store::is_slug(name) {
            out.push(format!(
                "{label}: the name is not a slug (lowercase letters, digits and dashes, starting with a letter or digit)"
            ));
        } else if !seen.insert(name.to_string()) {
            out.push(format!(
                "{label}: the name is used twice; each schedule needs its own"
            ));
        }
        let kind = if e.kind.trim().is_empty() {
            "task"
        } else {
            e.kind.trim()
        };
        let script_kind = schedules::SCRIPT_KINDS.contains(&kind);
        if !script_kind && !schedules::SCHEDULE_KINDS.contains(&kind) {
            out.push(format!(
                "{label}: kind {kind:?} is not one of {} (jobs) or {} (scripts)",
                schedules::SCHEDULE_KINDS.join(", "),
                schedules::SCRIPT_KINDS.join(", ")
            ));
        }
        if script_kind {
            if e.script.trim().is_empty() {
                out.push(format!(
                    "{label}: a {kind} needs `script`, the path of the script in the repository"
                ));
            } else if let Err(err) = schedules::validate_script_path(&e.script) {
                out.push(format!("{label}: {}", first_line(&err.to_string())));
            }
            let job_kind = e.job_kind.trim();
            if !job_kind.is_empty() && !schedules::SCRIPT_JOB_KINDS.contains(&job_kind) {
                out.push(format!(
                    "{label}: job_kind {job_kind:?} is not one of {}",
                    schedules::SCRIPT_JOB_KINDS.join(", ")
                ));
            }
            if !e.skill.trim().is_empty() {
                out.push(format!(
                    "{label}: `skill` is for a job kind; a {kind} runs its script"
                ));
            }
        } else {
            if !e.script.trim().is_empty() {
                out.push(format!(
                    "{label}: `script` is for a check or a watch, not a {kind}"
                ));
            }
            if !e.job_kind.trim().is_empty() {
                out.push(format!(
                    "{label}: `job_kind` is for a check or a watch, not a {kind}"
                ));
            }
            if e.brief.trim().is_empty() && e.skill.trim().is_empty() {
                out.push(format!(
                    "{label}: needs a `brief` (what each job does) or a `skill` to follow"
                ));
            }
        }
        if !e.skill.trim().is_empty() && !pulse_store::is_slug(e.skill.trim()) {
            out.push(format!(
                "{label}: skill {:?} is not a skill name (a slug)",
                e.skill.trim()
            ));
        }
        if e.tier != 0 && e.tier != 1 {
            out.push(format!("{label}: tier {} is not 0 or 1", e.tier));
        }
        if e.times < 0 {
            out.push(format!("{label}: times counts jobs; 0 means no end"));
        }
        if e.title.trim().len() > 200 {
            out.push(format!("{label}: the title is over 200 characters"));
        }
        if e.every.trim().is_empty() {
            out.push(format!(
                "{label}: needs `every`, the cadence (30m, 6h, 1d, 2w, hourly, daily, weekly)"
            ));
        } else {
            match schedules::parse_every(&e.every) {
                Err(err) => out.push(format!("{label}: {}", first_line(&err.to_string()))),
                Ok(every) => {
                    match schedules::parse_at(&e.at) {
                        Err(err) => out.push(format!("{label}: {}", first_line(&err.to_string()))),
                        Ok(Some(_)) if every % schedules::MINUTES_PER_DAY != 0 => {
                            out.push(format!("{label}: `at` needs a cadence of whole days (1d, 2w, daily, weekly)"));
                        }
                        Ok(_) => {}
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// What the daemon last applied for a project, kept in the store's `meta`
/// table under `manifest:<slug>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Status {
    /// The fingerprint of the trees the file and the skills live in.
    #[serde(default)]
    pub fingerprint: String,
    /// The default branch's commit when the daemon read it.
    #[serde(default)]
    pub commit: String,
    /// The file that was read; empty when the repository has none.
    #[serde(default)]
    pub path: String,
    /// When the daemon last applied it.
    #[serde(default)]
    pub at: String,
    /// The problems it reported then.
    #[serde(default)]
    pub problems: Vec<String>,
    /// Entries the file named then, by name.
    #[serde(default)]
    pub schedules: Vec<String>,
    /// Skills the repository kept then, by name.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// The status of a project's file, when the daemon has applied one.
pub fn status(s: &pulse_store::Store, slug: &str) -> Result<Option<Status>> {
    let (value, found) = s.meta(&format!("{META_PREFIX}{slug}"))?;
    if !found {
        return Ok(None);
    }
    Ok(serde_json::from_str(&value).ok())
}

fn write_status(s: &pulse_store::Store, slug: &str, st: &Status) -> Result<()> {
    let conn = s.conn()?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![format!("{META_PREFIX}{slug}"), serde_json::to_string(st)?],
    )?;
    Ok(())
}

/// How many rows one sync touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub added: usize,
    pub updated: usize,
    pub kept: usize,
    /// Schedules: ended; skills: disabled.
    pub removed: usize,
    pub paused: usize,
    pub resumed: usize,
}

impl Counts {
    /// `2 added, 1 updated, 3 kept`; empty when nothing was.
    pub fn summary(&self, removed: &str) -> String {
        let mut parts = Vec::new();
        for (n, word) in [
            (self.added, "added"),
            (self.updated, "updated"),
            (self.kept, "kept"),
            (self.paused, "paused"),
            (self.resumed, "resumed"),
            (self.removed, removed),
        ] {
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        parts.join(", ")
    }
}

/// What one sync of a project did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Synced {
    pub project: String,
    /// The file read; empty when the repository has none on the branch.
    pub path: String,
    /// The default branch and its commit (short).
    pub branch: String,
    pub commit: String,
    /// Nothing changed since the last sync, so nothing was read or written.
    pub skipped: bool,
    pub schedules: Counts,
    pub skills: Counts,
    pub problems: Vec<String>,
    /// The last sync had problems too (the daemon posts on the change).
    pub had_problems: bool,
    /// The project's needs when the file's `needs:` changed them.
    pub needs: Option<String>,
}

impl Synced {
    /// One line for the daemon's log and `pulse manifest sync`.
    pub fn line(&self) -> String {
        if self.skipped {
            return format!(
                "manifest {}: unchanged since {}",
                self.project,
                if self.commit.is_empty() {
                    "the last sync".to_string()
                } else {
                    self.commit.clone()
                }
            );
        }
        let mut line = format!("manifest {}: ", self.project);
        if self.path.is_empty() {
            line.push_str(&format!("no .pulse/pulse.yaml on {}", self.branch));
        } else {
            line.push_str(&format!("{} at {}", self.path, self.commit));
        }
        let sched = self.schedules.summary("ended");
        if !sched.is_empty() {
            line.push_str(&format!("; schedules: {sched}"));
        }
        let skills = self.skills.summary("disabled");
        if !skills.is_empty() {
            line.push_str(&format!("; skills: {skills}"));
        }
        if let Some(needs) = &self.needs {
            if needs.is_empty() {
                line.push_str("; needs cleared");
            } else {
                line.push_str(&format!("; needs {needs}"));
            }
        }
        if !self.problems.is_empty() {
            line.push_str(&format!(
                "; {} problem{}: {}",
                self.problems.len(),
                if self.problems.len() == 1 { "" } else { "s" },
                self.problems.join(" | ")
            ));
        }
        line
    }
}

/// A git command in `repo`, read-only, with PULSE's clean environment.
fn git(repo: &str, args: &[&str]) -> Option<(bool, Vec<u8>)> {
    let mut cmd = std::process::Command::new(pulse_gitenv::git_program());
    cmd.arg("-C").arg(repo).args(args);
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output().ok()?;
    Some((out.status.success(), out.stdout))
}

fn git_text(repo: &str, args: &[&str]) -> Option<String> {
    let (ok, out) = git(repo, args)?;
    ok.then(|| String::from_utf8_lossy(&out).trim_end().to_string())
}

/// A blob of the branch as text; `None` when there is none, `Some(Err)`
/// when it is too big or not text.
fn blob(repo: &str, branch: &str, path: &str) -> Option<Result<String, String>> {
    let (ok, out) = git(repo, &["show", &format!("{branch}:{path}")])?;
    if !ok {
        return None;
    }
    if out.len() > FILE_LIMIT {
        return Some(Err(format!(
            "{path} is {} KB; the limit is {} KB",
            out.len() / 1024,
            FILE_LIMIT / 1024
        )));
    }
    Some(match String::from_utf8(out) {
        Ok(text) => Ok(text),
        Err(_) => Err(format!("{path} is not UTF-8 text")),
    })
}

/// The repository and branch of a project the daemon can read: `None` when
/// the project has no checkout here or the branch is not in it.
pub fn repo_of(s: &pulse_store::Store, slug: &str) -> Result<Option<(String, String, String)>> {
    let Some(project) = s.project(slug)? else {
        return Err(crate::tasks::input(
            2,
            format!("No project {slug:?}"),
            "it is not registered in this store",
            "pulse projects",
        ));
    };
    let repo = project.path.trim().to_string();
    if repo.is_empty() || !std::path::Path::new(&repo).is_dir() {
        return Ok(None);
    }
    let branch = if project.default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        project.default_branch.trim().to_string()
    };
    let Some(commit) = git_text(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch}^{{commit}}"),
        ],
    ) else {
        return Ok(None);
    };
    if commit.is_empty() {
        return Ok(None);
    }
    Ok(Some((repo, branch, commit)))
}

/// A fingerprint of the trees the file and the skill folders live in on
/// `branch`: equal while no commit changed any of them.
pub fn fingerprint(repo: &str, branch: &str) -> Option<String> {
    let mut args = vec!["ls-tree", branch, "--"];
    args.push(".pulse");
    args.extend(
        SKILL_DIRS
            .iter()
            .filter(|d| !d.starts_with(".pulse/"))
            .copied(),
    );
    let (ok, out) = git(repo, &args)?;
    if !ok {
        return None;
    }
    Some(hex::encode(Sha256::digest(&out)))
}

/// A `SKILL.md` the repository keeps, read from the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSkill {
    pub name: String,
    pub path: String,
    pub description: String,
    pub body: String,
}

/// The skills of the branch: every `<dir>/<folder>/SKILL.md` under
/// [`SKILL_DIRS`], the first folder of a name winning; problems name the
/// files that could not be read or do not pass as skills.
pub fn repo_skills(repo: &str, branch: &str) -> (Vec<RepoSkill>, Vec<String>) {
    let mut args = vec!["ls-tree", "-r", "--name-only", branch, "--"];
    args.extend(SKILL_DIRS.iter().copied());
    let Some(listing) = git_text(repo, &args) else {
        return (Vec::new(), Vec::new());
    };
    // folder path -> (dir order, folder name, support files)
    let mut folders: BTreeMap<String, (usize, String, Vec<String>)> = BTreeMap::new();
    let mut has_skill: BTreeSet<String> = BTreeSet::new();
    for path in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((order, rest)) = SKILL_DIRS
            .iter()
            .enumerate()
            .find_map(|(i, d)| path.strip_prefix(&format!("{d}/")).map(|r| (i, r)))
        else {
            continue;
        };
        let Some((folder, file)) = rest.split_once('/') else {
            continue;
        };
        let folder_path = format!("{}/{folder}", SKILL_DIRS[order]);
        let entry = folders
            .entry(folder_path.clone())
            .or_insert_with(|| (order, folder.to_string(), Vec::new()));
        if file == "SKILL.md" {
            has_skill.insert(folder_path);
        } else {
            entry.2.push(file.to_string());
        }
    }
    let mut list: Vec<(usize, String, String, Vec<String>)> = folders
        .into_iter()
        .filter(|(p, _)| has_skill.contains(p))
        .map(|(p, (order, folder, files))| (order, folder, p, files))
        .collect();
    list.sort();
    let mut out: Vec<RepoSkill> = Vec::new();
    let mut problems = Vec::new();
    let mut taken: BTreeMap<String, String> = BTreeMap::new();
    for (_, folder, folder_path, mut files) in list {
        let path = format!("{folder_path}/SKILL.md");
        let text = match blob(repo, branch, &path) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                problems.push(format!("skill {path}: {e}"));
                continue;
            }
            None => continue,
        };
        let (front, body) = split_front_matter(&text);
        let mut name = front_value(front, "name").unwrap_or_default();
        if name.is_empty() {
            name = folder.clone();
        }
        if !pulse_store::is_slug(&name) {
            problems.push(format!(
                "skill {path}: the name {name:?} is not a slug (lowercase letters, digits and dashes); set `name:` in its front matter"
            ));
            continue;
        }
        if let Some(first) = taken.get(&name) {
            problems.push(format!(
                "skill {path}: {first} already gives the skill {name}; the first folder wins"
            ));
            continue;
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
        description = description.split_whitespace().collect::<Vec<_>>().join(" ");
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
        if description.is_empty() {
            description = format!("The {name} skill of this repository ({folder_path})");
        }
        let mut body = body.trim().to_string();
        if body.is_empty() {
            problems.push(format!(
                "skill {path}: the file has no text below its front matter"
            ));
            continue;
        }
        files.sort();
        if !files.is_empty() {
            let shown: Vec<&str> = files
                .iter()
                .take(SUPPORT_FILES_LIMIT)
                .map(String::as_str)
                .collect();
            let more = files.len().saturating_sub(shown.len());
            body.push_str(&format!(
                "\n\n---\nSupport files beside this SKILL.md in the repository, under `{folder_path}/`: {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        taken.insert(name.clone(), path.clone());
        out.push(RepoSkill {
            name,
            path,
            description,
            body,
        });
    }
    (out, problems)
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

/// The file of the branch: its path and text, or the problem that keeps it
/// from being read. `Ok(None)` when the repository has none.
fn read_manifest(repo: &str, branch: &str) -> Result<Option<(String, String)>, String> {
    let present: Vec<&str> = MANIFEST_PATHS
        .iter()
        .copied()
        .filter(|p| {
            matches!(
                git_text(repo, &["cat-file", "-t", &format!("{branch}:{p}")]).as_deref(),
                Some("blob")
            )
        })
        .collect();
    let Some(path) = present.first() else {
        return Ok(None);
    };
    match blob(repo, branch, path) {
        Some(Ok(text)) => Ok(Some(((*path).to_string(), text))),
        Some(Err(e)) => Err(e),
        None => Err(format!("{path} could not be read from {branch}")),
    }
}

/// Applies the project's file and skill folders to the store. Reads
/// nothing and writes nothing when no commit changed them since the last
/// sync, unless `force`. `Ok(None)` when the project has no checkout the
/// daemon can read here.
pub fn sync(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    force: bool,
) -> Result<Option<Synced>> {
    let Some((repo, branch, commit)) = repo_of(s, slug)? else {
        return Ok(None);
    };
    let short: String = commit.chars().take(12).collect();
    let before = status(s, slug)?.unwrap_or_default();
    let print = fingerprint(&repo, &branch).unwrap_or_default();
    if !force && !print.is_empty() && print == before.fingerprint {
        return Ok(Some(Synced {
            project: slug.to_string(),
            path: before.path.clone(),
            branch,
            commit: short,
            skipped: true,
            problems: before.problems.clone(),
            had_problems: !before.problems.is_empty(),
            ..Default::default()
        }));
    }
    let mut synced = Synced {
        project: slug.to_string(),
        branch: branch.clone(),
        commit: short.clone(),
        had_problems: !before.problems.is_empty(),
        ..Default::default()
    };
    let now = s.now();

    // Skills first: a schedule may name one of them.
    let (skills, mut skill_problems) = repo_skills(&repo, &branch);
    let skill_names = sync_skills(
        s,
        slug,
        actor,
        &skills,
        &mut synced.skills,
        &mut skill_problems,
    )?;
    synced.problems.extend(skill_problems);

    let mut names = Vec::new();
    match read_manifest(&repo, &branch) {
        Err(problem) => synced.problems.push(problem),
        Ok(None) => {
            // No file: whatever it kept before ends.
            end_missing(s, slug, actor, &BTreeSet::new(), &mut synced.schedules)?;
        }
        Ok(Some((path, text))) => {
            synced.path = path.clone();
            if MANIFEST_PATHS.iter().skip(1).any(|p| {
                matches!(
                    git_text(&repo, &["cat-file", "-t", &format!("{branch}:{p}")]).as_deref(),
                    Some("blob")
                )
            }) && path == MANIFEST_PATHS[0]
            {
                synced.problems.push(format!(
                    "both {} and {} exist; {} is read, remove the other",
                    MANIFEST_PATHS[0], MANIFEST_PATHS[1], MANIFEST_PATHS[0]
                ));
            }
            match parse(&text) {
                Err(e) => synced.problems.push(format!(
                    "{path} could not be read, so its schedules stay as they were: {e}"
                )),
                Ok(m) => {
                    names = m
                        .schedules
                        .iter()
                        .map(|e| e.name.trim().to_string())
                        .collect();
                    let problems = static_problems(&m);
                    let held: BTreeSet<String> = problems
                        .iter()
                        .filter_map(|p| {
                            p.strip_prefix("schedule `")
                                .and_then(|r| r.split_once('`'))
                                .map(|(n, _)| n.to_string())
                        })
                        .collect();
                    synced.problems.extend(problems);
                    sync_needs(s, slug, actor, &m, &mut synced)?;
                    sync_schedules(s, slug, actor, &path, &m, &held, &mut synced)?;
                }
            }
        }
    }

    let after = Status {
        fingerprint: print,
        commit: short,
        path: synced.path.clone(),
        at: now.clone(),
        problems: synced.problems.clone(),
        schedules: names,
        skills: skill_names,
    };
    write_status(s, slug, &after)?;
    let tx = s.begin()?;
    tx.event(
        actor,
        "manifest.synced",
        slug,
        pulse_store::Detail::json(json!({
            "path": synced.path,
            "branch": synced.branch,
            "commit": synced.commit,
            "schedules": synced.schedules,
            "skills": synced.skills,
            "problems": synced.problems,
        }))?,
    )?;
    tx.commit()?;
    Ok(Some(synced))
}

/// Sets the project's needs from the file's `needs:` when the key is there
/// and sound; a file without the key leaves them.
fn sync_needs(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    m: &Manifest,
    synced: &mut Synced,
) -> Result<()> {
    let Some(needs) = &m.needs else {
        return Ok(());
    };
    let Ok(words) = pulse_store::needs::canonical(&needs.join(",")) else {
        return Ok(());
    };
    let was: String =
        s.conn()?
            .query_row("SELECT needs FROM projects WHERE slug = ?", [slug], |r| {
                r.get(0)
            })?;
    if words != was {
        let set = if words.is_empty() {
            "-"
        } else {
            words.as_str()
        };
        synced.needs = Some(pulse_board::set_project_needs(s, slug, set, actor)?);
    }
    Ok(())
}

/// Imports the repository's skills into the project's registry; returns
/// their names.
fn sync_skills(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    skills: &[RepoSkill],
    counts: &mut Counts,
    problems: &mut Vec<String>,
) -> Result<Vec<String>> {
    let mine: Vec<pulse_store::Skill> = s
        .skills(slug)?
        .into_iter()
        .filter(|k| k.project == slug && k.from_repo())
        .collect();
    let mut names = Vec::new();
    for k in skills {
        names.push(k.name.clone());
        let origin = format!("{}{}", pulse_store::REPO_ORIGIN, k.path);
        if let Some(row) = mine.iter().find(|r| r.name == k.name) {
            let same =
                row.description == k.description && row.body == k.body && row.origin == origin;
            if !same {
                match s.update_skill_from_repo(
                    actor,
                    &row.id,
                    pulse_store::SkillPatch {
                        description: Some(k.description.clone()),
                        body: Some(k.body.clone()),
                        origin: Some(origin.clone()),
                        ..Default::default()
                    },
                ) {
                    Ok(_) => counts.updated += 1,
                    Err(e) => {
                        problems.push(format!("skill {}: {}", k.path, first_line(&e.to_string())));
                        continue;
                    }
                }
            }
            if row.status != "enabled" {
                s.set_skill_status(actor, &row.id, "enabled")?;
                counts.resumed += 1;
            } else if same {
                counts.kept += 1;
            }
            continue;
        }
        if let Some(other) = s.skill_in(slug, &k.name)? {
            problems.push(format!(
                "skill {}: {} already uses the name {} ({}); rename one of them",
                k.path,
                other.id,
                k.name,
                if other.from_repo() {
                    format!(
                        "kept by {}",
                        other.origin.trim_start_matches(pulse_store::REPO_ORIGIN)
                    )
                } else if other.project.is_empty() {
                    format!("added for every project by {}", other.created_by)
                } else {
                    format!("added for {} by {}", other.project, other.created_by)
                }
            ));
            continue;
        }
        match s.add_skill(
            actor,
            pulse_store::SkillInput {
                name: k.name.clone(),
                project: slug.to_string(),
                description: k.description.clone(),
                body: k.body.clone(),
                origin,
            },
        ) {
            Ok(_) => counts.added += 1,
            Err(e) => problems.push(format!("skill {}: {}", k.path, first_line(&e.to_string()))),
        }
    }
    for row in &mine {
        if row.status == "enabled" && !skills.iter().any(|k| k.name == row.name) {
            s.set_skill_status(actor, &row.id, "disabled")?;
            counts.removed += 1;
        }
    }
    Ok(names)
}

/// The live schedules the file keeps for a project, by entry name.
fn live_rows(s: &pulse_store::Store, slug: &str) -> Result<BTreeMap<String, schedules::Schedule>> {
    let mut out = BTreeMap::new();
    for row in schedules::list(s, slug, "")? {
        if row.status == "ended" {
            continue;
        }
        if let Some(name) = origin_name(&row.origin) {
            out.entry(name.to_string()).or_insert(row);
        }
    }
    Ok(out)
}

fn sync_schedules(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    path: &str,
    m: &Manifest,
    held: &BTreeSet<String>,
    synced: &mut Synced,
) -> Result<()> {
    let live = live_rows(s, slug)?;
    let mut keep: BTreeSet<String> = held.clone();
    for e in &m.schedules {
        let name = e.name.trim().to_string();
        if held.contains(&name) {
            continue;
        }
        keep.insert(name.clone());
        let input = e.input(slug, path, actor);
        let row = match live.get(&name) {
            Some(row) => match schedules::update_from_file(s, &row.id, &input, actor) {
                Ok((row, changed, warning)) => {
                    if !warning.is_empty() {
                        synced
                            .problems
                            .push(format!("schedule `{name}`: {warning}"));
                    }
                    if changed.is_empty() {
                        synced.schedules.kept += 1;
                    } else {
                        synced.schedules.updated += 1;
                    }
                    row
                }
                Err(err) => {
                    synced.problems.push(format!(
                        "schedule `{name}`: {} (the schedule stays as it was)",
                        first_line(&err.to_string())
                    ));
                    continue;
                }
            },
            None => match schedules::add_checked(s, input) {
                Ok((row, warning)) => {
                    if !warning.is_empty() {
                        synced
                            .problems
                            .push(format!("schedule `{name}`: {warning}"));
                    }
                    synced.schedules.added += 1;
                    row
                }
                Err(err) => {
                    synced.problems.push(format!(
                        "schedule `{name}`: {}",
                        first_line(&err.to_string())
                    ));
                    continue;
                }
            },
        };
        if e.paused && row.status == "active" {
            schedules::set_status_from_file(s, &row.id, "paused", PAUSED_NOTE, actor)?;
            synced.schedules.paused += 1;
        } else if !e.paused && row.status == "paused" && row.note == PAUSED_NOTE {
            schedules::set_status_from_file(s, &row.id, "active", "", actor)?;
            synced.schedules.resumed += 1;
        }
    }
    end_missing(s, slug, actor, &keep, &mut synced.schedules)
}

/// Ends every live schedule the file keeps whose name is not in `keep`.
fn end_missing(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    keep: &BTreeSet<String>,
    counts: &mut Counts,
) -> Result<()> {
    for (name, row) in live_rows(s, slug)? {
        if keep.contains(&name) {
            continue;
        }
        schedules::set_status_from_file(s, &row.id, "ended", ENDED_NOTE, actor)?;
        counts.removed += 1;
    }
    Ok(())
}
