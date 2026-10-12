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
    pub(crate) fn running(&self, r: &crate::httpx::Request) -> anyhow::Result<JsonMap> {
        let mut hub = JsonMap::new();
        hub.insert("version".into(), json!(pulse_cli::version()));
        hub.insert("revision".into(), json!(pulse_cli::revision()));
        hub.insert("pid".into(), json!(std::process::id()));
        let (ok, applicable) = pulse_cli::code_signed(&pulse_cli::executable());
        let signed = if applicable { json!(ok) } else { Value::Null };
        let Some(store) = &self.store else {
            return Ok(map_from_pairs(vec![
                ("available", json!(false)),
                ("self_update", json!("")),
                ("retry_minutes", json!(0)),
                ("hub", json!(hub)),
                ("daemon", Value::Null),
                ("brain", Value::Null),
                ("last", Value::Null),
                ("state", json!("no-store")),
                ("signed", signed),
            ]));
        };
        let self_update = store.setting("daemon.self_update", "on");
        let mut retry = store.setting_int("daemon.self_update_retry_minutes");
        if retry <= 0 {
            retry = 60;
        }
        let daemon = match store.meta("daemon") {
            Ok((raw, true)) => serde_json::from_str::<Value>(&raw).ok().map(|beat| {
                json!(map_from_pairs(vec![
                    ("pid", beat.get("pid").cloned().unwrap_or(Value::Null)),
                    (
                        "at",
                        json!(beat.get("at").and_then(Value::as_str).unwrap_or(""))
                    ),
                    (
                        "version",
                        json!(beat.get("version").and_then(Value::as_str).unwrap_or(""))
                    ),
                    (
                        "revision",
                        json!(beat.get("revision").and_then(Value::as_str).unwrap_or(""))
                    ),
                    ("bus", beat.get("bus").cloned().unwrap_or(Value::Null)),
                ]))
            }),
            Ok(_) => None,
            Err(err) => return Err(err),
        };
        let brain = store.brain()?.map(|p| {
            let head = crate::board::git_value(&p.path, &["rev-parse", "HEAD"]);
            let branch = crate::board::git_value(&p.path, &["rev-parse", "--abbrev-ref", "HEAD"]);
            let dirty = !crate::board::git_value(&p.path, &["status", "--porcelain"]).is_empty();
            json!(map_from_pairs(vec![
                ("slug", json!(p.slug)),
                ("default_branch", json!(p.default_branch)),
                ("branch", json!(branch)),
                ("head", json!(head)),
                ("dirty", json!(dirty)),
            ]))
        });
        let mut last = Value::Null;
        if let Ok(rows) = self.addition_rows(
            r,
            "SELECT at, detail FROM events WHERE action = 'daemon.self_update' ORDER BY seq DESC LIMIT 1",
            Vec::new(),
        ) && let Some(row) = rows.first() {
            let detail = row
                .get("detail")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or(Value::Null);
            last = json!(map_from_pairs(vec![
                ("at", row.get("at").cloned().unwrap_or(Value::String(String::new()))),
                ("from", json!(detail.get("from").and_then(Value::as_str).unwrap_or(""))),
                ("to", json!(detail.get("to").and_then(Value::as_str).unwrap_or(""))),
                ("inflight", json!(detail.get("inflight").and_then(Value::as_i64).unwrap_or(0))),
            ]));
        }
        let daemon_revision = daemon
            .as_ref()
            .and_then(|d| d.get("revision"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let state = if self_update != "on" {
            "off"
        } else if daemon.is_none() {
            "no-daemon"
        } else if brain.is_none() {
            "no-brain"
        } else if daemon_revision.is_empty() {
            "unstamped"
        } else if let Some(b) = brain.as_ref() {
            let head = b.get("head").and_then(Value::as_str).unwrap_or("");
            let branch = b.get("branch").and_then(Value::as_str).unwrap_or("");
            let default_branch = b
                .get("default_branch")
                .and_then(Value::as_str)
                .unwrap_or("");
            let dirty = b.get("dirty").and_then(Value::as_bool).unwrap_or(false);
            if !head.is_empty() && head != daemon_revision {
                if dirty || (branch != default_branch && !branch.starts_with("HEAD")) {
                    "waiting"
                } else {
                    "behind"
                }
            } else {
                "current"
            }
        } else {
            "no-brain"
        };
        Ok(map_from_pairs(vec![
            ("available", json!(true)),
            ("self_update", json!(self_update)),
            ("retry_minutes", json!(retry)),
            ("hub", json!(hub)),
            ("daemon", daemon.unwrap_or(Value::Null)),
            ("brain", brain.unwrap_or(Value::Null)),
            ("last", last),
            ("state", json!(state)),
            ("signed", signed),
        ]))
    }
    pub(crate) fn addition_running(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) {
        if !crate::server::read_method(w, r) {
            return;
        }
        match self.running(r) {
            Ok(data) => self.respond(w, json!(data), String::new()),
            Err(err) => self.respond(w, Value::Null, err.to_string()),
        }
    }
}

fn map_from_pairs(pairs: Vec<(&str, Value)>) -> JsonMap {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http as http;
    use serde_json::{Value, json};
    use std::process::Command;

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

    fn must_output(cmd: &mut Command) -> Vec<u8> {
        cmd.envs(pulse_gitenv::env().into_iter().filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        }));
        let out = cmd.output().expect("run command");
        assert!(
            out.status.success(),
            "{:?}: {}\n{}",
            cmd,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn documented_difference(slug: &str, route: &str, path: &str) -> &'static str {
        if route == "roster" && path.starts_with("$[") {
            let field = path.split_once("].").map(|(_, f)| f).unwrap_or("");
            match field {
                "presence"
                | "presence_reason"
                | "last_heartbeat_at"
                | "heartbeat_source"
                | "heartbeat_age_seconds"
                | "dead_after_seconds"
                | "presence_session"
                | "presence_checked_at"
                | "presence_expires_at" => {
                    return "WU-024 adds supervised presence separately from unchanged identity, score and history fields; the heartbeat feature adds the beat's source, age and the dead-after threshold";
                }
                "current_job" | "current_job_kind" | "current_job_title" | "current_thread"
                | "executor" | "focus_since" | "current_task" | "current_task_title"
                | "current_project" | "current_source" => {
                    return "agents-focus-roster adds current work/task fields separately from identity, score and history fields";
                }
                _ => {}
            }
        }
        if slug == "org" && route == "roster" && path.contains("active_claims[0].presence") {
            return "";
        }
        ""
    }

    fn roster_golden(value: Value, legacy: bool) -> Value {
        let mut rows = value.as_array().cloned().expect("roster array");
        for row in &mut rows {
            if legacy
                && row.get("status").and_then(Value::as_str) == Some("working")
                && row.get("agent_status").is_some()
            {
                let status = row["agent_status"].clone();
                row.as_object_mut().unwrap().insert("status".into(), status);
            }
        }
        rows.sort_by(|a, b| {
            a.get("agent")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("agent").and_then(Value::as_str).unwrap_or(""))
        });
        Value::Array(rows)
    }

    #[test]
    fn running_names_the_builds_and_the_state() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(true);
        let h = http::handler(&s);
        let (status, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(status, 200);
        assert_eq!(result["available"], true);
        assert_eq!(result["state"], "no-daemon");
        assert_eq!(result["self_update"], "on");
        assert_eq!(result["retry_minutes"], 60);
        let hub = result["hub"].as_object().unwrap();
        assert_ne!(hub["version"], "");
        assert!(hub["pid"] != Value::Null);
        assert_eq!(result["daemon"], Value::Null);
        assert_eq!(result["brain"], Value::Null);
        assert_eq!(result["last"], Value::Null);
        let repo = lifecycle_repo();
        let head = String::from_utf8(must_output(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("rev-parse")
                .arg("HEAD"),
        ))
        .unwrap()
        .trim()
        .to_string();
        s.conn().unwrap().execute("INSERT INTO projects(slug,name,path,brain,default_branch,added_at) VALUES('brain','brain',?,1,'main',?)", rusqlite::params![repo, s.now()]).unwrap();
        let beat = |revision: &str| {
            s.conn().unwrap().execute("INSERT INTO meta (key, value) VALUES ('daemon', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [format!(r#"{{"pid":4242,"at":"2026-09-06T11:59:00Z","loops":3,"interval":300,"note":"","version":"dev-{}","revision":"{}","bus":{{"subscribers":2,"dropped":7,"deduped":3}}}}"#, &revision[..std::cmp::min(12, revision.len())], revision)]).unwrap();
        };
        beat("");
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(result["state"], "unstamped");
        let brain = result["brain"].as_object().unwrap();
        assert_eq!(brain["slug"], "brain");
        assert_eq!(brain["branch"], "main");
        assert_eq!(brain["head"], head);
        assert_eq!(brain["dirty"], false);
        beat(&head);
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(result["state"], "current");
        let daemon = result["daemon"].as_object().unwrap();
        assert_eq!(daemon["pid"], 4242);
        assert_eq!(daemon["revision"], head);
        assert_eq!(daemon["version"], format!("dev-{}", &head[..12]));
        let bus = daemon["bus"].as_object().unwrap();
        assert_eq!(bus["subscribers"], 2);
        assert_eq!(bus["dropped"], 7);
        assert_eq!(bus["deduped"], 3);
        beat("0123456789abcdef0123456789abcdef01234567");
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(result["state"], "behind");
        must_output(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("checkout")
                .arg("-q")
                .arg("-b")
                .arg("topic"),
        );
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(result["state"], "waiting");
        must_output(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("checkout")
                .arg("-q")
                .arg("main"),
        );
        s.conn().unwrap().execute("INSERT INTO events (at, actor, action, subject, detail) VALUES ('2026-09-06T11:58:00Z','daemon','daemon.self_update','pulse',?)", [format!(r#"{{"from":"0123456789ab","to":"{}","binary":"/x/pulse","inflight":2}}"#, head)]).unwrap();
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_ne!(result["last"], Value::Null);
        let last = result["last"].as_object().unwrap();
        assert_eq!(last["to"], head);
        assert_eq!(last["inflight"], 2);
        assert_eq!(last["at"], "2026-09-06T11:58:00Z");
        s.conn().unwrap().execute("INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('daemon.self_update','off',?, 'owner') ON CONFLICT(name) DO UPDATE SET value = excluded.value", [s.now()]).unwrap();
        let (_, result) = http::additions_request(h.clone(), "GET", "/api/v1/running", "");
        assert_eq!(result["state"], "off");
        assert_eq!(result["self_update"], "off");
        let (status, _) = http::additions_request(h, "POST", "/api/v1/running", "{}");
        assert_eq!(status, 405);
    }

    #[test]
    fn roster_presence_history_claims_and_shared_clock() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let exec = |query: &str, args: &[&dyn rusqlite::ToSql]| {
            s.conn().unwrap().execute(query, args).unwrap();
        };
        // "offline" is a worker whose supervised heartbeat went silent six minutes ago: dead.
        // "unknown" is a session agent with no sign of life on record at all.
        let states = std::collections::BTreeMap::from([
            ("live", "online"),
            ("offline", "dead"),
            ("idle-worker", "idle"),
            ("idle-lead", "idle"),
            ("unknown", "unknown"),
            ("legacy", "legacy"),
            ("pruned", "pruned"),
        ]);
        for name in states.keys() {
            let kind = match *name {
                "unknown" => "session",
                "legacy" => "legacy",
                "idle-lead" => "lead",
                _ => "worker",
            };
            pulse_board::join_agent(
                &s,
                pulse_board::JoinInput {
                    name: (*name).into(),
                    kind: kind.into(),
                    session: format!("run-{name}"),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        exec(
            "UPDATE agents SET status='pruned',pruned_at=?,pruned_reason='no useful output for 14 days' WHERE name='pruned'",
            &[&s.now()],
        );
        // Joining counts as activity; these joined long before this clock reading, so the
        // supervised heartbeat is their latest sign of life.
        exec(
            "UPDATE agents SET last_active_at='2026-09-06T11:50:00Z' WHERE name IN ('live','offline')",
            &[],
        );
        exec(
            "UPDATE agents SET last_active_at='' WHERE name='unknown'",
            &[],
        );
        exec(
            "UPDATE agents SET jobs_done=3,posts=4,last_useful_at='2026-09-01T00:00:00Z' WHERE name='live'",
            &[],
        );
        exec(
            "INSERT INTO tasks(id,project,title,status,priority,created_by,created_at,updated_at,current_rev) VALUES('TASK-live','commons','Live owner task','open',0,'owner',?,?,1)",
            &[&s.now(), &s.now()],
        );
        let mut tokens = std::collections::BTreeMap::new();
        for name in ["live", "offline"] {
            let id = pulse_board::ExecutionIdentity {
                agent: name.into(),
                session: format!("run-{name}"),
                run_id: format!("run-{name}"),
                job_id: format!("job-{name}"),
                generation: 2,
                executor: "local".into(),
            };
            tokens.insert(name.to_string(), id.clone());
            let source = if name == "live" { "task:TASK-live" } else { "" };
            exec(
                "INSERT INTO jobs(id,project,kind,title,source,created_at,status,lease_agent,generation) VALUES(?,'commons','task','presence',?,?,'leased',?,2)",
                &[&id.job_id, &source, &s.now(), &name],
            );
            exec(
                "INSERT INTO runs(id,job_id,agent,project,started_at,status) VALUES(?,?,?,'commons','2026-09-06T11:50:00Z','running')",
                &[&id.run_id, &id.job_id, &name],
            );
            if name == "live" {
                exec(
                    "INSERT INTO posts(id,project,area,thread,kind,agent,date,expires,summary,created_at) VALUES('live-claim','commons','agents','live','claim','live',?,'2026-09-06T12:30:00Z','working here',?)",
                    &[&s.now(), &s.now()],
                );
            }
            let at = if name == "offline" {
                "2026-09-06T11:54:00Z"
            } else {
                "2026-09-06T11:59:00Z"
            };
            assert!(
                pulse_board::record_heartbeat(
                    &s,
                    pulse_board::ExecutionHeartbeat {
                        identity: id,
                        at: at.into(),
                        ..Default::default()
                    }
                )
                .unwrap()
            );
        }
        let views = vec![crate::read::ProjectView {
            project: pulse_store::Project {
                slug: pulse_store::COMMONS_SLUG.into(),
                ..Default::default()
            },
            data: Some(std::sync::Arc::new(crate::board::BoardData {
                params: std::collections::BTreeMap::from([(
                    "now".into(),
                    json!("2026-09-06T11:00:00Z"),
                )]),
                posts: vec![
                    crate::board::PostRow::from_map(&crate::board::object(
                        "id agent kind date",
                        vec![
                            json!("old-post"),
                            json!("live"),
                            json!("finding"),
                            json!("2026-09-01T00:00:00Z"),
                        ],
                    )),
                    crate::board::PostRow::from_map(&crate::board::object(
                        "id agent kind date",
                        vec![
                            json!("imported-post"),
                            json!("imported"),
                            json!("finding"),
                            json!("2026-08-01T00:00:00Z"),
                        ],
                    )),
                ],
                claims: vec![
                    crate::board::object(
                        "id agent status thread_key",
                        vec![
                            json!("live-claim"),
                            json!("live"),
                            json!("active"),
                            json!("agents/live"),
                        ],
                    ),
                    crate::board::object(
                        "id agent status thread_key",
                        vec![
                            json!("stale-claim"),
                            json!("offline"),
                            json!("active"),
                            json!("agents/offline"),
                        ],
                    ),
                ],
                ..Default::default()
            })),
            ..Default::default()
        }];
        let h = http::handler(&s);
        let rows = h.roster(&views).unwrap();
        let by_name = rows
            .iter()
            .map(|r| (r["agent"].as_str().unwrap().to_string(), r.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut states = states;
        states.insert("imported", "legacy");
        for (name, want) in states {
            let r = by_name
                .get(name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(r["presence"], want, "{name}");
            assert_eq!(r["presence_checked_at"], s.now(), "{name}");
        }
        assert_eq!(by_name["live"]["status"], "working");
        assert_eq!(by_name["live"]["agent_status"], "active");
        assert_ne!(by_name["offline"]["status"], "working");
        assert_eq!(
            by_name["offline"]["active_claims"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let live = &by_name["live"];
        assert_eq!(live["last_useful_at"], "2026-09-01T00:00:00Z");
        assert_eq!(live["last_heartbeat_at"], "2026-09-06T11:59:00Z");
        assert_eq!(live["presence_expires_at"], "2026-09-06T12:04:00Z");
        assert_eq!(live["jobs_done"], 3);
        assert_eq!(live["posts"], 4);
        assert_eq!(live["current_job"], "job-live");
        assert_eq!(live["current_job_kind"], "task");
        assert_eq!(live["current_job_title"], "presence");
        assert_eq!(live["current_thread"], "agents/live");
        assert_eq!(live["executor"], "local");
        assert_eq!(live["focus_since"], "2026-09-06T11:50:00Z");
        assert_eq!(live["current_task"], "TASK-live");
        assert_eq!(live["current_task_title"], "Live owner task");
        assert_eq!(live["current_project"], "commons");
        assert_eq!(live["current_source"], "task:TASK-live");
        exec(
            "UPDATE agents SET last_useful_at='2026-09-06 11:56:24' WHERE name='live'",
            &[],
        );
        let tie_rows = h.roster(&views).unwrap();
        let tie_live = tie_rows
            .iter()
            .find(|r| r["agent"] == "live")
            .expect("live row after space-separated timestamp");
        assert_eq!(tie_live["last_useful_at"], "2026-09-06 11:56:24");
        assert_eq!(tie_live["days_since"], json!(0.002));
        let cards = pulse_board::scorecards(&s, true).unwrap();
        let inspect_cards = pulse_inspect::agent_scorecards(&s, true).unwrap();
        for list in [
            serde_json::to_value(&cards).unwrap(),
            serde_json::to_value(&inspect_cards).unwrap(),
        ] {
            for card in list.as_array().unwrap() {
                let r = &by_name[card["name"].as_str().unwrap()];
                assert_eq!(r["presence"], card["presence"]);
                assert_eq!(r["last_heartbeat_at"], card["last_heartbeat_at"]);
                assert_eq!(r["presence_expires_at"], card["presence_expires_at"]);
            }
        }
        let later = pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            now: "2026-09-06T12:04:00Z".into(),
            ..Default::default()
        })
        .unwrap();
        let expired = http::handler(&later).roster(&views).unwrap();
        for r in expired {
            if r["agent"] == "live" {
                assert_eq!(r["presence"], "dead");
                assert_ne!(r["status"], "working");
            }
        }
        let before = s.agent("live").unwrap();
        let ok = pulse_board::record_heartbeat(
            &later,
            pulse_board::ExecutionHeartbeat {
                identity: tokens["live"].clone(),
                at: later.now(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(ok);
        let returned = http::handler(&later).roster(&views).unwrap();
        assert_eq!(returned.len(), rows.len());
        for r in returned {
            if r["agent"] == "live" {
                assert_eq!(r["presence"], "online");
            }
        }
        let after = s.agent("live").unwrap();
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap(),
            "roster filtering or heartbeat changed identity/history"
        );
    }

    #[test]
    fn roster_golden_keeps_identity_and_history_assertions() {
        let _serial = crate::test_helpers::serial();
        let legacy = json!([{"agent":"zz","status":"working","agent_status":"active","jobs_done":3},{"agent":"aa","status":"pruned","agent_status":"pruned","jobs_done":4}]);
        let rows = roster_golden(legacy.clone(), true);
        let rows = rows.as_array().unwrap();
        assert_eq!(rows[0]["status"], "pruned");
        assert_eq!(rows[1]["status"], "active");
        assert_eq!(rows[1]["jobs_done"], 3);
        assert_eq!(legacy[0]["status"], "working");
        for field in [
            "presence",
            "presence_reason",
            "last_heartbeat_at",
            "heartbeat_source",
            "heartbeat_age_seconds",
            "dead_after_seconds",
            "presence_session",
            "presence_checked_at",
            "presence_expires_at",
            "current_job",
            "current_job_kind",
            "current_job_title",
            "current_thread",
            "executor",
            "focus_since",
            "current_task",
            "current_task_title",
            "current_project",
            "current_source",
        ] {
            assert_ne!(
                documented_difference("org", "roster", &format!("$[0].{field}")),
                "",
                "added field {field} is undocumented"
            );
        }
        for field in [
            "status",
            "agent_status",
            "jobs_done",
            "pruned_reason",
            "sessions",
            "active_claims[0].presence",
            "length",
        ] {
            assert_eq!(
                documented_difference("org", "roster", &format!("$[0].{field}")),
                "",
                "legacy field {field} must still compare exactly"
            );
        }
    }

    #[test]
    fn runner_view_carries_landing_holds() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(true);
        let h = http::handler(&s);
        let (code, body) = http::additions_request(h.clone(), "GET", "/api/v1/runner", "");
        assert_eq!(code, 200);
        assert_eq!(body["available"], true);
        assert_eq!(body["landing"].as_array().unwrap().len(), 0);
        s.conn().unwrap().execute(r#"INSERT INTO meta(key,value) VALUES
        ('daemon','{"pid":123,"at":"2026-09-06T11:59:30Z","loops":4,"interval":30,"note":""}'),
        ('landing','{"proj":{"at":"2026-09-06T11:59:31Z","note":"3 branches waiting; deferred: the checkout of /w/proj has uncommitted changes"}}')"#, []).unwrap();
        let (code, body) = http::additions_request(h, "GET", "/api/v1/runner", "");
        assert_eq!(code, 200);
        let held = body["landing"].as_array().unwrap();
        assert_eq!(held.len(), 1);
        let note = held[0].as_object().unwrap();
        assert_eq!(note["project"], "proj");
        assert_eq!(note["stale"], false);
        assert_eq!(
            note["note"],
            "3 branches waiting; deferred: the checkout of /w/proj has uncommitted changes"
        );
    }

    #[test]
    fn runner_view_carries_capacity_hold() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(true);
        let h = http::handler(&s);
        const HOLD: &str = "workers on this machine wait for a GitHub Copilot login: Sign in with GitHub (Settings -> Harness) or run pulse harness login";
        s.conn().unwrap().execute(&format!(r#"INSERT INTO meta(key,value) VALUES ('capacity','{{"queued":2,"slots":0,"inflight":0,"starved_since":"","held":"{HOLD}"}}')"#), []).unwrap();
        let (code, body) = http::additions_request(h, "GET", "/api/v1/runner", "");
        assert_eq!(code, 200);
        let want_capacity = format!("2 job(s) queued, 0 slot(s), 0 in flight; {HOLD}");
        assert_eq!(body["capacity"], want_capacity);
        assert_eq!(body["hold"], HOLD);
    }

    #[test]
    fn setting_labels_cover_catalogue() {
        let _serial = crate::test_helpers::serial();
        let src = match std::fs::read_to_string("../../web/hub/src/lib/setting-labels.ts") {
            Ok(src) => src,
            Err(err) => {
                eprintln!("setting-labels.ts not in this checkout: {err}");
                return;
            }
        };
        let label_re =
            regex::Regex::new(r#"(?m)^\s*"?([a-z_][a-z0-9_.]*)"?:\s*"[^"]+",?$"#).unwrap();
        let labels = label_re
            .captures_iter(&src)
            .map(|m| m[1].to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            labels.len() >= 10,
            "parsed only {} labels from setting-labels.ts; the LABELS map changed shape",
            labels.len()
        );
        let mut missing = Vec::new();
        let mut names = pulse_store::setting_names();
        names.extend(pulse_store::extra_setting_names());
        for name in names {
            if !labels.contains(&name) {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "settings without a human label in web/hub/src/lib/setting-labels.ts: {}",
            missing.join(", ")
        );
    }
}
