use anyhow::Result;
use std::collections::BTreeMap;

pub fn recipe_blocks(s: &pulse_store::Store, executor: &str) -> Result<BTreeMap<String, String>> {
    Ok(pulse_runner::recipe_blocks(s, executor, &[]))
}

#[cfg(test)]
mod tests {
    use crate::status::executors_line;
    use crate::test_helpers::*;

    #[test]
    fn an_invalid_recipe_holds_dispatch_on_every_executor() {
        let (_dir, s) = temporary_store();
        let root = tempfile::tempdir().unwrap();
        fixture_command(root.path(), "", "git", &["init", "--quiet"]);
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects(slug,name,path,default_branch,added_at) VALUES ('recipe-project','Recipe',?,'main',?)",
                (root.path().to_str().unwrap(), TEST_NOW),
            )
            .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO executors(name,status,created_at) VALUES ('remote','active',?)",
                [TEST_NOW],
            )
            .unwrap();
        let cases: Vec<(&str, String, &str, &str)> = vec![
            ("empty", "---\nrecipe: 1\n---\n".into(), "", ""),
            (
                "services",
                "---\nrecipe: 1\nservices:\n  - postgres\n---\n".into(),
                "services",
                "services",
            ),
            (
                "tools",
                "---\nrecipe: 1\ntools:\n  - node@22\n---\n".into(),
                "",
                "hands outdated",
            ),
            (
                "env",
                "---\nrecipe: 1\nenv:\n  CI: '1'\n---\n".into(),
                "",
                "hands outdated",
            ),
            (
                "bad-inline-list",
                "---\nrecipe: 1\nservices: [postgres]\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "duplicate-key",
                "---\nrecipe: 1\nservices:\n  - postgres\nservices: []\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "unknown-key",
                "---\nrecipe: 1\nother: 1\nservices:\n  - postgres\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "bad-version",
                "---\nrecipe: 2\nservices:\n  - postgres\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "secret-env-name",
                "---\nrecipe: 1\nenv:\n  TOKEN: refused\nservices:\n  - postgres\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "lowercase-env",
                "---\nrecipe: 1\nenv:\n  ci: '1'\nservices:\n  - postgres\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "bad-quote",
                "---\nrecipe: 1\nservices:\n  - 'postgres\n---\n".into(),
                "recipe invalid",
                "recipe invalid",
            ),
            (
                "tool-plugin-name",
                "---\nrecipe: 1\ntools:\n  - aqua:node@22\n---\n".into(),
                "",
                "hands outdated",
            ),
            (
                "too-many-tools",
                format!(
                    "---\nrecipe: 1\ntools:\n{}services:\n  - postgres\n---\n",
                    "  - node@22\n".repeat(13)
                ),
                "recipe invalid",
                "recipe invalid",
            ),
        ];
        for (name, text, local, remote) in cases {
            let blob =
                fixture_command(root.path(), &text, "git", &["hash-object", "-w", "--stdin"]);
            let context = fixture_command(
                root.path(),
                &format!("100644 blob {blob}\tenvironment.md\n"),
                "git",
                &["mktree"],
            );
            let docs = fixture_command(
                root.path(),
                &format!("040000 tree {context}\tcontext\n"),
                "git",
                &["mktree"],
            );
            let tree = fixture_command(
                root.path(),
                &format!("040000 tree {docs}\tdocs\n"),
                "git",
                &["mktree"],
            );
            let commit = fixture_command(
                root.path(),
                &format!("{name}\n"),
                "git",
                &["commit-tree", &tree],
            );
            fixture_command(
                root.path(),
                "",
                "git",
                &["update-ref", "refs/heads/main", &commit],
            );
            // recipe_blocks reads the recipe with `git show` and treats a failed
            // read as "no recipe" (an empty block map). Read it back here first,
            // so a git failure on a loaded host reports git's own error instead
            // of a bare `{}` mismatch against the expected hold.
            let spec = "refs/heads/main:docs/context/environment.md";
            let shown = fixture_command(root.path(), "", "git", &["show", spec]);
            assert_eq!(shown, text.trim(), "{name}: recipe read back from {spec}");
            for (executor, want) in [("local", local), ("remote", remote)] {
                let blocks = super::recipe_blocks(&s, executor).unwrap();
                let got = blocks
                    .get("recipe-project")
                    .map(String::as_str)
                    .unwrap_or("");
                let readback = if got == want {
                    String::new()
                } else {
                    // The read succeeded a moment ago; show whether it still does.
                    let out = std::process::Command::new("git")
                        .args(["-C", root.path().to_str().unwrap(), "show", spec])
                        .output()
                        .unwrap();
                    format!(
                        "; git show {spec} now: {} {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim()
                    )
                };
                assert_eq!(got, want, "{executor} {name}: {blocks:?}{readback}");
            }
        }
    }

    #[test]
    fn sql_recipe_placement() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO projects(slug,name,path,added_at) VALUES ('sql-project','SQL','/does-not-exist','2026-09-06T19:00:00Z')",
        );
        let record = pulse_store::Record {
            id: "CTX-environment".into(),
            project: "sql-project".into(),
            kind: "context".into(),
            slug: "environment".into(),
            title: "Environment".into(),
            created_at: TEST_NOW.into(),
            updated_at: TEST_NOW.into(),
            origin: "store".into(),
            ..Default::default()
        };
        let mut data = std::collections::BTreeMap::new();
        data.insert("recipe".to_string(), serde_json::json!("1"));
        data.insert("services".to_string(), serde_json::json!(["postgres"]));
        s.put_record(
            record,
            data,
            "An environment in the store, not a Markdown file.",
            "human",
            "",
        )
        .unwrap();
        let blocks = super::recipe_blocks(&s, "local").unwrap();
        assert_eq!(
            blocks.get("sql-project").map(String::as_str),
            Some("services")
        );
        assert!(
            executors_line(&s)
                .unwrap()
                .contains("needs services for sql-project")
        );
    }
}
