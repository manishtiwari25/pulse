//! Push credential references and ephemeral key material.

use anyhow::{Result, anyhow};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const SHAPES: &str = "op://VAULT/ITEM/FIELD[?ssh-format=openssh], keychain://SERVICE/ACCOUNT, or file:///ABSOLUTE/PATH";
pub const TOKEN_ENV: &str = "OP_SERVICE_ACCOUNT_TOKEN";
pub const KEYCHAIN_SERVICE: &str = "pulse";
const TOKEN_FILE_NAME: &str = "op-service-account-token";

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

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

pub fn parse(text: &str) -> Result<Ref> {
    let raw = text.trim();
    if raw.is_empty() {
        anyhow::bail!("empty push credential reference");
    }
    if raw.contains(['\n', '\r', '\t']) {
        anyhow::bail!("a reference is one line");
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        anyhow::bail!("{raw:?} is not a reference; use {SHAPES}");
    };
    let mut r = Ref {
        raw: raw.to_string(),
        scheme: scheme.to_ascii_lowercase(),
        ..Ref::default()
    };
    match r.scheme.as_str() {
        "op" => {
            let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
            let parts: Vec<_> = path.split('/').collect();
            if parts.len() < 3
                || parts[0].is_empty()
                || parts[1].is_empty()
                || parts.last().is_some_and(|s| s.is_empty())
            {
                anyhow::bail!("{raw:?} needs a vault, an item and a field: op://VAULT/ITEM/FIELD");
            }
            r.vault = parts[0].into();
            r.item = parts[1].into();
            r.field = parts[2..].join("/");
            r.query = query.into();
        }
        "keychain" => {
            let Some((service, account)) = rest.split_once('/') else {
                anyhow::bail!("{raw:?} needs a service and an account: keychain://SERVICE/ACCOUNT");
            };
            if service.is_empty() || account.is_empty() || account.contains('/') {
                anyhow::bail!("{raw:?} needs a service and an account: keychain://SERVICE/ACCOUNT");
            }
            r.service = service.into();
            r.account = account.into();
        }
        "file" => {
            if !Path::new(rest).is_absolute() {
                anyhow::bail!("{raw:?} needs an absolute path: file:///ABSOLUTE/PATH");
            }
            r.path = Path::new(rest).display().to_string();
        }
        _ => anyhow::bail!("{raw:?} has an unknown scheme {scheme:?}; use {SHAPES}"),
    }
    Ok(r)
}

#[derive(Debug, Clone)]
pub struct Secret {
    pub key: Vec<u8>,
    pub source: String,
}

#[derive(Debug, thiserror::Error)]
#[error("push credentials are never resolved inside a run (ADR-041 D4)")]
pub struct ErrWorker;

#[derive(Clone)]
pub struct Resolver {
    pub home: String,
    pub goos: String,
    pub timeout: Duration,
    pub worker: bool,
}

pub fn new(home: &str) -> Resolver {
    Resolver {
        home: home.to_string(),
        goos: String::new(),
        timeout: Duration::from_secs(30),
        worker: false,
    }
}

impl Resolver {
    pub fn os(&self) -> String {
        if self.goos.is_empty() {
            std::env::consts::OS.to_string()
        } else {
            self.goos.clone()
        }
    }

    pub fn resolve(&self, reference: Ref) -> Result<Secret> {
        if self.worker || std::env::var("PULSE_JOB").is_ok_and(|v| !v.is_empty()) {
            return Err(ErrWorker.into());
        }
        let (mut key, source) = match reference.scheme.as_str() {
            "file" => {
                let meta = fs::symlink_metadata(&reference.path)?;
                if !meta.file_type().is_file() {
                    anyhow::bail!(
                        "{} is not a regular file (symlinks are refused)",
                        reference.path
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        anyhow::bail!(
                            "{} is readable by others (mode {:04o}); chmod 600 it",
                            reference.path,
                            meta.permissions().mode() & 0o777
                        );
                    }
                }
                (fs::read(&reference.path)?, "file".to_string())
            }
            "op" => {
                let (out, src) = self.op(&["read", "-n", &reference.raw])?;
                (out.into_bytes(), format!("1Password (token from {src})"))
            }
            "keychain" => {
                anyhow::bail!("keychain:// references need macOS; use file:// or op:// here")
            }
            s => anyhow::bail!("unknown scheme {s:?}"),
        };
        while key.last().is_some_and(u8::is_ascii_whitespace) {
            key.pop();
        }
        if key.is_empty() {
            anyhow::bail!("{} resolved to nothing", reference.raw);
        }
        if !is_private_key(&key) {
            let hint = if reference.scheme == "op" && reference.query.is_empty() {
                "; for an SSH Key item add ?ssh-format=openssh to the reference"
            } else {
                ""
            };
            anyhow::bail!(
                "{} is not an SSH private key (it does not start with -----BEGIN ... PRIVATE KEY-----){}",
                reference.raw,
                hint
            );
        }
        key.push(b'\n');
        Ok(Secret { key, source })
    }

