use crate::server::{Handler, new_handler};
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

pub(crate) fn additions_project(store: &pulse_store::Store, slug: &str) {
    let conn = store.conn().expect("store conn");
    conn.execute(
        "INSERT INTO projects(slug,name,path,added_at) VALUES(?,?,?,?)",
        rusqlite::params![
            slug,
            slug,
            std::path::Path::new(&store.home)
                .join(slug)
                .to_string_lossy(),
            store.now()
        ],
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
            std::collections::BTreeMap::from([
                ("title".to_string(), serde_json::json!(rec.title)),
                ("status".to_string(), serde_json::json!(rec.status)),
            ]),
            body,
            "human",
            "initial",
        )
        .expect("put record");
    rec
}

pub(crate) fn event_subjects(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["subject"].as_str().unwrap_or("").to_string())
        .collect()
}
