use super::*;
use pulse_provider::apikey::{StoredApiKey, key_user};
use pulse_provider::http::parse_error_body;
use pulse_provider::{Tool, provider_error, squeeze, write_private_json};
use pulse_testutil::httptest::{Response as TestResponse, Server};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const FAKE_KEY: &str = "sk-test-secret-abcdefghijklmnop";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

const EXPECTED_VENDORS: [&str; 14] = [
    "openai",
    "azure-openai",
    "openrouter",
    "groq",
    "mistral",
    "deepseek",
    "xai",
    "together",
    "fireworks",
    "cerebras",
    "github-models",
    "ollama",
    "lmstudio",
    "vllm",
];

fn new_test_vendor(
    name: &str,
    base: &str,
    env: Option<BTreeMap<String, String>>,
    settings: Option<BTreeMap<String, String>>,
) -> OpenAiCompatible {
    let dir = tempfile::tempdir().unwrap();
    new_test_vendor_with_home(
        name,
        dir.path().to_string_lossy().as_ref(),
        base,
        env,
        settings,
    )
}

fn new_test_vendor_with_home(
    name: &str,
    home: &str,
    base: &str,
    env: Option<BTreeMap<String, String>>,
    settings: Option<BTreeMap<String, String>>,
) -> OpenAiCompatible {
    let vendor = vendor(name).unwrap_or_else(|| panic!("unknown vendor {name}"));
    let mut settings = settings.unwrap_or_default();
    if !base.is_empty() {
        settings.insert(vendor.settings_key(), base.to_string());
    }
    let env = Arc::new(env.unwrap_or_default());
    from_options(
        vendor,
        &mut Options {
            home: home.to_string(),
            settings,
            env: Some(Arc::new(move |key| {
                env.get(key).cloned().unwrap_or_default()
            })),
            ..Options::default()
        },
    )
    .unwrap()
}

fn new_test_provider(
    base: &str,
    env: Option<BTreeMap<String, String>>,
    settings: Option<BTreeMap<String, String>>,
) -> OpenAiCompatible {
    new_test_vendor(NAME, base, env, settings)
}

fn new_test_provider_with_home(
    home: &str,
    base: &str,
    env: Option<BTreeMap<String, String>>,
    settings: Option<BTreeMap<String, String>>,
) -> OpenAiCompatible {
    new_test_vendor_with_home(NAME, home, base, env, settings)
}

fn env_of(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

fn user_request(content: &str) -> Request {
    Request {
        model: "gpt-4o-mini".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
        }],
        ..Request::default()
    }
}

fn ok_chat_body() -> Value {
    json!({
        "model": "gpt-4o-mini",
        "choices": [{
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": "ok"}
        }]
    })
}

fn model_ids(models: &[Model]) -> Vec<String> {
    models.iter().map(|m| m.id.clone()).collect()
}

fn model<'a>(models: &'a [Model], id: &str) -> &'a Model {
    models
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("model {id} missing from {:?}", model_ids(models)))
}

#[test]
fn models_list_default_pick_and_cache() {
    let calls = Arc::new(AtomicI32::new(0));
    let calls_log = calls.clone();
    let srv = Server::new(move |req| {
        calls_log.fetch_add(1, Ordering::SeqCst);
        assert_eq!(req.path(), "/models");
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {FAKE_KEY}").as_str())
        );
        TestResponse::json(
            200,
            &json!({"data": [
                {"id": "z-local", "owned_by": "local"},
                {"id": "gpt-4o-mini", "owned_by": "openai"},
                {"id": "a-first"}
            ]}),
        )
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&(srv.url() + "/"), Some(env), None);
    let models = p.models(&CancelToken::new()).unwrap();
    assert_eq!(
        model_ids(&models),
        vec!["a-first", "gpt-4o-mini", "z-local"]
    );
    for m in &models {
        if m.id == "gpt-4o-mini" {
            assert!(m.default && m.chat && m.tool_calls && m.context_window == 0);
        }
        if m.id == "a-first" {
            assert_eq!(m.vendor, "openai");
        }
    }
    p.models(&CancelToken::new()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn chat_tool_call_round_trip() {
    let saw = Arc::new(Mutex::new(Value::Null));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        assert_eq!(req.path(), "/chat/completions");
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {FAKE_KEY}").as_str())
        );
        *saw_log.lock().unwrap() = req.json().unwrap();
        TestResponse::json(
            200,
            &json!({
                "id": "req-body",
                "model": "gpt-4o-mini",
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "lookup", "arguments": "{\"id\":1}"}
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 5, "completion_tokens": 3}
            }),
        )
        .header("x-request-id", "req-header")
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), None);
    let resp = p
        .chat(
            &CancelToken::new(),
            Request {
                model: "gpt-4o-mini".to_string(),
                messages: vec![
                    Message {
                        role: Role::User,
                        content: "use a tool".to_string(),
                        tool_calls: Vec::new(),
                        tool_call_id: String::new(),
                        name: String::new(),
                    },
                    Message {
                        role: Role::Assistant,
                        tool_calls: vec![ToolCall {
                            id: "call-0".to_string(),
                            name: "lookup".to_string(),
                            arguments: r#"{"id":0}"#.to_string(),
                        }],
                        ..Message::new(Role::Assistant)
                    },
                    Message {
                        role: Role::Tool,
                        tool_call_id: "call-0".to_string(),
                        name: "lookup".to_string(),
                        content: r#"{"ok":true}"#.to_string(),
                        tool_calls: Vec::new(),
                    },
                ],
                tools: vec![Tool {
                    name: "lookup".to_string(),
                    description: "Looks up an item.".to_string(),
                    parameters: json!({"type":"object"}),
                }],
                max_tokens: 42,
                temperature: Some(0.3),
                ..Request::default()
            },
        )
        .unwrap();
    let saw = saw.lock().unwrap().clone();
    assert_eq!(saw["tool_choice"], "auto");
    assert_eq!(saw["stream"], false);
    assert_eq!(saw["max_tokens"], 42);
    assert_eq!(saw["tools"][0]["type"], "function");
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.request_id, "req-header");
    assert_eq!(resp.usage.prompt_tokens, 5);
    assert_eq!(resp.usage.completion_tokens, 3);
    assert_eq!(resp.usage.cost, 0.0);
    let got = &resp.message.unwrap().tool_calls;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "lookup");
    assert_eq!(got[0].arguments, r#"{"id":1}"#);
}

#[test]
fn unauthorized_error_and_status_detail() {
    let srv = Server::new(|_| {
        TestResponse::json(
            401,
            &json!({"error": {"message": format!("no {FAKE_KEY}")}}),
        )
    });
    let home = tempfile::tempdir().unwrap();
    let settings = BTreeMap::from([("provider.openai.base_url".to_string(), srv.url())]);
    let p = new_test_provider_with_home(
        home.path().to_string_lossy().as_ref(),
        &srv.url(),
        None,
        Some(settings),
    );
    // A file written by the previous release keeps working as is.
    write_private_json(
        home.path(),
        NAME,
        &StoredApiKey {
            api_key: FAKE_KEY.to_string(),
            user: key_user(FAKE_KEY),
            ..StoredApiKey::default()
        },
    )
    .unwrap();
    let err = p.chat(&CancelToken::new(), user_request("hi")).unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.provider, "openai");
    assert_eq!(perr.code, "unauthorized");
    assert!(!perr.retryable);
    assert!(!err.to_string().contains(FAKE_KEY));
    let login = p.status(&CancelToken::new()).unwrap();
    assert!(!login.logged_in);
    assert!(login.detail.contains("key was refused"));
    let path = home
        .path()
        .join("private")
        .join("providers")
        .join("openai.json");
    assert!(path.exists());
}

