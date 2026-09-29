use anyhow::{Result, anyhow};
use serde_json::Value;

pub fn post_for_owner(extra: &str) -> Result<bool> {
    let v: Value = serde_json::from_str(extra)?;
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
            _ => Err(anyhow!("for_owner must be true or false")),
        },
        _ => Err(anyhow!("for_owner must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_post_for_owner() {
        assert!(!post_for_owner(r#"{}"#).unwrap());
        assert!(post_for_owner(r#"{"for_owner":" TRUE "}"#).unwrap());
        assert!(post_for_owner(r#"{"for_owner":1}"#).is_err());
        assert!(post_for_owner(r#"[]"#).is_err());
    }
}
