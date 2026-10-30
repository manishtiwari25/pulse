use crate::mcp_servers::{is_slug, next_id};
use crate::store::{Store, input_err};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A recipe one worker wrote for the next: a setup, a debugging path, a
/// check. Shared with every agent of the project (or of every project), so
/// it never holds a credential.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub project: String,
    pub description: String,
    pub body: String,
    pub status: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub uses: i64,
    /// Empty for a skill added by hand or by an agent; `repo:<path>` for one
    /// a `SKILL.md` of the project keeps (the daemon loads it from the
    /// default branch and re-applies the file whenever it changes).
    #[serde(default)]
    pub origin: String,
}

impl Skill {
    pub fn enabled(&self) -> bool {
        self.status == "enabled"
    }
    /// Whether a file of the project's repository keeps this skill.
    pub fn from_repo(&self) -> bool {
        self.origin.starts_with(REPO_ORIGIN)
    }
    /// The `SKILL.md` path the repository keeps this skill in; empty for a
    /// skill added here.
    pub fn repo_path(&self) -> &str {
        self.origin.strip_prefix(REPO_ORIGIN).unwrap_or("")
    }
}

/// The prefix of an `origin` a repository file keeps: `repo:<path>`.
pub const REPO_ORIGIN: &str = "repo:";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SkillInput {
    pub name: String,
    pub project: String,
    pub description: String,
    pub body: String,
    /// `repo:<path>` when a file of the project keeps the skill; empty
    /// otherwise.
    #[serde(default)]
    pub origin: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SkillPatch {
    pub name: Option<String>,
    pub project: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    /// The daemon sets `repo:<path>` when a file of the project keeps the
    /// skill; `pulse skill update` leaves it alone.
    #[serde(default)]
    pub origin: Option<String>,
}

const COLS: &str = "id, name, project, description, body, status, created_by, created_at, updated_at, uses, origin";
const MAX_BODY: usize = 64 * 1024;

fn scan(r: &rusqlite::Row<'_>) -> rusqlite::Result<Skill> {
    Ok(Skill {
        id: r.get(0)?,
        name: r.get(1)?,
        project: r.get(2)?,
        description: r.get(3)?,
        body: r.get(4)?,
        status: r.get(5)?,
        created_by: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        uses: r.get(9)?,
        origin: r.get(10)?,
    })
}

fn validate(s: &Store, k: &Skill) -> Result<()> {
    if !is_slug(&k.name) {
        return Err(input_err(format!(
            "Invalid name {:?}: lowercase letters, digits and dashes, starting with a letter or digit",
            k.name
        )));
    }
    if k.description.is_empty() {
        return Err(input_err(
            "Invalid description: one line saying what the skill covers is needed",
        ));
    }
    if k.description.contains('\n') || k.description.chars().count() > 200 {
        return Err(input_err(
            "Invalid description: one line of at most 200 characters (the body holds the rest)",
        ));
    }
    if k.body.trim().is_empty() {
        return Err(input_err(
            "Invalid body: the skill text is needed (--file, --body or stdin)",
        ));
    }
    if k.body.len() > MAX_BODY {
        return Err(input_err(format!(
            "Invalid body: {} bytes; keep a skill under {} KB and link to the rest",
            k.body.len(),
            MAX_BODY / 1024
        )));
    }
    for (what, text) in [("description", &k.description), ("body", &k.body)] {
        if pulse_cred::is_private_key(text.as_bytes())
            || text.split_whitespace().any(crate::secret_shaped)
        {
            return Err(input_err(format!(
                "Invalid {what}: that looks like a credential; skills are shared with every agent, so name where a secret lives (a pulse_cred reference, an env var) instead of pasting it"
            )));
        }
    }
    if !k.project.is_empty()
        && k.project != crate::store::COMMONS_SLUG
        && s.project(&k.project)?.is_none()
    {
        return Err(input_err(format!(
            "No project {}; pass an existing slug (see pulse projects) or none for every project",
            k.project
        )));
    }
    Ok(())
}

impl Store {
    pub fn add_skill(&self, actor: &str, input: SkillInput) -> Result<Skill> {
        let now = self.now();
        let k = Skill {
            id: String::new(),
            name: input.name.trim().to_string(),
            project: input.project.trim().to_string(),
            description: input.description.trim().to_string(),
            body: input.body.trim_end().to_string(),
            status: "enabled".to_string(),
            created_by: actor.to_string(),
            created_at: now.clone(),
            updated_at: now,
            uses: 0,
            origin: input.origin.trim().to_string(),
        };
        validate(self, &k)?;
        let tx = self.begin()?;
        let taken: Option<String> = tx
            .query_row("SELECT id FROM skills WHERE name = ?", [&k.name], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = taken {
            return Err(input_err(format!(
                "Invalid name {}: {id} already uses it; pick another or update that one (pulse skill update {})",
                k.name, k.name
            )));
        }
        let id = next_id(&tx, "skills", "SKILL-")?;
        tx.execute(
            "INSERT INTO skills (id, name, project, description, body, status, created_by, created_at, updated_at, origin) VALUES (?, ?, ?, ?, ?, 'enabled', ?, ?, ?, ?)",
            params![
                id,
                k.name,
                k.project,
                k.description,
                k.body,
                k.created_by,
                k.created_at,
                k.updated_at,
                k.origin
            ],
        )?;
        tx.event(
            actor,
            "skill.added",
            &id,
            crate::store::Detail::json(json!({
                "name": k.name,
                "project": k.project,
                "description": k.description,
                "origin": k.origin,
            }))?,
        )?;
        tx.commit()?;
        Ok(Skill { id, ..k })
    }

    pub fn update_skill(&self, actor: &str, id: &str, patch: SkillPatch) -> Result<Skill> {
        let old = self.skill_by_id(id)?;
        if old.from_repo()
            && (patch.name.is_some()
                || patch.project.is_some()
                || patch.description.is_some()
                || patch.body.is_some())
        {
            return Err(input_err(format!(
                "{} is kept by the repository: edit {} on the project's default branch and commit; the daemon loads that file and would write over a change made here",
                old.id,
                old.repo_path()
            )));
        }
        self.update_skill_inner(actor, old, patch)
    }

    /// The file sync's update: the one writer that changes a skill the
    /// repository keeps.
    pub fn update_skill_from_repo(
        &self,
        actor: &str,
        id: &str,
        patch: SkillPatch,
    ) -> Result<Skill> {
        let old = self.skill_by_id(id)?;
        self.update_skill_inner(actor, old, patch)
    }

    fn update_skill_inner(&self, actor: &str, old: Skill, patch: SkillPatch) -> Result<Skill> {
        let mut k = old.clone();
        if let Some(v) = patch.name {
            k.name = v.trim().to_string();
        }
        if let Some(v) = patch.project {
            k.project = v.trim().to_string();
        }
        if let Some(v) = patch.description {
            k.description = v.trim().to_string();
        }
        if let Some(v) = patch.body {
            k.body = v.trim_end().to_string();
        }
        if let Some(v) = patch.origin {
            k.origin = v.trim().to_string();
        }
        validate(self, &k)?;
        let mut changed = Vec::new();
        for (field, differs) in [
            ("name", k.name != old.name),
            ("project", k.project != old.project),
            ("description", k.description != old.description),
            ("body", k.body != old.body),
            ("origin", k.origin != old.origin),
        ] {
            if differs {
                changed.push(field);
            }
        }
        if changed.is_empty() {
            return Ok(old);
        }
        k.updated_at = self.now();
        let tx = self.begin()?;
        let taken: Option<String> = tx
            .query_row(
                "SELECT id FROM skills WHERE name = ? AND id <> ?",
                params![k.name, k.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = taken {
            return Err(input_err(format!(
                "Invalid name {}: {other} already uses it; pick another",
                k.name
            )));
        }
        tx.execute(
            "UPDATE skills SET name = ?, project = ?, description = ?, body = ?, origin = ?, updated_at = ? WHERE id = ?",
            params![k.name, k.project, k.description, k.body, k.origin, k.updated_at, k.id],
        )?;
        tx.event(
            actor,
            "skill.updated",
            &k.id,
            crate::store::Detail::json(json!({
                "name": k.name,
                "project": k.project,
                "changed": changed,
            }))?,
        )?;
        tx.commit()?;
        Ok(k)
    }

    /// Every skill when `project` is empty; otherwise the skills of every
    /// project plus that project's own, enabled or not. Global first, by name.
    pub fn skills(&self, project: &str) -> Result<Vec<Skill>> {
        let conn = self.conn()?;
        let project = project.trim();
        let (sql, params): (String, Vec<&str>) = if project.is_empty() {
            (
                format!("SELECT {COLS} FROM skills ORDER BY project <> '', project, name"),
                vec![],
            )
        } else {
            (
                format!(
                    "SELECT {COLS} FROM skills WHERE project IN ('', ?1) ORDER BY project <> '', name"
                ),
                vec![project],
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), scan)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The enabled skills a worker in `project` reads: global ones and the
    /// project's own, in name order.
    pub fn skills_for(&self, project: &str) -> Result<Vec<Skill>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM skills WHERE status = 'enabled' AND project IN ('', ?1) ORDER BY name"
        ))?;
        let rows = stmt
            .query_map([project.trim()], scan)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn skill_by_id(&self, id: &str) -> Result<Skill> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {COLS} FROM skills WHERE id = ?"),
            [id],
            scan,
        )
        .optional()?
        .ok_or_else(|| input_err(format!("No skill {id}")))
    }

    /// Finds a skill by id or name (names are unique across projects).
    pub fn skill(&self, key: &str) -> Result<Skill> {
        let key = key.trim();
        if key.starts_with("SKILL-") {
            return self.skill_by_id(key);
        }
        self.skill_named(key)?
            .ok_or_else(|| input_err(format!("No skill named {key}; see pulse skill list")))
    }

    /// The skill of that name, whatever its project or status; `None` when
    /// no row has it.
    pub fn skill_named(&self, name: &str) -> Result<Option<Skill>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {COLS} FROM skills WHERE name = ?"),
                [name.trim()],
                scan,
            )
            .optional()?)
    }

    pub fn set_skill_status(&self, actor: &str, id: &str, status: &str) -> Result<Skill> {
        if status != "enabled" && status != "disabled" {
            return Err(input_err(format!(
                "Invalid status {status:?}: enabled or disabled"
            )));
        }
        let mut k = self.skill_by_id(id)?;
        if k.status == status {
            return Ok(k);
        }
        k.status = status.to_string();
        k.updated_at = self.now();
        let tx = self.begin()?;
        tx.execute(
            "UPDATE skills SET status = ?, updated_at = ? WHERE id = ?",
            params![k.status, k.updated_at, k.id],
        )?;
        tx.event(
            actor,
            if status == "enabled" {
                "skill.enabled"
            } else {
                "skill.disabled"
            },
            &k.id,
            crate::store::Detail::json(json!({"name": k.name, "project": k.project}))?,
        )?;
        tx.commit()?;
        Ok(k)
    }

    /// Counts a read; no event, a brief's worth of reads a day is noise.
    pub fn bump_skill_uses(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("UPDATE skills SET uses = uses + 1 WHERE id = ?", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Options;

    fn new_store() -> (tempfile::TempDir, Store) {
        let _guard = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                params![home.path().join("demo").display().to_string(), s.now()],
            )
            .unwrap();
        (home, s)
    }

    fn input(name: &str, project: &str) -> SkillInput {
        SkillInput {
            name: name.into(),
            project: project.into(),
            description: format!("How to {name}"),
            body: format!("# {name}\n\n1. Run the thing.\n2. Check the output.\n"),
            origin: String::new(),
        }
    }

    fn actions(s: &Store, action: &str) -> Vec<(String, String)> {
        let conn = s.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT subject, detail FROM events WHERE action = ? ORDER BY seq")
            .unwrap();
        stmt.query_map([action], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn add_lookup_and_events() {
        let (_h, s) = new_store();
        let a = s
            .add_skill("cinder", input("release-checklist", ""))
            .unwrap();
        assert_eq!(a.id, "SKILL-001");
        assert_eq!(a.status, "enabled");
        assert_eq!(a.uses, 0);
        assert_eq!(a.created_by, "cinder");
        assert!(a.body.ends_with("Check the output."));
        let b = s
            .add_skill("human", input("debug-flaky-tests", "demo"))
            .unwrap();
        assert_eq!(b.id, "SKILL-002");
        assert_eq!(s.skill("release-checklist").unwrap(), a);
        assert_eq!(s.skill("SKILL-002").unwrap(), b);
        let err = s.skill("nothing").unwrap_err().to_string();
        assert!(err.starts_with("No skill named nothing"), "{err}");
        assert_eq!(
            s.skill_by_id("SKILL-009").unwrap_err().to_string(),
            "No skill SKILL-009"
        );
        let added = actions(&s, "skill.added");
        assert_eq!(added.len(), 2);
        assert!(added[1].1.contains("debug-flaky-tests"), "{}", added[1].1);
        let err = s
            .add_skill("human", input("release-checklist", "demo"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("SKILL-001 already uses it"), "{err}");
    }

    #[test]
    fn validation() {
        let (_h, s) = new_store();
        let err = |i: SkillInput| {
            let e = s.add_skill("human", i).unwrap_err();
            assert!(e.is::<crate::store::InputError>(), "{e}");
            e.to_string()
        };
        assert!(err(input("Bad Name", "")).contains("Invalid name"));
        assert!(err(input("x", "nope")).contains("No project nope"));
        let mut i = input("x", "");
        i.description = String::new();
        assert!(err(i).contains("Invalid description"));
        let mut i = input("x", "");
        i.description = "two\nlines".into();
        assert!(err(i).contains("one line"));
        let mut i = input("x", "");
        i.body = "  \n".into();
        assert!(err(i).contains("Invalid body"));
        let mut i = input("x", "");
        i.body = "x".repeat(MAX_BODY + 1);
        assert!(err(i).contains("KB"));
        let mut i = input("x", "");
        i.body = "export TOKEN=ghp_abcdefghijklmnopq and go".into();
        assert!(err(i).contains("looks like a credential"));
        let mut i = input("x", "");
        i.description = "-----BEGIN OPENSSH PRIVATE KEY-----".into();
        assert!(err(i).contains("looks like a credential"));
    }

    #[test]
    fn a_repository_kept_skill_changes_only_through_the_file() {
        let (_h, s) = new_store();
        let mut i = input("release", "demo");
        i.origin = format!("{REPO_ORIGIN}.pulse/skills/release/SKILL.md");
        let k = s.add_skill("daemon", i).unwrap();
        assert!(k.from_repo());
        assert_eq!(k.repo_path(), ".pulse/skills/release/SKILL.md");
        let err = s
            .update_skill(
                "human",
                &k.id,
                SkillPatch {
                    body: Some("# Mine\n".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.is::<crate::store::InputError>(), "{err}");
        assert!(
            err.to_string().starts_with(
                "SKILL-001 is kept by the repository: edit .pulse/skills/release/SKILL.md"
            ),
            "{err}"
        );
        let same = s
            .update_skill_from_repo(
                "daemon",
                &k.id,
                SkillPatch {
                    body: Some("# Release v2\n".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(same.body, "# Release v2");
        // A skill added here says so and stays editable.
        let mine = s.add_skill("human", input("mine", "")).unwrap();
        assert!(!mine.from_repo() && mine.repo_path().is_empty());
        s.update_skill(
            "human",
            &mine.id,
            SkillPatch {
                description: Some("How to do mine".into()),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn scopes_status_uses_and_update() {
        let (_h, s) = new_store();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('other', 'Other', '/x', ?)",
                [s.now()],
            )
            .unwrap();
        let g = s.add_skill("human", input("zeta", "")).unwrap();
        let d = s.add_skill("human", input("alpha", "demo")).unwrap();
        s.add_skill("human", input("gamma", "other")).unwrap();
        let names = |rows: Vec<Skill>| rows.into_iter().map(|k| k.name).collect::<Vec<_>>();
        assert_eq!(names(s.skills("").unwrap()), vec!["zeta", "alpha", "gamma"]);
        assert_eq!(names(s.skills("demo").unwrap()), vec!["zeta", "alpha"]);
        assert_eq!(names(s.skills_for("demo").unwrap()), vec!["alpha", "zeta"]);
        assert_eq!(names(s.skills_for("none").unwrap()), vec!["zeta"]);
        let off = s.set_skill_status("human", &g.id, "disabled").unwrap();
        assert_eq!(off.status, "disabled");
        assert_eq!(names(s.skills_for("demo").unwrap()), vec!["alpha"]);
        assert_eq!(names(s.skills("demo").unwrap()), vec!["zeta", "alpha"]);
        s.set_skill_status("human", &g.id, "disabled").unwrap();
        assert_eq!(actions(&s, "skill.disabled").len(), 1);
        s.set_skill_status("human", &g.id, "enabled").unwrap();
        assert_eq!(actions(&s, "skill.enabled").len(), 1);
        assert!(s.set_skill_status("human", &g.id, "gone").is_err());

        s.bump_skill_uses(&d.id).unwrap();
        s.bump_skill_uses(&d.id).unwrap();
        assert_eq!(s.skill_by_id(&d.id).unwrap().uses, 2);

        let same = s
            .update_skill("human", &d.id, SkillPatch::default())
            .unwrap();
        assert_eq!(same.uses, 2);
        assert!(actions(&s, "skill.updated").is_empty());
        let up = s
            .update_skill(
                "cinder",
                &d.id,
                SkillPatch {
                    description: Some("How to find a flaky test".into()),
                    body: Some("# alpha\n\nRun it ten times.\n".into()),
                    project: Some(String::new()),
                    ..SkillPatch::default()
                },
            )
            .unwrap();
        assert_eq!(up.description, "How to find a flaky test");
        assert_eq!(up.project, "");
        assert_eq!(up.uses, 2);
        let updated = actions(&s, "skill.updated");
        assert_eq!(updated.len(), 1);
        assert!(updated[0].1.contains("body"), "{}", updated[0].1);
        let err = s
            .update_skill(
                "human",
                &d.id,
                SkillPatch {
                    name: Some("zeta".into()),
                    ..SkillPatch::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("SKILL-001 already uses it"), "{err}");
    }
}
