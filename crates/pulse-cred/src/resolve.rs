use crate::reference::Ref;
use anyhow::{Result, anyhow, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

pub const TOKEN_ENV: &str = "OP_SERVICE_ACCOUNT_TOKEN";
pub const KEYCHAIN_SERVICE: &str = "pulse";
const TOKEN_FILE_NAME: &str = "op-service-account-token";

pub fn token_file_name() -> &'static str {
    TOKEN_FILE_NAME
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RunError {
    pub message: String,
}

impl RunError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub type Runner = Arc<
    dyn Fn(&str, &[String], &str, Option<&[String]>) -> Result<(String, String), RunError>
        + Send
        + Sync,
>;

pub type Getenv = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Resolver turns a Ref into an SSH private key.
#[derive(Clone)]
pub struct Resolver {
    pub home: String,
    pub run: Option<Runner>,
    pub getenv: Option<Getenv>,
    pub goos: String,
    pub timeout: Duration,
    pub worker: bool,
}

impl Default for Resolver {
    fn default() -> Self {
        Self {
            home: String::new(),
            run: None,
            getenv: None,
            goos: String::new(),
            timeout: Duration::ZERO,
            worker: false,
        }
    }
}

/// New returns a Resolver with the real runner and environment.
pub fn new(home: &str) -> Resolver {
    Resolver {
        home: home.to_string(),
        ..Resolver::default()
    }
}

impl Resolver {
    fn run(&self) -> Runner {
        self.run.clone().unwrap_or_else(|| Arc::new(exec))
    }

    fn getenv(&self, k: &str) -> String {
        self.getenv
            .as_ref()
            .map(|f| f(k))
            .unwrap_or_else(|| std::env::var(k).unwrap_or_default())
    }

    fn goos(&self) -> String {
        if self.goos.is_empty() {
            pulse_gitenv::GOOS.to_string()
        } else {
            self.goos.clone()
        }
    }

    pub fn os(&self) -> String {
        self.goos()
    }

    /// Resolve reads the key the reference names. Errors never carry the secret.
    pub fn resolve(&self, reference: Ref) -> Result<Secret> {
        if self.worker || !self.getenv("PULSE_JOB").is_empty() {
            return Err(ErrWorker.into());
        }
        let (mut key, source) = match reference.scheme.as_str() {
            "op" => {
                let (out, token_source) = self.op(&["read", "-n", &reference.raw])?;
                (
                    out.into_bytes(),
                    format!("1Password (token from {token_source})"),
                )
            }
            "keychain" => {
                if self.goos() != "darwin" {
                    bail!("keychain:// references need macOS; use file:// or op:// here");
                }
                let args = vec![
                    "find-generic-password".to_string(),
                    "-s".to_string(),
                    reference.service.clone(),
                    "-a".to_string(),
                    reference.account.clone(),
                    "-w".to_string(),
                ];
                let (out, stderr) = self.run()("security", &args, "", None).map_err(|e| {
                    anyhow!(
                        "keychain item {}/{}: {}",
                        reference.service,
                        reference.account,
                        redact(&first_line("", Some(&e)), &[])
                    )
                })?;
                let _ = stderr;
                (keychain_bytes(&out), "keychain".to_string())
            }
            "file" => {
                let meta = fs::symlink_metadata(&reference.path)
                    .map_err(|e| anyhow!("{}: {}", reference.path, e))?;
                if !meta.file_type().is_file() {
                    bail!(
                        "{} is not a regular file (symlinks are refused)",
                        reference.path
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        bail!(
                            "{} is readable by others (mode {:04o}); chmod 600 it",
                            reference.path,
                            meta.permissions().mode() & 0o777
                        );
                    }
                }
                (
                    fs::read(&reference.path).map_err(|e| anyhow!("{}: {}", reference.path, e))?,
                    "file".to_string(),
                )
            }
            s => bail!("unknown scheme {s:?}"),
        };
        while key.last().is_some_and(u8::is_ascii_whitespace) {
            key.pop();
        }
        if key.is_empty() {
            bail!("{} resolved to nothing", reference.raw);
        }
        if !is_private_key(&key) {
            let hint = if reference.scheme == "op" && reference.query.is_empty() {
                "; for an SSH Key item add ?ssh-format=openssh to the reference"
            } else {
                ""
            };
            bail!(
                "{} is not an SSH private key (it does not start with -----BEGIN ... PRIVATE KEY-----){}",
                reference.raw,
                hint
            );
        }
        key.push(b'\n');
        Ok(Secret { key, source })
    }

    pub fn op_available(&self) -> bool {
        if self.run.is_some() {
            return true;
        }
        path_lookup("op").is_some()
    }

    pub fn op(&self, args: &[&str]) -> Result<(String, String)> {
        let (token, token_source) = self.token()?;
        if !self.op_available() {
            bail!("op not found; install 1Password CLI (brew install 1password-cli)");
        }
        let args = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let env = op_env(
            std::env::vars().map(|(k, v)| format!("{k}={v}")).collect(),
            &token,
        );
        match self.run()("op", &args, "", Some(&env)) {
            Ok((out, _)) => Ok((out, token_source)),
            Err(e) => {
                let verb = args
                    .first()
                    .map(|a| format!("op {a}"))
                    .unwrap_or_else(|| "op".to_string());
                bail!(
                    "{} failed: {}",
                    verb,
                    redact(&first_line("", Some(&e)), &[&token])
                );
            }
        }
    }

    pub(crate) fn token_file(&self) -> PathBuf {
        Path::new(&self.home).join("secrets").join(TOKEN_FILE_NAME)
    }

    /// Token finds the 1Password service-account token.
    pub fn token(&self) -> Result<(String, String)> {
        let env = self.getenv(TOKEN_ENV).trim().to_string();
        if !env.is_empty() {
            return Ok((env, "the environment".to_string()));
        }
        if self.goos() == "darwin" {
            let args = vec![
                "find-generic-password".to_string(),
                "-s".to_string(),
                KEYCHAIN_SERVICE.to_string(),
                "-a".to_string(),
                TOKEN_ENV.to_string(),
                "-w".to_string(),
            ];
            if let Ok((out, _)) = self.run()("security", &args, "", None) {
                let out = out.trim();
                if !out.is_empty() {
                    return Ok((out.to_string(), "the keychain".to_string()));
                }
            }
        }
        if let Ok(b) = fs::read_to_string(self.token_file()) {
            let b = b.trim();
            if !b.is_empty() {
                return Ok((b.to_string(), "the token file".to_string()));
            }
        }
        bail!("no service-account token: pulse cred token")
    }

    /// StoreToken keeps a service-account token argv-free.
    pub fn store_token(&self, token: &str) -> Result<String> {
        static TOKEN_SHAPE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.=-]{20,}$").unwrap());
        let token = token.trim();
        if !TOKEN_SHAPE.is_match(token) {
            bail!(
                "that does not look like a 1Password service-account token (one line, letters, digits, _ . = -)"
            );
        }
        if self.goos() == "darwin" {
            let cmd = format!(
                "add-generic-password -a {TOKEN_ENV} -s {KEYCHAIN_SERVICE} -U -w '{token}'\n"
            );
            let args = vec!["-i".to_string()];
            if let Err(e) = self.run()("security", &args, &cmd, None) {
                bail!(
                    "keychain write failed: {}",
                    redact(&first_line("", Some(&e)), &[token])
                );
            }
            return Ok(format!(
                "the keychain (service {KEYCHAIN_SERVICE}, account {TOKEN_ENV})"
            ));
        }
        let path = self.token_file();
        let dir = path.parent().expect("token file has parent");
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        fs::write(&path, format!("{token}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(path.display().to_string())
    }

    pub fn clear_token(&self) -> Result<Vec<String>> {
        let mut removed = Vec::new();
        if self.goos() == "darwin" {
            let args = vec![
                "delete-generic-password".to_string(),
                "-s".to_string(),
                KEYCHAIN_SERVICE.to_string(),
                "-a".to_string(),
                TOKEN_ENV.to_string(),
            ];
            if self.run()("security", &args, "", None).is_ok() {
                removed.push("the keychain item".to_string());
            }
        }
        match fs::remove_file(self.token_file()) {
            Ok(()) => removed.push(self.token_file().display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(removed)
    }

    /// Inspect derives the public key and fingerprint of a materialized key.
    pub fn inspect(&self, key_path: &str) -> Result<(String, String)> {
        let y_args = vec!["-y".to_string(), "-f".to_string(), key_path.to_string()];
        let (pubkey, _) = self.run()("ssh-keygen", &y_args, "", None).map_err(|e| {
            anyhow!(
                "ssh-keygen could not read the key: {}",
                redact(&first_line("", Some(&e)), &[])
            )
        })?;
        let l_args = vec!["-l".to_string(), "-f".to_string(), key_path.to_string()];
        let (fp, _) = self.run()("ssh-keygen", &l_args, "", None).map_err(|e| {
            anyhow!(
                "ssh-keygen could not fingerprint the key: {}",
                redact(&first_line("", Some(&e)), &[])
            )
        })?;
        let fingerprint = fp.split_whitespace().nth(1).unwrap_or("").to_string();
        Ok((pubkey.trim().to_string(), fingerprint))
    }
}

/// Exec is the Runner that starts real processes.
pub fn exec(
    name: &str,
    args: &[String],
    stdin: &str,
    env: Option<&[String]>,
) -> Result<(String, String), RunError> {
    let mut cmd = Command::new(name);
    cmd.args(args);
    if let Some(env) = env {
        cmd.env_clear();
        for kv in env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
    }
    if !stdin.is_empty() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| RunError::new(e.to_string()))?;
    if !stdin.is_empty()
        && let Some(mut pipe) = child.stdin.take()
    {
        pipe.write_all(stdin.as_bytes())
            .map_err(|e| RunError::new(e.to_string()))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| RunError::new(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(RunError::new(if stderr.trim().is_empty() {
            format!("exit status {}", out.status)
        } else {
            stderr.trim().to_string()
        }));
    }
    Ok((stdout, stderr))
}

fn path_lookup(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Secret is a resolved push credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    pub key: Vec<u8>,
    pub source: String,
}

#[derive(Debug, thiserror::Error)]
#[error("push credentials are never resolved inside a run (ADR-041 D4)")]
pub struct ErrWorker;

pub fn op_env(base: Vec<String>, token: &str) -> Vec<String> {
    let mut env = Vec::with_capacity(base.len() + 1);
    for kv in base {
        if kv.starts_with("OP_CONNECT_HOST=")
            || kv.starts_with("OP_CONNECT_TOKEN=")
            || kv.starts_with(&format!("{TOKEN_ENV}="))
        {
            continue;
        }
        env.push(kv);
    }
    env.push(format!("{TOKEN_ENV}={token}"));
    env
}

pub fn keychain_bytes(out: &str) -> Vec<u8> {
    let text = out.trim();
    if !text.is_empty()
        && text.len().is_multiple_of(2)
        && text
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        && let Ok(raw) = hex::decode(text)
        && is_private_key(&raw)
    {
        return raw;
    }
    text.as_bytes().to_vec()
}

pub fn is_private_key(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(b);
    let head = head.trim();
    head.starts_with("-----BEGIN ") && head.contains("PRIVATE KEY-----")
}

static PEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(-----END [A-Z ]*PRIVATE KEY-----|\z)")
        .unwrap()
});
static TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bops_[A-Za-z0-9_.=-]{16,}").unwrap());

/// Redact removes private key blocks, service-account tokens and explicit secrets.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut text = PEM.replace_all(text, "[private key]").to_string();
    text = TOKEN.replace_all(&text, "[token]").to_string();
    for s in secrets {
        let s = s.trim();
        if !s.is_empty() {
            text = text.replace(s, "[redacted]");
        }
    }
    text
}

pub fn first_line(stderr: &str, err: Option<&RunError>) -> String {
    for line in stderr.lines() {
        let line = line.trim();
        if !line.is_empty() {
            return line.to_string();
        }
    }
    err.map(ToString::to_string)
        .unwrap_or_else(|| "unknown error".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n";

    #[derive(Clone, Debug)]
    struct Call {
        name: String,
        args: Vec<String>,
    }

    type Reply = Box<
        dyn Fn(&[String], &str, Option<&[String]>) -> Result<(String, String), RunError>
            + Send
            + Sync,
    >;

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<Call>>>,
        reply: Arc<BTreeMap<String, Reply>>,
    }

    impl Fake {
        fn runner(&self) -> Runner {
            let calls = Arc::clone(&self.calls);
            let reply = Arc::clone(&self.reply);
            Arc::new(move |name, args, stdin, env| {
                calls.lock().unwrap().push(Call {
                    name: name.to_string(),
                    args: args.to_vec(),
                });
                reply.get(name).map_or_else(
                    || Err(RunError::new(format!("not faked: {name}"))),
                    |f| f(args, stdin, env),
                )
            })
        }
    }

    fn env_value(env: &[String], key: &str) -> Option<String> {
        env.iter()
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(ToString::to_string))
    }

    fn resolver(fake: &Fake, goos: &str, getenv: BTreeMap<String, String>) -> Resolver {
        Resolver {
            home: tempdir().unwrap().keep().display().to_string(),
            run: Some(fake.runner()),
            goos: goos.to_string(),
            getenv: Some(Arc::new(move |k| {
                getenv.get(k).cloned().unwrap_or_default()
            })),
            ..Resolver::default()
        }
    }

    fn fake_with(reply: BTreeMap<String, Reply>) -> Fake {
        Fake {
            calls: Arc::default(),
            reply: Arc::new(reply),
        }
    }

    #[test]
    fn resolve_op_uses_service_account_token() {
        let _guard = pulse_testutil::env_lock();
        // Like Go's t.Setenv: the resolver must strip these before running op.
        let saved: Vec<(&str, Option<String>)> = ["OP_CONNECT_HOST", "OP_CONNECT_TOKEN", TOKEN_ENV]
            .into_iter()
            .map(|k| (k, std::env::var(k).ok()))
            .collect();
        unsafe {
            std::env::set_var("OP_CONNECT_HOST", "https://connect.example");
            std::env::set_var("OP_CONNECT_TOKEN", "connect-secret");
            std::env::set_var(TOKEN_ENV, "from-process-env-should-be-replaced");
        }
        struct Restore(Vec<(&'static str, Option<String>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (k, v) in self.0.drain(..) {
                    unsafe {
                        match v {
                            Some(v) => std::env::set_var(k, v),
                            None => std::env::remove_var(k),
                        }
                    }
                }
            }
        }
        let _restore = Restore(saved);
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "security".to_string(),
            Box::new(|args, _, _| {
                if args.join(" ") == "find-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN -w"
                {
                    Ok((
                        "ops_abcdefghijklmnopqrstuvwxyz0123456789\n".to_string(),
                        String::new(),
                    ))
                } else {
                    Err(RunError::new("unexpected security call"))
                }
            }),
        );
        replies.insert(
            "op".to_string(),
            Box::new(|args, _, env| {
                if args.join(" ")
                    != "read -n op://PULSE/pulse push key/private key?ssh-format=openssh"
                {
                    return Err(RunError::new("bad args"));
                }
                let env = env.unwrap();
                if env_value(env, TOKEN_ENV).as_deref()
                    != Some("ops_abcdefghijklmnopqrstuvwxyz0123456789")
                {
                    return Err(RunError::new("token not in env"));
                }
                if env_value(env, "OP_CONNECT_HOST").is_some()
                    || env_value(env, "OP_CONNECT_TOKEN").is_some()
                {
                    return Err(RunError::new("connect host/token leaked"));
                }
                Ok((TEST_KEY.to_string(), String::new()))
            }),
        );
        let fake = fake_with(replies);
        let r = resolver(&fake, "darwin", BTreeMap::new());
        let sec = r
            .resolve(parse("op://PULSE/pulse push key/private key?ssh-format=openssh").unwrap())
            .unwrap();
        assert_eq!(String::from_utf8(sec.key).unwrap(), TEST_KEY);
        assert_eq!(sec.source, "1Password (token from the keychain)");
        for c in fake.calls.lock().unwrap().iter() {
            for a in &c.args {
                assert!(!a.contains("ops_"), "token passed on argv: {:?}", c.args);
            }
        }
    }

    #[test]
    fn resolve_op_without_token_says_how() {
        let fake = Fake::default();
        let r = resolver(&fake, "linux", BTreeMap::new());
        let err = r
            .resolve(parse("op://V/I/F").unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("pulse cred token"));
        assert!(!fake.calls.lock().unwrap().iter().any(|c| c.name == "op"));
    }

    #[test]
    fn resolve_op_token_from_file_and_error_redaction() {
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "op".to_string(),
            Box::new(|_, _, env| {
                let tok = env_value(env.unwrap(), TOKEN_ENV).unwrap();
                Err(RunError::new(format!(
                    "[ERROR] 2026/09/15 token {tok} rejected: {TEST_KEY}"
                )))
            }),
        );
        let fake = fake_with(replies);
        let r = resolver(&fake, "linux", BTreeMap::new());
        fs::create_dir_all(r.token_file().parent().unwrap()).unwrap();
        fs::write(r.token_file(), "ops_filetoken_0123456789abcdefghij\n").unwrap();
        let err = r
            .resolve(parse("op://V/I/F").unwrap())
            .unwrap_err()
            .to_string();
        assert!(!err.contains("ops_filetoken"));
        assert!(!err.contains("b3BlbnNzaC"));
        assert!(err.contains("op read failed"));
    }

    #[test]
    fn resolve_op_refuses_non_key_with_hint() {
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "op".to_string(),
            Box::new(|_, _, _| Ok(("ghp_not_a_key_at_all".to_string(), String::new()))),
        );
        let fake = fake_with(replies);
        let mut env = BTreeMap::new();
        env.insert(
            TOKEN_ENV.to_string(),
            "ops_envtoken_0123456789abcdefghij".to_string(),
        );
        let r = resolver(&fake, "linux", env);
        let err = r
            .resolve(parse("op://V/I/private key").unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an SSH private key"));
        assert!(err.contains("ssh-format=openssh"));
        assert!(!err.contains("ghp_"));
    }

    #[test]
    fn resolve_keychain_decodes_hex() {
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "security".to_string(),
            Box::new(|args, _, _| {
                if args.join(" ") != "find-generic-password -s pulse -a push-key -w" {
                    return Err(RunError::new("bad args"));
                }
                Ok((hex::encode(TEST_KEY), String::new()))
            }),
        );
        let fake = fake_with(replies);
        let r = resolver(&fake, "darwin", BTreeMap::new());
        let sec = r
            .resolve(parse("keychain://pulse/push-key").unwrap())
            .unwrap();
        assert_eq!(String::from_utf8(sec.key).unwrap(), TEST_KEY);
        let lin = resolver(&fake, "linux", BTreeMap::new());
        let err = lin
            .resolve(parse("keychain://pulse/push-key").unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("macOS"));
    }

    #[test]
    fn resolve_file() {
        let r = resolver(&Fake::default(), "linux", BTreeMap::new());
        let dir = tempdir().unwrap();
        let good = dir.path().join("key");
        fs::write(&good, TEST_KEY).unwrap();
        fs::set_permissions(&good, fs::Permissions::from_mode(0o600)).unwrap();
        let sec = r
            .resolve(parse(&format!("file://{}", good.display())).unwrap())
            .unwrap();
        assert_eq!(String::from_utf8(sec.key).unwrap(), TEST_KEY);
        assert_eq!(sec.source, "file");

        let loose = dir.path().join("loose");
        fs::write(&loose, TEST_KEY).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
        let err = r
            .resolve(parse(&format!("file://{}", loose.display())).unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("readable by others"));

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        let err = r
            .resolve(parse(&format!("file://{}", link.display())).unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("symlinks are refused"));

        let missing = dir.path().join("missing");
        let err = r
            .resolve(parse(&format!("file://{}", missing.display())).unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("No such file") || err.contains("no such file"));

        let not_key = dir.path().join("notkey");
        fs::write(&not_key, "hello\n").unwrap();
        fs::set_permissions(&not_key, fs::Permissions::from_mode(0o600)).unwrap();
        let err = r
            .resolve(parse(&format!("file://{}", not_key.display())).unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an SSH private key"));
    }

    #[test]
    fn resolve_refused_inside_worker() {
        let dir = tempdir().unwrap();
        let good = dir.path().join("key");
        fs::write(&good, TEST_KEY).unwrap();
        fs::set_permissions(&good, fs::Permissions::from_mode(0o600)).unwrap();
        let reference = parse(&format!("file://{}", good.display())).unwrap();
        let mut env = BTreeMap::new();
        env.insert("PULSE_JOB".to_string(), "job-x".to_string());
        let r = resolver(&Fake::default(), "linux", env);
        assert!(r.resolve(reference.clone()).unwrap_err().is::<ErrWorker>());
        let mut r = resolver(&Fake::default(), "linux", BTreeMap::new());
        r.worker = true;
        assert!(r.resolve(reference).unwrap_err().is::<ErrWorker>());
    }

    #[test]
    fn store_token() {
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "security".to_string(),
            Box::new(|args, stdin, _| {
                if args.join(" ") != "-i" {
                    return Err(RunError::new("token on argv"));
                }
                if stdin != "add-generic-password -a OP_SERVICE_ACCOUNT_TOKEN -s pulse -U -w 'ops_abcdefghijklmnopqrstuvwxyz0123456789'\n" {
                    return Err(RunError::new(format!("bad stdin: {stdin}")));
                }
                Ok((String::new(), String::new()))
            }),
        );
        let fake = fake_with(replies);
        let r = resolver(&fake, "darwin", BTreeMap::new());
        let where_ = r
            .store_token("  ops_abcdefghijklmnopqrstuvwxyz0123456789\n")
            .unwrap();
        assert!(where_.contains("keychain"));

        let lin = resolver(&Fake::default(), "linux", BTreeMap::new());
        let where_ = lin
            .store_token("ops_abcdefghijklmnopqrstuvwxyz0123456789")
            .unwrap();
        assert_eq!(
            fs::metadata(&where_).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (tok, src) = lin.token().unwrap();
        assert_eq!(tok, "ops_abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(src, "the token file");
        assert_eq!(lin.clear_token().unwrap().len(), 1);
        assert!(lin.token().is_err());
        for bad in [
            "",
            "short",
            "has space in it and is long enough",
            "quote'inside_and_long_enough_x",
            "two\nlines_long_enough_abcdefgh",
        ] {
            assert!(lin.store_token(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn token_prefers_environment() {
        let fake = Fake::default();
        let mut env = BTreeMap::new();
        env.insert(
            TOKEN_ENV.to_string(),
            "ops_envtoken_0123456789abcdefghij".to_string(),
        );
        let r = resolver(&fake, "darwin", env);
        let (tok, src) = r.token().unwrap();
        assert_eq!(tok, "ops_envtoken_0123456789abcdefghij");
        assert_eq!(src, "the environment");
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn inspect() {
        let mut replies: BTreeMap<String, Reply> = BTreeMap::new();
        replies.insert(
            "ssh-keygen".to_string(),
            Box::new(|args, _, _| match args[0].as_str() {
                "-y" => Ok((
                    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample pulse-push\n".to_string(),
                    String::new(),
                )),
                "-l" => Ok((
                    "256 SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG pulse-push (ED25519)\n"
                        .to_string(),
                    String::new(),
                )),
                _ => Err(RunError::new("bad args")),
            }),
        );
        let fake = fake_with(replies);
        let r = resolver(&fake, "linux", BTreeMap::new());
        let (pubkey, fp) = r.inspect("/tmp/key").unwrap();
        assert_eq!(
            pubkey,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample pulse-push"
        );
        assert_eq!(fp, "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG");
    }

    #[test]
    fn redact_cases() {
        let input = format!(
            "failed: {TEST_KEY} token ops_abcdefghijklmnopqrstuvwxyz0123456789 and plain-secret end"
        );
        let out = redact(&input, &["plain-secret"]);
        for leak in ["b3BlbnNzaC", "ops_abc", "plain-secret"] {
            assert!(!out.contains(leak), "{out}");
        }
        assert!(out.contains("[private key]"));
        assert!(out.contains("[token]"));
        assert!(out.contains("[redacted]"));
        assert!(out.ends_with("end"));
        assert_eq!(redact("clean text", &[]), "clean text");
        assert!(!redact("x -----BEGIN RSA PRIVATE KEY-----\nabc", &[]).contains("abc"));
    }

    #[test]
    fn is_private_key_cases() {
        for ok in [
            TEST_KEY,
            "-----BEGIN RSA PRIVATE KEY-----\nx\n-----END RSA PRIVATE KEY-----",
            "  -----BEGIN PRIVATE KEY-----\n",
            "-----BEGIN EC PRIVATE KEY-----",
        ] {
            assert!(is_private_key(ok.as_bytes()), "{ok}");
        }
        for no in [
            "",
            "ssh-ed25519 AAAA",
            "-----BEGIN CERTIFICATE-----",
            "ghp_token",
            "-----BEGIN PUBLIC KEY-----",
        ] {
            assert!(!is_private_key(no.as_bytes()), "{no}");
        }
    }
}
