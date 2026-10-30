//! The daemon's side of the project's own PULSE file (`pulse manifest`):
//! once per pass, every project whose `.pulse/pulse.yaml` or skill folders
//! changed on the default branch has them applied to the store, and the
//! file's problems reach the project board when they appear and when they
//! clear.

use pulse_tasks::manifest;
use serde_json::json;

/// Applies `slug`'s file when a commit changed it; says what it did.
pub(crate) fn tend_manifest(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    if s.setting("manifest.load", "on") == "off" {
        return Vec::new();
    }
    let synced = match manifest::sync(s, slug, actor, false) {
        Ok(Some(synced)) => synced,
        Ok(None) => return Vec::new(),
        Err(e) => return vec![format!("manifest {slug}: {}", crate::run::err_message(&e))],
    };
    if synced.skipped {
        return Vec::new();
    }
    let mut lines = vec![synced.line()];
    if synced.problems.is_empty() == synced.had_problems {
        // A transition: problems appeared, or the last ones cleared.
        let (summary, body) = finding(&synced);
        match crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: slug.to_string(),
                area: manifest::BOARD_AREA.into(),
                thread: slug.to_string(),
                summary: crate::util::trunc(&summary, 230),
                body,
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: now.to_string(),
                for_owner: !synced.problems.is_empty(),
                ..Default::default()
            },
        ) {
            Ok(p) => lines.push(format!("manifest {slug}: posted {}", p.str("id"))),
            Err(e) => lines.push(format!(
                "manifest {slug}: could not post: {}",
                crate::run::err_message(&e)
            )),
        }
        let detail =
            json!({"path": synced.path, "commit": synced.commit, "problems": synced.problems})
                .to_string();
        let _ = pulse_bus::emit(
            s,
            actor,
            if synced.problems.is_empty() {
                "manifest.problems_cleared"
            } else {
                "manifest.problems"
            },
            slug,
            detail.as_str(),
        );
    }
    lines
}

/// The board post of a transition: what the file gets wrong, or that it is
/// sound again.
fn finding(synced: &manifest::Synced) -> (String, String) {
    let file = if synced.path.is_empty() {
        manifest::MANIFEST_PATHS[0].to_string()
    } else {
        synced.path.clone()
    };
    if synced.problems.is_empty() {
        let summary = format!("{file} is sound again at {}", synced.commit);
        let body = format!(
            "The project's PULSE file and skill folders on {} ({}) applied with no problem.\n\nSchedules: {}.\nSkills: {}.\n\n`pulse manifest show --project {}` lists what the file keeps.",
            synced.branch,
            synced.commit,
            or_none(&synced.schedules.summary("ended")),
            or_none(&synced.skills.summary("disabled")),
            synced.project
        );
        return (summary, body);
    }
    let n = synced.problems.len();
    let summary = format!(
        "{file} at {}: {n} problem{}",
        synced.commit,
        if n == 1 { "" } else { "s" }
    );
    let mut body = format!(
        "The daemon read the project's PULSE file and skill folders on {} ({}) and could not apply everything. A schedule with a problem stays as it was; the rest applied (schedules: {}; skills: {}).\n\n",
        synced.branch,
        synced.commit,
        or_none(&synced.schedules.summary("ended")),
        or_none(&synced.skills.summary("disabled"))
    );
    for p in &synced.problems {
        body.push_str(&format!("- {p}\n"));
    }
    body.push_str(&format!(
        "\nFix the file on {} and commit; the daemon applies the next commit by itself. `pulse manifest check` runs these checks on a working copy before the commit; `pulse manifest show --project {}` shows what is kept now.",
        synced.branch, synced.project
    ));
    (summary, body)
}

fn or_none(text: &str) -> String {
    if text.is_empty() {
        "none".to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn commit(dir: &str, path: &str, text: &str, message: &str) {
        let full = std::path::Path::new(dir).join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        for args in [vec!["add", "-A", "."], vec!["commit", "-q", "-m", message]] {
            let mut full = vec!["-C", dir];
            full.extend(args);
            pulse_testutil::git(&full).unwrap();
        }
    }

    #[test]
    fn tend_manifest_applies_the_file_and_posts_on_transitions_only() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, work) = new_bare_repo(&s);
        pulse_testutil::git(&["-C", &work, "config", "user.email", "t@example.com"]).unwrap();
        pulse_testutil::git(&["-C", &work, "config", "user.name", "t"]).unwrap();
        commit(
            &work,
            ".pulse/pulse.yaml",
            "schedules:\n  - name: audit\n    every: 1d\n    brief: Audit the deps.\n",
            "file",
        );
        let lines = tend_manifest(&s, "proj", TEST_NOW, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("schedules: 1 added"), "{}", lines[0]);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE area = 'manifest'",
                &[]
            ),
            0
        );
        assert!(tend_manifest(&s, "proj", TEST_NOW, "daemon").is_empty());

        commit(
            &work,
            ".pulse/pulse.yaml",
            "schedules:\n  - name: audit\n    every: 1d\n    brief: Audit the deps.\n  - name: broken\n    every: 5m\n    brief: Too often.\n",
            "broken",
        );
        let lines = tend_manifest(&s, "proj", TEST_NOW, "daemon");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("1 problem"), "{}", lines[0]);
        assert!(
            lines[1].contains("posted 20260906T100000Z-daemon"),
            "{}",
            lines[1]
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE area = 'manifest' AND kind = 'finding'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM schedules WHERE status = 'active'",
                &[]
            ),
            1
        );

        commit(
            &work,
            ".pulse/pulse.yaml",
            "schedules:\n  - name: audit\n    every: 1d\n    brief: Audit the deps.\n  - name: broken\n    every: 2d\n    brief: Fixed.\n",
            "fixed",
        );
        let lines = tend_manifest(&s, "proj", TEST_NOW, "daemon");
        assert!(lines[0].contains("1 added"), "{lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE area = 'manifest'",
                &[]
            ),
            2
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'manifest.problems_cleared'",
                &[]
            ),
            1
        );

        s.set_setting("human", "manifest.load", "off").unwrap();
        commit(&work, ".pulse/pulse.yaml", "schedules: []\n", "cleared");
        assert!(tend_manifest(&s, "proj", TEST_NOW, "daemon").is_empty());
    }
}
