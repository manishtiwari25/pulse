//! PULSE: shared project goals (port of Go package internal/goals).

use anyhow::{Result, anyhow};
use chrono::DateTime;
use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const MAX_LENGTH: usize = 4000;
pub const OWNER: &str = "owner";
pub const GENERATED: &str = "generated";
pub const GENERATED_BY: &str = "pulse:generated";
pub const KEY_PREFIX: &str = "project.goal.";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub text: String,
    pub source: String,
    pub updated_at: String,
    pub updated_by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub project: String,
    pub goal: Goal,
    pub can_edit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGoal {
    text: String,
    source: String,
}

static SLUG_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$").unwrap());
static ACTOR_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^human(?::[a-zA-Z0-9_-]{1,64})?$").unwrap());
static PRIVATE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    let private_component = r"(?:private|heldout|held-out|\.ssh|\.aws|\.azure|\.pulse)";
    Regex::new(&format!(
        r#"(?i)(?:^|[\s"'`\(\[<{{=:,;])(?:file:(?://[^/\\\s"'`<>]*)?)?(?:~[/\\]|/(?:Users|home|private|tmp|var)/|/?[a-z]:[/\\])|(?:^|[\s/\\"'`\(\[<{{=:,;]){}[/\\]|[/\\]{}(?:[/\\]|$|[\s"'`\(\)\[\]{{}}<>,;])"#,
        private_component, private_component
    ))
    .unwrap()
});

pub fn normalize(text: &str) -> Result<String> {
    fn bad<T>(why: &str) -> Result<T> {
        Err(pulse_cli::errorf(
            2,
            "Invalid project goal",
            why,
            "Use 1..4000 Unicode code points of public project direction, without credentials or private paths.",
        )
        .into())
    }

    let text = text.replace("\r\n", "\n");
    for r in text.chars() {
        if (r.is_control() && r != '\n' && r != '\t')
            || (is_unsafe_format_char(r) && r != '\u{200c}' && r != '\u{200d}')
        {
            return bad("The goal must not contain NUL or unsafe control characters.");
        }
    }
    let text = text.trim().to_string();
    let n = text.chars().count();
    if n == 0 || n > MAX_LENGTH {
        return bad("The trimmed goal must contain 1..4000 Unicode code points.");
    }
    if goal_secret_shaped(&text) || PRIVATE_PATH.is_match(&text) {
        return bad(
            "Goals are shared with every agent; secret-shaped values and private paths are not allowed.",
        );
    }
    for word in text.split_whitespace() {
        let word = word.trim_matches(
            &[
                '"', '\'', '`', '(', ')', '[', ']', '{', '}', '<', '>', ',', ';',
            ][..],
        );
        if goal_secret_shaped(word) {
            return bad("Goals are shared with every agent; secret-shaped values are not allowed.");
        }
    }
    Ok(text)
}

fn is_unsafe_format_char(r: char) -> bool {
    matches!(
        r as u32,
        0x00ad
            | 0x0600..=0x0605
            | 0x061c
            | 0x06dd
            | 0x070f
            | 0x0890..=0x0891
            | 0x08e2
            | 0x180e
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x206f
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0x110bd
            | 0x110cd
            | 0x13430..=0x1343f
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0001
            | 0xe0020..=0xe007f
    )
}

fn goal_secret_shaped(value: &str) -> bool {
    pulse_store::secret_shaped(value)
}

fn public_references(s: &pulse_store::Store, text: &str) -> Result<()> {
    let redacted = pulse_bus::redactor(Some(s))(pulse_bus::Event {
        detail: text.to_string(),
        ..pulse_bus::Event::default()
    });
    if redacted.detail != text {
        return Err(pulse_cli::errorf(
            2,
            "Project goals cannot contain owner-private paths",
            "The goal is shared with every agent; the store's existing privacy rules apply.",
            "Describe the public product outcome without referencing private inputs.",
        )
        .into());
    }
    Ok(())
}

pub fn require_owner(home: &str, actor: &str) -> Result<()> {
    let env_actor = std::env::var("PULSE_AGENT").unwrap_or_default();
    if !owner_actor(actor)
        || (!env_actor.is_empty() && !owner_actor(&env_actor))
        || pulse_store::worker_guard(home)
    {
        return Err(pulse_cli::errorf(
            2,
            "Only the owner can change a project goal",
            "Agents may read the goal and suggest changes, but cannot set or reset it.",
            "Have the owner use Project Overview or pulse goal from an owner session.",
        )
        .into());
    }
    Ok(())
}