#[test]
fn retry_after429_then_success() {
    let calls = Arc::new(AtomicI32::new(0));
    let calls_log = calls.clone();
    let srv = Server::new(move |_| {
        if calls_log.fetch_add(1, Ordering::SeqCst) == 0 {
            return TestResponse::json(
                429,
                &json!({"error": {"code": "rate_limit", "message": "slow down"}}),
            )
            .header("Retry-After", "1");
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), None);
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn chat_stream_shows_text_as_it_arrives_and_folds_the_reply() {
    let saw = Arc::new(Mutex::new(Value::Null));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        assert_eq!(req.header("accept"), Some("text/event-stream"));
        *saw_log.lock().unwrap() = req.json().unwrap();
        let chunks = [
            json!({"id":"chatcmpl-9","model":"gpt-4o-mini","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"lookup","arguments":"{\"id\""}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":":1}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":3}}),
        ];
        let mut body = String::new();
        for c in chunks {
            body.push_str(&format!("data: {c}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        TestResponse::new(200)
            .header("Content-Type", "text/event-stream")
            .header("x-request-id", "req-stream")
            .body(body.into_bytes())
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), None);
    let mut shown: Vec<String> = Vec::new();
    let resp = p
        .chat_stream(&CancelToken::new(), user_request("hi"), &mut |t| {
            shown.push(t.to_string());
        })
        .unwrap();
    assert_eq!(shown, vec!["Hel", "lo"]);
    let saw = saw.lock().unwrap().clone();
    assert_eq!(saw["stream"], true);
    assert_eq!(saw["stream_options"]["include_usage"], true);
    assert_eq!(resp.request_id, "req-stream");
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.usage.prompt_tokens, 5);
    assert_eq!(resp.usage.completion_tokens, 3);
    assert_eq!(resp.model, "gpt-4o-mini");
    let msg = resp.message.unwrap();
    assert_eq!(msg.content, "Hello");
    assert_eq!(msg.tool_calls.len(), 1);
    assert_eq!(msg.tool_calls[0].arguments, r#"{"id":1}"#);
}

#[test]
fn chat_stream_falls_back_to_whole_replies_when_the_endpoint_refuses_streaming() {
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let bodies_log = bodies.clone();
    let srv = Server::new(move |req| {
        let body = req.json().unwrap();
        let streamed = body["stream"] == true;
        let with_options = body.get("stream_options").is_some();
        bodies_log.lock().unwrap().push(body);
        if with_options {
            return TestResponse::json(
                400,
                &json!({"error": {"code": "invalid_request_error", "message": "Unrecognized request argument supplied: stream_options"}}),
            );
        }
        if streamed {
            return TestResponse::json(
                400,
                &json!({"error": {"code": "invalid_request_error", "message": "streaming is not supported by this model"}}),
            );
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), None);
    let mut shown = String::new();
    let resp = p
        .chat_stream(&CancelToken::new(), user_request("hi"), &mut |t| {
            shown.push_str(t)
        })
        .unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    assert_eq!(shown, "ok", "a whole reply is still shown once");
    // The second streamed call does not try streaming again.
    let mut shown = String::new();
    p.chat_stream(&CancelToken::new(), user_request("again"), &mut |t| {
        shown.push_str(t)
    })
    .unwrap();
    assert_eq!(shown, "ok");
    let bodies = bodies.lock().unwrap();
    let shape: Vec<(bool, bool)> = bodies
        .iter()
        .map(|b| (b["stream"] == true, b.get("stream_options").is_some()))
        .collect();
    assert_eq!(
        shape,
        vec![(true, true), (true, false), (false, false), (false, false)],
        "with usage, then plain, then whole, and whole again: {bodies:?}"
    );
}

#[test]
fn chat_stream_reads_a_plain_json_answer_to_a_streamed_request() {
    let srv = Server::new(move |_| TestResponse::json(200, &ok_chat_body()));
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), None);
    let mut shown = String::new();
    let resp = p
        .chat_stream(&CancelToken::new(), user_request("hi"), &mut |t| {
            shown.push_str(t)
        })
        .unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    assert_eq!(shown, "ok");
}

#[test]
fn chat_response_over_body_cap_is_non_retryable() {
    let srv = Server::new(move |_| TestResponse::text(200, "x".repeat(1024 * 1024 + 1)));
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let settings = BTreeMap::from([("provider.max_body_mb".to_string(), "1".to_string())]);
    let p = new_test_provider(&srv.url(), Some(env), Some(settings));
    let err = p.chat(&CancelToken::new(), user_request("hi")).unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.code, "body_too_large");
    assert_eq!(perr.message, "response exceeded 1 MiB");
    assert!(!perr.retryable);
}

#[test]
fn network_error_retryable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p = new_test_provider(&url, Some(env), None);
    let err = p.models(&CancelToken::new()).unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert!(perr.retryable);
    assert_eq!(perr.code, "network");
    assert!(!err.to_string().contains(FAKE_KEY));
}

#[test]
fn token_login_stores0600_refuses_bad_key_and_exports_stored_only() {
    let allow = Arc::new(AtomicBool::new(false));
    let allow_log = allow.clone();
    let srv = Server::new(move |req| {
        assert_eq!(req.path(), "/models");
        if !allow_log.load(Ordering::SeqCst) {
            return TestResponse::json(
                403,
                &json!({"error": {"message": format!("bad {FAKE_KEY}")}}),
            );
        }
        TestResponse::json(200, &json!({"data": [{"id": "gpt-4o"}]}))
    });
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let env_provider = new_test_provider(&srv.url(), Some(env), None);
    assert!(
        env_provider
            .as_login_exporter()
            .unwrap()
            .export_login()
            .is_err()
    );
    let home = tempfile::tempdir().unwrap();
    let p = new_test_provider_with_home(
        home.path().to_string_lossy().as_ref(),
        &srv.url(),
        None,
        None,
    );
    let login = p.as_token_login().unwrap();
    let err = login
        .login_with_token(&CancelToken::new(), FAKE_KEY)
        .unwrap_err();
    assert!(!err.to_string().contains(FAKE_KEY));
    assert_eq!(provider_error(&err).unwrap().status, 401);
    assert!(!p.keys.path().exists());
    allow.store(true, Ordering::SeqCst);
    let logged_in = login
        .login_with_token(&CancelToken::new(), FAKE_KEY)
        .unwrap();
    assert_eq!(logged_in.user, key_user(FAKE_KEY));
    assert_eq!(logged_in.source, "token login");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let info = fs::metadata(p.keys.path()).unwrap();
        assert_eq!(info.permissions().mode() & 0o777, 0o600);
        let dir = fs::metadata(p.keys.path().parent().unwrap()).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o700);
    }
    let stored = p.keys.read_stored().unwrap().unwrap();
    assert_eq!(stored.base_url, srv.url());
    let (tok, exported) = p.as_login_exporter().unwrap().export_login().unwrap();
    assert_eq!(tok, FAKE_KEY);
    assert_eq!(exported.user, key_user(FAKE_KEY));
    assert_eq!(exported.source, "stored");
}

#[test]
fn base_url_precedence_and_trim() {
    let env = BTreeMap::from([(
        "OPENAI_BASE_URL".to_string(),
        "https://env.test/v1///".to_string(),
    )]);
    let p = new_test_provider("", Some(env), None);
    assert_eq!(p.base, "https://env.test/v1");
    let env = BTreeMap::from([(
        "OPENAI_BASE_URL".to_string(),
        "https://env.test/v1".to_string(),
    )]);
    let settings = BTreeMap::from([(
        "provider.openai.base_url".to_string(),
        "https://settings.test/v1/".to_string(),
    )]);
    let p = new_test_provider("", Some(env), Some(settings));
    assert_eq!(p.base, "https://settings.test/v1");
    let p = new_test_provider("", None, None);
    assert_eq!(p.base, DEFAULT_BASE_URL);
    assert_eq!(p.base_url(), DEFAULT_BASE_URL);
    assert_eq!(p.vendor().name, "openai");
    // The store hands providers the catalogue default for an unset
    // setting; that must not shadow the environment.
    let env = BTreeMap::from([(
        "OPENAI_BASE_URL".to_string(),
        "https://env.test/v1".to_string(),
    )]);
    let settings = BTreeMap::from([(
        "provider.openai.base_url".to_string(),
        format!("{DEFAULT_BASE_URL}/"),
    )]);
    let p = new_test_provider("", Some(env), Some(settings));
    assert_eq!(p.base, "https://env.test/v1");
}

#[test]
fn fake_key_never_leaks_in_provider_errors() {
    remember_secret(FAKE_KEY);
    let err = ProviderError::new(
        NAME,
        429,
        format!("rate_{FAKE_KEY}"),
        format!("message {FAKE_KEY}"),
        true,
    );
    assert!(!err.to_string().contains(FAKE_KEY));
    assert!(!err.code.contains(FAKE_KEY));
    assert!(!err.message.contains(FAKE_KEY));
}

#[test]
fn register_idempotent() {
    register();
    register();
    assert_eq!(pulse_provider::label_of(NAME), LABEL);
}

#[test]
fn request_json_omits_empty_content_and_sets_tool_choice() {
    let req = open_ai_request(Request {
        model: "m".to_string(),
        messages: vec![Message {
            role: Role::Assistant,
            tool_calls: vec![ToolCall {
                id: "c".to_string(),
                name: "n".to_string(),
                arguments: "{}".to_string(),
            }],
            ..Message::new(Role::Assistant)
        }],
        tools: vec![Tool {
            name: "n".to_string(),
            description: "d".to_string(),
            parameters: json!({"type":"object"}),
        }],
        ..Request::default()
    });
    let got = serde_json::to_value(req).unwrap();
    assert!(got["messages"][0].get("content").is_none());
    assert_eq!(got["tool_choice"], "auto");
    assert!(
        got.get("reasoning_effort").is_none(),
        "no effort asked, none sent: {got}"
    );
}

#[test]
fn request_json_carries_reasoning_effort_only_when_asked() {
    let req = open_ai_request(Request {
        model: "o4-mini".to_string(),
        reasoning: "high".to_string(),
        ..Request::default()
    });
    let got = serde_json::to_value(req).unwrap();
    assert_eq!(got["reasoning_effort"], "high");
}

#[test]
fn parse_error_body_variants() {
    assert_eq!(
        parse_error_body(br#"{"error":{"code":"c","message":"m"}}"#),
        ("c".to_string(), "m".to_string())
    );
    assert_eq!(
        parse_error_body(br#"{"type":"t","error":"m"}"#),
        ("t".to_string(), "m".to_string())
    );
}

#[test]
fn split_choices_merge_into_one_message() {
    // A proxy that answers with the narration in choices[0] and the tool
    // call in choices[1] must not lose the call.
    let out: ChatResponse = serde_json::from_value(json!({
        "id": "req-split",
        "model": "gpt-4o-mini",
        "choices": [
            {"finish_reason": "stop", "message": {"role": "assistant", "content": "Lint passes. Running tests."}},
            {"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null,
                "tool_calls": [{"id": "call-9", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"cargo test\"}"}}]}}
        ],
        "usage": {"prompt_tokens": 5, "completion_tokens": 3}
    }))
    .unwrap();
    let resp = out.provider_response();
    assert_eq!(resp.finish_reason, "tool_calls");
    let msg = resp.message.unwrap();
    assert_eq!(msg.content, "Lint passes. Running tests.");
    assert_eq!(msg.tool_calls.len(), 1);
    assert_eq!(msg.tool_calls[0].id, "call-9");
    assert_eq!(msg.tool_calls[0].name, "bash");
}

