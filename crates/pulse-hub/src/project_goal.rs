use crate::additions::{ADDITION_WRITES, addition_available, addition_failure, addition_method};
use crate::server::{Handler, write_error, write_json};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde_json::{Value, json};
use std::fmt;

impl Handler {
    pub(crate) fn addition_project_goal<S: AsRef<str>>(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: S,
    ) {
        let slug = slug.as_ref();
        if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()])
            || !addition_available(
                self,
                w,
                r,
                vec!["projects".to_string(), "settings".to_string()],
            )
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let mut actor = r.header("X-Pulse-Agent");
        if actor.is_empty() {
            actor = pulse_store::HUMAN_AGENT.to_string();
        }
        let goal = if r.method == "POST" {
            let (text, reset, ok) = read_project_goal_payload(w, r);
            if !ok {
                return;
            }
            if pulse_goals::require_owner(&store.home, &actor).is_err()
                || !r.header("X-Pulse-Job").is_empty()
            {
                write_error(
                    w,
                    403,
                    "Only the owner can change a project goal".to_string(),
                    "An agent or worker cannot set or reset project direction.".to_string(),
                    "Use an owner session.".to_string(),
                );
                return;
            }
            let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
            if reset {
                pulse_goals::reset(store, slug, pulse_store::HUMAN_AGENT)
            } else {
                pulse_goals::set(store, slug, &text, pulse_store::HUMAN_AGENT)
            }
        } else {
            pulse_goals::get(store, slug)
        };
        let goal = match goal {
            Ok(v) => v,
            Err(e) => {
                let text = e.to_string();
                if text.starts_with("Cannot change an inactive project's goal") {
                    write_error(
                        w,
                        400,
                        "Cannot change an inactive project's goal".to_string(),
                        "Removed projects keep their goal for restoration.".to_string(),
                        "Restore the project before editing its goal.".to_string(),
                    );
                    return;
                }
                if text.starts_with("Project goals require a registered project") {
                    write_error(
                        w,
                        400,
                        "Project goals require a registered project".to_string(),
                        "Goals belong to registered projects, not the commons.".to_string(),
                        "pulse projects".to_string(),
                    );
                    return;
                }
                addition_failure(w, text);
                return;
            }
        };
        let p = match store.project(slug) {
            Ok(Some(v)) => v,
            Ok(None) => pulse_store::Project {
                slug: slug.to_string(),
                ..Default::default()
            },
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let can_edit =
            pulse_goals::can_edit(store, &p, &actor) && r.header("X-Pulse-Job").is_empty();
        write_json(
            w,
            200,
            json!({"project": slug, "goal": goal_value(goal), "can_edit": can_edit}),
        );
    }
}

pub(crate) fn read_project_goal_payload(
    w: &mut crate::httpx::ResponseWriter,
    r: &crate::httpx::Request,
) -> (String, bool, bool) {
    let origin = r.header("Origin");
    if !origin.is_empty() {
        let parsed = url::Url::parse(&origin);
        if parsed
            .ok()
            .filter(|u| {
                u.host_str()
                    .map(|host| {
                        let host = match u.port() {
                            Some(port) => format!("{host}:{port}"),
                            None => host.to_string(),
                        };
                        host == r.host() && (u.scheme() == "http" || u.scheme() == "https")
                    })
                    .unwrap_or(false)
            })
            .is_none()
        {
            write_error(
                w,
                403,
                "Cross-origin writes are not allowed".to_string(),
                "The request did not originate at this hub.".to_string(),
                "Open the hub on 127.0.0.1.".to_string(),
            );
            return (String::new(), false, false);
        }
    }
    if r.header("Sec-Fetch-Site") == "cross-site" {
        write_error(
            w,
            403,
            "Cross-origin writes are not allowed".to_string(),
            "The browser reports a cross-site request.".to_string(),
            "Open the hub on 127.0.0.1.".to_string(),
        );
        return (String::new(), false, false);
    }
    let media = r
        .header("Content-Type")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if media != "application/json" {
        write_error(
            w,
            415,
            "Content-Type must be application/json".to_string(),
            "Only JSON writes are accepted.".to_string(),
            "Send Content-Type: application/json.".to_string(),
        );
        return (String::new(), false, false);
    }
    const EXTRA: usize = 1024;
    let max_body = pulse_goals::MAX_LENGTH * 12 + EXTRA;
    let bad = |w: &mut crate::httpx::ResponseWriter| {
        write_error(
            w,
            400,
            "Invalid project goal request".to_string(),
            "Send exactly one UTF-8 JSON object: {text:string} or {reset:true}, without duplicate or unknown fields.".to_string(),
            "Edit the goal text (at most 4000 Unicode code points) or reset to the generated default.".to_string(),
        );
        (String::new(), false, false)
    };
    if r.body.is_empty() || r.body.len() > max_body || std::str::from_utf8(&r.body).is_err() {
        return bad(w);
    }
    let mut de = serde_json::Deserializer::from_slice(&r.body);
    let parsed = ProjectGoalSeed.deserialize(&mut de);
    let Ok((text, reset)) = parsed else {
        return bad(w);
    };
    if de.end().is_err() {
        return bad(w);
    }
    if text.contains('\u{fffd}') {
        return bad(w);
    }
    (text, reset, true)
}

