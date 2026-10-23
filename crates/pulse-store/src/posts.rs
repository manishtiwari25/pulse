use anyhow::{Result, anyhow};
use serde_json::Value;

pub fn post_for_owner(extra: &str) -> Result<bool> {
    let v: Value = serde_json::from_str(extra).map_err(|e| anyhow!("post extra: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("post extra must be a JSON object"))?;
    let Some(flag) = obj.get("for_owner") else {
        return Ok(false);
    };
    match flag {
        Value::Bool(b) => Ok(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(anyhow!("post extra: for_owner must be true or false")),
        },
        _ => Err(anyhow!("post extra: for_owner must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn post_for_owner() {
        let _serial = crate::test_helpers::serial();
        for (name, extra, want, want_error) in [
            ("missing", r#"{}"#, false, false),
            (
                "unrelated metadata",
                r#"{"mentions":["human"],"ticket":"owner-review"}"#,
                false,
                false,
            ),
            ("true", r#"{"for_owner":true}"#, true, false),
            ("false", r#"{"for_owner":false}"#, false, false),
            ("frontmatter true", r#"{"for_owner":"true"}"#, true, false),
            (
                "frontmatter false",
                r#"{"for_owner":"false"}"#,
                false,
                false,
            ),
            ("normalized true", r#"{"for_owner":" TRUE "}"#, true, false),
            (
                "normalized false",
                r#"{"for_owner":" FALSE "}"#,
                false,
                false,
            ),
            ("number", r#"{"for_owner":1}"#, false, true),
            ("zero", r#"{"for_owner":0}"#, false, true),
            ("null flag", r#"{"for_owner":null}"#, false, true),
            ("object", r#"{"for_owner":{}}"#, false, true),
            ("array flag", r#"{"for_owner":[]}"#, false, true),
            ("invalid text", r#"{"for_owner":"yes"}"#, false, true),
            ("empty text", r#"{"for_owner":""}"#, false, true),
            ("invalid json", r#"{"for_owner":"#, false, true),
            ("array metadata", r#"[]"#, false, true),
        ] {
            let res = super::post_for_owner(extra);
            let got = *res.as_ref().unwrap_or(&false);
            assert!(
                res.is_err() == want_error && got == want,
                "{name}: PostForOwner({extra}) = {got}, {res:?}; want {want}, error={want_error}"
            );
        }
    }
}