fn owner_actor(actor: &str) -> bool {
    pulse_store::human_actor(actor) && ACTOR_PATTERN.is_match(actor) && !goal_secret_shaped(actor)
}

pub fn can_edit(s: &pulse_store::Store, p: &pulse_store::Project, actor: &str) -> bool {
    p.status == "active"
        && p.slug != pulse_store::COMMONS_SLUG
        && require_owner(&s.home, actor).is_ok()
}

fn project(conn: &rusqlite::Connection, slug: &str) -> Result<pulse_store::Project> {
    if !SLUG_PATTERN.is_match(slug) || slug == pulse_store::COMMONS_SLUG {
        return Err(pulse_cli::errorf(
            2,
            "Project goals require a registered project",
            "Goals belong to registered projects, not the commons.",
            "pulse projects",
        )
        .into());
    }
    let row = conn
        .query_row(
            "SELECT slug, name, brain, status, added_at, verify_cmd FROM projects WHERE slug = ?",
            [slug],
            |r| {
                let brain: i64 = r.get(2)?;
                Ok(pulse_store::Project {
                    slug: r.get(0)?,
                    name: r.get(1)?,
                    brain: brain != 0,
                    status: r.get(3)?,
                    added_at: r.get(4)?,
                    verify_cmd: r.get(5)?,
                    ..pulse_store::Project::default()
                })
            },
        )
        .optional()?;
    row.ok_or_else(|| {
        pulse_cli::errorf(
            2,
            &format!("No project {slug}"),
            "Only registered projects have goals.",
            "pulse projects",
        )
        .into()
    })
}

fn parse_rfc3339(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s).is_ok()
}

pub fn get(s: &pulse_store::Store, slug: &str) -> Result<Goal> {
    let conn = s.conn()?;
    let p = project(&conn, slug)?;
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT value, updated_at, updated_by FROM settings WHERE name = ?",
            [format!("{KEY_PREFIX}{slug}")],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((value, updated_at, updated_by)) = row else {
        return Ok(generated(&p));
    };
    let saved: StoredGoal = serde_json::from_str(&value)
        .map_err(|_| anyhow!("stored project goal is not valid JSON"))?;
    let text =
        normalize(&saved.text).map_err(|_| anyhow!("stored project goal text is invalid"))?;
    if text != saved.text {
        return Err(anyhow!("stored project goal text is invalid"));
    }
    public_references(s, &text)?;
    let owner = owner_actor(&updated_by);
    let valid_provenance = match saved.source.as_str() {
        OWNER => owner,
        GENERATED => owner || updated_by == GENERATED_BY,
        _ => false,
    };
    if !valid_provenance {
        return Err(anyhow!("stored project goal has invalid provenance"));
    }
    if !parse_rfc3339(&updated_at) {
        return Err(anyhow!("stored project goal has an invalid update time"));
    }
    Ok(Goal {
        text: saved.text,
        source: saved.source,
        updated_at,
        updated_by,
    })
}

fn generated(p: &pulse_store::Project) -> Goal {
    let mut name = "this project".to_string();
    for candidate in [&p.name, &p.slug] {
        let label = candidate.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Ok(text) = normalize(&label)
            && text.chars().count() <= 120
            && !label.contains(['/', '\\', '`', '$', '~'])
        {
            name = text;
            break;
        }
    }
    let mut text = if p.brain {
        format!(
            "Make {name}'s PULSE workflow help owners and agents deliver useful, evidence-backed project improvements safely and reliably."
        )
    } else {
        format!("Make {name} more useful to its users within its existing product scope.")
    };
    text.push_str(
        " Prioritize real product defects, correctness, reliability and user experience over repeated operating-layer churn. ",
    );
    text.push_str(
        "Connect each change to a concrete user outcome and measure progress with the project's existing checks.",
    );
    if !p.verify_cmd.trim().is_empty() {
        text.push_str(" Use the registered project verification command where applicable.");
    }
    text.push_str(
        " Stay within assigned tasks, accepted decisions and product-scope gates; do not invent domain requirements or bypass hard stops, approvals or constraints.",
    );
    let updated_at = if parse_rfc3339(&p.added_at) {
        p.added_at.clone()
    } else {
        "1970-01-01T00:00:00Z".to_string()
    };
    Goal {
        text,
        source: GENERATED.to_string(),
        updated_at,
        updated_by: GENERATED_BY.to_string(),
    }
}

pub fn ensure_default(s: &pulse_store::Store, slug: &str) -> Result<()> {
    save(s, slug, "", GENERATED_BY, true, true).map(|_| ())
}

