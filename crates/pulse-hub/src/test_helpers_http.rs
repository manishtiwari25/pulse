use crate::httpx::HandlerFn;
use crate::server::{Handler, JsonMap, new_handler};
use serde_json::Value;
use std::sync::Arc;

pub(crate) struct TestStore {
    pub(crate) store: pulse_store::Store,
    _guards: Vec<pulse_testutil::SetenvGuard>,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestStore {
    type Target = pulse_store::Store;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

pub(crate) fn additions_store(skip_records: bool) -> TestStore {
    let guards = vec![
        pulse_testutil::setenv("PULSE_JOB", ""),
        pulse_testutil::setenv("PULSE_AGENT", ""),
        pulse_testutil::setenv("PULSE_SESSION", ""),
        pulse_testutil::setenv("PULSE_PROJECT", ""),
    ];
    let dir = tempfile::tempdir().expect("temp home");
    let store = pulse_store::Store::open(pulse_store::Options {
        home: dir.path().display().to_string(),
        create: true,
        skip_records,
        now: "2026-09-06T12:00:00Z".to_string(),
        ..pulse_store::Options::default()
    })
    .expect("open test store");
    TestStore {
        store,
        _guards: guards,
        _dir: dir,
    }
}

pub(crate) fn handler(store: &pulse_store::Store) -> Handler {
    new_handler(Some(Arc::new(store.clone())), None)
}

pub(crate) fn server(handler: Handler) -> crate::test_helpers::TestServer {
    let h = Arc::new(handler);
    let f: HandlerFn = Arc::new(move |w, r| h.serve_http(w, r));
    crate::test_helpers::TestServer::start(f)
}

pub(crate) fn request(
    h: Handler,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (u16, JsonMap, String) {
    let srv = server(h);
    let client = crate::test_helpers::client();
    let url = format!("{}{}", srv.url, path);
    let mut req = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "DELETE" => client.delete(&url),
        _ => client.request(method.parse().expect("method"), &url),
    };
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if method == "POST" || method == "DELETE" {
        req = req
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }
    let resp = req.send().expect("send test request");
    let status = resp.status().as_u16();
    let raw = resp.text().expect("response text");
    let value: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);
    let map = value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    (status, map, raw)
}

pub(crate) fn additions_project(store: &pulse_store::Store, slug: &str) {
    let path = std::path::Path::new(&store.home).join(slug);
    std::fs::create_dir_all(&path).expect("project dir");
    store
        .conn()
        .expect("conn")
        .execute(
            "INSERT OR REPLACE INTO projects(slug,name,path,added_at,status) VALUES(?,?,?,?, 'active')",
            rusqlite::params![slug, slug, path.display().to_string(), store.now()],
        )
        .expect("insert project");
}

pub(crate) fn additions_record(
    store: &pulse_store::Store,
    id: &str,
    project: &str,
    kind: &str,
    slug: &str,
    body: &str,
) -> pulse_store::Record {
    let rec = pulse_store::Record {
        id: id.to_string(),
        project: project.to_string(),
        kind: kind.to_string(),
        slug: slug.to_string(),
        title: "A record".to_string(),
        status: "Proposed".to_string(),
        areas: vec!["hub".to_string()],
        tags: vec!["test".to_string()],
        ..pulse_store::Record::default()
    };
    store
        .put_record(
            rec.clone(),
            serde_json::json!({"title": rec.title, "status": rec.status})
                .as_object()
                .expect("frontmatter")
                .clone()
                .into_iter()
                .collect(),
            body,
            "human",
            "initial",
        )
        .expect("put record");
    rec
}

pub(crate) fn json_body(v: serde_json::Value) -> String {
    pulse_store::gojson::to_string(&v).expect("json body")
}

pub(crate) fn strconv_int64(n: i64) -> String {
    n.to_string()
}

pub(crate) fn additions_request(
    handler: Handler,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, serde_json::Map<String, Value>) {
    let h = Arc::new(handler);
    let server = crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
    let client = crate::test_helpers::client();
    let url = format!("{}{}", server.url, path);
    let mut req = match method {
        "POST" => client.post(&url).header("Content-Type", "application/json"),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };
    if !body.is_empty() {
        req = req.body(body.to_string());
    }
    let response = req.send().expect("send request");
    let status = response.status().as_u16();
    let text = response.text().expect("read response");
    let result = serde_json::from_str::<Value>(&text).unwrap_or_else(|err| {
        panic!("{method} {path}: status {status}, non-JSON response {text:?}: {err}")
    });
    (status, result.as_object().expect("object response").clone())
}

pub(crate) fn event_subjects(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["subject"].as_str().unwrap_or("").to_string())
        .collect()
}