struct ProjectGoalSeed;

impl<'de> DeserializeSeed<'de> for ProjectGoalSeed {
    type Value = (String, bool);

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(ProjectGoalVisitor)
    }
}

struct ProjectGoalVisitor;

impl<'de> Visitor<'de> for ProjectGoalVisitor {
    type Value = (String, bool);

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("one-field project goal object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let Some(field) = map.next_key::<String>()? else {
            return Err(de::Error::custom("empty"));
        };
        let out = match field.as_str() {
            "text" => {
                let text = map.next_value::<String>()?;
                (text, false)
            }
            "reset" => {
                let reset = map.next_value::<bool>()?;
                if !reset {
                    return Err(de::Error::custom("reset false"));
                }
                (String::new(), true)
            }
            _ => return Err(de::Error::custom("unknown field")),
        };
        if map.next_key::<String>()?.is_some() {
            return Err(de::Error::custom("extra field"));
        }
        Ok(out)
    }
}

fn goal_value(goal: pulse_goals::Goal) -> Value {
    json!({
        "text": goal.text,
        "source": goal.source,
        "updated_at": goal.updated_at,
        "updated_by": goal.updated_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_f2 as f2;
    use serde_json::{Value, json};

    fn must_json<T: serde::Serialize>(v: &T) -> String {
        pulse_store::gojson::to_string(v).unwrap()
    }

    fn project_goal_request(
        handler: Handler,
        headers: &[(&str, &str)],
        content_type: &str,
    ) -> (u16, String) {
        let h = std::sync::Arc::new(handler);
        let server = crate::test_helpers::TestServer::start(std::sync::Arc::new(move |w, r| {
            h.serve_http(w, r)
        }));
        let mut req = crate::test_helpers::client()
            .post(format!("{}/api/v1/projects/one/goal", server.url))
            .header("Content-Type", content_type)
            .body(r#"{"text":"Impersonate owner."}"#.to_string());
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let response = req.send().unwrap();
        let status = response.status().as_u16();
        let body = response.text().unwrap();
        (status, body)
    }

    #[test]
    fn project_goal_api_contract_persistence_and_reset() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        f2::additions_project(&s, "two");
        let (status, result) =
            f2::additions_request(f2::handler(&s), "GET", "/api/v1/projects/one/goal", "");
        assert_eq!(status, 200);
        assert_eq!(result["project"], "one");
        assert_eq!(result["can_edit"], true);
        assert_eq!(result.len(), 3);
        let initial = result["goal"].as_object().unwrap().clone();
        assert_eq!(initial.len(), 4);
        assert_eq!(initial["source"], pulse_goals::GENERATED);
        assert_eq!(initial["updated_by"], pulse_goals::GENERATED_BY);
        assert_ne!(initial["updated_at"], "");
        assert_ne!(initial["text"], "");
        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        let want = "Keep search useful, correct and reliable.";
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            "/api/v1/projects/one/goal",
            &format!(r#"{{"text":"  {want}  "}}"#),
        );
        assert_eq!(status, 200);
        assert_eq!(result["goal"]["text"], want);
        assert_eq!(result["goal"]["source"], pulse_goals::OWNER);
        assert_eq!(result["goal"]["updated_by"], pulse_store::HUMAN_AGENT);
        let (status, other) =
            f2::additions_request(f2::handler(&s), "GET", "/api/v1/projects/two/goal", "");
        assert_eq!(status, 200);
        assert_eq!(other["goal"]["source"], pulse_goals::GENERATED);
        let ro = pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            read_only: true,
            ..Default::default()
        })
        .unwrap();
        let (status, result) =
            f2::additions_request(f2::handler(&ro), "GET", "/api/v1/projects/one/goal", "");
        assert_eq!(status, 200);
        assert_eq!(result["goal"]["text"], want);
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            "/api/v1/projects/one/goal",
            r#"{"reset":true}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["goal"]["text"], initial["text"]);
        assert_eq!(result["goal"]["source"], pulse_goals::GENERATED);
        assert_eq!(result["goal"]["updated_by"], pulse_store::HUMAN_AGENT);
    }

    #[test]
    fn project_goal_api_rejects_invalid_input_without_writes() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        let bodies = vec![
            "{}".to_string(),
            "null".to_string(),
            "[]".to_string(),
            r#"{"text":null}"#.to_string(),
            r#"{"text":false}"#.to_string(),
            r#"{"text":42}"#.to_string(),
            r#"{"text":[]}"#.to_string(),
            r#"{"text":""}"#.to_string(),
            "{\"text\":\" \\t\\n \"}".to_string(),
            r#"{"reset":false}"#.to_string(),
            r#"{"reset":"true"}"#.to_string(),
            r#"{"reset":null}"#.to_string(),
            r#"{"text":"Direction.","reset":true}"#.to_string(),
            r#"{"text":"Direction.","reset":false}"#.to_string(),
            r#"{"unknown":true}"#.to_string(),
            r#"{"text":"Direction.","actor":"human"}"#.to_string(),
            r#"{"text":"Direction.","source":"generated"}"#.to_string(),
            r#"{"text":"Direction.","text":"Different direction."}"#.to_string(),
            r#"{"reset":true,"reset":true}"#.to_string(),
            r#"{"text":"Direction."} {}"#.to_string(),
            r#"{"text":"Direction.""#.to_string(),
            r#"{"text":"NUL\u0000"}"#.to_string(),
            r#"{"text":"unsafe\u001b"}"#.to_string(),
            r#"{"text":"misleading\u202e"}"#.to_string(),
            r#"{"text":"Use ghp_syntheticvalue12345"}"#.to_string(),
            r#"{"text":"Read ~/.ssh/id_test"}"#.to_string(),
            must_json(&json!({"text": "界".repeat(4001)})),
            "{\"text\":\"\u{fffd}\"}".to_string(),
        ];
        let before: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        for body in bodies {
            let (status, result) =
                f2::additions_request(f2::handler(&s), "POST", "/api/v1/projects/one/goal", &body);
            assert_eq!(status, 400, "{body} {result:?}");
            assert!(result.get("error").is_some());
            assert!(result.get("why").is_some());
            assert!(result.get("fix").is_some());
        }
        let after: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, before);
        let settings: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(settings, 0);
    }

    #[test]
    fn project_goal_api_private_path_references() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        let owner =
            pulse_goals::set(&s, "one", "Improve /api/search reliability.", "human").unwrap();
        let before: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        for text in [
            "Use file:///Users/example/plan.md.",
            "Prioritize path=/home/example/plan.md.",
            "Prioritize path=\"FILE://LOCALHOST/HOME/x\".",
            "Use [file://server/Users/example/plan.md].",
            "Use file://[::1]/home/x.",
            "Use file:///C:/work/plan.md.",
        ] {
            let (status, result) = f2::additions_request(
                f2::handler(&s),
                "POST",
                "/api/v1/projects/one/goal",
                &must_json(&json!({"text": text})),
            );
            assert_eq!(status, 400, "{result:?}");
        }
        assert_eq!(pulse_goals::get(&s, "one").unwrap(), owner);
        let after: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, before);
        let text = "Prioritize path=/api/search.";
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            "/api/v1/projects/one/goal",
            &must_json(&json!({"text": text})),
        );
        assert_eq!(status, 200);
        assert_eq!(result["goal"]["text"], text);
    }

    #[test]
    fn project_goal_api_unicode_limit_including_escapes() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        let text = "🚀".repeat(4000);
        for body in [
            must_json(&json!({"text": text})),
            format!(r#"{{"text":"{}"}}"#, r#"\ud83d\ude80"#.repeat(4000)),
        ] {
            let (status, result) =
                f2::additions_request(f2::handler(&s), "POST", "/api/v1/projects/one/goal", &body);
            assert_eq!(status, 200);
            assert_eq!(result["goal"]["text"], text);
        }
    }

    #[test]
    fn project_goal_api_scope_ownership_and_methods() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        f2::additions_project(&s, "inactive");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET status = 'inactive' WHERE slug = 'inactive'",
                [],
            )
            .unwrap();
        for (slug, want) in [("missing", 404), ("inactive", 400), ("commons", 400)] {
            for body in [r#"{"text":"Change direction."}"#, r#"{"reset":true}"#] {
                let (status, _) = f2::additions_request(
                    f2::handler(&s),
                    "POST",
                    &format!("/api/v1/projects/{slug}/goal"),
                    body,
                );
                assert_eq!(status, want, "{slug}");
            }
        }
        let (status, result) =
            f2::additions_request(f2::handler(&s), "GET", "/api/v1/projects/inactive/goal", "");
        assert_eq!(status, 200);
        assert_eq!(result["can_edit"], false);
        let (status, _) =
            f2::additions_request(f2::handler(&s), "DELETE", "/api/v1/projects/one/goal", "");
        assert_eq!(status, 405);
        for (name, agent, job) in [
            ("agent", "aurora", ""),
            ("worker", "human", "job-worker"),
            ("cleared-agent", "", "job-worker"),
        ] {
            let _agent = pulse_testutil::setenv("PULSE_AGENT", agent);
            let _job = pulse_testutil::setenv("PULSE_JOB", job);
            let (status, result) =
                f2::additions_request(f2::handler(&s), "GET", "/api/v1/projects/one/goal", "");
            assert_eq!(status, 200, "{name}");
            assert_eq!(result["can_edit"], false, "{name}");
            for body in [r#"{"text":"Impersonate owner."}"#, r#"{"reset":true}"#] {
                let (status, result) = f2::additions_request(
                    f2::handler(&s),
                    "POST",
                    "/api/v1/projects/one/goal",
                    body,
                );
                assert_eq!(status, 403, "worker write: {name} {result:?}");
            }
        }
        for headers in [
            vec![("X-Pulse-Agent", "aurora")],
            vec![("X-Pulse-Agent", "human"), ("X-Pulse-Job", "job-worker")],
            vec![("Origin", "http://evil.example")],
            vec![("Sec-Fetch-Site", "cross-site")],
        ] {
            let (status, body) =
                project_goal_request(f2::handler(&s), &headers, "application/json");
            assert_eq!(
                status, 403,
                "request headers {headers:?} bypassed guard: {status} {body}"
            );
        }
        let (status, _) = project_goal_request(f2::handler(&s), &[], "text/plain");
        assert_eq!(status, 415);
    }

    #[test]
    fn project_goal_api_storage_failure_is_not_success() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        s.conn().unwrap().execute(
            "CREATE TRIGGER fail_goal BEFORE INSERT ON events WHEN NEW.action LIKE 'project.goal.%' BEGIN SELECT RAISE(ABORT, 'goal audit failed'); END",
            [],
        ).unwrap();
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            "/api/v1/projects/one/goal",
            r#"{"text":"Keep search correct."}"#,
        );
        assert_eq!(status, 409);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("goal audit failed")
        );
        assert_eq!(
            pulse_goals::get(&s, "one").unwrap().source,
            pulse_goals::GENERATED
        );
        let encoded = pulse_store::gojson::to_string(&result).unwrap();
        assert!(!encoded.contains(&s.home));
        assert!(!encoded.contains("Keep search correct."));
    }
}