    fn token_file(&self) -> PathBuf {
        Path::new(&self.home).join("secrets").join(TOKEN_FILE_NAME)
    }

    pub fn token(&self) -> Result<(String, String)> {
        if let Ok(t) = std::env::var(TOKEN_ENV)
            && !t.trim().is_empty()
        {
            return Ok((t.trim().to_string(), "the environment".into()));
        }
        if let Ok(b) = fs::read_to_string(self.token_file())
            && !b.trim().is_empty()
        {
            return Ok((b.trim().to_string(), "the token file".into()));
        }
        anyhow::bail!("no service-account token: pulse cred token")
    }

    pub fn store_token(&self, token: &str) -> Result<String> {
        static TOKEN_SHAPE: once_cell::sync::Lazy<Regex> =
            once_cell::sync::Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.=-]{20,}$").unwrap());
        let token = token.trim();
        if !TOKEN_SHAPE.is_match(token) {
            anyhow::bail!(
                "that does not look like a 1Password service-account token (one line, letters, digits, _ . = -)"
            );
        }
        let path = self.token_file();
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, format!("{token}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(path.display().to_string())
    }

    pub fn clear_token(&self) -> Result<Vec<String>> {
        let path = self.token_file();
        match fs::remove_file(&path) {
            Ok(()) => Ok(vec![path.display().to_string()]),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn op(&self, args: &[&str]) -> Result<(String, String)> {
        let (token, source) = self.token()?;
        let out = Command::new("op")
            .args(args)
            .env(TOKEN_ENV, &token)
            .output()
            .map_err(|_| {
                anyhow!("op not found; install 1Password CLI (brew install 1password-cli)")
            })?;
        if !out.status.success() {
            anyhow::bail!(
                "op {} failed: {}",
                args.first().copied().unwrap_or(""),
                redact(&String::from_utf8_lossy(&out.stderr), &[&token])
            );
        }
        Ok((String::from_utf8_lossy(&out.stdout).to_string(), source))
    }

    pub fn materialize(&self, secret: &Secret) -> Result<Material> {
        let base = Path::new(&self.home).join("tmp");
        fs::create_dir_all(&base)?;
        let dir = tempfile::Builder::new().prefix("push-").tempdir_in(&base)?;
        let dir_path = dir.keep();
        let key_path = dir_path.join("key");
        fs::write(&key_path, &secret.key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir_path, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Material {
            dir: dir_path.display().to_string(),
            key_path: key_path.display().to_string(),
            ssh_command: ssh_command(&key_path.display().to_string()),
        })
    }
}

pub fn is_private_key(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(b);
    let head = head.trim();
    head.starts_with("-----BEGIN ") && head.contains("PRIVATE KEY-----")
}

pub fn redact(text: &str, secrets: &[&str]) -> String {
    static PEM: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(-----END [A-Z ]*PRIVATE KEY-----|\z)",
        )
        .unwrap()
    });
    static TOKEN: once_cell::sync::Lazy<Regex> =
        once_cell::sync::Lazy::new(|| Regex::new(r"\bops_[A-Za-z0-9_.=-]{16,}").unwrap());
    let mut out = PEM.replace_all(text, "[private key]").to_string();
    out = TOKEN.replace_all(&out, "[token]").to_string();
    for s in secrets {
        let s = s.trim();
        if !s.is_empty() {
            out = out.replace(s, "[redacted]");
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Material {
    pub dir: String,
    pub key_path: String,
    pub ssh_command: String,
}

pub fn ssh_command(key_path: &str) -> String {
    format!(
        "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '{}'",
        key_path.replace('\'', r#"'\''"#)
    )
}

impl Material {
    pub fn cleanup(&self) {
        if !self.key_path.is_empty()
            && let Ok(info) = fs::metadata(&self.key_path)
        {
            let _ = fs::write(&self.key_path, vec![0; info.len() as usize]);
        }
        if !self.dir.is_empty() {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}