#[test]
fn logout_removes_stored_and_status_reports_environment_detail() {
    let home = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
    let p =
        new_test_provider_with_home(home.path().to_string_lossy().as_ref(), "", Some(env), None);
    p.keys.save(FAKE_KEY, "").unwrap();
    p.as_logouter().unwrap().logout().unwrap();
    assert!(!p.keys.path().exists());
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, "environment");
    assert_eq!(
        status.detail,
        "Stored OpenAI-compatible login removed; still using the environment API key."
    );
}

#[test]
fn status_without_login_reports_guidance() {
    let p = new_test_provider("", None, None);
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(!status.logged_in);
    assert_eq!(
        status.detail,
        "Paste an OpenAI-compatible API key in Settings -> Harness or run pulse harness login --provider openai --with-token"
    );
}

#[test]
fn empty_token_login_is_invalid_token() {
    let p = new_test_provider("", None, None);
    let err = p
        .as_token_login()
        .unwrap()
        .login_with_token(&CancelToken::new(), " ")
        .unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.code, "invalid_token");
    assert_eq!(perr.message, "OpenAI-compatible API key is empty");
}

#[test]
fn key_user_masks_last_four() {
    assert_eq!(key_user("abc"), "…abc");
    assert_eq!(key_user("abcdef"), "…cdef");
}

#[test]
fn squeeze_and_redact_are_provider_shims() {
    remember_secret(FAKE_KEY);
    assert_eq!(squeeze("a \n b", 80), "a b");
    assert_eq!(redact(FAKE_KEY), "[redacted]");
}

#[test]
fn vendors_table_is_complete_unique_and_registered() {
    let names: Vec<&str> = VENDORS.iter().map(|v| v.name).collect();
    assert_eq!(names, EXPECTED_VENDORS);
    assert_eq!(VENDORS[0].name, "openai");
    assert_eq!(vendors().len(), VENDORS.len());
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "vendor names must be unique");
    for v in VENDORS {
        assert!(!v.label.is_empty(), "{} needs a label", v.name);
        assert!(!v.api_key_env.is_empty(), "{} needs a key variable", v.name);
        assert!(
            !v.base_url_env.is_empty(),
            "{} needs a base URL variable",
            v.name
        );
        assert!(
            !v.default_base_url.is_empty() || !v.base_url_hint.is_empty(),
            "{} needs a default base URL or a hint",
            v.name
        );
        assert!(
            !v.default_base_url.ends_with('/'),
            "{} default must not end in a slash",
            v.name
        );
        assert_eq!(vendor(v.name).unwrap().name, v.name);
        assert_eq!(v.settings_key(), format!("provider.{}.base_url", v.name));
    }
    assert!(vendor("nope").is_none());
    assert_eq!(vendor(" groq ").unwrap().label, "Groq");

    register();
    let registered = pulse_provider::names();
    for v in VENDORS {
        assert!(
            registered.iter().any(|n| n == v.name),
            "{} not registered",
            v.name
        );
        assert_eq!(pulse_provider::label_of(v.name), v.label);
    }
    let dir = tempfile::tempdir().unwrap();
    let opened = pulse_provider::open(
        "groq",
        Options {
            home: dir.path().to_string_lossy().into_owned(),
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(opened.name(), "groq");
    assert_eq!(opened.label(), "Groq");
    assert!(opened.as_token_login().is_some());
    assert!(opened.as_login_exporter().is_some());
    assert!(opened.as_logouter().is_some());
    let openai = new(Options {
        home: dir.path().to_string_lossy().into_owned(),
        ..Options::default()
    })
    .unwrap();
    assert_eq!(openai.name(), "openai");
}

#[test]
fn each_vendor_reads_settings_then_env_then_default() {
    for v in VENDORS {
        let p = new_test_vendor(v.name, "", None, None);
        assert_eq!(p.base, v.default_base_url, "{} default", v.name);
        assert_eq!(p.name(), v.name);
        assert_eq!(p.label(), v.label);
        assert_eq!(p.keys.name(), v.name);
        assert_eq!(p.keys.label(), v.label);
        assert_eq!(p.keys.env_keys(), v.api_key_env);

        for (i, key) in v.base_url_env.iter().enumerate() {
            let env = env_of(&[(key, "https://env.test/v1/")]);
            let p = new_test_vendor(v.name, "", env, None);
            assert_eq!(p.base, "https://env.test/v1", "{} via {key}", v.name);
            if i > 0 {
                // An earlier variable wins over a later one.
                let env = env_of(&[
                    (v.base_url_env[0], "https://first.test/v1"),
                    (key, "https://later.test/v1"),
                ]);
                let p = new_test_vendor(v.name, "", env, None);
                assert_eq!(p.base, "https://first.test/v1", "{} order", v.name);
            }
        }

        let env = env_of(&[(v.base_url_env[0], "https://env.test/v1")]);
        let settings =
            BTreeMap::from([(v.settings_key(), "https://settings.test/v1//".to_string())]);
        let p = new_test_vendor(v.name, "", env, Some(settings));
        assert_eq!(p.base, "https://settings.test/v1", "{} setting", v.name);

        // A blank setting falls through to the environment.
        let env = env_of(&[(v.base_url_env[0], "https://env.test/v1")]);
        let settings = BTreeMap::from([(v.settings_key(), "  ".to_string())]);
        let p = new_test_vendor(v.name, "", env, Some(settings));
        assert_eq!(p.base, "https://env.test/v1", "{} blank setting", v.name);
    }
}

#[test]
fn key_required_vendors_refuse_calls_without_a_key() {
    let srv = Server::new(|_| panic!("no request expected without a key"));
    for v in VENDORS.iter().filter(|v| !v.key_optional) {
        let p = new_test_vendor(v.name, &srv.url(), None, None);
        let err = p.chat(&CancelToken::new(), user_request("hi")).unwrap_err();
        assert!(pulse_provider::is_not_logged_in(&err), "{} chat", v.name);
        let err = p.models(&CancelToken::new()).unwrap_err();
        assert!(pulse_provider::is_not_logged_in(&err), "{} models", v.name);
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in, "{} status", v.name);
        assert!(
            status
                .detail
                .contains(&format!("--provider {} --with-token", v.name)),
            "{} hint: {}",
            v.name,
            status.detail
        );
        // The vendor's key variable signs it in.
        let p = new_test_vendor(
            v.name,
            &srv.url(),
            env_of(&[(v.api_key_env[0], FAKE_KEY)]),
            None,
        );
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in, "{} env key", v.name);
        assert_eq!(status.source, "environment");
        assert_eq!(status.user, "…mnop");
    }
}

