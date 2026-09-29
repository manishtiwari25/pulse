use crate::bus::Event;
use std::collections::BTreeSet;

/// PrivateValue replaces an owner-private value in anything agents read.
pub const PRIVATE_VALUE: &str = "[owner-private]";
pub const PRIVATE_SETTINGS: &[&str] = &["evolve.heldout_dir"];

/// Redact masks private setting events for agents.
pub fn redact(mut e: Event) -> Event {
    if e.action == "setting"
        && PRIVATE_SETTINGS.contains(&e.subject.as_str())
        && !e.detail.is_empty()
    {
        e.detail = PRIVATE_VALUE.to_string();
    }
    e
}

/// Redactor returns Redact extended with the store's current and historical private values.
pub fn redactor(s: Option<&pulse_store::Store>) -> impl Fn(Event) -> Event {
    let needles = private_needles(s);
    move |mut e| {
        e = redact(e);
        for n in &needles {
            e.subject = e.subject.replace(n, PRIVATE_VALUE);
            e.detail = e.detail.replace(n, PRIVATE_VALUE);
        }
        e
    }
}

pub fn private_needles(s: Option<&pulse_store::Store>) -> Vec<String> {
    let Some(s) = s else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut seen = BTreeSet::new();
    for name in PRIVATE_SETTINGS {
        add_value(&mut values, &mut seen, &s.setting(name, ""));
    }
    if let Ok(conn) = s.conn() {
        let placeholders = std::iter::repeat_n("?", PRIVATE_SETTINGS.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT detail FROM events WHERE action = 'setting' AND subject IN ({placeholders})"
        );
        if let Ok(mut stmt) = conn.prepare(&sql)
            && let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(PRIVATE_SETTINGS), |r| {
                r.get::<_, String>(0)
            })
        {
            for v in rows.flatten() {
                add_value(&mut values, &mut seen, &v);
            }
        }
    }
    let mut needles = Vec::new();
    let mut enc_seen = BTreeSet::new();
    for v in values {
        for enc in encodings(&v) {
            if enc_seen.insert(enc.clone()) {
                needles.push(enc);
            }
        }
    }
    needles.sort_by_key(|b| std::cmp::Reverse(b.len()));
    needles
}

fn add_value(values: &mut Vec<String>, seen: &mut BTreeSet<String>, v: &str) {
    let v = v.trim();
    if v.len() >= 4 && seen.insert(v.to_string()) {
        values.push(v.to_string());
    }
}

fn encodings(v: &str) -> Vec<String> {
    let first = vec![
        v.to_string(),
        py_encoded(v),
        go_encoded(v, true),
        go_encoded(v, false),
    ];
    let mut out = first.clone();
    for f in first.iter().skip(1) {
        out.push(py_encoded(f));
        out.push(go_encoded(f, true));
        out.push(go_encoded(f, false));
    }
    out
}

fn py_encoded(v: &str) -> String {
    json_string_body(v, true, false)
}

fn go_encoded(v: &str, html: bool) -> String {
    json_string_body(v, false, html)
}

fn json_string_body(v: &str, ascii: bool, html: bool) -> String {
    let mut out = String::new();
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '<' if html => out.push_str("\\u003c"),
            '>' if html => out.push_str("\\u003e"),
            '&' if html => out.push_str("\\u0026"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ascii && !c.is_ascii() => {
                let mut buf = [0_u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::since;
    use serde_json::json;
    use std::path::Path;
    use tempfile::{TempDir, tempdir};

    struct TestStore {
        _dir: TempDir,
        store: pulse_store::Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    fn temp_store() -> TestStore {
        let dir = tempdir().unwrap();
        pulse_store::Store::create(Path::new(dir.path()).join(pulse_store::STORE_NAME)).unwrap();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            now: "2026-09-06T21:00:00Z".to_string(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        TestStore { _dir: dir, store }
    }

    #[test]
    fn redact_masks_private_settings() {
        let _serial = crate::test_helpers::serial();
        let _guard = crate::test_global_guard();
        let s = temp_store();
        s.event(
            pulse_store::HUMAN_AGENT,
            "setting",
            "evolve.heldout_dir",
            "/Users/owner/private-fixtures",
        )
        .unwrap();
        let redact = redactor(Some(&s));
        let e = redact(Event {
            id: 1,
            action: "setting".to_string(),
            subject: "evolve.heldout_dir".to_string(),
            detail: "/Users/owner/private-fixtures".to_string(),
            ..Event::default()
        });
        assert_eq!(e.detail, PRIVATE_VALUE);
        let e = redact(Event {
            id: 2,
            action: "setting".to_string(),
            subject: "workers".to_string(),
            detail: "2".to_string(),
            ..Event::default()
        });
        assert_eq!(e.detail, "2");
        let e = redact(Event {
            id: 3,
            action: "evolve.cycle".to_string(),
            subject: "cycle-1".to_string(),
            detail: r#"{"fixtures": "/Users/owner/private-fixtures/run.json"}"#.to_string(),
            ..Event::default()
        });
        assert!(!e.detail.contains("owner/private"));
        assert!(e.detail.contains(PRIVATE_VALUE));
        let e = redactor(None)(Event {
            id: 4,
            action: "setting".to_string(),
            subject: "evolve.heldout_dir".to_string(),
            detail: "x".to_string(),
            ..Event::default()
        });
        assert_eq!(e.detail, PRIVATE_VALUE);
        let e = redact(Event {
            id: 5,
            action: "setting".to_string(),
            subject: "evolve.heldout_dir".to_string(),
            detail: String::new(),
            ..Event::default()
        });
        assert_eq!(e.detail, "");
    }

    #[test]
    fn redactor_scrubs_encoded_and_historical_values() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "\",\"b\":\"",
            "/private/\\u263a-\\\"quoted\\\"-heldout-canary",
            "\\u263a",
            "masked detail is not JSON:",
            "quoted",
            "{\"a\":\"",
        );
        let _guard = crate::test_global_guard();
        let s = temp_store();
        let canary = r#"/private/☺-"quoted"-heldout-canary"#;
        let old = r#"/private/<old>&former-heldout"#;
        for v in [old, canary] {
            s.event(pulse_store::HUMAN_AGENT, "setting", "evolve.heldout_dir", v)
                .unwrap();
        }
        s.event(
            "demo-lead",
            "post.created",
            "p1",
            pulse_store::Detail::json(json!({"note": format!("Location: {canary}"), "old": old}))
                .unwrap(),
        )
        .unwrap();
        s.event(
            pulse_store::HUMAN_AGENT,
            "setting",
            "evolve.heldout_dir",
            "",
        )
        .unwrap();
        let events = since(
            &s,
            0,
            crate::Filter {
                actions: vec!["post.created".to_string()],
                ..crate::Filter::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        let got = redactor(Some(&s))(events[0].clone());
        let doc: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&got.detail).unwrap();
        assert_eq!(doc["note"], format!("Location: {PRIVATE_VALUE}"));
        assert_eq!(doc["old"], PRIVATE_VALUE);
        let go_enc = go_encoded(canary, true);
        let e = redactor(Some(&s))(Event {
            id: 9,
            action: "x".to_string(),
            detail: format!(
                r#"{{"a":"{}","b":"{}"}}"#,
                go_enc,
                go_encoded(&go_enc, false)
            ),
            ..Event::default()
        });
        assert!(!e.detail.contains("heldout-canary"));
        let _: serde_json::Value = serde_json::from_str(&e.detail).unwrap();
    }
}