pub fn set(s: &pulse_store::Store, slug: &str, text: &str, actor: &str) -> Result<Goal> {
    require_owner(&s.home, actor)?;
    let text = normalize(text)?;
    public_references(s, &text)?;
    save(s, slug, &text, actor, false, false)
}

pub fn reset(s: &pulse_store::Store, slug: &str, actor: &str) -> Result<Goal> {
    require_owner(&s.home, actor)?;
    save(s, slug, "", actor, true, false)
}

fn save(
    s: &pulse_store::Store,
    slug: &str,
    text: &str,
    actor: &str,
    reset: bool,
    only_absent: bool,
) -> Result<Goal> {
    let now = s.now();
    if !parse_rfc3339(&now) {
        return Err(anyhow!(
            "project goal updates require a valid RFC3339 store clock"
        ));
    }
    let tx = s.begin()?;
    let p = project(&tx, slug)?;
    if p.status != "active" {
        return Err(pulse_cli::errorf(
            2,
            "Cannot change an inactive project's goal",
            "Removed projects keep their goal for restoration.",
            "Restore the project before editing its goal.",
        )
        .into());
    }

    let mut goal = Goal {
        text: text.to_string(),
        source: OWNER.to_string(),
        updated_at: now,
        updated_by: actor.to_string(),
    };
    let mut action = "project.goal.set";
    if reset {
        goal.text = generated(&p).text;
        goal.source = GENERATED.to_string();
        action = "project.goal.reset";
    }
    let mut conflict = "DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by";
    if only_absent {
        conflict = "DO NOTHING";
        action = "project.goal.generated";
    }
    let value = serde_json::to_string(&StoredGoal {
        text: goal.text.clone(),
        source: goal.source.clone(),
    })?;
    let sql = format!(
        "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) ON CONFLICT(name) {conflict}"
    );
    let n = tx.execute(
        &sql,
        params![
            format!("{KEY_PREFIX}{slug}"),
            value,
            goal.updated_at,
            goal.updated_by
        ],
    )?;
    if n == 0 {
        tx.commit()?;
        return Ok(Goal::default());
    }
    let hash = Sha256::digest(goal.text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    tx.event(
        actor,
        action,
        slug,
        pulse_store::Detail::json(serde_json::json!({
            "project": slug,
            "source": goal.source,
            "text_hash": hash,
        }))?,
    )?;
    tx.commit()?;
    Ok(goal)
}

pub fn context(goal: &Goal) -> String {
    format!(
        "## Project goal\n\nSource: {}; updated by {} at {}.\n\n> {}\n\n\
Connect suggestions, votes and results to this goal and concrete evidence from the project's existing checks. \
Prioritize useful product outcomes and real defects over repeated operating-layer churn. \
The goal and board are direction, not permission: preserve the assigned task's scope, product-scope gates, \
hard stops, caps, evaluators and approval requirements. Never expand a task or change constraints to pursue a goal. \
Only the owner may set or reset the goal; suggest a change rather than writing it yourself.\n",
        goal.source,
        goal.updated_by,
        goal.updated_at,
        goal.text.replace('\n', "\n> ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_store::{Options, Record, Store};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::thread;

    const GOAL_NOW: &str = "2026-09-13T06:00:00Z";

    fn set_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn clear_goal_env(home: &Path) {
        for key in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"] {
            set_env(key, "");
        }
        set_env("PULSE_HOME", home.to_str().unwrap());
    }

    fn goal_store() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir, Store) {
        let guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        clear_goal_env(home.path());
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: GOAL_NOW.to_string(),
            ..Options::default()
        })
        .unwrap();
        (guard, home, s)
    }

    fn goal_project(s: &Store, slug: &str, name: &str, brain: bool) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, brain, verify_cmd, added_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    slug,
                    name,
                    Path::new(&s.home).join("missing-checkout").join(slug).display().to_string(),
                    if brain { 1 } else { 0 },
                    "go test ./...",
                    GOAL_NOW
                ],
            )
            .unwrap();
    }

    fn must_goal(s: &Store, slug: &str) -> Goal {
        get(s, slug).unwrap()
    }

    fn goal_snapshot(s: &Store) -> String {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT
        (SELECT count(*) FROM settings) || ':' ||
        (SELECT count(*) FROM events) || ':' ||
        (SELECT count(*) FROM records) || ':' ||
        (SELECT group_concat(value || updated_at || updated_by, '|') FROM settings ORDER BY name)",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn goal_legacy_fallback_is_meaningful_deterministic_and_read_only() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "pulse", "PULSE", true);
        goal_project(&s, "catalog", "Catalog", false);
        s.put_record(
            Record {
                id: "CTX-product".into(),
                project: "pulse".into(),
                kind: "context".into(),
                slug: "product".into(),
                ..Record::default()
            },
            BTreeMap::new(),
            "An obsolete Python documentation product.",
            "human",
            "old context",
        )
        .unwrap();
        let before = goal_snapshot(&s);
        let goal = must_goal(&s, "pulse");
        for want in [
            "PULSE workflow",
            "real product defects",
            "correctness",
            "reliability",
            "user experience",
            "concrete user outcome",
            "existing checks",
            "product-scope gates",
            "hard stops",
        ] {
            assert!(
                goal.text.contains(want),
                "generated goal lacks {want}: {}",
                goal.text
            );
        }
        assert_eq!(goal.source, GENERATED);
        assert_eq!(goal.updated_by, GENERATED_BY);
        assert_eq!(goal.updated_at, GOAL_NOW);
        assert!(!goal.text.contains(&s.home));
        assert!(!goal.text.contains("Python"));
        assert!(!goal.text.contains("go test"));
        let catalog = must_goal(&s, "catalog");
        assert!(catalog.text.contains("Catalog"));
        assert!(catalog.text.contains("existing product scope"));
        assert!(!catalog.text.contains("PULSE workflow"));
        assert_ne!(catalog, goal);
        let ro = Store::open(Options {
            home: s.home.clone(),
            read_only: true,
            now: "2030-01-01T00:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(must_goal(&ro, "pulse"), goal);
        assert_eq!(goal_snapshot(&s), before);
    }

    #[test]
    fn goal_persistence_isolation_reset_and_audit() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        goal_project(&s, "two", "Two", false);
        let def = must_goal(&s, "one");
        ensure_default(&s, "one").unwrap();
        let snapshot = goal_snapshot(&s);
        ensure_default(&s, "one").unwrap();
        assert_eq!(goal_snapshot(&s), snapshot);
        let value: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE name = ?",
                [format!("{KEY_PREFIX}one")],
                |r| r.get(0),
            )
            .unwrap();
        let fields: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&value).unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields["source"], GENERATED);
        let text = "Make offline search reliable; measure failed searches with the existing regression cases.";
        let want = set(&s, "one", &format!(" \n{text}\t "), "human:dashboard").unwrap();
        assert_eq!(want.text, text);
        assert_eq!(want.source, OWNER);
        assert_eq!(want.updated_by, "human:dashboard");
        assert_eq!(want.updated_at, GOAL_NOW);
        ensure_default(&s, "one").unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET name = 'Changed metadata', brain = 1 WHERE slug = 'one'",
                [],
            )
            .unwrap();
        assert_eq!(must_goal(&s, "one"), want);
        assert_eq!(must_goal(&s, "two").source, GENERATED);
        s.close().unwrap();
        let reopened = Store::open(Options {
            home: s.home.clone(),
            skip_records: true,
            now: GOAL_NOW.into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(must_goal(&reopened, "one"), want);
        let reset_goal = reset(&reopened, "one", "human").unwrap();
        assert_eq!(reset_goal.source, GENERATED);
        assert_eq!(reset_goal.updated_by, "human");
        assert!(reset_goal.text.contains("Changed metadata"));
        assert_ne!(reset_goal.text, def.text);
        assert_eq!(must_goal(&reopened, "one"), reset_goal);
        let conn = reopened.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT action, actor, detail FROM events WHERE action LIKE 'project.goal.%' ORDER BY seq")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut actions = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let action: String = row.get(0).unwrap();
            let actor: String = row.get(1).unwrap();
            let detail: String = row.get(2).unwrap();
            actions.push(action);
            let event: serde_json::Value = serde_json::from_str(&detail).unwrap();
            assert_eq!(event["project"], "one");
            assert!(event["source"].is_string());
            assert!(event["text_hash"].is_string());
            assert!(!actor.is_empty());
            assert!(!detail.contains(text));
            assert!(!detail.contains(&s.home));
            assert!(!detail.contains("\"text\":"));
        }
        assert_eq!(
            actions.join(","),
            "project.goal.generated,project.goal.set,project.goal.reset"
        );
    }

    #[test]
    fn goal_validation_unicode_and_public_text() {
        for text in [
            "",
            " \n\t ",
            &"界".repeat(MAX_LENGTH + 1),
            "goal\0",
            "\0goal",
            "goal\u{1b}[2J",
            "goal\rhidden",
            "goal\u{7f}",
            "\u{b}goal",
            "goal\u{85}",
            "goal\u{202e}",
            "goal\u{2066}",
            "Use ghp_syntheticvalue12345",
            "Use https://user:password@example.invalid",
            "Use -----BEGIN PRIVATE KEY-----",
            "Use AKIAABCDEFGHIJKLMNOP",
            "Use sk-syntheticvalue12345",
            &format!("/api is public but {} is opaque", "a".repeat(45)),
            "Read ~/.ssh/id_test",
            "Read /Users/example/private/file",
            "Read .pulse/private/input",
        ] {
            assert!(normalize(text).is_err(), "accepted invalid input {text:?}");
        }
        for text in [
            "🚀".repeat(MAX_LENGTH),
            "界".repeat(MAX_LENGTH),
            format!("{}ok", "a ".repeat(1999)),
            "Useful work\nmeasured by existing checks.".to_string(),
            "Reliable 👩‍💻 workflows".to_string(),
            "Improve /api/search response errors.".to_string(),
        ] {
            assert_eq!(normalize(&text).unwrap(), text);
        }
        assert_eq!(
            normalize(" \r\nFirst\r\nSecond\t ").unwrap(),
            "First\nSecond"
        );
    }

    #[test]
    fn goal_owner_guards_at_helper_and_generic_settings() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let want = set(&s, "one", "Deliver reliable search.", "human").unwrap();
        let p = s.project("one").unwrap().unwrap();
        for (name, actor, env_actor, job) in [
            ("worker", "aurora", "", ""),
            ("lead", "one-lead", "", ""),
            ("empty-actor", "", "", ""),
            ("flag-impersonation", "human", "aurora", ""),
            ("job-impersonation", "human", "human", "job-1"),
            ("job-cleared-agent", "human:dashboard", "", "job-1"),
            ("unsafe-actor", "human:/private/example", "", ""),
            (
                "secret-shaped-actor",
                "human:ghp_syntheticvalue12345",
                "",
                "",
            ),
        ] {
            set_env("PULSE_AGENT", env_actor);
            set_env("PULSE_JOB", job);
            let before = goal_snapshot(&s);
            assert!(
                set(&s, "one", "Replace owner direction.", actor).is_err(),
                "{name}"
            );
            assert!(reset(&s, "one", actor).is_err(), "{name}");
            assert!(!can_edit(&s, &p, actor), "{name}");
            assert_eq!(must_goal(&s, "one"), want);
            assert_eq!(goal_snapshot(&s), before);
        }
        set_env("PULSE_AGENT", "");
        set_env("PULSE_JOB", "");
        for actor in ["human", "aurora"] {
            assert!(
                s.set_setting(
                    actor,
                    &format!("{KEY_PREFIX}one"),
                    r#"{"text":"bypass","source":"owner"}"#
                )
                .is_err()
            );
        }
        assert!(can_edit(&s, &p, "human"));
        assert_eq!(must_goal(&s, "one"), want);
    }

    #[test]
    fn goal_scope_refusals_and_inactive_reads() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let want = set(&s, "one", "Keep saved searches correct.", "human").unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET status = 'inactive' WHERE slug = 'one'",
                [],
            )
            .unwrap();
        let before = goal_snapshot(&s);
        for slug in ["one", "missing", pulse_store::COMMONS_SLUG, "../elsewhere"] {
            assert!(set(&s, slug, "Change the direction.", "human").is_err());
            assert!(reset(&s, slug, "human").is_err());
            assert!(ensure_default(&s, slug).is_err());
        }
        assert_eq!(must_goal(&s, "one"), want);
        assert_eq!(goal_snapshot(&s), before);
        for slug in ["missing", pulse_store::COMMONS_SLUG] {
            assert!(get(&s, slug).is_err());
        }
    }

    #[test]
    fn goal_storage_errors_and_atomic_audit() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let want = set(&s, "one", "Keep successful imports reliable.", "human").unwrap();
        s.conn()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_goal_audit BEFORE INSERT ON events
        WHEN NEW.action LIKE 'project.goal.%' BEGIN SELECT RAISE(ABORT, 'goal audit failure'); END",
            )
            .unwrap();
        let before = goal_snapshot(&s);
        let err = set(&s, "one", "This must not persist.", "human").unwrap_err();
        assert!(err.to_string().contains("goal audit failure"));
        let err = reset(&s, "one", "human").unwrap_err();
        assert!(err.to_string().contains("goal audit failure"));
        assert_eq!(must_goal(&s, "one"), want);
        assert_eq!(goal_snapshot(&s), before);
        s.conn()
            .unwrap()
            .execute("DROP TABLE settings", [])
            .unwrap();
        assert!(get(&s, "one").unwrap_err().to_string().contains("settings"));
        assert!(
            set(&s, "one", "This must also fail.", "human")
                .unwrap_err()
                .to_string()
                .contains("settings")
        );
    }

    #[test]
    fn goal_rejects_invalid_stored_provenance() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        for (value, actor, at) in [
            ("not-json", "human", GOAL_NOW),
            (
                r#"{"text":"Use safe text.","source":"owner"}"#,
                "aurora",
                GOAL_NOW,
            ),
            (
                r#"{"text":"Use safe text.","source":"generated"}"#,
                "one-lead",
                GOAL_NOW,
            ),
            (
                r#"{"text":"Use safe text.","source":"inferred"}"#,
                "human",
                GOAL_NOW,
            ),
            (r#"{"text":" ","source":"owner"}"#, "human", GOAL_NOW),
            (
                r#"{"text":"Use safe text.","source":"owner"}"#,
                "human",
                "/private/not-a-time",
            ),
            (
                r#"{"text":"Use safe text.","source":"owner"}"#,
                "human:ghp_syntheticvalue12345",
                GOAL_NOW,
            ),
            (
                r#"{"text":"Use safe text.","source":"owner","updated_by":"human"}"#,
                "human",
                GOAL_NOW,
            ),
            (
                r#"{"text":"Use ghp_syntheticvalue12345","source":"owner"}"#,
                "human",
                GOAL_NOW,
            ),
        ] {
            s.conn()
                .unwrap()
                .execute(
                    "INSERT OR REPLACE INTO settings (name,value,updated_by,updated_at) VALUES (?,?,?,?)",
                    params![format!("{KEY_PREFIX}one"), value, actor, at],
                )
                .unwrap();
            let before = goal_snapshot(&s);
            assert!(get(&s, "one").is_err(), "{value}, {actor}");
            assert_eq!(goal_snapshot(&s), before);
        }
    }

    #[test]
    fn goal_generation_cannot_race_owner_edit() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let s = Arc::new(s);
        let a = Arc::clone(&s);
        let b = Arc::clone(&s);
        let t1 = thread::spawn(move || ensure_default(&a, "one"));
        let t2 = thread::spawn(move || {
            set(&b, "one", "Make search results correct.", "human").map(|_| ())
        });
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();
        let goal = must_goal(&s, "one");
        assert_eq!(goal.source, OWNER);
        assert_eq!(goal.text, "Make search results correct.");
    }

    #[test]
    fn goal_generated_metadata_never_leaks_private_facts() {
        let (_guard, _home, s) = goal_store();
        for name in [
            "/Users/example/private/work",
            "ghp_syntheticvalue12345",
            "unsafe\0name",
            "",
        ] {
            let goal = generated(&pulse_store::Project {
                slug: "safe-project".into(),
                name: name.into(),
                added_at: "/private/not-a-date".into(),
                path: s.home.clone(),
                remote: "https://user:password@example.invalid".into(),
                verify_cmd: "echo ghp_syntheticvalue12345".into(),
                ..pulse_store::Project::default()
            });
            assert!(normalize(&goal.text).is_ok());
            assert!(!goal.text.contains("example"));
            assert!(!goal.text.contains(&s.home));
            assert_eq!(goal.updated_at, "1970-01-01T00:00:00Z");
        }
    }

    #[test]
    fn goal_explicit_default_is_still_owner_direction() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let generated_goal = must_goal(&s, "one");
        let owner = set(&s, "one", &generated_goal.text, "human").unwrap();
        assert_eq!(owner.source, OWNER);
        assert_eq!(owner.text, generated_goal.text);
        ensure_default(&s, "one").unwrap();
        assert_eq!(must_goal(&s, "one"), owner);
    }

    #[test]
    fn goal_uses_existing_private_path_redaction_policy() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let private = "/srv/example-fixture-box";
        s.set_setting("human", "evolve.heldout_dir", private)
            .unwrap();
        s.set_setting("human", "evolve.heldout_dir", "").unwrap();
        let before = goal_snapshot(&s);
        let err = set(
            &s,
            "one",
            &format!("Improve the results from {private}."),
            "human",
        )
        .unwrap_err();
        assert!(!err.to_string().contains(private));
        assert_eq!(goal_snapshot(&s), before);
        let value = serde_json::to_string(&StoredGoal {
            text: format!("Improve the results from {private}."),
            source: OWNER.into(),
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO settings (name,value,updated_by,updated_at) VALUES (?,?,?,?)",
                params![format!("{KEY_PREFIX}one"), value, "human", GOAL_NOW],
            )
            .unwrap();
        let err = get(&s, "one").unwrap_err();
        assert!(!err.to_string().contains(private));
    }

    #[test]
    fn goal_invalid_clock_does_not_write_unreadable_provenance() {
        let (_guard, _home, s) = goal_store();
        goal_project(&s, "one", "One", false);
        let bad_clock = Store::open(Options {
            home: s.home.clone(),
            skip_records: true,
            now: "invalid-clock".into(),
            ..Options::default()
        })
        .unwrap();
        let before = goal_snapshot(&s);
        assert!(
            set(
                &bad_clock,
                "one",
                "Make product failures understandable.",
                "human"
            )
            .is_err()
        );
        assert_eq!(goal_snapshot(&s), before);
    }

    #[test]
    fn goal_canonical_registered_slugs() {
        let (_guard, _home, s) = goal_store();
        let canonical = "customer-support-ticket-management-syste";
        assert_eq!(canonical.len(), 40);
        for slug in [canonical, &format!("{canonical}-2"), &"a".repeat(40)] {
            goal_project(&s, slug, "Customer support ticket management", false);
            let before = goal_snapshot(&s);
            let fallback = get(&s, slug).unwrap();
            assert_eq!(fallback.source, GENERATED);
            assert!(fallback.text.contains("real product defects"));
            assert_eq!(goal_snapshot(&s), before);
            ensure_default(&s, slug).unwrap();
            let saved = must_goal(&s, slug);
            assert_eq!(saved.text, fallback.text);
            assert_eq!(saved.source, GENERATED);
            let owner = set(
                &s,
                slug,
                "Reduce incorrect ticket results using the existing checks.",
                "human",
            )
            .unwrap();
            assert_eq!(must_goal(&s, slug), owner);
            let reset_goal = reset(&s, slug, "human").unwrap();
            assert_eq!(reset_goal.text, fallback.text);
            assert_eq!(reset_goal.source, GENERATED);
        }
        for slug in [
            "",
            "unknown-project",
            &"b".repeat(40),
            pulse_store::COMMONS_SLUG,
            "../elsewhere",
            "/home/example",
            "file:///Users/example",
            "path=/home/example",
            "one/two",
            "bad\0slug",
        ] {
            let before = goal_snapshot(&s);
            assert!(get(&s, slug).is_err(), "{slug:?}");
            assert!(set(&s, slug, "Keep product results correct.", "human").is_err());
            assert_eq!(goal_snapshot(&s), before);
        }
        assert!(normalize(&format!("Use this opaque value {}", "a".repeat(40))).is_err());
    }

    #[test]
    fn goal_private_path_references() {
        for text in [
            "Use /Users/example/plan.md.",
            "Use file:///Users/example/plan.md.",
            "Use FILE:///uSeRs/example/plan.md.",
            "Use file:/Users/example/plan.md.",
            "Use file://localhost/Users/example/plan.md.",
            "Use file://server/home/example/plan.md.",
            "Use file://[::1]/home/x.",
            r#"Use "FILE://HOST/HOME/example/plan.md"."#,
            "Use [file:///Users/example/plan.md].",
            "Use <file://host/Users/example/plan.md>.",
            "Use `file://host/tmp/plan.md`.",
            "Prioritize path=/home/example/plan.md.",
            r#"Prioritize path="/home/example/plan.md"."#,
            "Prioritize path='file://host/home/x'.",
            "Prioritize path:FILE://HOST/HOME/x.",
            "Prioritize path=~/.config/plan.md.",
            "Prioritize path=private/plan.md.",
            "Prioritize path=.ssh/id_test.",
            "Use results/private",
            r#"Use "results/heldout""#,
            "Use file:///data/private",
            "Use file:///C:/work/plan.md.",
            "Use file://host/C:/work/plan.md.",
            r#"Prioritize path=C:\work\plan.md."#,
        ] {
            assert!(normalize(text).is_err(), "{text}");
        }
        for text in [
            "Improve /api/search error handling.",
            "Prioritize path=/api/search.",
            r#"Prioritize path="/api/search"."#,
            "Improve [GET /api/search] responses.",
            "Improve https://example.test/home/docs.",
            "Make https://example.test/api/search reliable.",
            "Keep user data private",
            "Make project settings private",
            "Improve docs/private.md",
        ] {
            assert_eq!(normalize(text).unwrap(), text);
        }
    }

    #[test]
    fn goal_private_path_mutation_and_stored_read_refusals() {
        let (_guard, _home, s) = goal_store();
        for text in [
            "Use file:///Users/example/plan.md.",
            "Prioritize path=/home/example/plan.md.",
        ] {
            s.conn()
                .unwrap()
                .execute("DELETE FROM settings WHERE name LIKE 'project.goal.%'", [])
                .unwrap();
            s.conn()
                .unwrap()
                .execute("DELETE FROM projects", [])
                .unwrap();
            goal_project(&s, "one", "One", false);
            let owner = set(&s, "one", "Improve /api/search reliability.", "human").unwrap();
            let before = goal_snapshot(&s);
            let err = set(&s, "one", text, "human").unwrap_err();
            assert!(!err.to_string().contains(text));
            assert_eq!(must_goal(&s, "one"), owner);
            assert_eq!(goal_snapshot(&s), before);
            let value = serde_json::to_string(&StoredGoal {
                text: text.into(),
                source: OWNER.into(),
            })
            .unwrap();
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE settings SET value = ? WHERE name = ?",
                    params![value, format!("{KEY_PREFIX}one")],
                )
                .unwrap();
            let before = goal_snapshot(&s);
            assert!(get(&s, "one").is_err());
            assert_eq!(goal_snapshot(&s), before);
        }
    }

    #[test]
    fn context_matches_go_text() {
        let goal = Goal {
            text: "First\nSecond".into(),
            source: OWNER.into(),
            updated_at: GOAL_NOW.into(),
            updated_by: "human".into(),
        };
        let got = context(&goal);
        assert!(got.starts_with("## Project goal\n\nSource: owner; updated by human at 2026-09-13T06:00:00Z.\n\n> First\n> Second\n\n"));
        assert!(got.contains("Only the owner may set or reset the goal"));
    }

    #[test]
    fn oracle_goal_rows_match_rust_for_same_inputs() {
        let _guard = pulse_testutil::env_lock();
        let oracle = std::env::var("PULSE_GO_ORACLE").unwrap_or_else(|_| {
            "/Users/work/.copilot/session-state/eb2b69f5-ae4f-4495-904e-534f0b2fbf3e/files/oracle/pulse".into()
        });
        if !Path::new(&oracle).exists() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        let git = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["init", "-q"])
            .status();
        if git.is_err() || !git.unwrap().success() {
            return;
        }
        let go_home = tempfile::tempdir().unwrap();
        let out = Command::new(&oracle)
            .current_dir(repo.path())
            .env("PULSE_HOME", go_home.path())
            .env("PULSE_NOW", GOAL_NOW)
            .args(["init", "--name", "Goal Parity"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let go = Store::open(Options {
            home: go_home.path().display().to_string(),
            read_only: true,
            skip_records: true,
            ..Options::default()
        })
        .unwrap();
        let slug: String = go
            .conn()
            .unwrap()
            .query_row("SELECT slug FROM projects ORDER BY slug LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let go_goal = get(&go, &slug).unwrap();
        let go_setting: (String, String, String, String) = go
            .conn()
            .unwrap()
            .query_row(
                "SELECT name, value, updated_at, updated_by FROM settings WHERE name = ?",
                [format!("{KEY_PREFIX}{slug}")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let go_event: (String, String, String, String) = go
            .conn()
            .unwrap()
            .query_row(
                "SELECT action, actor, subject, detail FROM events WHERE action = 'project.goal.generated'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        let rust_home = tempfile::tempdir().unwrap();
        let rust = Store::open(Options {
            home: rust_home.path().display().to_string(),
            create: true,
            now: GOAL_NOW.into(),
            ..Options::default()
        })
        .unwrap();
        rust.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, brain, verify_cmd, added_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    slug,
                    "Goal Parity",
                    repo.path().display().to_string(),
                    0,
                    "",
                    GOAL_NOW
                ],
            )
            .unwrap();
        let rust_goal = get(&rust, &slug).unwrap();
        assert_eq!(rust_goal, go_goal);
        ensure_default(&rust, &slug).unwrap();
        let rust_setting: (String, String, String, String) = rust
            .conn()
            .unwrap()
            .query_row(
                "SELECT name, value, updated_at, updated_by FROM settings WHERE name = ?",
                [format!("{KEY_PREFIX}{slug}")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let rust_event: (String, String, String, String) = rust
            .conn()
            .unwrap()
            .query_row(
                "SELECT action, actor, subject, detail FROM events WHERE action = 'project.goal.generated'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(rust_setting, go_setting);
        assert_eq!(rust_event, go_event);
        assert!(!rust_event.3.contains("Goal Parity"));
        assert!(rust_event.3.contains("text_hash"));
    }
}