#[test]
fn azure_sends_api_key_header_and_normalizes_the_resource_root() {
    let saw = Arc::new(Mutex::new(
        Vec::<(Option<String>, Option<String>, String)>::new(),
    ));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        saw_log.lock().unwrap().push((
            req.header("api-key").map(str::to_string),
            req.header("authorization").map(str::to_string),
            req.path().to_string(),
        ));
        if req.path() == "/models" {
            return TestResponse::json(200, &json!({"data": [{"id": "gpt-4o"}]}));
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let env = env_of(&[("AZURE_OPENAI_API_KEY", FAKE_KEY)]);
    let p = new_test_vendor("azure-openai", &srv.url(), env, None);
    assert_eq!(p.base, srv.url());
    p.models(&CancelToken::new()).unwrap();
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    let saw = saw.lock().unwrap().clone();
    assert_eq!(saw.len(), 2);
    for (api_key, authorization, _) in &saw {
        assert_eq!(api_key.as_deref(), Some(FAKE_KEY));
        assert!(authorization.is_none());
    }
    assert_eq!(saw[0].2, "/models");
    assert_eq!(saw[1].2, "/chat/completions");

    let cases = [
        (
            "https://myres.openai.azure.com",
            "https://myres.openai.azure.com/openai/v1",
        ),
        (
            "https://myres.openai.azure.com/",
            "https://myres.openai.azure.com/openai/v1",
        ),
        (
            "https://myres.cognitiveservices.azure.com",
            "https://myres.cognitiveservices.azure.com/openai/v1",
        ),
        (
            "https://myres.services.ai.azure.com",
            "https://myres.services.ai.azure.com/openai/v1",
        ),
        (
            "https://myres.openai.azure.com/openai",
            "https://myres.openai.azure.com/openai/v1",
        ),
        (
            "https://myres.openai.azure.com/openai/",
            "https://myres.openai.azure.com/openai/v1",
        ),
        (
            "https://myres.openai.azure.com/openai/v1",
            "https://myres.openai.azure.com/openai/v1",
        ),
        (
            "https://gateway.example.com/azure",
            "https://gateway.example.com/azure",
        ),
        ("https://gateway.example.com", "https://gateway.example.com"),
    ];
    for (configured, want) in cases {
        let p = new_test_vendor("azure-openai", configured, None, None);
        assert_eq!(p.base, want, "setting {configured}");
        let env = env_of(&[("AZURE_OPENAI_ENDPOINT", configured)]);
        let p = new_test_vendor("azure-openai", "", env, None);
        assert_eq!(p.base, want, "env {configured}");
    }
}

#[test]
fn azure_without_a_base_url_asks_for_setup() {
    let detail = "Set provider.azure-openai.base_url to https://<resource>.openai.azure.com/openai/v1, then sign in with Microsoft in Settings -> Harness or run pulse harness login --provider azure-openai; a key also works with pulse harness login --provider azure-openai --with-token";
    for env in [None, env_of(&[("AZURE_OPENAI_API_KEY", FAKE_KEY)])] {
        let p = new_test_vendor("azure-openai", "", env, None);
        assert_eq!(p.base, "");
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(status.detail, detail);
        for err in [
            p.chat(&CancelToken::new(), user_request("hi")).unwrap_err(),
            p.models(&CancelToken::new()).unwrap_err(),
            p.as_token_login()
                .unwrap()
                .login_with_token(&CancelToken::new(), FAKE_KEY)
                .unwrap_err(),
        ] {
            let perr = provider_error(&err).unwrap();
            assert_eq!(perr.provider, "azure-openai");
            assert_eq!(perr.code, "no_base_url");
            assert!(!perr.retryable);
            assert!(perr.message.contains("provider.azure-openai.base_url"));
        }
        assert!(!p.keys.path().exists());
    }
}

#[test]
fn openrouter_sends_its_attribution_headers() {
    let saw = Arc::new(Mutex::new(Vec::<(Option<String>, Option<String>)>::new()));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        saw_log.lock().unwrap().push((
            req.header("http-referer").map(str::to_string),
            req.header("x-title").map(str::to_string),
        ));
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {FAKE_KEY}").as_str())
        );
        if req.path() == "/models" {
            return TestResponse::json(200, &json!({"data": [{"id": "openai/gpt-4o"}]}));
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let env = env_of(&[("OPENROUTER_API_KEY", FAKE_KEY)]);
    let p = new_test_vendor("openrouter", &srv.url(), env, None);
    p.models(&CancelToken::new()).unwrap();
    p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    let saw = saw.lock().unwrap().clone();
    assert_eq!(saw.len(), 2);
    for (referer, title) in &saw {
        assert_eq!(
            referer.as_deref(),
            Some("https://github.com/manishtiwari25/pulse")
        );
        assert_eq!(title.as_deref(), Some("PULSE"));
    }
    let p = new_test_vendor("openrouter", "", None, None);
    assert_eq!(p.base, "https://openrouter.ai/api/v1");
}

#[test]
fn github_models_uses_the_catalog_and_the_token_fallbacks() {
    let v = vendor("github-models").unwrap();
    assert_eq!(v.default_base_url, "https://models.github.ai/inference");
    assert_eq!(
        v.models_url(v.default_base_url),
        "https://models.github.ai/catalog/models"
    );
    assert_eq!(
        v.models_url("http://127.0.0.1:1/inference"),
        "http://127.0.0.1:1/inference/models"
    );
    for other in VENDORS.iter().filter(|o| o.name != "github-models") {
        assert_eq!(
            other.models_url(other.default_base_url),
            format!("{}/models", other.default_base_url),
            "{} models URL",
            other.name
        );
    }

    let srv = Server::new(|req| {
        assert_eq!(req.path(), "/models");
        assert_eq!(req.header("x-github-api-version"), Some("2022-11-28"));
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {FAKE_KEY}").as_str())
        );
        TestResponse::json(
            200,
            &json!([
                {
                    "id": "openai/gpt-4.1",
                    "name": "OpenAI GPT-4.1",
                    "publisher": "OpenAI",
                    "registry": "azure-openai",
                    "capabilities": ["streaming", "tool-calling"],
                    "limits": {"max_input_tokens": 1048576, "max_output_tokens": 32768},
                    "supported_input_modalities": ["text", "image"]
                },
                {
                    "id": "openai/text-embedding-3-small",
                    "name": "OpenAI Text Embedding 3 (small)",
                    "publisher": "OpenAI",
                    "capabilities": ["streaming"],
                    "limits": {"max_input_tokens": 8191, "max_output_tokens": 0}
                },
                {
                    "id": "meta/llama-3.3-70b-instruct",
                    "name": "Llama-3.3-70B-Instruct",
                    "publisher": "Meta",
                    "capabilities": ["streaming", "tool-calling"],
                    "limits": {"max_input_tokens": 128000, "max_output_tokens": 4096}
                },
                {"name": "no id, skipped"}
            ]),
        )
    });
    let env = env_of(&[("GITHUB_TOKEN", FAKE_KEY)]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    let models = p.models(&CancelToken::new()).unwrap();
    assert_eq!(
        model_ids(&models),
        vec![
            "meta/llama-3.3-70b-instruct",
            "openai/gpt-4.1",
            "openai/text-embedding-3-small"
        ]
    );
    let gpt = model(&models, "openai/gpt-4.1");
    assert_eq!(gpt.name, "OpenAI GPT-4.1");
    assert_eq!(gpt.vendor, "OpenAI");
    assert!(gpt.chat && gpt.tool_calls && gpt.default);
    assert_eq!(gpt.context_window, 1048576);
    assert_eq!(gpt.max_output, 32768);
    let embed = model(&models, "openai/text-embedding-3-small");
    assert!(!embed.tool_calls && !embed.default);
    assert_eq!(embed.context_window, 8191);
    assert_eq!(embed.max_output, 0);
    let llama = model(&models, "meta/llama-3.3-70b-instruct");
    assert_eq!(llama.vendor, "Meta");
    assert!(llama.tool_calls && !llama.default);

    // GITHUB_MODELS_TOKEN beats GH_TOKEN beats GITHUB_TOKEN.
    let p = new_test_vendor("github-models", &srv.url(), None, None);
    assert!(!p.status(&CancelToken::new()).unwrap().logged_in);
    let env = env_of(&[("GITHUB_TOKEN", "github-token-1111")]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, "environment");
    assert_eq!(status.user, "…1111");
    let env = env_of(&[
        ("GITHUB_TOKEN", "github-token-1111"),
        ("GH_TOKEN", "gh-token-2222"),
    ]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    assert_eq!(p.keys.source().unwrap().token, "gh-token-2222");
    assert_eq!(p.status(&CancelToken::new()).unwrap().user, "…2222");
    let env = env_of(&[
        ("GITHUB_TOKEN", "github-token-1111"),
        ("GH_TOKEN", "gh-token-2222"),
        ("GITHUB_MODELS_TOKEN", "models-token-3333"),
    ]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    assert_eq!(p.keys.source().unwrap().token, "models-token-3333");
    assert_eq!(p.status(&CancelToken::new()).unwrap().user, "…3333");
}

#[test]
fn ollama_works_without_a_key_and_normalizes_ollama_host() {
    let saw = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        saw_log
            .lock()
            .unwrap()
            .push(req.header("authorization").map(str::to_string));
        // The path-less server URL is normalized to `<url>/v1`, like a
        // bare OLLAMA_HOST would be.
        if req.path() == "/v1/models" {
            return TestResponse::json(
                200,
                &json!({"data": [
                    {"id": "qwen2.5:7b", "owned_by": "library"},
                    {"id": "llama3.2:latest", "owned_by": "library"},
                    {"id": "llama3.2:1b", "owned_by": "library"}
                ]}),
            );
        }
        assert_eq!(req.path(), "/v1/chat/completions");
        TestResponse::json(200, &ok_chat_body())
    });
    let p = new_test_vendor("ollama", &srv.url(), None, None);
    assert_eq!(p.base, format!("{}/v1", srv.url()));
    let host = srv.url().trim_start_matches("http://").to_string();
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, SOURCE_NO_KEY);
    assert_eq!(status.user, host);
    assert!(status.detail.is_empty());
    let models = p.models(&CancelToken::new()).unwrap();
    assert_eq!(
        model_ids(&models),
        vec!["llama3.2:1b", "llama3.2:latest", "qwen2.5:7b"]
    );
    assert!(model(&models, "llama3.2:1b").default);
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    assert_eq!(saw.lock().unwrap().as_slice(), &[None, None]);
    assert!(p.as_login_exporter().unwrap().export_login().is_err());

    // A key, when present, travels as a Bearer token and shows on the card.
    saw.lock().unwrap().clear();
    let env = env_of(&[("OLLAMA_API_KEY", FAKE_KEY)]);
    let p = new_test_vendor("ollama", &srv.url(), env, None);
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, "environment");
    assert_eq!(status.user, "…mnop");
    p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(
        saw.lock().unwrap().as_slice(),
        &[Some(format!("Bearer {FAKE_KEY}"))]
    );

    // A pasted key is validated against the server and stored.
    let home = tempfile::tempdir().unwrap();
    let p = new_test_vendor_with_home(
        "ollama",
        home.path().to_string_lossy().as_ref(),
        &srv.url(),
        None,
        None,
    );
    let login = p
        .as_token_login()
        .unwrap()
        .login_with_token(&CancelToken::new(), FAKE_KEY)
        .unwrap();
    assert_eq!(login.source, "token login");
    assert!(p.keys.path().is_file());
    assert_eq!(p.status(&CancelToken::new()).unwrap().source, "token login");
    p.as_logouter().unwrap().logout().unwrap();
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, SOURCE_NO_KEY);

    // OLLAMA_HOST may be a bare host:port; OLLAMA_BASE_URL is the fallback.
    let cases = [
        ("127.0.0.1:11434", "http://127.0.0.1:11434/v1"),
        ("localhost:11434", "http://localhost:11434/v1"),
        ("http://gpu-box:11434", "http://gpu-box:11434/v1"),
        ("http://gpu-box:11434/", "http://gpu-box:11434/v1"),
        (
            "https://ollama.example.com/v1",
            "https://ollama.example.com/v1",
        ),
        ("http://gpu-box:11434/ollama", "http://gpu-box:11434/ollama"),
    ];
    for (configured, want) in cases {
        let p = new_test_vendor("ollama", "", env_of(&[("OLLAMA_HOST", configured)]), None);
        assert_eq!(p.base, want, "OLLAMA_HOST={configured}");
        let p = new_test_vendor("ollama", configured, None, None);
        assert_eq!(p.base, want, "setting {configured}");
    }
    let p = new_test_vendor(
        "ollama",
        "",
        env_of(&[("OLLAMA_BASE_URL", "http://other:11434/v1")]),
        None,
    );
    assert_eq!(p.base, "http://other:11434/v1");
    let p = new_test_vendor("ollama", "", None, None);
    assert_eq!(p.base, "http://127.0.0.1:11434/v1");
    assert_eq!(
        p.status(&CancelToken::new()).unwrap().user,
        "127.0.0.1:11434"
    );
    for (name, default) in [
        ("lmstudio", "http://127.0.0.1:1234/v1"),
        ("vllm", "http://127.0.0.1:8000/v1"),
    ] {
        let p = new_test_vendor(name, "", None, None);
        assert_eq!(p.base, default);
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in, "{name} needs no key");
        assert_eq!(status.source, SOURCE_NO_KEY);
    }
}

