use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) const HELDOUT_SETTING: &str = "evolve.heldout_dir";
pub(crate) const PRIVATE_VALUE: &str = pulse_bus::PRIVATE_VALUE;
pub(crate) fn hidden_setting_value(value: String) -> String {
    if value.is_empty() {
        String::new()
    } else {
        PRIVATE_VALUE.to_string()
    }
}
pub(crate) fn private_setting_event(event: serde_json::Value) -> serde_json::Value {
    let mut event = event;
    let subject = event
        .get("subject")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if pulse_bus::PRIVATE_SETTINGS.contains(&subject.as_str()) {
        if let Some(obj) = event.as_object_mut() {
            let value = obj
                .get("detail")
                .and_then(|v| v.as_str())
                .map(|s| hidden_setting_value(s.to_string()))
                .unwrap_or_else(|| PRIVATE_VALUE.to_string());
            obj.insert("detail".into(), serde_json::Value::String(value));
        }
        return event;
    }
    let private_detail = event
        .get("detail")
        .and_then(|v| v.as_object())
        .map(|detail| {
            ["name", "key"].iter().any(|k| {
                detail
                    .get(*k)
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| pulse_bus::PRIVATE_SETTINGS.contains(&name))
            })
        })
        .unwrap_or(false);
    if private_detail && let Some(obj) = event.as_object_mut() {
        obj.insert(
            "detail".into(),
            serde_json::Value::String(PRIVATE_VALUE.into()),
        );
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::additions::AdditionEvent;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader};
    use std::sync::Arc;
    use std::time::Duration;

    fn assert_private(value: &Value, paths: &[&str]) {
        match value {
            Value::String(s) => {
                for path in paths {
                    assert!(
                        !s.contains(path),
                        "API exposed held-out path {path:?} in {s:?}"
                    );
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    assert_private(&Value::String(key.clone()), paths);
                    assert_private(item, paths);
                }
            }
            Value::Array(items) => {
                for item in items {
                    assert_private(item, paths);
                }
            }
            _ => {}
        }
    }

    fn confirmed_request(
        h: crate::server::Handler,
        route: &str,
        body: String,
    ) -> (u16, serde_json::Map<String, Value>) {
        let h = Arc::new(h);
        let server =
            crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
        let response = crate::test_helpers::client()
            .post(format!("{}/api/v1/{}", server.url, route))
            .header("Content-Type", "application/json")
            .header("X-Pulse-Decide", "yes")
            .header("X-Pulse-Settings", "yes")
            .body(body)
            .send()
            .expect("send confirmed request");
        let status = response.status().as_u16();
        let text = response.text().expect("read confirmed response");
        let payload: Value = serde_json::from_str(&text).expect("write returned invalid JSON");
        (
            status,
            payload.as_object().expect("object response").clone(),
        )
    }

    #[test]
    fn heldout_settings_never_leave_api() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        let h = crate::server::new_handler(Some(Arc::new((*s).clone())), None);
        let server = crate::test_helpers_http::server(h.clone_for_test());
        let paths = [
            "/private/old-heldout-canary",
            "/private/\u{263a}-\"quoted\"-heldout-canary",
        ];
        let set = |path: &str| {
            let body =
                pulse_store::gojson::to_string(&json!({"name": HELDOUT_SETTING, "value": path}))
                    .unwrap();
            let (status, payload) = confirmed_request(h.clone_for_test(), "settings", body);
            assert_eq!(status, 200, "private setting update: {payload:#?}");
            let row = payload["setting"].as_object().expect("setting row");
            assert_eq!(row["secret"], true);
            assert_eq!(row["value"], hidden_setting_value(path.to_string()));
            assert_private(&Value::Object(payload), &paths);
            assert_eq!(s.setting(HELDOUT_SETTING, ""), path);
        };
        let cursor: i64 = s
            .conn()
            .expect("conn")
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .expect("cursor");
        set(paths[0]);
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
            .get(format!(
                "{}/api/v1/events/stream?since={}",
                server.url, cursor
            ))
            .send()
            .expect("event stream");
        assert_eq!(response.status().as_u16(), 200);
        set(paths[1]);
        let mut reader = BufReader::new(response);
        let mut read_event = || -> AdditionEvent {
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("reading event stream");
                assert_private(&Value::String(line.clone()), &paths);
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let event: AdditionEvent = serde_json::from_str(data).expect("event JSON");
                assert_private(&Value::String(event.subject.clone()), &paths);
                assert_private(&event.detail, &paths);
                return event;
            }
        };
        let mut setting_events = 0;
        while setting_events < 2 {
            let event = read_event();
            if event.subject == HELDOUT_SETTING {
                setting_events += 1;
                assert_eq!(
                    event.detail, PRIVATE_VALUE,
                    "SSE event was not masked: {event:#?}"
                );
            }
        }
        for path in paths {
            s.event(
                "agent",
                "post",
                &format!("location:{path}"),
                json!({"note": format!("Location: {path}")}),
            )
            .expect("post event");
            loop {
                let event = read_event();
                if event.action == "post" {
                    assert_eq!(event.subject, format!("location:{PRIVATE_VALUE}"));
                    assert_eq!(event.detail["note"], format!("Location: {PRIVATE_VALUE}"));
                    break;
                }
            }
        }
        let detail: String = s
            .conn()
            .expect("conn")
            .query_row(
                "SELECT detail FROM events WHERE subject=? ORDER BY seq DESC LIMIT 1",
                [HELDOUT_SETTING],
                |r| r.get(0),
            )
            .expect("detail");
        assert_eq!(detail, paths[1]);
        set("");
        loop {
            let event = read_event();
            if event.subject == HELDOUT_SETTING {
                assert_eq!(
                    event.detail, "",
                    "SSE reported a cleared setting as configured: {event:#?}"
                );
                break;
            }
        }
        s.event(
            "agent",
            "post",
            &format!("location:{}", paths[1]),
            json!({"note": format!("Location: {}", paths[1])}),
        )
        .expect("post after clear");
        loop {
            let event = read_event();
            if event.action == "post" {
                assert_eq!(event.subject, format!("location:{PRIVATE_VALUE}"));
                assert_eq!(event.detail["note"], format!("Location: {PRIVATE_VALUE}"));
                break;
            }
        }
        drop(reader);
        for path in ["/api/v1/settings", "/api/v1/events", "/api/v1/timeline"] {
            let (_, payload, raw) =
                crate::test_helpers_http::request(h.clone_for_test(), "GET", path, "", &[]);
            assert!(!raw.is_empty(), "{path} returned empty body");
            assert_private(&Value::Object(payload.into_iter().collect()), &paths);
        }
    }

    #[test]
    fn private_setting_event_shapes() {
        let _serial = crate::test_helpers::serial();
        for (name, subject, detail) in [
            ("legacy scalar", HELDOUT_SETTING, "/private/canary"),
            ("quoted scalar", HELDOUT_SETTING, r#""/private/canary""#),
            (
                "named payload",
                "",
                r#"{"name":"evolve.heldout_dir","value":"/private/canary","old_value":"/private/older"}"#,
            ),
            (
                "keyed payload",
                "",
                r#"{"key":"evolve.heldout_dir","value":"/private/canary"}"#,
            ),
            (
                "nested payload",
                HELDOUT_SETTING,
                r#"{"previous":{"path":"/private/canary"}}"#,
            ),
        ] {
            let event = crate::additions::addition_bus_event(pulse_bus::Event {
                id: 1,
                action: "setting".into(),
                subject: subject.into(),
                detail: detail.into(),
                ..pulse_bus::Event::default()
            });
            assert_eq!(
                event.detail, PRIVATE_VALUE,
                "private event detail is not masked for {name}: {event:#?}"
            );
        }
        let public = crate::additions::addition_bus_event(pulse_bus::Event {
            id: 2,
            action: "setting".into(),
            subject: "workers".into(),
            detail: "3".into(),
            ..pulse_bus::Event::default()
        });
        assert_eq!(
            public.detail,
            json!(3.0),
            "public setting event changed: {public:#?}"
        );
        let cleared = crate::additions::addition_bus_event(pulse_bus::Event {
            id: 3,
            action: "setting".into(),
            subject: HELDOUT_SETTING.into(),
            detail: "".into(),
            ..pulse_bus::Event::default()
        });
        assert_eq!(
            cleared.detail, "",
            "cleared directory was reported as configured: {cleared:#?}"
        );
    }
}
