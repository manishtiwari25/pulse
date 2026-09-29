use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Go `PyJSON`: encoding/json's output (struct fields in declaration order,
/// map keys sorted) re-spaced the way Python's `json.dumps` prints it. The
/// compact text is re-read token by token so a struct's field order survives
/// (`serde_json::Value` objects would sort it).
pub fn py_json<T: Serialize>(value: T) -> Result<String> {
    py_json_str(&crate::gojson::to_string_no_html(&value)?)
}

/// Go `PyJSON`'s token loop over compact JSON text.
pub(crate) fn py_json_str(compact: &str) -> Result<String> {
    // stack of container states: 'k' object expecting key, 'v' object expecting value, 'a' array
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut out = String::with_capacity(compact.len() + compact.len() / 4);
    let bytes = compact.as_bytes();
    let mut i = 0;
    fn before(out: &mut String, stack: &mut [(u8, usize)]) {
        let Some(f) = stack.last_mut() else {
            return;
        };
        match f.0 {
            b'a' => {
                if f.1 > 0 {
                    out.push_str(", ");
                }
                f.1 += 1;
            }
            b'k' => {
                if f.1 > 0 {
                    out.push_str(", ");
                }
                f.1 += 1;
                f.0 = b'v';
            }
            b'v' => {
                out.push_str(": ");
                f.0 = b'k';
            }
            _ => {}
        }
    }
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' => i += 1,
            b'{' => {
                before(&mut out, &mut stack);
                out.push('{');
                stack.push((b'k', 0));
                i += 1;
            }
            b'[' => {
                before(&mut out, &mut stack);
                out.push('[');
                stack.push((b'a', 0));
                i += 1;
            }
            b'}' | b']' => {
                out.push(c as char);
                stack.pop();
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                let literal = compact
                    .get(start..i)
                    .ok_or_else(|| anyhow!("unterminated JSON string"))?;
                let text: String = serde_json::from_str(literal)?;
                before(&mut out, &mut stack);
                write_string(&mut out, &text);
            }
            _ => {
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b',' | b':' | b'}' | b']' | b' ') {
                    i += 1;
                }
                let literal = &compact[start..i];
                match literal {
                    "true" | "false" | "null" => {}
                    _ if literal.parse::<f64>().is_ok() => {}
                    _ => return Err(anyhow!("unexpected JSON token {literal:?}")),
                }
                before(&mut out, &mut stack);
                out.push_str(literal);
            }
        }
    }
    Ok(out)
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) >= 0x80 && (c as u32) <= 0xffff => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c if (c as u32) > 0xffff => {
                let n = c as u32 - 0x1_0000;
                let hi = 0xd800 + (n >> 10);
                let lo = 0xdc00 + (n & 0x3ff);
                out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub(crate) fn py_json_value(value: &Value) -> Result<String> {
    fn write_string(out: &mut String, s: &str) {
        out.push('"');
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{08}' => out.push_str("\\b"),
                '\u{0c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c if (c as u32) >= 0x80 && (c as u32) <= 0xffff => {
                    out.push_str(&format!("\\u{:04x}", c as u32))
                }
                c if (c as u32) > 0xffff => {
                    let n = c as u32 - 0x1_0000;
                    let hi = 0xd800 + (n >> 10);
                    let lo = 0xdc00 + (n & 0x3ff);
                    out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn write(out: &mut String, v: &Value) -> Result<()> {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => write_string(out, s),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(out, x)?;
                }
                out.push(']');
            }
            Value::Object(m) => {
                out.push('{');
                let ordered: BTreeMap<_, _> = m.iter().collect();
                for (i, (k, v)) in ordered.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_string(out, k);
                    out.push_str(": ");
                    write(out, v)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    write(&mut out, value).map_err(|e| anyhow!(e))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn py_json_keeps_struct_field_order_and_sorts_maps() {
        #[derive(Serialize)]
        struct Inner {
            id: String,
            action: String,
            rev: i64,
        }
        #[derive(Serialize)]
        struct Outer {
            project: String,
            constitution: Inner,
            tags: BTreeMap<String, String>,
            flags: Vec<bool>,
            none: Option<String>,
        }
        let v = Outer {
            project: "demo".into(),
            constitution: Inner {
                id: "EVO-c".into(),
                action: "copied".into(),
                rev: 1,
            },
            tags: BTreeMap::from([
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
            ]),
            flags: vec![true, false],
            none: None,
        };
        assert_eq!(
            py_json(&v).unwrap(),
            r#"{"project": "demo", "constitution": {"id": "EVO-c", "action": "copied", "rev": 1}, "tags": {"a": "1", "b": "2"}, "flags": [true, false], "none": null}"#
        );
        assert_eq!(
            py_json_str(r#"{"s":"a,b:\"c\" {}[]","e":[],"o":{}}"#).unwrap(),
            r#"{"s": "a,b:\"c\" {}[]", "e": [], "o": {}}"#
        );
        assert_eq!(py_json_str("-1.5e3").unwrap(), "-1.5e3");
    }

    #[test]
    fn py_json_sorts_spaces_and_escapes_non_ascii() {
        let v = serde_json::json!({"b": [1, true], "a": "é", "emoji":"😀"});
        assert_eq!(
            py_json(v).unwrap(),
            r#"{"a": "\u00e9", "b": [1, true], "emoji": "\ud83d\ude00"}"#
        );
    }
}
