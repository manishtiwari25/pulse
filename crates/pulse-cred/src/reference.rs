use anyhow::{Result, anyhow, bail};
use std::path::{Component, Path, PathBuf};

/// Ref is a parsed push credential reference.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ref {
    pub raw: String,
    pub scheme: String,
    pub vault: String,
    pub item: String,
    pub field: String,
    pub query: String,
    pub service: String,
    pub account: String,
    pub path: String,
}

/// Shapes lists the accepted reference shapes for errors and help.
pub const SHAPES: &str = "op://VAULT/ITEM/FIELD[?ssh-format=openssh], keychain://SERVICE/ACCOUNT, or file:///ABSOLUTE/PATH";

/// Validates a reference. An empty reference is an error.
pub fn parse(text: &str) -> Result<Ref> {
    let raw = text.trim();
    let mut r = Ref {
        raw: raw.to_string(),
        ..Ref::default()
    };
    if raw.is_empty() {
        bail!("empty push credential reference");
    }
    if raw.contains(['\n', '\r', '\t']) {
        bail!("a reference is one line");
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        bail!("{raw:?} is not a reference; use {SHAPES}");
    };
    r.scheme = scheme.to_ascii_lowercase();
    match r.scheme.as_str() {
        "op" => {
            let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
            let parts = path.split('/').collect::<Vec<_>>();
            if parts.len() < 3
                || parts[0].is_empty()
                || parts[1].is_empty()
                || parts.last().is_some_and(|s| s.is_empty())
            {
                bail!("{raw:?} needs a vault, an item and a field: op://VAULT/ITEM/FIELD");
            }
            if !query.is_empty()
                && let Err(e) = validate_query(query)
            {
                bail!("{raw:?} has a malformed query: {e}");
            }
            r.vault = parts[0].to_string();
            r.item = parts[1].to_string();
            r.field = parts[2..].join("/");
            r.query = query.to_string();
        }
        "keychain" => {
            let Some((service, account)) = rest.split_once('/') else {
                bail!("{raw:?} needs a service and an account: keychain://SERVICE/ACCOUNT");
            };
            if service.is_empty() || account.is_empty() || account.contains('/') {
                bail!("{raw:?} needs a service and an account: keychain://SERVICE/ACCOUNT");
            }
            r.service = service.to_string();
            r.account = account.to_string();
        }
        "file" => {
            if !Path::new(rest).is_absolute() {
                bail!("{raw:?} needs an absolute path: file:///ABSOLUTE/PATH");
            }
            r.path = clean_abs(rest).display().to_string();
        }
        _ => bail!("{raw:?} has an unknown scheme {scheme:?}; use {SHAPES}"),
    }
    Ok(r)
}

fn validate_query(query: &str) -> Result<()> {
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit()
            {
                return Err(anyhow!("invalid URL escape"));
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn clean_abs(p: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in Path::new(p).components() {
        match c {
            Component::RootDir | Component::Prefix(_) => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Check = Box<dyn Fn(Ref) -> bool>;
    type Case = (&'static str, &'static str, Check);

    #[test]
    fn parse() {
        let _serial = crate::test_helpers::serial();
        let cases: Vec<Case> = vec![
            (
                "op://PULSE/pulse push key/private key?ssh-format=openssh",
                "",
                Box::new(|r| {
                    r.scheme == "op"
                        && r.vault == "PULSE"
                        && r.item == "pulse push key"
                        && r.field == "private key"
                        && r.query == "ssh-format=openssh"
                }),
            ),
            (
                "op://V/I/section/field",
                "",
                Box::new(|r| r.field == "section/field"),
            ),
            ("OP://V/I/F", "", Box::new(|r| r.scheme == "op")),
            (
                "op://V/I",
                "needs a vault, an item and a field",
                Box::new(|_| false),
            ),
            (
                "op://V//F",
                "needs a vault, an item and a field",
                Box::new(|_| false),
            ),
            ("op://V/I/F?%zz", "malformed query", Box::new(|_| false)),
            (
                "keychain://pulse/push-key",
                "",
                Box::new(|r| r.service == "pulse" && r.account == "push-key"),
            ),
            (
                "keychain://pulse",
                "needs a service and an account",
                Box::new(|_| false),
            ),
            (
                "keychain://pulse/a/b",
                "needs a service and an account",
                Box::new(|_| false),
            ),
            (
                "file:///Users/me/.pulse/secrets/push_key",
                "",
                Box::new(|r| r.path == "/Users/me/.pulse/secrets/push_key"),
            ),
            (
                "file://relative/key",
                "needs an absolute path",
                Box::new(|_| false),
            ),
            ("file:///a/../b", "", Box::new(|r| r.path == "/b")),
            ("", "empty", Box::new(|_| false)),
            ("   ", "empty", Box::new(|_| false)),
            ("ssh-key-here", "is not a reference", Box::new(|_| false)),
            ("vault://x/y", "unknown scheme", Box::new(|_| false)),
            ("op://V/I/F\nfile:///x", "one line", Box::new(|_| false)),
        ];
        for (input, want_err, check) in cases {
            let got = super::parse(input);
            if !want_err.is_empty() {
                let err = got.expect_err("expected error").to_string();
                assert!(err.contains(want_err), "{input}: {err}");
                continue;
            }
            let r = got.unwrap();
            assert!(check(r.clone()), "{input}: {r:?}");
            if !input.starts_with("OP") {
                assert_eq!(r.to_string(), input.trim());
            }
        }
    }
}
