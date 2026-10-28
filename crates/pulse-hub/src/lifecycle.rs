use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
impl Handler {
    pub(crate) fn addition_projects(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> bool {
        if !lifecycle_route(parts.clone()) {
            return false;
        }
        if parts.len() == 1 {
            if !crate::addition_method(w, r, vec!["GET".into(), "POST".into()])
                || !self.addition_available(w, r, vec!["projects".into()])
            {
                return true;
            }
            if r.method == "POST" {
                self.addition_project_register(w, r);
            } else {
                self.addition_project_list(w, r);
            }
            return true;
        }
        if parts.get(2).is_some_and(|p| p == "goal") {
            if parts.len() == 4 {
                self.addition_project_goal_review(w, r, parts[1].clone());
            } else {
                self.addition_project_goal(w, r, parts[1].clone());
            }
            return true;
        }
        if !crate::addition_method(w, r, vec!["POST".into()])
            || !self.addition_available(w, r, vec!["projects".into()])
        {
            return true;
        }
        match parts[2].as_str() {
            "remove" => self.addition_project_remove(w, r, parts[1].clone()),
            "rename" => self.addition_project_rename(w, r, parts[1].clone()),
            "move" => self.addition_project_move(w, r, parts[1].clone()),
            _ => self.addition_project_restore(w, r, parts[1].clone()),
        }
        true
    }
    // POST /api/v1/projects/<slug>/move {"path": "...", "force": bool}: the
    // checkout the project now lives in, for a folder the owner moved or
    // cloned again; the slug and everything filed under it stay. A folder
    // that tracks another remote is refused unless `force` says it is the
    // right one.
    pub(crate) fn addition_project_move(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: String,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (path, ok) = crate::addition_string(w, &payload, "path", true);
        if !ok {
            return;
        }
        let force = payload
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match pulse_board::move_project(store, &slug, &path, force, pulse_store::HUMAN_AGENT) {
            Ok(moved) => crate::server::write_json(w, 200, go_value(&moved)),
            Err(err) => crate::addition_failure(w, err.to_string()),
        }
    }
    // POST /api/v1/projects/<slug>/rename {"name": "..."}: the project's
    // display name, shown wherever the console names the project; the slug
    // (the id in every URL, job and post) never changes.
    pub(crate) fn addition_project_rename(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: String,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            crate::server::write_error(
                w,
                400,
                "Missing name".into(),
                "A project needs a name.".into(),
                "Send {\"name\": \"...\"} with the new display name.".into(),
            );
            return;
        }
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let before = match store.project(&slug) {
            Ok(Some(p)) => p,
            Ok(None) => {
                crate::addition_failure(
                    w,
                    pulse_cli::errorf(
                        2,
                        &format!("No project {slug}"),
                        "only a registered project can be renamed",
                        "pulse projects",
                    )
                    .to_string(),
                );
                return;
            }
            Err(err) => {
                crate::addition_failure(w, err.to_string());
                return;
            }
        };
        if let Err(err) = store.rename_project(&slug, &name) {
            crate::addition_failure(w, err.to_string());
            return;
        }
        if before.name != name {
            let _ = store.event(
                pulse_store::HUMAN_AGENT,
                "project.renamed",
                &slug,
                json!({"from": before.name, "to": name}),
            );
        }
        crate::server::write_json(
            w,
            200,
            json!(map_from_pairs(vec![
                ("slug", json!(slug)),
                ("name", json!(name)),
                ("was", json!(before.name)),
            ])),
        );
    }
    pub(crate) fn addition_project_list(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        _r: &crate::httpx::Request,
    ) {
        let Some(store) = &self.store else {
            crate::server::write_error(
                w,
                409,
                "store is not available".into(),
                "No writable PULSE store is open.".into(),
                "Run pulse init, then restart the hub.".into(),
            );
            return;
        };
        match store.projects(false) {
            Ok(projects) => {
                let brain = projects
                    .iter()
                    .find(|p| p.brain)
                    .map(|p| p.slug.clone())
                    .unwrap_or_default();
                let rows: Vec<Value> = std::thread::scope(|scope| {
                    let handles = projects
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| {
                            scope.spawn(move || {
                                let goal = goal_summary(store, &p.slug);
                                let mut row = json!({
                                    "slug": p.slug, "name": p.name, "path": p.path, "remote": p.remote,
                                    "brain": p.brain, "status": p.status, "verify_cmd": p.verify_cmd,
                                    "default_branch": p.default_branch, "added_at": p.added_at,
                                    "added_by": p.added_by, "last_seen": p.last_seen,
                                    "present": self.present_cached(&p),
                                    "goal_source": goal.source, "goal_preview": goal.preview,
                                });
                                if let Some(obj) = row.as_object_mut() {
                                    for (key, value) in self.project_gates_cached(store, &p.slug) {
                                        obj.insert(key, value);
                                    }
                                }
                                (i, row)
                            })
                        })
                        .collect::<Vec<_>>();
                    let mut rows = handles
                        .into_iter()
                        .map(|h| h.join().expect("project list probe panicked"))
                        .collect::<Vec<_>>();
                    rows.sort_by_key(|(i, _)| *i);
                    rows.into_iter().map(|(_, row)| row).collect()
                });
                crate::server::write_json(
                    w,
                    200,
                    json!(map_from_pairs(vec![
                        ("projects", json!(rows)),
                        ("brain", json!(brain))
                    ])),
                );
            }
            Err(err) => crate::server::write_error(
                w,
                500,
                err.to_string(),
                "The store could not list projects.".into(),
                "Check pulse status.".into(),
            ),
        }
    }
    pub(crate) fn addition_project_register(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        if payload.contains_key("brain") {
            crate::server::write_error(
                w,
                400,
                "The brain is chosen in the terminal".into(),
                "The console does not move the brain flag.".into(),
                "Run pulse init PATH --brain.".into(),
            );
            return;
        }
        let (path, ok) = crate::addition_string(w, &payload, "path", true);
        if !ok {
            return;
        }
        let (name, ok) = crate::addition_string(w, &payload, "name", false);
        if !ok {
            return;
        }
        let (verify, ok) = crate::addition_string(w, &payload, "verify", false);
        if !ok {
            return;
        }
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match pulse_board::init(
            store,
            &path,
            &name,
            "",
            false,
            &verify,
            pulse_store::HUMAN_AGENT,
        ) {
            Ok(result) => crate::server::write_json(w, 200, go_value(&result)),
            Err(err) => crate::addition_failure(w, err.to_string()),
        }
    }
    pub(crate) fn addition_project_remove(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: String,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (dry_run, ok) = addition_dry_run(w, payload);
        if !ok {
            return;
        }
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match pulse_board::remove_project(store, &slug, pulse_store::HUMAN_AGENT, dry_run) {
            Ok(preview) => crate::server::write_json(w, 200, go_value(&preview)),
            Err(err) => crate::addition_failure(w, err.to_string()),
        }
    }
    pub(crate) fn addition_project_restore(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: String,
    ) {
        if !crate::server::read_payload(w, r).1 {
            return;
        }
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let project = match store.project(&slug) {
            Ok(Some(p)) => p,
            Ok(None) => {
                crate::addition_failure(
                    w,
                    pulse_cli::errorf(
                        2,
                        &format!("No project {slug}"),
                        "only a registered project can be restored",
                        "pulse projects",
                    )
                    .to_string(),
                );
                return;
            }
            Err(err) => {
                crate::addition_failure(w, err.to_string());
                return;
            }
        };
        if project.status == "active" {
            crate::addition_failure(
                w,
                pulse_cli::errorf(
                    2,
                    &format!("Project {slug} is already active"),
                    "restore registers an inactive project again",
                    "pulse projects",
                )
                .to_string(),
            );
            return;
        }
        match pulse_board::init(
            store,
            &project.path,
            "",
            "",
            false,
            "",
            pulse_store::HUMAN_AGENT,
        ) {
            Ok(result) => crate::server::write_json(w, 200, go_value(&result)),
            Err(err) => crate::addition_failure(w, err.to_string()),
        }
    }
    pub(crate) fn addition_executor_remove(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        name: String,
    ) {
        if !crate::addition_method(w, r, vec!["POST".into()])
            || !self.addition_available(w, r, vec!["executors".into()])
        {
            return;
        }
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (dry_run, ok) = addition_dry_run(w, payload);
        if !ok {
            return;
        }
        let _lock = crate::additions::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match pulse_board::remove_executor(store, &name, pulse_store::HUMAN_AGENT, dry_run) {
            Ok(preview) => crate::server::write_json(w, 200, go_value(&preview)),
            Err(err) => crate::addition_failure(w, err.to_string()),
        }
    }
}
pub(crate) fn lifecycle_route(parts: Vec<String>) -> bool {
    match parts.len() {
        1 => true,
        3 => matches!(
            parts[2].as_str(),
            "remove" | "restore" | "rename" | "move" | "goal"
        ),
        4 => parts[2] == "goal" && parts[3] == "review",
        _ => false,
    }
}
pub(crate) fn project_gates(s: &pulse_store::Store, slug: String) -> JsonMap {
    let mut gates = map_from_pairs(vec![
        ("fixtures_enabled", json!(0)),
        ("baseline", json!(false)),
        ("evolve_level", json!("")),
        ("evolve_next", json!("")),
        (
            "constitution",
            loop_record_summary(s, slug.clone(), "constitution".into()),
        ),
        (
            "evaluators",
            loop_record_summary(s, slug.clone(), "evaluators".into()),
        ),
        ("baseline_job", Value::Null),
        ("baseline_failed_at", json!("")),
    ]);
    if let Ok(status) = pulse_evolve::status(s, &slug) {
        gates.insert("fixtures_enabled".into(), json!(status.public_fixtures));
        gates.insert("baseline".into(), json!(status.baseline.is_some()));
        gates.insert("evolve_level".into(), json!(status.level));
        gates.insert("evolve_next".into(), json!(status.next_human_step));
        if let Some(job) = status.baseline_job {
            gates.insert(
                "baseline_job".into(),
                json!(map_from_pairs(vec![
                    ("id", json!(job.id)),
                    ("status", json!(job.status))
                ])),
            );
        }
        gates.insert(
            "baseline_failed_at".into(),
            json!(status.baseline_failed_at),
        );
    }
    gates
}