#[test]
fn model_lists_are_parsed_tolerantly() {
    let mistral = vendor("mistral").unwrap();
    let models = parse_models(
        mistral,
        &serde_json::to_vec(&json!({"object": "list", "data": [
            {
                "id": "mistral-embed",
                "object": "model",
                "owned_by": "mistralai",
                "capabilities": {"completion_chat": false, "completion_fim": false, "function_calling": false},
                "max_context_length": 8192
            },
            {
                "id": "mistral-large-2411",
                "capabilities": {"completion_chat": true, "function_calling": true},
                "max_context_length": 131072
            },
            {
                "id": "mistral-large-latest",
                "name": "mistral-large-latest",
                "capabilities": {"completion_chat": true, "function_calling": true},
                "max_context_length": 131072
            },
            {
                "id": "codestral-mamba-latest",
                "capabilities": {"completion_chat": true, "function_calling": false}
            },
            {"id": "   "},
            {"object": "model"},
            "not an object"
        ]}))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        model_ids(&models),
        vec![
            "codestral-mamba-latest",
            "mistral-large-2411",
            "mistral-large-latest",
            "mistral-embed"
        ]
    );
    let embed = model(&models, "mistral-embed");
    assert!(!embed.chat && !embed.tool_calls && !embed.default);
    assert_eq!(embed.vendor, "mistralai");
    assert_eq!(embed.context_window, 8192);
    let large = model(&models, "mistral-large-latest");
    assert!(large.chat && large.tool_calls && large.default);
    assert_eq!(large.vendor, "mistral");
    assert_eq!(large.context_window, 131072);
    assert_eq!(large.max_output, 0);
    assert!(!model(&models, "mistral-large-2411").default);
    let mamba = model(&models, "codestral-mamba-latest");
    assert!(mamba.chat && !mamba.tool_calls);
    assert_eq!(mamba.context_window, 0);

    let together = vendor("together").unwrap();
    let models = parse_models(
        together,
        &serde_json::to_vec(&json!([
            {
                "id": "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                "object": "model",
                "display_name": "Meta Llama 3.3 70B Instruct Turbo",
                "organization": "Meta",
                "context_length": 131072,
                "type": "chat"
            },
            {
                "id": "BAAI/bge-large-en-v1.5",
                "display_name": "BGE Large EN v1.5",
                "organization": "BAAI",
                "context_length": 512,
                "type": "embedding"
            }
        ]))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        model_ids(&models),
        vec![
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "BAAI/bge-large-en-v1.5"
        ]
    );
    let llama = model(&models, "meta-llama/Llama-3.3-70B-Instruct-Turbo");
    assert_eq!(llama.name, "Meta Llama 3.3 70B Instruct Turbo");
    assert_eq!(llama.vendor, "Meta");
    assert_eq!(llama.context_window, 131072);
    assert!(llama.chat && llama.tool_calls && llama.default);
    let bge = model(&models, "BAAI/bge-large-en-v1.5");
    assert!(!bge.chat && !bge.default);
    assert_eq!(bge.vendor, "BAAI");

    let openrouter = vendor("openrouter").unwrap();
    let models = parse_models(
        openrouter,
        &serde_json::to_vec(&json!({"data": [
            {
                "id": "x/no-tools",
                "name": "No Tools",
                "context_length": 4096,
                "supported_parameters": ["temperature", "top_p"]
            },
            {
                "id": "openai/gpt-4o",
                "name": "OpenAI: GPT-4o",
                "context_length": 128000,
                "top_provider": {"context_length": 128000, "max_completion_tokens": 16384},
                "supported_parameters": ["tools", "tool_choice", "temperature"]
            },
            {
                "id": "openai/gpt-4o-2024-11-20",
                "context_length": 128000.0,
                "supported_parameters": ["tools"]
            }
        ]}))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        model_ids(&models),
        vec!["openai/gpt-4o", "openai/gpt-4o-2024-11-20", "x/no-tools"]
    );
    let no_tools = model(&models, "x/no-tools");
    assert!(no_tools.chat && !no_tools.tool_calls && !no_tools.default);
    assert_eq!(no_tools.vendor, "openrouter");
    assert_eq!(no_tools.name, "No Tools");
    let gpt = model(&models, "openai/gpt-4o");
    assert!(gpt.tool_calls && gpt.default);
    assert_eq!(gpt.max_output, 16384);
    assert_eq!(
        model(&models, "openai/gpt-4o-2024-11-20").context_window,
        128000
    );

    // vLLM reports max_model_len; an unknown object shape is an empty list;
    // a scalar body is a decode error.
    let vllm = vendor("vllm").unwrap();
    let models = parse_models(
        vllm,
        br#"{"object":"list","data":[{"id":"Qwen/Qwen2.5-7B-Instruct","owned_by":"vllm","max_model_len":32768}]}"#,
    )
    .unwrap();
    assert_eq!(models[0].context_window, 32768);
    assert!(models[0].default);
    assert!(
        parse_models(vllm, br#"{"object":"list"}"#)
            .unwrap()
            .is_empty()
    );
    let err = parse_models(vllm, br#""nope""#).unwrap_err();
    assert_eq!(provider_error(&err).unwrap().code, "decode");
    let err = parse_models(vllm, b"<html>").unwrap_err();
    assert_eq!(provider_error(&err).unwrap().provider, "vllm");

    // Default families: prefixes in preference order, separators -, / and :.
    let groq = vendor("groq").unwrap();
    let ids = |ids: &[&str]| -> Vec<Model> {
        parse_models(
            groq,
            &serde_json::to_vec(
                &json!({"data": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()}),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let models = ids(&[
        "llama-3.3-70b-specdec",
        "llama-3.3-70b-versatile",
        "gemma2-9b-it",
    ]);
    assert!(model(&models, "llama-3.3-70b-versatile").default);
    let models = ids(&["llama-3.3-70b-specdec", "gemma2-9b-it"]);
    assert!(model(&models, "llama-3.3-70b-specdec").default);
    let models = ids(&["zeta", "alpha"]);
    assert!(model(&models, "alpha").default);
    assert!(ids(&[]).is_empty());
    assert!(id_in_family("GPT-4o", "gpt-4o"));
    assert!(id_in_family("gpt-4o-mini", "gpt-4o"));
    assert!(id_in_family("openai/gpt-4o", "openai"));
    assert!(id_in_family("llama3.2:latest", "llama3.2"));
    assert!(!id_in_family("gpt-4o1", "gpt-4o"));
    assert!(!id_in_family("gpt-4", "gpt-4o"));
}

#[test]
fn a_refused_stored_key_flips_status_until_logout() {
    let srv = Server::new(|_| {
        TestResponse::json(
            401,
            &json!({"error": {"message": format!("invalid key {FAKE_KEY}")}}),
        )
    });
    let home = tempfile::tempdir().unwrap();
    let p = new_test_vendor_with_home(
        "groq",
        home.path().to_string_lossy().as_ref(),
        &srv.url(),
        None,
        None,
    );
    p.keys.save(FAKE_KEY, &srv.url()).unwrap();
    assert!(
        home.path()
            .join("private")
            .join("providers")
            .join("groq.json")
            .is_file()
    );
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.source, "token login");

    let err = p.models(&CancelToken::new()).unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.provider, "groq");
    assert_eq!(perr.code, "unauthorized");
    assert_eq!(perr.status, 401);
    assert!(!perr.retryable);
    assert!(!err.to_string().contains(FAKE_KEY));
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(!status.logged_in);
    assert_eq!(
        status.detail,
        "the stored Groq API key was refused; sign in again"
    );

    p.as_logouter().unwrap().logout().unwrap();
    assert!(!p.keys.path().exists());
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(!status.logged_in);
    assert_eq!(
        status.detail,
        "Paste a Groq API key in Settings -> Harness or run pulse harness login --provider groq --with-token"
    );
    assert!(pulse_provider::is_not_logged_in(
        &p.models(&CancelToken::new()).unwrap_err()
    ));
    let err = p
        .as_token_login()
        .unwrap()
        .login_with_token(&CancelToken::new(), "")
        .unwrap_err();
    assert_eq!(
        provider_error(&err).unwrap().message,
        "Groq API key is empty"
    );
}

#[test]
fn errors_carry_the_vendor_name_and_retry_through_the_shared_loop() {
    let calls = Arc::new(AtomicI32::new(0));
    let calls_log = calls.clone();
    let srv = Server::new(move |_| {
        if calls_log.fetch_add(1, Ordering::SeqCst) == 0 {
            return TestResponse::json(
                503,
                &json!({"error": {"message": "overloaded", "type": "server_error"}}),
            );
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let logs_sink = logs.clone();
    let vendor = vendor("deepseek").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let env = Arc::new(BTreeMap::from([(
        "DEEPSEEK_API_KEY".to_string(),
        FAKE_KEY.to_string(),
    )]));
    let p = from_options(
        vendor,
        &mut Options {
            home: dir.path().to_string_lossy().into_owned(),
            settings: BTreeMap::from([(vendor.settings_key(), srv.url())]),
            env: Some(Arc::new(move |key| {
                env.get(key).cloned().unwrap_or_default()
            })),
            log: Some(Arc::new(move |line| {
                logs_sink.lock().unwrap().push(line.to_string())
            })),
            ..Options::default()
        },
    )
    .unwrap();
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let logs = logs.lock().unwrap().clone();
    assert_eq!(logs.len(), 1);
    assert!(
        logs[0].starts_with("deepseek chat retry in "),
        "unexpected log line {:?}",
        logs[0]
    );
    assert!(logs[0].contains("deepseek: overloaded (HTTP 503)"));

    let srv = Server::new(|_| {
        TestResponse::json(
            400,
            &json!({"error": {"code": "bad_model", "message": "unknown"}}),
        )
    });
    let p = new_test_vendor(
        "xai",
        &srv.url(),
        env_of(&[("XAI_API_KEY", FAKE_KEY)]),
        None,
    );
    let err = p.chat(&CancelToken::new(), user_request("hi")).unwrap_err();
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.provider, "xai");
    assert_eq!(perr.code, "bad_model");
    assert_eq!(perr.status, 400);
    assert!(!perr.retryable);
}

fn jwt(claims: Value) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.sig",
        b64.encode(br#"{"alg":"none","typ":"JWT"}"#),
        b64.encode(serde_json::to_vec(&claims).unwrap())
    )
}

const GH_DEVICE_TOKEN: &str = "gho_devicetoken0123456789";
/// The authorization header a GitHub device token travels in.
const GH_BEARER: &str = "Bearer gho_devicetoken0123456789";

fn device_login(p: &OpenAiCompatible) -> &dyn DeviceLogin {
    p.as_device_login().expect("device login offered")
}

#[test]
fn only_github_models_and_azure_offer_device_login() {
    let srv = Server::new(|_| panic!("no request expected"));
    for v in VENDORS {
        let p = new_test_vendor(v.name, &srv.url(), None, None);
        let offered = p.as_device_login().is_some();
        let expected = matches!(v.name, "github-models" | "azure-openai");
        assert_eq!(offered, expected, "{}", v.name);
        assert_eq!(v.device_login.is_some(), expected, "{}", v.name);
        if !expected {
            assert_eq!(v.resolve_tenant(&BTreeMap::new(), |_| String::new()), "");
            let err = p.device_setup().unwrap_err();
            assert_eq!(
                provider_error(&err).unwrap().code,
                "device_login_unsupported"
            );
        }
    }
    assert_eq!(
        vendor("github-models").unwrap().device_login,
        Some(DeviceKind::GitHub)
    );
    assert_eq!(
        vendor("azure-openai").unwrap().device_login,
        Some(DeviceKind::Entra)
    );

    // The status card of a device vendor without a key names the sign-in.
    let p = new_test_vendor("github-models", &srv.url(), None, None);
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(!status.logged_in);
    assert_eq!(
        status.detail,
        "Sign in with GitHub in Settings -> Harness or run pulse harness login --provider github-models; a GitHub token also works with pulse harness login --provider github-models --with-token"
    );
}

#[test]
fn github_models_device_login_signs_in_validates_and_stores() {
    let polls = Arc::new(AtomicI32::new(0));
    let polls_seen = polls.clone();
    let saw = Arc::new(Mutex::new(Vec::<(String, String, Option<String>)>::new()));
    let saw_log = saw.clone();
    let srv = Server::new(move |req| {
        saw_log.lock().unwrap().push((
            req.method.clone(),
            req.path().to_string(),
            req.header("authorization").map(str::to_string),
        ));
        match (req.method.as_str(), req.path()) {
            ("POST", "/login/device/code") => {
                assert_eq!(req.header("accept"), Some("application/json"));
                assert_eq!(
                    req.body_str(),
                    format!("client_id={GITHUB_MODELS_CLIENT_ID}&scope=read%3Auser")
                );
                TestResponse::json(
                    200,
                    &json!({
                        "device_code": "gh-device-code-0123456789abcdef",
                        "user_code": "WDJB-MJHT",
                        "verification_uri": "https://github.com/login/device",
                        "expires_in": 899,
                        "interval": 5
                    }),
                )
            }
            ("POST", "/login/oauth/access_token") => {
                assert!(
                    req.body_str()
                        .contains("device_code=gh-device-code-0123456789abcdef")
                );
                assert!(
                    req.body_str()
                        .contains(&format!("client_id={GITHUB_MODELS_CLIENT_ID}"))
                );
                match polls_seen.fetch_add(1, Ordering::SeqCst) {
                    0 => TestResponse::json(200, &json!({"error": "authorization_pending"})),
                    _ => TestResponse::json(
                        200,
                        &json!({"access_token": GH_DEVICE_TOKEN, "token_type": "bearer", "scope": "read:user"}),
                    ),
                }
            }
            ("GET", "/user") => {
                assert_eq!(req.header("authorization"), Some(GH_BEARER));
                assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
                TestResponse::json(200, &json!({"login": "octocat", "id": 1}))
            }
            ("GET", "/models") => {
                assert_eq!(req.header("authorization"), Some(GH_BEARER));
                assert_eq!(req.header("x-github-api-version"), Some("2022-11-28"));
                TestResponse::json(200, &json!({"data": [{"id": "openai/gpt-4.1"}]}))
            }
            ("POST", "/chat/completions") => {
                assert_eq!(req.header("authorization"), Some(GH_BEARER));
                TestResponse::json(200, &ok_chat_body())
            }
            _ => TestResponse::text(404, "nope"),
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_string_lossy().into_owned();
    let env = env_of(&[
        ("GITHUB_MODELS_OAUTH_BASE", &srv.url()),
        ("GITHUB_MODELS_API_BASE", &format!("{}/", srv.url())),
    ]);
    let p = new_test_vendor_with_home("github-models", &home, &srv.url(), env, None);
    assert!(!p.status(&CancelToken::new()).unwrap().logged_in);

    let flow = device_login(&p)
        .begin_device_login(&CancelToken::new())
        .unwrap();
    assert_eq!(flow.user_code, "WDJB-MJHT");
    assert_eq!(flow.verification_uri, "https://github.com/login/device");
    assert_eq!(flow.interval, 5);
    assert_eq!(flow.expires_in, 899);

    let (word, login) = device_login(&p)
        .poll_device_login(&CancelToken::new(), &flow.device_code)
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_PENDING);
    assert_eq!(login, Login::default());
    assert!(!p.keys.path().exists(), "nothing stored while pending");

    let (word, login) = device_login(&p)
        .poll_device_login(&CancelToken::new(), &flow.device_code)
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_COMPLETE);
    assert!(login.logged_in);
    assert_eq!(login.user, "octocat");
    assert_eq!(login.source, "device login");
    assert!(login.expires_at.is_empty(), "GitHub tokens do not expire");

    let stored = p.keys.read_stored().unwrap().unwrap();
    assert_eq!(stored.api_key, GH_DEVICE_TOKEN);
    assert_eq!(stored.user, "octocat");
    assert_eq!(stored.source, "device login");
    assert_eq!(stored.base_url, srv.url());
    assert!(stored.refresh_token.is_empty());
    assert!(stored.expires_at.is_empty());
    assert_eq!(
        stored.issuer,
        format!("{}/login/oauth/access_token", srv.url())
    );
    assert_eq!(
        pulse_provider::redact("gho_devicetoken0123456789 gh-device-code-0123456789abcdef"),
        "[redacted] [redacted]"
    );

    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.user, "octocat");
    assert_eq!(status.source, "device login");

    // The stored token is sent as a bearer token on model and chat calls.
    let models = p.models(&CancelToken::new()).unwrap();
    assert_eq!(model_ids(&models), ["openai/gpt-4.1"]);
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    let (token, exported) = p.as_login_exporter().unwrap().export_login().unwrap();
    assert_eq!(token, GH_DEVICE_TOKEN);
    assert_eq!(exported.user, "octocat");
    assert_eq!(exported.source, "stored");

    let paths: Vec<(String, String)> = saw
        .lock()
        .unwrap()
        .iter()
        .map(|(m, p, _)| (m.clone(), p.clone()))
        .collect();
    assert_eq!(
        paths,
        [
            ("POST".to_string(), "/login/device/code".to_string()),
            ("POST".to_string(), "/login/oauth/access_token".to_string()),
            ("POST".to_string(), "/login/oauth/access_token".to_string()),
            ("GET".to_string(), "/user".to_string()),
            ("GET".to_string(), "/models".to_string()),
            ("POST".to_string(), "/chat/completions".to_string()),
        ]
    );

    p.as_logouter().unwrap().logout().unwrap();
    assert!(!p.keys.path().exists());
    assert!(!p.status(&CancelToken::new()).unwrap().logged_in);
}

#[test]
fn github_models_device_login_is_not_stored_when_the_token_is_refused() {
    let srv = Server::new(move |req| match (req.method.as_str(), req.path()) {
        ("POST", "/login/device/code") => TestResponse::json(
            200,
            &json!({"device_code": "gh-device-code-2", "user_code": "AAAA-BBBB", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5}),
        ),
        ("POST", "/login/oauth/access_token") => {
            if req.body_str().contains("device_code=denied") {
                return TestResponse::json(200, &json!({"error": "access_denied"}));
            }
            if req.body_str().contains("device_code=expired") {
                return TestResponse::json(200, &json!({"error": "expired_token"}));
            }
            TestResponse::json(
                200,
                &json!({"access_token": "gho_nomodels0123456789", "token_type": "bearer"}),
            )
        }
        ("GET", "/user") => TestResponse::json(200, &json!({"login": "octocat"})),
        ("GET", "/models") => TestResponse::json(403, &json!({"message": "Forbidden"})),
        _ => TestResponse::text(404, "nope"),
    });
    let env = env_of(&[
        ("GITHUB_MODELS_OAUTH_BASE", &srv.url()),
        ("GITHUB_MODELS_API_BASE", &srv.url()),
    ]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    let login = device_login(&p);
    let flow = login.begin_device_login(&CancelToken::new()).unwrap();
    let err = login
        .poll_device_login(&CancelToken::new(), &flow.device_code)
        .unwrap_err();
    assert!(pulse_provider::http::is_unauthorized(&err), "{err}");
    assert_eq!(provider_error(&err).unwrap().provider, "github-models");
    assert!(!p.keys.path().exists(), "a refused token is not stored");
    assert!(!p.status(&CancelToken::new()).unwrap().logged_in);

    // The owner turning the sign-in down or letting the code expire are
    // words, not errors.
    let (word, _) = login
        .poll_device_login(&CancelToken::new(), "denied")
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_DENIED);
    let (word, _) = login
        .poll_device_login(&CancelToken::new(), "expired")
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_EXPIRED);

    // A cancelled token stops before any request.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(login.begin_device_login(&cancel).is_err());
    assert!(login.poll_device_login(&cancel, "x").is_err());
}

#[test]
fn azure_device_login_uses_entra_bearer_tokens_and_refreshes_them() {
    let polls = Arc::new(AtomicI32::new(0));
    let polls_seen = polls.clone();
    let refreshes = Arc::new(AtomicI32::new(0));
    let refreshes_seen = refreshes.clone();
    let refresh_fails = Arc::new(AtomicBool::new(false));
    let refresh_fails_seen = refresh_fails.clone();
    let seen_auth = Arc::new(Mutex::new(Vec::<(Option<String>, Option<String>)>::new()));
    let seen_auth_log = seen_auth.clone();
    let id_token = jwt(json!({
        "preferred_username": "owner@contoso.com",
        "name": "Owner",
        "tid": "11111111-2222-3333-4444-555555555555"
    }));
    let access_1 = jwt(json!({"aud": "https://cognitiveservices.azure.com", "n": 1}));
    let access_2 = jwt(json!({"aud": "https://cognitiveservices.azure.com", "n": 2}));
    let (id_token_srv, access_1_srv, access_2_srv) =
        (id_token.clone(), access_1.clone(), access_2.clone());
    let srv = Server::new(move |req| match (req.method.as_str(), req.path()) {
        ("POST", "/organizations/oauth2/v2.0/devicecode") => {
            let body = req.body_str();
            assert!(
                body.contains(&format!("client_id={AZURE_OPENAI_CLIENT_ID}")),
                "{body}"
            );
            assert!(
                body.contains("scope=https%3A%2F%2Fcognitiveservices.azure.com%2F.default+offline_access+openid+profile"),
                "{body}"
            );
            TestResponse::json(
                200,
                &json!({
                    "device_code": "entra-device-code-0123456789abcdef",
                    "user_code": "F7G8H9J",
                    "verification_uri": "https://microsoft.com/devicelogin",
                    "expires_in": 900,
                    "interval": 5,
                    "message": "To sign in, use a web browser to open https://microsoft.com/devicelogin and enter the code F7G8H9J."
                }),
            )
        }
        ("POST", "/organizations/oauth2/v2.0/token") => {
            let body = req.body_str();
            if body.contains("grant_type=refresh_token") {
                refreshes_seen.fetch_add(1, Ordering::SeqCst);
                assert!(body.contains("refresh_token=refresh-1"), "{body}");
                assert!(body.contains(&format!("client_id={AZURE_OPENAI_CLIENT_ID}")));
                if refresh_fails_seen.load(Ordering::SeqCst) {
                    return TestResponse::json(
                        400,
                        &json!({"error": "invalid_grant", "error_description": "AADSTS70008: The refresh token has expired."}),
                    );
                }
                return TestResponse::json(
                    200,
                    &json!({
                        "access_token": access_2_srv,
                        "refresh_token": "refresh-2",
                        "expires_in": 3600,
                        "token_type": "Bearer",
                        "scope": "https://cognitiveservices.azure.com/.default"
                    }),
                );
            }
            assert!(body.contains("device_code=entra-device-code-0123456789abcdef"));
            match polls_seen.fetch_add(1, Ordering::SeqCst) {
                0 => TestResponse::json(
                    400,
                    &json!({"error": "authorization_pending", "error_description": "AADSTS70016: Pending end-user authorization."}),
                ),
                _ => TestResponse::json(
                    200,
                    &json!({
                        "access_token": access_1_srv,
                        "refresh_token": "refresh-1",
                        "id_token": id_token_srv,
                        "expires_in": 3599,
                        "token_type": "Bearer",
                        "scope": "https://cognitiveservices.azure.com/.default openid profile"
                    }),
                ),
            }
        }
        ("GET", "/models") | ("POST", "/chat/completions") => {
            seen_auth_log.lock().unwrap().push((
                req.header("authorization").map(str::to_string),
                req.header("api-key").map(str::to_string),
            ));
            if req.path() == "/models" {
                TestResponse::json(200, &json!({"data": [{"id": "gpt-4o"}]}))
            } else {
                TestResponse::json(200, &ok_chat_body())
            }
        }
        _ => TestResponse::text(404, "nope"),
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_string_lossy().into_owned();
    let env = env_of(&[("AZURE_OPENAI_AUTHORITY", &format!("{}/", srv.url()))]);
    let p = new_test_vendor_with_home("azure-openai", &home, &srv.url(), env.clone(), None);
    let setup = p.device.as_ref().unwrap();
    assert_eq!(setup.kind, DeviceKind::Entra);
    assert_eq!(setup.tenant, "organizations");
    assert_eq!(
        setup.cfg.device_url,
        format!("{}/organizations/oauth2/v2.0/devicecode", srv.url())
    );

    let login = device_login(&p);
    let flow = login.begin_device_login(&CancelToken::new()).unwrap();
    assert_eq!(flow.user_code, "F7G8H9J");
    assert_eq!(flow.verification_uri, "https://microsoft.com/devicelogin");
    let (word, _) = login
        .poll_device_login(&CancelToken::new(), &flow.device_code)
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_PENDING);
    let (word, signed_in) = login
        .poll_device_login(&CancelToken::new(), &flow.device_code)
        .unwrap();
    assert_eq!(word, pulse_provider::DEVICE_COMPLETE);
    assert!(signed_in.logged_in);
    assert_eq!(signed_in.user, "owner@contoso.com");
    assert_eq!(signed_in.source, "device login");
    assert!(
        signed_in.expires_at.ends_with('Z'),
        "{}",
        signed_in.expires_at
    );

    let stored = p.keys.read_stored().unwrap().unwrap();
    assert_eq!(stored.api_key, access_1);
    assert_eq!(stored.refresh_token, "refresh-1");
    assert_eq!(stored.user, "owner@contoso.com");
    assert_eq!(
        stored.issuer,
        format!("{}/organizations/oauth2/v2.0/token", srv.url())
    );
    assert!(!stored.expired());
    assert!(!stored.expires_within(REFRESH_AHEAD));
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(status.logged_in);
    assert_eq!(status.expires_at, stored.expires_at);

    // Validation and the first chat send the Entra token as a bearer, not
    // in the api-key header, and no refresh happens while it is fresh.
    let resp = p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    assert_eq!(resp.message.unwrap().content, "ok");
    {
        let seen = seen_auth.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for (authorization, api_key) in seen.iter() {
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {access_1}").as_str())
            );
            assert!(api_key.is_none());
        }
    }
    assert_eq!(refreshes.load(Ordering::SeqCst), 0);

    // A token about to expire is refreshed before the call and the
    // rotated refresh token stored.
    p.keys
        .save_login(StoredApiKey {
            expires_at: pulse_provider::device::expires_at(60),
            ..stored.clone()
        })
        .unwrap();
    p.chat(&CancelToken::new(), user_request("again")).unwrap();
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let refreshed = p.keys.read_stored().unwrap().unwrap();
    assert_eq!(refreshed.api_key, access_2);
    assert_eq!(refreshed.refresh_token, "refresh-2");
    assert_eq!(refreshed.user, "owner@contoso.com");
    assert_eq!(refreshed.source, "device login");
    assert_eq!(refreshed.issuer, stored.issuer);
    assert!(!refreshed.expires_within(REFRESH_AHEAD));
    assert_eq!(
        seen_auth.lock().unwrap().last().unwrap().0.as_deref(),
        Some(format!("Bearer {access_2}").as_str())
    );
    // A fresh token is used as is on the next call.
    p.chat(&CancelToken::new(), user_request("third")).unwrap();
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);

    // A refresh that fails while the token still has time left is not
    // fatal: the current token is used and the sign-in stays.
    refresh_fails.store(true, Ordering::SeqCst);
    p.keys
        .save_login(StoredApiKey {
            refresh_token: "refresh-1".into(),
            expires_at: pulse_provider::device::expires_at(60),
            ..refreshed.clone()
        })
        .unwrap();
    p.chat(&CancelToken::new(), user_request("fourth")).unwrap();
    assert_eq!(refreshes.load(Ordering::SeqCst), 2);
    assert!(p.status(&CancelToken::new()).unwrap().logged_in);

    // Once the token has expired and the refresh still fails, the call
    // fails clearly and the status card asks for a new sign-in.
    p.keys
        .save_login(StoredApiKey {
            refresh_token: "refresh-1".into(),
            expires_at: "2000-01-01T00:00:00Z".into(),
            ..refreshed.clone()
        })
        .unwrap();
    let err = p
        .chat(&CancelToken::new(), user_request("fifth"))
        .unwrap_err();
    assert!(pulse_provider::is_not_logged_in(&err), "{err:#}");
    let perr = provider_error(&err).unwrap();
    assert_eq!(perr.provider, "azure-openai");
    assert_eq!(perr.code, "refresh_failed");
    assert!(!perr.retryable);
    assert!(
        perr.message.contains("could not be refreshed"),
        "{}",
        perr.message
    );
    assert!(perr.message.contains("AADSTS70008"), "{}", perr.message);
    let status = p.status(&CancelToken::new()).unwrap();
    assert!(!status.logged_in);
    assert!(status.detail.contains("sign in again"), "{}", status.detail);
    assert_eq!(refreshes.load(Ordering::SeqCst), 3);

    // Without a base URL the sign-in cannot even begin.
    let p = new_test_vendor("azure-openai", "", env, None);
    let err = device_login(&p)
        .begin_device_login(&CancelToken::new())
        .unwrap_err();
    assert_eq!(provider_error(&err).unwrap().code, "no_base_url");
}

#[test]
fn azure_tenant_comes_from_the_setting_then_the_environment() {
    let v = vendor("azure-openai").unwrap();
    let no_env = |_: &str| String::new();
    assert_eq!(v.resolve_tenant(&BTreeMap::new(), no_env), "organizations");
    let defaulted = BTreeMap::from([(
        AZURE_TENANT_SETTING.to_string(),
        "organizations".to_string(),
    )]);
    assert_eq!(v.resolve_tenant(&defaulted, no_env), "organizations");
    let env = |key: &str| match key {
        "AZURE_OPENAI_TENANT_ID" => "openai-tenant".to_string(),
        "AZURE_TENANT_ID" => "az-tenant".to_string(),
        _ => String::new(),
    };
    assert_eq!(v.resolve_tenant(&defaulted, env), "openai-tenant");
    let only_az = |key: &str| match key {
        "AZURE_TENANT_ID" => "az-tenant".to_string(),
        _ => String::new(),
    };
    assert_eq!(v.resolve_tenant(&BTreeMap::new(), only_az), "az-tenant");
    let chosen = BTreeMap::from([(
        AZURE_TENANT_SETTING.to_string(),
        " contoso.onmicrosoft.com/ ".to_string(),
    )]);
    assert_eq!(v.resolve_tenant(&chosen, env), "contoso.onmicrosoft.com");

    // The resolved tenant, authority and client land in the endpoints.
    let srv = Server::new(|_| panic!("no request expected"));
    let settings = Some(BTreeMap::from([(
        AZURE_TENANT_SETTING.to_string(),
        "contoso.onmicrosoft.com".to_string(),
    )]));
    let env = env_of(&[
        ("AZURE_OPENAI_AUTHORITY", "https://login.microsoftonline.us"),
        ("AZURE_OPENAI_CLIENT_ID", "my-client"),
        (
            "AZURE_OPENAI_SCOPE",
            "https://cognitiveservices.azure.us/.default offline_access",
        ),
    ]);
    let p = new_test_vendor("azure-openai", &srv.url(), env, settings);
    let setup = p.device.as_ref().unwrap();
    assert_eq!(setup.tenant, "contoso.onmicrosoft.com");
    assert_eq!(
        setup.cfg.device_url,
        "https://login.microsoftonline.us/contoso.onmicrosoft.com/oauth2/v2.0/devicecode"
    );
    assert_eq!(
        setup.cfg.token_url,
        "https://login.microsoftonline.us/contoso.onmicrosoft.com/oauth2/v2.0/token"
    );
    assert_eq!(setup.cfg.client_id, "my-client");
    assert_eq!(
        setup.cfg.scope,
        "https://cognitiveservices.azure.us/.default offline_access"
    );

    // GitHub's endpoints follow their own overrides.
    let env = env_of(&[
        ("GITHUB_MODELS_OAUTH_BASE", "https://ghe.example.com/"),
        ("GITHUB_MODELS_API_BASE", "https://ghe.example.com/api/v3"),
        ("GITHUB_MODELS_CLIENT_ID", "gh-client"),
    ]);
    let p = new_test_vendor("github-models", &srv.url(), env, None);
    let setup = p.device.as_ref().unwrap();
    assert_eq!(setup.kind, DeviceKind::GitHub);
    assert_eq!(
        setup.cfg.device_url,
        "https://ghe.example.com/login/device/code"
    );
    assert_eq!(
        setup.cfg.token_url,
        "https://ghe.example.com/login/oauth/access_token"
    );
    assert_eq!(setup.cfg.client_id, "gh-client");
    assert_eq!(setup.cfg.scope, "read:user");
    assert_eq!(setup.api_base, "https://ghe.example.com/api/v3");
    let p = new_test_vendor("github-models", &srv.url(), None, None);
    let setup = p.device.as_ref().unwrap();
    assert_eq!(setup.cfg.device_url, "https://github.com/login/device/code");
    assert_eq!(setup.cfg.client_id, GITHUB_MODELS_CLIENT_ID);
    assert_eq!(setup.api_base, "https://api.github.com");
}

#[test]
fn entra_user_label_prefers_the_account_name() {
    assert_eq!(
        entra_user(
            &jwt(json!({"preferred_username": " owner@contoso.com ", "name": "Owner"})),
            "t"
        ),
        "owner@contoso.com"
    );
    assert_eq!(
        entra_user(&jwt(json!({"upn": "owner@contoso.com"})), "t"),
        "owner@contoso.com"
    );
    assert_eq!(
        entra_user(&jwt(json!({"email": "o@c.com"})), "t"),
        "o@c.com"
    );
    assert_eq!(entra_user(&jwt(json!({"name": "Owner"})), "t"), "Owner");
    assert_eq!(
        entra_user(&jwt(json!({"name": "", "oid": "x"})), "contoso"),
        "tenant contoso"
    );
    assert_eq!(entra_user("", ""), "Microsoft account");
    assert_eq!(
        entra_user("not.a.jwt", "organizations"),
        "tenant organizations"
    );
}

#[test]
fn a_pasted_jwt_reaches_azure_as_a_bearer_token() {
    let seen = Arc::new(Mutex::new(Vec::<(Option<String>, Option<String>)>::new()));
    let seen_log = seen.clone();
    let srv = Server::new(move |req| {
        seen_log.lock().unwrap().push((
            req.header("authorization").map(str::to_string),
            req.header("api-key").map(str::to_string),
        ));
        if req.path() == "/models" {
            return TestResponse::json(200, &json!({"data": [{"id": "gpt-4o"}]}));
        }
        TestResponse::json(200, &ok_chat_body())
    });
    let token = jwt(json!({"aud": "https://cognitiveservices.azure.com"}));
    let p = new_test_vendor("azure-openai", &srv.url(), None, None);
    let login = p
        .as_token_login()
        .unwrap()
        .login_with_token(&CancelToken::new(), &token)
        .unwrap();
    assert_eq!(login.source, "token login");
    p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    let first = seen.lock().unwrap().clone();
    assert_eq!(first.len(), 2);
    for (authorization, api_key) in &first {
        assert_eq!(
            authorization.as_deref(),
            Some(format!("Bearer {token}").as_str())
        );
        assert!(api_key.is_none(), "a JWT never goes in the api-key header");
    }

    // A plain key still travels in the api-key header.
    let p = new_test_vendor(
        "azure-openai",
        &srv.url(),
        env_of(&[("AZURE_OPENAI_API_KEY", FAKE_KEY)]),
        None,
    );
    p.chat(&CancelToken::new(), user_request("hi")).unwrap();
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.1.as_deref(), Some(FAKE_KEY));
    assert!(last.0.is_none());
}
