use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub fn py_json<T: Serialize>(value: T) -> Result<String> {
    py_json_value(&serde_json::to_value(value)?)
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
    fn py_json_sorts_spaces_and_escapes_non_ascii() {
        let v = serde_json::json!({"b": [1, true], "a": "é", "emoji":"😀"});
        assert_eq!(
            py_json(v).unwrap(),
            r#"{"a": "\u00e9", "b": [1, true], "emoji": "\ud83d\ude00"}"#
        );
    }
}