/// How much of a goal text the registry and organization rows carry.
pub(crate) const GOAL_PREVIEW_CHARS: usize = 120;

/// Where a project's direction comes from, as the project rows report it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GoalSummary {
    /// `pulse_goals::OWNER` or `pulse_goals::GENERATED`.
    pub(crate) source: String,
    /// The head of the goal text, one line, at most [`GOAL_PREVIEW_CHARS`]
    /// characters; empty when no text is stored.
    pub(crate) preview: String,
}

/// Classifies `slug`'s goal from the stored `project.goal.<slug>` setting
/// alone. `pulse_goals::get` composes the generated text from the checkout's
/// README and commit history on every call, which a row built per project per
/// request must not pay. No row is the generated default (projects registered before goals
/// existed); a row says `owner` or `generated` itself, and one that does not
/// parse falls back to who wrote it.
pub(crate) fn goal_summary(s: &pulse_store::Store, slug: &str) -> GoalSummary {
    let generated = GoalSummary {
        source: pulse_goals::GENERATED.to_string(),
        preview: String::new(),
    };
    let Ok(conn) = s.conn() else {
        return generated;
    };
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT value, updated_by FROM settings WHERE name = ?",
            [format!("{}{slug}", pulse_goals::KEY_PREFIX)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((value, updated_by)) = row else {
        return generated;
    };
    let stored: Option<Map<String, Value>> = serde_json::from_str(&value).ok();
    let source = match stored
        .as_ref()
        .and_then(|m| m.get("source"))
        .and_then(Value::as_str)
    {
        Some(pulse_goals::OWNER) => pulse_goals::OWNER,
        Some(pulse_goals::GENERATED) => pulse_goals::GENERATED,
        _ if updated_by == pulse_goals::GENERATED_BY => pulse_goals::GENERATED,
        _ => pulse_goals::OWNER,
    };
    let preview = stored
        .as_ref()
        .and_then(|m| m.get("text"))
        .and_then(Value::as_str)
        .filter(|text| pulse_goals::normalize(text).is_ok())
        .map(|text| goal_preview(text, GOAL_PREVIEW_CHARS))
        .unwrap_or_default();
    GoalSummary {
        source: source.to_string(),
        preview,
    }
}

/// The first `max` characters of `text` on one line, cut at a word boundary
/// and marked with an ellipsis when something was left out.
pub(crate) fn goal_preview(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let head: String = line.chars().take(max).collect();
    let cut = head
        .rfind(' ')
        .filter(|&at| head[..at].chars().count() >= max / 2)
        .unwrap_or(head.len());
    let mut clipped = head[..cut]
        .trim_end_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_string();
    clipped.push('…');
    clipped
}

pub(crate) fn loop_record_summary(
    s: &pulse_store::Store,
    slug: String,
    name: String,
) -> serde_json::Value {
    let rows = match s.list_records(pulse_store::RecordFilter {
        project: slug.clone(),
        kind: "evolution".into(),
        slug: name,
        ..Default::default()
    }) {
        Ok(rows) => rows,
        Err(_) => return Value::Null,
    };
    let Some(row) = rows.first() else {
        return Value::Null;
    };
    let Ok((head, Some(revision))) = s.get_record(&row.id, 0) else {
        return Value::Null;
    };
    let (seeded, from) = crate::evolution::seeded_record(&head, &revision);
    json!(map_from_pairs(vec![
        ("id", json!(head.id)),
        ("rev", json!(head.current_rev)),
        ("seeded", json!(seeded)),
        ("seeded_from", json!(from)),
        ("draft", crate::evolution::draft_summary(s, &head.id, false)),
        ("draft_job", crate::evolution::draft_job_ref(s, &slug)),
    ]))
}
pub(crate) fn addition_dry_run(
    w: &mut crate::httpx::ResponseWriter,
    payload: JsonMap,
) -> (bool, bool) {
    match payload.get("dry_run") {
        None => (true, true),
        Some(Value::Bool(v)) => (*v, true),
        _ => {
            crate::server::write_error(
                w,
                400,
                "Invalid dry_run".into(),
                "dry_run must be true or false.".into(),
                "Send dry_run:true to preview or dry_run:false to apply.".into(),
            );
            (false, false)
        }
    }
}

fn map_from_pairs(pairs: Vec<(&str, Value)>) -> JsonMap {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http as http;
    use serde_json::{Value, json};
    use std::process::Command;

    fn json_string(s: &str) -> String {
        pulse_store::gojson::to_string(s).unwrap()
    }
    fn must_json(v: &Value) -> Vec<u8> {
        pulse_store::gojson::to_vec(v).unwrap()
    }

    fn lifecycle_repo() -> String {
        let dir = crate::test_helpers::hold_tempdir(tempfile::tempdir().expect("temp repo"));
        let dir = std::fs::canonicalize(dir).expect("canonical temp repo");
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .envs(pulse_gitenv::env().into_iter().filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                }))
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@x")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@x")
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "git {args:?}: {}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "# t\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        dir.display().to_string()
    }

    fn starter_store() -> (http::TestStore, String) {
        pulse_evolve::init();
        let s = http::additions_store(false);
        let p = pulse_board::add_project(
            &s,
            &lifecycle_repo(),
            "Seeded",
            "",
            false,
            "",
            "human",
            "seeded",
        )
        .unwrap();
        (s, p.slug)
    }

    #[test]
    fn a_project_is_renamed_from_the_console_and_keeps_its_slug() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = starter_store();
        let h = http::handler(&s);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/rename"),
            r#"{"name":"  Distillery (prod)  "}"#,
        );
        assert_eq!(status, 200, "rename: {result:?}");
        assert_eq!(result["slug"], slug);
        assert_eq!(result["name"], "Distillery (prod)");
        assert_eq!(result["was"], "Seeded");
        let project = s.project(&slug).unwrap().unwrap();
        assert_eq!(project.name, "Distillery (prod)");
        assert_eq!(project.slug, slug, "the slug is the id and never moves");
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        assert_eq!(status, 200);
        assert_eq!(result["projects"][0]["name"], "Distillery (prod)");
        let noted: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE action='project.renamed' AND subject=?",
                [&slug],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(noted, 1);

        // An empty name, an unknown project and a GET are refused.
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/rename"),
            r#"{"name":"   "}"#,
        );
        assert_eq!(status, 400, "{result:?}");
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects/nobody/rename",
            r#"{"name":"x"}"#,
        );
        assert_ne!(status, 200);
        let (status, _) =
            http::additions_request(h, "GET", &format!("/api/v1/projects/{slug}/rename"), "");
        assert_eq!(status, 405);
        assert_eq!(s.project(&slug).unwrap().unwrap().name, "Distillery (prod)");
    }

    /// A project whose checkout the owner moved is pointed at the new folder
    /// from the console: the slug and its history stay, a folder that is
    /// another project's or tracks another remote is refused, and `force`
    /// adopts a different remote on purpose.
    #[test]
    fn a_project_is_moved_to_another_checkout_from_the_console() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = starter_store();
        let h = http::handler(&s);
        let was = s.project(&slug).unwrap().unwrap().path;
        let gone = "/Volumes/pulse-no-such-volume-7f3a/seeded";
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET path = ?, remote = 'git@github.com:acme/seeded.git' WHERE slug = ?",
                rusqlite::params![gone, slug],
            )
            .unwrap();
        // The new checkout tracks the same repository over https.
        let again = lifecycle_repo();
        std::process::Command::new("git")
            .args([
                "-C",
                &again,
                "remote",
                "add",
                "origin",
                "https://github.com/acme/seeded",
            ])
            .output()
            .unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            &format!(r#"{{"path":"{again}"}}"#),
        );
        assert_eq!(status, 200, "move: {result:?}");
        assert_eq!(result["slug"], slug);
        assert_eq!(result["from"], gone);
        assert_eq!(
            result["remote"], "",
            "the same repository: no remote change"
        );
        let project = s.project(&slug).unwrap().unwrap();
        assert_eq!(project.path, result["to"].as_str().unwrap());
        assert_ne!(project.path, was);
        assert_eq!(project.remote, "git@github.com:acme/seeded.git");
        assert_eq!(project.slug, slug, "the slug is the id and never moves");
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        assert_eq!(status, 200);
        assert_eq!(result["projects"][0]["path"], project.path);
        let noted: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE action='project.moved' AND subject=?",
                [&slug],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(noted, 1);

        // Another project's folder, a folder that is no checkout, a missing
        // path and an unknown project are refused; so is a checkout of
        // another repository, until `force` says it is the right one.
        let other = pulse_board::add_project(
            &s,
            &lifecycle_repo(),
            "Other",
            "",
            false,
            "",
            "human",
            "other",
        )
        .unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            &format!(r#"{{"path":"{}"}}"#, other.path),
        );
        assert_ne!(status, 200, "{result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap_or_default()
                .contains("already the project other"),
            "{result:?}"
        );
        let plain = tempfile::tempdir().unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            &format!(r#"{{"path":"{}"}}"#, plain.path().display()),
        );
        assert_ne!(status, 200, "{result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap_or_default()
                .contains("Not a git checkout"),
            "{result:?}"
        );
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            r#"{"path":""}"#,
        );
        assert_eq!(status, 400);
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects/nobody/move",
            r#"{"path":"/tmp"}"#,
        );
        assert_ne!(status, 200);
        let elsewhere = lifecycle_repo();
        std::process::Command::new("git")
            .args([
                "-C",
                &elsewhere,
                "remote",
                "add",
                "origin",
                "https://github.com/acme/elsewhere.git",
            ])
            .output()
            .unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            &format!(r#"{{"path":"{elsewhere}"}}"#),
        );
        assert_ne!(status, 200, "{result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap_or_default()
                .contains("tracks https://github.com/acme/elsewhere.git"),
            "{result:?}"
        );
        assert_eq!(s.project(&slug).unwrap().unwrap().path, project.path);
        let (status, result) = http::additions_request(
            h,
            "POST",
            &format!("/api/v1/projects/{slug}/move"),
            &format!(r#"{{"path":"{elsewhere}","force":true}}"#),
        );
        assert_eq!(status, 200, "{result:?}");
        assert_eq!(result["remote"], "https://github.com/acme/elsewhere.git");
        let project = s.project(&slug).unwrap().unwrap();
        assert_eq!(project.remote, "https://github.com/acme/elsewhere.git");
        assert!(
            project.path.ends_with(&elsewhere) || project.path == elsewhere,
            "{}",
            project.path
        );
    }

    fn assert_evolution_shape(data: &crate::server::JsonMap) {
        for key in [
            "constitution",
            "evaluators",
            "archive",
            "loop",
            "gates",
            "fixtures",
        ] {
            assert!(
                data.contains_key(key),
                "evolution lacks {key}: {:?}",
                data.keys().collect::<Vec<_>>()
            );
        }
    }

    fn assert_evolution_fixtures(value: &Value) -> Vec<serde_json::Map<String, Value>> {
        let rows = value
            .as_array()
            .expect("fixtures list")
            .iter()
            .map(|v| v.as_object().unwrap().clone())
            .collect::<Vec<_>>();
        for row in &rows {
            let keys = row.keys().cloned().collect::<Vec<_>>();
            assert_eq!(
                keys,
                [
                    "at",
                    "created_by",
                    "enabled",
                    "kind",
                    "metric",
                    "name",
                    "starter",
                    "weight"
                ],
                "fixture row keys"
            );
        }
        rows
    }

    fn template_store() -> (http::TestStore, String) {
        pulse_evolve::init();
        let s = http::additions_store(false);
        pulse_board::add_project(
            &s,
            &lifecycle_repo(),
            "Brain",
            "",
            true,
            "",
            "human",
            "brain",
        )
        .unwrap();
        for name in ["constitution", "evaluators"] {
            let title = format!(
                "<Project> Evolution {}{}",
                name[..1].to_uppercase(),
                &name[1..]
            );
            let rec = pulse_store::Record {
                id: format!("TPL-evolution-{name}"),
                project: pulse_store::COMMONS_SLUG.into(),
                kind: "template".into(),
                slug: format!("evolution-{name}"),
                title: title.clone(),
                status: "Active".into(),
                path: format!("docs/evolution/_template-{name}.md"),
                ..Default::default()
            };
            s.put_record(
                rec,
                std::collections::BTreeMap::from([
                    ("id".into(), json!(name.to_uppercase())),
                    ("title".into(), json!(title)),
                    ("status".into(), json!("Active")),
                    ("loop".into(), json!("off")),
                ]),
                &format!("# {title}\n\nThe <Project> loop.\n"),
                "human",
                "shaped",
            )
            .unwrap();
        }
        let p = pulse_board::add_project(
            &s,
            &lifecycle_repo(),
            "Lamplit Labs",
            "",
            false,
            "",
            "alice",
            "lamplitlabs",
        )
        .unwrap();
        (s, p.slug)
    }

    fn raw_request(
        h: Handler,
        method: &str,
        path: &str,
        body: &str,
        headers: &[(&str, &str)],
    ) -> (u16, String) {
        let server = crate::test_helpers_http::server(h);
        let client = crate::test_helpers::client();
        let url = format!("{}{}", server.url, path);
        let mut req = match method {
            "POST" => client.post(&url),
            "DELETE" => client.delete(&url),
            _ => client.get(&url),
        };
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if !body.is_empty() {
            req = req.body(body.to_string());
        }
        let resp = req.send().expect("send test request");
        let status = resp.status().as_u16();
        (status, resp.text().expect("response text"))
    }

    #[test]
    fn lifecycle_register_remove_restore() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let h = http::handler(&s);
        let repo = lifecycle_repo();
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        assert_eq!(status, 200);
        assert_eq!(result["projects"].as_array().unwrap().len(), 0);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(
                r#"{{"path":{},"name":"My App","verify":"go test ./..."}}"#,
                json_string(&repo)
            ),
        );
        assert_eq!(status, 200, "register: {result:?}");
        assert_eq!(result["created"], true);
        assert_eq!(result["job_queued"], true);
        let project = result["project"].as_object().unwrap();
        let slug = project["slug"].as_str().unwrap().to_string();
        assert_eq!(slug, "my-app");
        assert_eq!(project["path"], repo);
        assert_eq!(project["verify_cmd"], "go test ./...");
        assert_eq!(project["status"], "active");
        let job = result["job"].as_object().unwrap();
        assert_eq!(job["project"], slug);
        assert_ne!(job["id"], "");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{}}}"#, json_string(&repo)),
        );
        assert_eq!(status, 200, "refresh: {result:?}");
        assert_eq!(result["created"], false);
        assert_eq!(result["job_queued"], false);
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        let rows = result["projects"].as_array().unwrap();
        assert_eq!(status, 200);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["slug"], slug);
        assert_eq!(rows[0]["present"], true);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/remove"),
            "{}",
        );
        assert_eq!(status, 200, "remove preview: {result:?}");
        assert_eq!(result["dry_run"], true);
        assert_eq!(result["applied"], false);
        assert_eq!(result["kind"], "project");
        assert_eq!(result["cancelled_jobs"].as_array().unwrap().len(), 1);
        let count: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM projects WHERE status='active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/remove"),
            r#"{"dry_run":false}"#,
        );
        assert_eq!(status, 200, "remove apply: {result:?}");
        assert_eq!(result["applied"], true);
        assert_eq!(result["dry_run"], false);
        let state: String = s
            .conn()
            .unwrap()
            .query_row("SELECT status FROM projects WHERE slug=?", [&slug], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "inactive");
        let count: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM jobs WHERE project=? AND status='cancelled'",
                [&slug],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let count: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE action='project.removed' AND actor='human'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/remove"),
            r#"{"dry_run":false}"#,
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("already inactive")
        );
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        let rows = result["projects"].as_array().unwrap();
        assert_eq!(status, 200);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "inactive");
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/org", "");
        assert_eq!(status, 200);
        assert!(
            !String::from_utf8(must_json(&result["projects"]))
                .unwrap()
                .contains(&format!("\"{slug}\""))
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/projects/{slug}/restore"),
            "{}",
        );
        assert_eq!(status, 200, "restore: {result:?}");
        assert_eq!(result["created"], false);
        assert_eq!(result["job_queued"], true);
        let state: String = s
            .conn()
            .unwrap()
            .query_row("SELECT status FROM projects WHERE slug=?", [&slug], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "active");
        let (status, result) =
            http::additions_request(h, "POST", &format!("/api/v1/projects/{slug}/restore"), "{}");
        assert_eq!(status, 400);
        assert!(result["error"].as_str().unwrap().contains("already active"));
    }

    #[test]
    fn lifecycle_refusals() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let h = http::handler(&s);
        let repo = lifecycle_repo();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{}}}"#, json_string(&format!("{repo}/missing"))),
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("Not a directory")
        );
        let plain = tempfile::tempdir().unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(
                r#"{{"path":{}}}"#,
                json_string(&plain.path().display().to_string())
            ),
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("Not a git checkout")
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{},"brain":true}}"#, json_string(&repo)),
        );
        assert_eq!(status, 400);
        assert!(result["error"].as_str().unwrap().contains("brain"));
        let (status, _) =
            http::additions_request(h.clone(), "POST", "/api/v1/projects", r#"{"path":""}"#);
        assert_eq!(status, 400);
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{},"name":7}}"#, json_string(&repo)),
        );
        assert_eq!(status, 400);
        s.conn()
            .unwrap()
            .query_row::<i64, _, _>("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        let (status, result) =
            http::additions_request(h.clone(), "POST", "/api/v1/projects/nobody/remove", "{}");
        assert_eq!(status, 404);
        assert!(result["error"].as_str().unwrap().starts_with("No project "));
        let (status, _) =
            http::additions_request(h.clone(), "POST", "/api/v1/projects/nobody/restore", "{}");
        assert_eq!(status, 404);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects/nobody/remove",
            r#"{"dry_run":"yes"}"#,
        );
        assert_eq!(status, 400);
        assert_eq!(result["error"], "Invalid dry_run");
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects(slug,name,path,brain,added_at) VALUES('brain','brain',?,1,?)",
                rusqlite::params![repo, s.now()],
            )
            .unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/projects/brain/remove",
            r#"{"dry_run":false}"#,
        );
        assert_eq!(status, 400);
        assert!(result["error"].as_str().unwrap().contains("brain"));
        let (status, _) = raw_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{}}}"#, json_string(&repo)),
            &[
                ("Content-Type", "application/json"),
                ("Origin", "http://evil.example"),
            ],
        );
        assert_eq!(status, 403);
        let (status, _) = raw_request(
            h.clone(),
            "POST",
            "/api/v1/projects",
            &format!("path={repo}"),
            &[("Content-Type", "text/plain")],
        );
        assert_eq!(status, 415);
        let (status, _) = http::additions_request(h.clone(), "DELETE", "/api/v1/projects", "");
        assert_eq!(status, 405);
        let (status, _) =
            http::additions_request(h.clone(), "GET", "/api/v1/projects/brain/remove", "");
        assert_eq!(status, 405);
        let (status, result) =
            http::additions_request(h, "GET", "/api/v1/projects/brain/overview", "");
        assert_eq!(status, 200);
        assert_eq!(result.get("error"), None);
    }

    #[test]
    fn lifecycle_executor_remove() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let h = http::handler(&s);
        s.conn().unwrap().execute("INSERT INTO executors (name, address, projects, status, created_at) VALUES ('vm-one', 'one@10.0.0.9', 'one', 'active', ?)", [s.now()]).unwrap();
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors/vm-one/remove",
            "{}",
        );
        assert_eq!(status, 200, "executor preview: {result:?}");
        assert_eq!(result["kind"], "executor");
        assert_eq!(result["dry_run"], true);
        assert_eq!(result["applied"], false);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors/vm-one/remove",
            r#"{"dry_run":false}"#,
        );
        assert_eq!(status, 200, "executor remove: {result:?}");
        assert_eq!(result["applied"], true);
        let state: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT status FROM executors WHERE name='vm-one'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(state, "active");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors/vm-one/remove",
            r#"{"dry_run":false}"#,
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("already removed")
        );
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors/nobody/remove",
            "{}",
        );
        assert_eq!(status, 404);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors/local/remove",
            "{}",
        );
        assert_eq!(status, 400);
        assert!(result["error"].as_str().unwrap().contains("local executor"));
        let (status, _) =
            http::additions_request(h, "GET", "/api/v1/compute/executors/vm-one/remove", "");
        assert_eq!(status, 405);
    }

    #[test]
    fn evolution_gates_and_fixtures_never_expose_texts() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = starter_store();
        pulse_evolve::add_fixture(
            &s,
            pulse_evolve::FixtureInput {
                project: slug.clone(),
                name: "owner-fixture".into(),
                request: "OWNER_REQUEST_TEXT".into(),
                scorer: "OWNER_SCORER_TEXT".into(),
                kind: "public".into(),
                weight: 2.0,
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_evolve::set_fixture_enabled(&s, &slug, "starter-hardstop-publish", false, "human")
            .unwrap();
        let h = http::handler(&s);
        let (status, data) =
            http::additions_request(h, "GET", &format!("/api/v1/projects/{slug}/evolution"), "");
        assert_eq!(status, 200, "evolution: {data:?}");
        assert_evolution_shape(&data.clone().into_iter().collect());
        assert!(data.get("gates_error").is_none());
        let gates = data["gates"].as_object().unwrap();
        for key in [
            "project",
            "stop",
            "loop",
            "limits",
            "tolerance",
            "constitution",
            "evaluators",
            "baseline",
            "baseline_job",
            "baseline_failed_at",
            "public_fixtures",
            "heldout",
            "kept",
            "discarded",
            "voted",
            "signal",
            "product_scope",
            "cycles",
            "hours",
            "cap_reached",
            "level",
            "reason",
            "next_human_step",
            "planning",
        ] {
            assert!(gates.contains_key(key), "gates lacks {key}: {gates:?}");
        }
        assert_eq!(gates["project"], slug);
        assert_eq!(gates["public_fixtures"], 6);
        assert_eq!(gates["level"], "B");
        assert_eq!(gates["baseline"], Value::Null);
        assert_eq!(gates["baseline_job"], Value::Null);
        assert_eq!(gates["baseline_failed_at"], "");
        let planning = gates["planning"].as_object().unwrap();
        assert_eq!(planning["state"], "gated");
        assert!(
            planning["reason"]
                .as_str()
                .unwrap()
                .contains(gates["reason"].as_str().unwrap())
        );
        let rows = assert_evolution_fixtures(&data["fixtures"]);
        assert_eq!(rows.len(), 7);
        let (mut starters, mut disabled) = (0, 0);
        for row in rows {
            let name = row["name"].as_str().unwrap();
            assert_eq!(row["starter"], name.starts_with("starter-"));
            assert_eq!(row["kind"], "public");
            if row["starter"] == true {
                starters += 1;
            }
            if row["enabled"] == false {
                disabled += 1;
                assert_eq!(name, "starter-hardstop-publish");
            }
            if name == "owner-fixture" {
                assert_eq!(row["weight"], 2);
            }
        }
        assert_eq!((starters, disabled), (6, 1));
        let encoded = pulse_store::gojson::to_string(&data).unwrap();
        for forbidden in [
            "OWNER_REQUEST_TEXT",
            "OWNER_SCORER_TEXT",
            "pulse evolve score starter-",
            "recieve",
            "squash every commit",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "fixture text {forbidden:?} escaped into the evolution payload"
            );
        }
    }

    #[test]
    fn evolution_gates_fail_softly() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "raw");
        let data = http::handler(&s).evolution("raw".into()).unwrap();
        assert_evolution_shape(&data);
        assert_eq!(data["fixtures"].as_array().unwrap().len(), 0);
        let (s2, slug) = starter_store();
        let data = http::handler(&s2).evolution(slug).unwrap();
        assert_evolution_shape(&data);
        let gates = data["gates"].as_object().unwrap();
        assert_eq!(gates["constitution"], true);
        assert_eq!(gates["evaluators"], true);
        assert_eq!(
            gates["reason"],
            "no baseline measured yet; the daemon records it once the loop is on"
        );
        assert!(
            gates["next_human_step"]
                .as_str()
                .unwrap()
                .contains("evolve.loop")
        );
        let data = http::handler(&s2)
            .evolution(pulse_store::COMMONS_SLUG.into())
            .unwrap();
        assert_evolution_shape(&data);
        assert_eq!(data["gates"], Value::Null);
        assert_eq!(data["gates_error"], "Evolution needs a registered project");
    }

    #[test]
    fn project_rows_carry_the_loop_position() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = starter_store();
        http::additions_project(&s, "raw");
        let h = http::handler(&s);
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        assert_eq!(status, 200, "projects: {result:?}");
        let rows = result["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["slug"].as_str().unwrap().to_string(),
                    v.as_object().unwrap().clone(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let seeded = rows.get(&slug).unwrap();
        let raw = rows.get("raw").unwrap();
        for row in rows.values() {
            for key in [
                "fixtures_enabled",
                "baseline",
                "evolve_level",
                "evolve_next",
                "constitution",
                "evaluators",
                "baseline_job",
                "baseline_failed_at",
            ] {
                assert!(row.contains_key(key), "row lacks {key}: {row:?}");
            }
            assert_eq!(row["baseline_job"], Value::Null);
            assert_eq!(row["baseline_failed_at"], "");
        }
        assert_eq!(seeded["fixtures_enabled"], 6);
        assert_eq!(seeded["baseline"], false);
        assert_eq!(seeded["evolve_level"], "B");
        assert!(
            seeded["evolve_next"]
                .as_str()
                .unwrap()
                .contains("evolve.loop")
        );
        assert_eq!(
            seeded["constitution"]["seeded_from"],
            format!(
                "{}/TPL-evolution-constitution@r1",
                pulse_store::COMMONS_SLUG
            )
        );
        assert_eq!(
            seeded["evaluators"]["seeded_from"],
            format!("{}/TPL-evolution-evaluators@r1", pulse_store::COMMONS_SLUG)
        );
        assert_eq!(raw["fixtures_enabled"], 0);
        assert_eq!(raw["baseline"], false);
        assert_eq!(raw["evolve_level"], "B");
        assert_ne!(raw["evolve_next"], "");
        pulse_evolve::set_fixture_enabled(&s, &slug, "starter-hardstop-publish", false, "human")
            .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET status = 'inactive' WHERE slug = 'raw'",
                [],
            )
            .unwrap();
        let (_, result) = http::additions_request(h, "GET", "/api/v1/projects", "");
        for item in result["projects"].as_array().unwrap() {
            let row = item.as_object().unwrap();
            match row["slug"].as_str().unwrap() {
                s if s == slug => assert_eq!(row["fixtures_enabled"], 5),
                "raw" => {
                    assert_eq!(row["status"], "inactive");
                    assert_eq!(row["evolve_level"], "B");
                    assert_eq!(row["fixtures_enabled"], 0);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn project_rows_carry_the_goal_source() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let p =
            pulse_board::add_project(&s, &lifecycle_repo(), "My App", "", false, "", "human", "")
                .unwrap();
        let slug = p.slug.clone();
        http::additions_project(&s, "raw");
        let h = http::handler(&s);
        let registry_row = |slug: &str| {
            let (status, result) =
                http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
            assert_eq!(status, 200, "projects: {result:?}");
            result["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["slug"] == slug)
                .cloned()
                .unwrap_or_else(|| panic!("no registry row for {slug}: {result:?}"))
        };
        let org_row = |slug: &str| {
            let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/org", "");
            assert_eq!(status, 200, "org: {result:?}");
            result["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["slug"] == slug)
                .cloned()
                .unwrap_or_else(|| panic!("no org row for {slug}: {result:?}"))
        };

        // Registration stored the generated default; "raw" predates goals and
        // has no row at all. Both run on a generated goal.
        let fresh = registry_row(&slug);
        assert_eq!(fresh["goal_source"], pulse_goals::GENERATED);
        let preview = fresh["goal_preview"].as_str().unwrap();
        assert!(
            preview.starts_with("Make My App more useful"),
            "generated preview: {preview}"
        );
        assert!(preview.chars().count() <= GOAL_PREVIEW_CHARS + 1);
        assert!(!preview.contains('\n'));
        let raw = registry_row("raw");
        assert_eq!(raw["goal_source"], pulse_goals::GENERATED);
        assert_eq!(raw["goal_preview"], "");
        let org = org_row(&slug);
        assert_eq!(org["present"], true);
        assert_eq!(org["goal_source"], pulse_goals::GENERATED);
        assert_eq!(org["goal_preview"], preview);
        // The shared board and a checkout missing on disk carry no goal.
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/org", "");
        assert!(result["commons"].get("goal_source").is_none());
        assert!(org_row("raw").get("goal_source").is_none());
        assert_eq!(org_row("raw")["present"], false);

        let want = "Keep search useful, correct and reliable.";
        pulse_goals::set(&s, &slug, want, "human").unwrap();
        let owned = registry_row(&slug);
        assert_eq!(owned["goal_source"], pulse_goals::OWNER);
        assert_eq!(owned["goal_preview"], want);
        let org = org_row(&slug);
        assert_eq!(org["goal_source"], pulse_goals::OWNER);
        assert_eq!(org["goal_preview"], want);

        // A long owner goal is cut at a word, on one line, with an ellipsis.
        let long = format!(
            "Ship the {} release to every user.",
            "very ".repeat(40).trim()
        );
        pulse_goals::set(&s, &slug, &long, "human").unwrap();
        let preview = registry_row(&slug)["goal_preview"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(preview.ends_with("very…"), "clipped preview: {preview}");
        assert!(preview.chars().count() <= GOAL_PREVIEW_CHARS + 1);
        assert!(long.starts_with(preview.trim_end_matches('…')));

        // The owner's reset is a decision too, but it hands the lead back the
        // generated default.
        pulse_goals::reset(&s, &slug, "human").unwrap();
        assert_eq!(registry_row(&slug)["goal_source"], pulse_goals::GENERATED);
        assert_eq!(org_row(&slug)["goal_source"], pulse_goals::GENERATED);
    }

    #[test]
    fn goal_summary_reads_the_setting_without_composing_a_goal() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(true);
        http::additions_project(&s, "one");
        assert_eq!(
            goal_summary(&s, "one"),
            GoalSummary {
                source: pulse_goals::GENERATED.into(),
                preview: String::new()
            }
        );
        let put = |value: &str, by: &str| {
            s.conn()
                .unwrap()
                .execute(
                    "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?)
                     ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by",
                    rusqlite::params![
                        format!("{}one", pulse_goals::KEY_PREFIX),
                        value,
                        s.now(),
                        by
                    ],
                )
                .unwrap();
        };
        put(
            r#"{"text":"Make one more useful.","source":"generated"}"#,
            pulse_goals::GENERATED_BY,
        );
        let g = goal_summary(&s, "one");
        assert_eq!(g.source, pulse_goals::GENERATED);
        assert_eq!(g.preview, "Make one more useful.");
        put(
            "{\"text\":\"Serve  the\\nreaders first.\",\"source\":\"owner\"}",
            "human",
        );
        let g = goal_summary(&s, "one");
        assert_eq!(g.source, pulse_goals::OWNER);
        assert_eq!(g.preview, "Serve the readers first.");
        // A row that is not valid JSON falls back to who wrote it; a text
        // that would not pass the goal rules is not previewed.
        put("not json", pulse_goals::GENERATED_BY);
        assert_eq!(goal_summary(&s, "one").source, pulse_goals::GENERATED);
        put("not json", "human");
        let g = goal_summary(&s, "one");
        assert_eq!(g.source, pulse_goals::OWNER);
        assert_eq!(g.preview, "");
        put(
            r#"{"text":"Read ~/.ssh/id_test first.","source":"owner"}"#,
            "human",
        );
        let g = goal_summary(&s, "one");
        assert_eq!(g.source, pulse_goals::OWNER);
        assert_eq!(g.preview, "");
        assert_eq!(goal_preview("short", 10), "short");
        assert_eq!(goal_preview("  two\n\twords  ", 10), "two words");
        assert_eq!(
            goal_preview("abcdefghijklmnopqrstuvwxyz", 10),
            "abcdefghij…"
        );
        assert_eq!(goal_preview("alpha beta gamma delta", 12), "alpha beta…");
    }

    #[test]
    fn project_rows_carry_the_baseline_job() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = template_store();
        let h = http::handler(&s);
        let project_row = || {
            let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
            result["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["slug"] == slug)
                .unwrap()
                .as_object()
                .unwrap()
                .clone()
        };
        let gates = || {
            let (status, data) = http::additions_request(
                h.clone(),
                "GET",
                &format!("/api/v1/projects/{slug}/evolution"),
                "",
            );
            assert_eq!(status, 200, "evolution: {data:?}");
            assert_evolution_shape(&data.clone().into_iter().collect());
            data["gates"].as_object().unwrap().clone()
        };
        let job = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: slug.clone(),
                kind: "baseline".into(),
                title: "Record baseline".into(),
                priority: 20,
                source: format!("baseline:{slug}"),
                created_by: "daemon".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .0;
        let want = json!({"id": job.id, "status": "queued"});
        let row = project_row();
        assert_eq!(row["baseline_job"], want);
        assert_eq!(row["baseline_failed_at"], "");
        assert_eq!(row["baseline"], false);
        let g = gates();
        assert_eq!(g["baseline_job"], want);
        assert_eq!(g["baseline_failed_at"], "");
        assert!(
            g["reason"]
                .as_str()
                .unwrap()
                .contains(&format!("baseline measuring (job {}, queued)", job.id))
        );
        pulse_board::finish_job(
            &s,
            &job.id,
            "aurora",
            "failed",
            "measurement failed",
            "",
            None,
        )
        .unwrap();
        let row = project_row();
        assert_eq!(row["baseline_job"], Value::Null);
        assert_eq!(row["baseline_failed_at"], s.now());
        assert_eq!(row["baseline"], false);
        let g = gates();
        assert_eq!(g["baseline_job"], Value::Null);
        assert_eq!(g["baseline_failed_at"], s.now());
        let retry_hours = s.setting("baseline.retry_hours", "");
        assert!(
            g["reason"]
                .as_str()
                .unwrap()
                .contains(&format!("retries after {retry_hours} hours")),
            "{}",
            g["reason"]
        );
        let scores = pulse_evolve::Scores {
            project: slug.clone(),
            total: 1.0,
            fixtures: vec![pulse_evolve::FixtureScore {
                name: "starter-readme".into(),
                kind: "public".into(),
                score: 1.0,
                weight: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let id = pulse_evolve::archive_row(
            &s,
            pulse_evolve::ArchiveRow {
                project: slug.clone(),
                hypothesis: pulse_evolve::BASELINE_HYPOTHESIS.into(),
                branch: "main".into(),
                baseline: Some(scores.clone()),
                variant: Some(scores),
                tolerance: 0.02,
                kept: true,
                run_id: "run-1".into(),
                note: "baseline measurement".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let row = project_row();
        assert_eq!(row["baseline"], true);
        assert_eq!(row["baseline_failed_at"], s.now());
        let (_, data) =
            http::additions_request(h, "GET", &format!("/api/v1/projects/{slug}/evolution"), "");
        let archive = data["archive"]["rows"].as_array().unwrap();
        assert_eq!(archive.len(), 1);
        let arow = archive[0].as_object().unwrap();
        assert_eq!(arow["id"], id);
        assert_eq!(arow["hypothesis"], "baseline");
        assert_eq!(arow["kept"], 1);
        let g = data["gates"].as_object().unwrap();
        assert_eq!(g["baseline"], 1);
        assert_eq!(g["kept"], 0);
        assert_eq!(g["cycles"], 0);
    }

    #[test]
    fn project_rows_and_evolution_tell_seeded_records_apart() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = template_store();
        http::additions_project(&s, "raw");
        let h = http::handler(&s);
        let loop_record = |row: &serde_json::Map<String, Value>, name: &str| {
            let value = row[name].as_object().unwrap();
            let keys = value.keys().cloned().collect::<Vec<_>>();
            assert_eq!(
                keys,
                ["draft", "draft_job", "id", "rev", "seeded", "seeded_from"]
            );
            value.clone()
        };
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        let rows = result["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["slug"].as_str().unwrap().to_string(),
                    v.as_object().unwrap().clone(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let constitution = loop_record(rows.get(&slug).unwrap(), "constitution");
        assert_eq!(constitution["id"], "EVO-constitution-lamplitlabs");
        assert_eq!(constitution["rev"], 1);
        assert_eq!(constitution["seeded"], true);
        assert_eq!(
            constitution["seeded_from"],
            format!(
                "{}/TPL-evolution-constitution@r2",
                pulse_store::COMMONS_SLUG
            )
        );
        assert_eq!(constitution["draft"], Value::Null);
        assert_eq!(constitution["draft_job"], Value::Null);
        let evaluators = loop_record(rows.get(&slug).unwrap(), "evaluators");
        assert_eq!(evaluators["id"], "EVO-evaluators-lamplitlabs");
        assert_eq!(evaluators["seeded"], true);
        assert_eq!(
            evaluators["seeded_from"],
            format!("{}/TPL-evolution-evaluators@r2", pulse_store::COMMONS_SLUG)
        );
        assert_eq!(rows["raw"]["constitution"], Value::Null);
        assert_eq!(rows["raw"]["evaluators"], Value::Null);
        assert_eq!(rows["brain"]["constitution"], Value::Null);
        let next = rows[&slug]["evolve_next"].as_str().unwrap();
        assert!(!next.contains("evolve seed") && !next.contains("record new"));
        let (status, data) = http::additions_request(
            h.clone(),
            "GET",
            &format!("/api/v1/projects/{slug}/evolution"),
            "",
        );
        assert_eq!(status, 200, "evolution: {data:?}");
        assert_evolution_shape(&data.clone().into_iter().collect());
        let doc = data["constitution"].as_object().unwrap();
        assert_eq!(doc["seeded"], true);
        assert_eq!(
            doc["seeded_from"],
            format!(
                "{}/TPL-evolution-constitution@r2",
                pulse_store::COMMONS_SLUG
            )
        );
        assert!(
            doc["body"]
                .as_str()
                .unwrap()
                .contains("The Lamplit Labs loop.")
        );
        let doc = data["evaluators"].as_object().unwrap();
        assert_eq!(doc["seeded"], true);
        assert_eq!(
            doc["seeded_from"],
            format!("{}/TPL-evolution-evaluators@r2", pulse_store::COMMONS_SLUG)
        );
        pulse_records::amend(
            &s,
            "EVO-constitution-lamplitlabs",
            "# Ours\n\nShaped by the owner.\n",
            "human",
            "shaped",
        )
        .unwrap();
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        for item in result["projects"].as_array().unwrap() {
            let row = item.as_object().unwrap();
            if row["slug"] != slug {
                continue;
            }
            let constitution = loop_record(row, "constitution");
            let evaluators = loop_record(row, "evaluators");
            assert_eq!(constitution["rev"], 2);
            assert_eq!(constitution["seeded"], false);
            assert_eq!(
                constitution["seeded_from"],
                format!(
                    "{}/TPL-evolution-constitution@r2",
                    pulse_store::COMMONS_SLUG
                )
            );
            assert_eq!(evaluators["seeded"], true);
        }
        let (_, data) =
            http::additions_request(h, "GET", &format!("/api/v1/projects/{slug}/evolution"), "");
        let doc = data["constitution"].as_object().unwrap();
        assert_eq!(doc["seeded"], false);
        assert_eq!(
            doc["seeded_from"],
            format!(
                "{}/TPL-evolution-constitution@r2",
                pulse_store::COMMONS_SLUG
            )
        );
        http::additions_record(
            &s,
            "EVO-constitution-raw",
            "raw",
            "evolution",
            "constitution",
            "# Raw\n",
        );
        let data = http::handler(&s).evolution("raw".into()).unwrap();
        let doc = data["constitution"].as_object().unwrap();
        assert_eq!(doc["seeded"], false);
        assert_eq!(doc["seeded_from"], "");
    }

    #[test]
    fn console_adopts_or_discards_a_draft_and_queues_the_drafting_job() {
        let _serial = crate::test_helpers::serial();
        let (s, slug) = template_store();
        let h = http::handler(&s);
        let id = format!("EVO-constitution-{slug}");
        let (status, data) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/draft-job"),
            "{}",
        );
        assert_eq!(status, 200, "draft-job: {data:?}");
        let job = data["job"].as_object().unwrap().clone();
        assert_eq!(job["status"], "queued");
        assert_eq!(data["created"], true);
        assert_eq!(data["reason"], "");
        let mut kind = String::new();
        let mut source = String::new();
        let mut created_by = String::new();
        s.conn()
            .unwrap()
            .query_row(
                "SELECT kind, source, created_by FROM jobs WHERE id = ?",
                [job["id"].as_str().unwrap()],
                |r| {
                    kind = r.get(0)?;
                    source = r.get(1)?;
                    created_by = r.get(2)?;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(kind, "reflect");
        assert!(source.starts_with(&format!("draft:{slug}:")));
        assert_eq!(created_by, pulse_store::HUMAN_AGENT);
        let (status, again) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/draft-job"),
            "{}",
        );
        assert_eq!(status, 200);
        assert_eq!(again["created"], false);
        assert_eq!(again["job"]["id"], job["id"]);
        assert!(
            again["reason"]
                .as_str()
                .unwrap()
                .contains("a draft job is queued")
        );
        let (_, rows) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        for item in rows["projects"].as_array().unwrap() {
            let row = item.as_object().unwrap();
            if row["slug"] != slug {
                continue;
            }
            let c = row["constitution"].as_object().unwrap();
            let ref_job = c["draft_job"].as_object().unwrap();
            assert_eq!(ref_job["id"], job["id"]);
            assert_eq!(ref_job["status"], "queued");
            assert_eq!(c["draft"], Value::Null);
        }
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status = 'done' WHERE id = ?",
                [job["id"].as_str().unwrap()],
            )
            .unwrap();
        let body = "# Lamplit Labs Evolution Constitution\n\nThe Lamplit Labs loop edits `src/`.\n";
        pulse_records::draft(
            &s,
            &id,
            pulse_records::DraftInput {
                body: body.into(),
                author: "aurora".into(),
                note: "from the README".into(),
                job_id: job["id"].as_str().unwrap().into(),
            },
        )
        .unwrap();
        let (_, rows) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        for item in rows["projects"].as_array().unwrap() {
            let row = item.as_object().unwrap();
            if row["slug"] != slug {
                continue;
            }
            let c = row["constitution"].as_object().unwrap();
            let draft = c["draft"].as_object().unwrap();
            assert_eq!(draft["author"], "aurora");
            assert_eq!(draft["note"], "from the README");
            assert_eq!(draft["base_rev"], 1);
            assert_eq!(draft["job_id"], job["id"]);
            assert_eq!(draft["body"], Value::Null);
            assert_eq!(c["draft_job"], Value::Null);
            assert_eq!(c["seeded"], true);
        }
        let (_, doc) = http::additions_request(
            h.clone(),
            "GET",
            &format!("/api/v1/projects/{slug}/evolution"),
            "",
        );
        let draft = doc["constitution"]["draft"].as_object().unwrap();
        assert_eq!(draft["body"], body);
        assert_eq!(draft["author"], "aurora");
        let (status, rec) =
            http::additions_request(h.clone(), "GET", &format!("/api/v1/records/{id}"), "");
        assert_eq!(status, 200);
        assert_eq!(rec["draft"]["body"], body);
        assert_ne!(rec["body"], body);
        let (status, data) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/draft-job"),
            "{}",
        );
        let title: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT title FROM jobs WHERE id = ?",
                [data["job"]["id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(data["created"], true);
        assert_eq!(
            title,
            format!("Draft the evaluators of {slug} from its repository")
        );
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status = 'done' WHERE id = ?",
                [data["job"]["id"].as_str().unwrap()],
            )
            .unwrap();
        pulse_records::draft(
            &s,
            &format!("EVO-evaluators-{slug}"),
            pulse_records::DraftInput {
                body: "# Lamplit Labs Evaluators\n\nThe Lamplit Labs loop scores `make test`.\n"
                    .into(),
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (status, data) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/draft-job"),
            "{}",
        );
        assert_eq!(status, 409);
        assert!(data["why"].as_str().unwrap().contains("draft waiting"));
        let (status, data) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/discard"),
            r#"{"words":"wrong paths"}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(data["discarded"], true);
        assert!(s.get_draft(&id).unwrap().is_none());
        pulse_records::draft(
            &s,
            &id,
            pulse_records::DraftInput {
                body: body.into(),
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (status, data) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/adopt"),
            r#"{"words":"paths checked"}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(data["record"]["current_rev"], 2);
        let (head, rev) = s.get_record(&id, 0).unwrap();
        let rev = rev.unwrap();
        assert_eq!(rev.body, body);
        assert_eq!(rev.author, pulse_store::HUMAN_AGENT);
        assert_eq!(
            rev.note,
            "adopted the draft by aurora: paths checked (via console)"
        );
        assert_eq!(head.current_rev, 2);
        let (_, rows) = http::additions_request(h.clone(), "GET", "/api/v1/projects", "");
        for item in rows["projects"].as_array().unwrap() {
            let row = item.as_object().unwrap();
            if row["slug"] != slug {
                continue;
            }
            let c = row["constitution"].as_object().unwrap();
            assert_eq!(c["seeded"], false);
            assert_eq!(c["rev"], 2);
            assert_eq!(c["draft"], Value::Null);
        }
        let (status, _) = http::additions_request(
            h.clone(),
            "POST",
            &format!("/api/v1/records/{id}/adopt"),
            "{}",
        );
        assert_ne!(status, 200);
        let (status, _) =
            http::additions_request(h, "GET", &format!("/api/v1/records/{id}/adopt"), "");
        assert_eq!(status, 405);
    }
}
