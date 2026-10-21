//! `/api/v1/ask`: the console's quick answer box.
//!
//! `GET` tells the console which model would answer and whether the
//! provider is signed in; `POST` answers one question through
//! [`pulse_ask::ask`], read-only except for one `ask` event.

use crate::go_value;
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap};
use serde_json::Value;
use std::time::Duration;

/// How long one answer may take; the model call itself gets a little less.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(45);

impl Handler {
    pub(crate) fn addition_ask(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() != 1 {
            crate::addition_not_found(w);
            return;
        }
        if !self.addition_available(
            w,
            r,
            vec!["posts".to_string(), "jobs".to_string(), "tasks".to_string()],
        ) {
            return;
        }
        if !crate::addition_method(w, r, vec!["GET".to_string(), "POST".to_string()]) {
            return;
        }
        if r.method == "GET" {
            self.addition_ask_status(w, r);
        } else {
            self.addition_ask_answer(w, r);
        }
    }

    fn addition_ask_status(&self, w: &mut ResponseWriter, r: &Request) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let mut out = JsonMap::new();
        let ask_model = store.setting("ask.model", "");
        let light = store.light_model();
        out.insert("ask_model".into(), Value::String(ask_model.clone()));
        out.insert("light_model".into(), Value::String(light.clone()));
        let chosen = if !ask_model.trim().is_empty() {
            ask_model.trim().to_string()
        } else {
            light
        };
        out.insert("model".into(), Value::String(chosen));
        out.insert("max_question".into(), Value::from(pulse_ask::MAX_QUESTION));
        match self.harness_provider(String::new()) {
            Ok(p) => {
                let login = self.harness_provider_status(&r.ctx, p.clone(), Duration::from_secs(3));
                let logged_in = login
                    .get("logged_in")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                out.insert("logged_in".into(), Value::Bool(logged_in));
                out.insert(
                    "provider".into(),
                    Value::Object(
                        self.harness_provider_summary(p, login)
                            .into_iter()
                            .collect(),
                    ),
                );
            }
            Err(err) => {
                out.insert("logged_in".into(), Value::Bool(false));
                let mut p = JsonMap::new();
                p.insert("name".into(), Value::String(store.setting("provider", "")));
                p.insert("label".into(), Value::String(String::new()));
                p.insert(
                    "error".into(),
                    Value::String(self.redacted_provider_error(err.to_string(), vec![])),
                );
                out.insert("provider".into(), Value::Object(p.into_iter().collect()));
            }
        }
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }

    fn addition_ask_answer(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let mut req: pulse_ask::AskRequest = match serde_json::from_value(Value::Object(
            payload.into_iter().collect(),
        )) {
            Ok(req) => req,
            Err(e) => {
                crate::write_error(
                    w,
                    400,
                    "The question is malformed".into(),
                    format!(
                        "The JSON body does not fit {{project, question, history, model, all_projects}}: {e}."
                    ),
                    "Send {\"project\": \"<slug>\", \"question\": \"...\"}.".into(),
                );
                return;
            }
        };
        if let Err(why) = pulse_ask::check_question(&req.question) {
            crate::write_error(
                w,
                400,
                "Nothing to ask".into(),
                format!("{why}."),
                format!(
                    "Ask one question of up to {} characters.",
                    pulse_ask::MAX_QUESTION
                ),
            );
            return;
        }
        if req.project.trim().is_empty() && !req.all_projects {
            req.project = self.addition_project(r);
        }
        if req.project.trim().is_empty() {
            req.all_projects = true;
        }
        if req.history.len() > pulse_ask::HISTORY_TURNS {
            let skip = req.history.len() - pulse_ask::HISTORY_TURNS;
            req.history.drain(..skip);
        }
        req.actor = "human:console".into();

        // An unopenable provider (a typo in the provider setting) still gets
        // the facts; the answer says why no model spoke.
        let provider = self.harness_provider(String::new()).ok();
        let answer = match pulse_ask::ask(
            store,
            provider
                .as_deref()
                .map(|p| p as &dyn pulse_provider::Provider),
            &req,
            ANSWER_TIMEOUT,
        ) {
            Ok(a) => a,
            Err(e) if e.downcast_ref::<pulse_tally::NoProjectError>().is_some() => {
                crate::write_error(
                    w,
                    404,
                    "No such project".into(),
                    format!("{e}."),
                    "Pick a project from the list.".into(),
                );
                return;
            }
            Err(e) => {
                crate::addition_failure(w, e);
                return;
            }
        };
        let mut out = go_value(&answer);
        if let Some(obj) = out.as_object_mut()
            && provider.is_none()
            && answer.facts_only
        {
            obj.insert(
                "note".into(),
                Value::String(format!(
                    "the model provider {:?} did not open",
                    store.setting("provider", pulse_provider::DEFAULT)
                )),
            );
        }
        crate::write_json(w, 200, out);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers_http::{additions_project, additions_store, handler, request};
    use std::sync::Once;

    struct AskFake;

    impl pulse_provider::Provider for AskFake {
        fn name(&self) -> &str {
            "askfake"
        }
        fn label(&self) -> &str {
            "Ask Fake"
        }
        fn chat(
            &self,
            _cancel: &pulse_provider::CancelToken,
            req: pulse_provider::Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            let facts = &req.messages[0].content;
            assert!(
                facts.contains("demo") && facts.contains("[job-1]"),
                "the facts reach the model: {facts}"
            );
            let mut m = pulse_provider::Message::new(pulse_provider::Role::Assistant);
            m.content =
                "One job is blocked on you [job-1]; see the finding [20261012T100000Z-nimbus-1]."
                    .into();
            Ok(pulse_provider::Response {
                message: Some(m),
                model: req.model,
                usage: pulse_provider::Usage {
                    prompt_tokens: 120,
                    completion_tokens: 30,
                    cost: 0.0,
                },
                ..pulse_provider::Response::default()
            })
        }
        fn models(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<Vec<pulse_provider::Model>> {
            Ok(vec![pulse_provider::Model {
                id: "ask-default".into(),
                name: "Ask Default".into(),
                vendor: "fake".into(),
                chat: true,
                tool_calls: false,
                premium: 0.0,
                preview: false,
                context_window: 0,
                max_output: 0,
                default: true,
            }])
        }
        fn status(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<pulse_provider::Login> {
            Ok(pulse_provider::Login {
                logged_in: true,
                user: "octocat".into(),
                ..pulse_provider::Login::default()
            })
        }
    }

    struct AskLoggedOut;

    impl pulse_provider::Provider for AskLoggedOut {
        fn name(&self) -> &str {
            "askloggedout"
        }
        fn label(&self) -> &str {
            "Ask Logged Out"
        }
        fn chat(
            &self,
            _cancel: &pulse_provider::CancelToken,
            _req: pulse_provider::Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            Err(pulse_provider::err_not_logged_in().into())
        }
        fn models(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<Vec<pulse_provider::Model>> {
            Err(pulse_provider::err_not_logged_in().into())
        }
        fn status(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<pulse_provider::Login> {
            Ok(pulse_provider::Login::default())
        }
    }

    fn register_fakes() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            pulse_provider::register("askfake", "Ask Fake", |_| Ok(Box::new(AskFake)));
            pulse_provider::register("askloggedout", "Ask Logged Out", |_| {
                Ok(Box::new(AskLoggedOut))
            });
        });
    }

    fn ask_store(provider: &str) -> crate::test_helpers_http::TestStore {
        register_fakes();
        let s = additions_store(false);
        additions_project(&s, "demo");
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('provider', ?, ?, 'human') ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            [provider, &s.now()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO posts (id,project,area,thread,kind,agent,date,summary,body,created_at) VALUES ('20261012T100000Z-nimbus-1','demo','hub','hub/memory','finding','nimbus',?, 'The hub keeps the whole board in RAM','measured 2.1 GB',?)",
            [s.now(), s.now()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (id,project,kind,title,status,created_at,result) VALUES ('job-1','demo','task','Fix the memory use of the hub','blocked',?,'needs the owner')",
            [s.now()],
        )
        .unwrap();
        drop(conn);
        s
    }

    #[test]
    fn ask_status_names_the_model_and_login() {
        let _serial = crate::test_helpers::serial();
        let s = ask_store("askfake");
        s.set_setting("human", "light_model", "light-1").unwrap();
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/ask", "", &[]);
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["model"], "light-1");
        assert_eq!(body["ask_model"], "");
        assert_eq!(body["logged_in"], true);
        assert_eq!(body["provider"]["name"], "askfake");
        assert_eq!(body["max_question"], 2000);
        s.set_setting("human", "ask.model", "quick-1").unwrap();
        let (_, body, _) = request(handler(&s), "GET", "/api/v1/ask", "", &[]);
        assert_eq!(body["model"], "quick-1");
    }

    #[test]
    fn ask_answers_with_sources_and_logs_one_event() {
        let _serial = crate::test_helpers::serial();
        let s = ask_store("askfake");
        let body = serde_json::json!({
            "project": "demo",
            "question": "what is blocked in the hub memory work?",
            "history": [{"role": "user", "content": "hi"}, {"role": "assistant", "content": "hello"}]
        })
        .to_string();
        let (code, out, _) = request(
            handler(&s),
            "POST",
            "/api/v1/ask",
            &body,
            &[("Content-Type", "application/json")],
        );
        assert_eq!(code, 200, "{out:?}");
        assert_eq!(out["facts_only"], false);
        assert_eq!(
            out["model"], "ask-default",
            "no model setting: the provider's default chat model"
        );
        assert_eq!(out["provider"], "askfake");
        assert_eq!(out["project"], "demo");
        let ids: Vec<&str> = out["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["job-1", "20261012T100000Z-nimbus-1"]);
        assert_eq!(out["sources"][0]["source"], "job");
        assert_eq!(out["usage"]["prompt_tokens"], 120);
        let events: Vec<(String, String)> = s
            .conn()
            .unwrap()
            .prepare("SELECT actor, subject FROM events WHERE action = 'ask'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            events,
            vec![("human:console".to_string(), "demo".to_string())]
        );
    }

    #[test]
    fn ask_without_a_login_returns_the_facts() {
        let _serial = crate::test_helpers::serial();
        let s = ask_store("askloggedout");
        let body = serde_json::json!({"question": "hub memory?"}).to_string();
        let (code, out, _) = request(
            handler(&s),
            "POST",
            "/api/v1/ask?project=demo",
            &body,
            &[("Content-Type", "application/json")],
        );
        assert_eq!(code, 200, "{out:?}");
        assert_eq!(out["facts_only"], true);
        assert_eq!(out["note"], "not signed in");
        assert_eq!(
            out["project"], "demo",
            "the project comes from the query when the body has none"
        );
        assert!(
            out["answer"]
                .as_str()
                .unwrap()
                .contains("Matching the question:"),
            "{out:?}"
        );
        assert_eq!(out["sources"][0]["id"], "20261012T100000Z-nimbus-1");
    }

    #[test]
    fn ask_refuses_empty_questions_and_unknown_projects() {
        let _serial = crate::test_helpers::serial();
        let s = ask_store("askfake");
        let (code, out, _) = request(
            handler(&s),
            "POST",
            "/api/v1/ask",
            r#"{"project":"demo","question":"   "}"#,
            &[("Content-Type", "application/json")],
        );
        assert_eq!(code, 400, "{out:?}");
        assert_eq!(out["error"], "Nothing to ask");
        let (code, out, _) = request(
            handler(&s),
            "POST",
            "/api/v1/ask",
            r#"{"project":"nope","question":"hi"}"#,
            &[("Content-Type", "application/json")],
        );
        assert_eq!(code, 404, "{out:?}");
        assert_eq!(out["error"], "No such project");
        let (code, out, _) = request(
            handler(&s),
            "POST",
            "/api/v1/ask",
            r#"{"question":"what is going on everywhere?"}"#,
            &[("Content-Type", "application/json")],
        );
        assert_eq!(code, 200, "{out:?}");
        assert_eq!(
            out["project"], "",
            "no project anywhere means every project"
        );
        let (code, _, _) = request(handler(&s), "DELETE", "/api/v1/ask", "", &[]);
        assert_eq!(code, 405);
        let (code, _, _) = request(handler(&s), "GET", "/api/v1/ask/more", "", &[]);
        assert_eq!(code, 404);
    }
}
