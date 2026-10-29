//! Backups of the store itself, off this machine. `pulse backup` and the
//! daemon's maintenance pass take a consistent copy of `pulse.sqlite`
//! (`VACUUM INTO`, so the daemon's writers and the hub's readers are never
//! blocked), check it, gzip it, note its sha256 and deliver it where
//! `backup.to` points:
//!
//! - a directory (`/Volumes/Kingston/pulse`, an iCloud or Dropbox folder, a
//!   network share): the file is copied there. The directory must exist
//!   already - a drive that is not mounted is a refusal, never a folder made
//!   quietly on the boot disk under `/Volumes`;
//! - `az://ACCOUNT/CONTAINER[/PREFIX]`: Azure Blob Storage through the Azure
//!   CLI the owner is logged in to. The account key is read for the seconds
//!   of the upload (`az storage account keys list`) and handed over in the
//!   process environment, never written anywhere; when the key cannot be
//!   read, or the account refuses shared-key access, the upload goes with the
//!   login itself (`--auth-mode login`). `backup.subscription` names the
//!   subscription holding the account when it is not the CLI's default;
//! - `run: COMMAND {file} {name}`: anything else - rclone, `aws s3 cp`, scp,
//!   restic - run through `sh -c` with the two placeholders filled in.
//!
//! `backup.keep` copies of this machine stay at a directory or Azure
//! destination, older ones go. The last result lives in meta `backup`, every
//! attempt in the timeline (`backup.done`, `backup.failed`). A failed
//! attempt is retried after `RETRY_HOURS`, whatever the cadence says.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::util::{event_at, exec1, must_iso, plus_hours};

/// Where backups go: a directory, `az://ACCOUNT/CONTAINER[/PREFIX]`, or
/// `run: COMMAND {file} {name}`. Empty: no backups.
pub(crate) const TO: &str = "backup.to";
/// Hours between two backups the daemon takes; 0 leaves them to `pulse backup`.
pub(crate) const EVERY_HOURS: &str = "backup.every_hours";
/// Copies of this machine kept at the destination; 0 keeps every one.
pub(crate) const KEEP: &str = "backup.keep";
/// The Azure subscription holding the `az://` account when it is not the
/// CLI's default; passed as `--subscription` to every `az` call. Empty: the default.
pub(crate) const SUBSCRIPTION: &str = "backup.subscription";
/// The meta key holding the last outcome.
pub(crate) const META: &str = "backup";
/// After a failed attempt the daemon tries again this much later.
pub(crate) const RETRY_HOURS: f64 = 1.0;
/// The event actions.
pub(crate) const EVENT_DONE: &str = "backup.done";
pub(crate) const EVENT_FAILED: &str = "backup.failed";
/// How long one Azure CLI call, or the owner's command, may take.
const AZ_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// A copy left under PULSE_HOME/tmp/backup by a crashed attempt goes after this.
const STALE_TEMP: Duration = Duration::from_secs(24 * 3600);

/// A program run with arguments and extra environment, answering (exit
/// code, stdout, stderr). The Azure CLI and the owner's `run:` command go
/// through it; tests put a fake in its place.
pub(crate) type Runner = dyn Fn(&str, &[String], &[(String, String)], Duration) -> Result<(i32, String, String)>
    + Send
    + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    Dir(PathBuf),
    Azure {
        account: String,
        container: String,
        prefix: String,
    },
    Command(String),
}

impl Destination {
    /// Reads `backup.to`. A path may start with `~`; `az://` needs an account
    /// and a container; `run:` needs `{file}` somewhere in the command.
    pub(crate) fn parse(text: &str) -> Result<Destination> {
        let text = text.trim();
        if text.is_empty() {
            return Err(anyhow!(
                "{TO} is empty: set it to a directory, az://ACCOUNT/CONTAINER[/PREFIX], or `run: COMMAND {{file}} {{name}}` (pulse settings set {TO} ...)"
            ));
        }
        if let Some(rest) = text.strip_prefix("az://") {
            let mut parts = rest.trim_matches('/').splitn(3, '/');
            let account = parts.next().unwrap_or_default().trim().to_string();
            let container = parts.next().unwrap_or_default().trim().to_string();
            let prefix = parts
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches('/')
                .to_string();
            if account.is_empty() || container.is_empty() {
                return Err(anyhow!(
                    "{TO} {text}: an Azure destination is az://ACCOUNT/CONTAINER[/PREFIX]"
                ));
            }
            return Ok(Destination::Azure {
                account,
                container,
                prefix,
            });
        }
        if let Some(rest) = text.strip_prefix("run:") {
            let cmd = rest.trim().to_string();
            if !cmd.contains("{file}") {
                return Err(anyhow!(
                    "{TO} {text}: a run: command must name {{file}} (the backup to send; {{name}} is its file name)"
                ));
            }
            return Ok(Destination::Command(cmd));
        }
        let mut path = PathBuf::from(text);
        if text == "~" || text.starts_with("~/") {
            path = PathBuf::from(crate::util::home_dir())
                .join(text.trim_start_matches('~').trim_start_matches('/'));
        }
        if path.is_relative() {
            return Err(anyhow!(
                "{TO} {text}: a directory destination is an absolute path (or ~/...)"
            ));
        }
        Ok(Destination::Dir(path))
    }

    /// The destination as the owner would write it.
    pub(crate) fn label(&self) -> String {
        match self {
            Destination::Dir(p) => p.display().to_string(),
            Destination::Azure {
                account,
                container,
                prefix,
            } if prefix.is_empty() => format!("az://{account}/{container}"),
            Destination::Azure {
                account,
                container,
                prefix,
            } => format!("az://{account}/{container}/{prefix}"),
            Destination::Command(c) => format!("run: {c}"),
        }
    }
}

/// What one attempt did; the last one is meta `backup`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) at: String,
    pub(crate) ok: bool,
    /// The destination as written (never anything secret).
    pub(crate) to: String,
    /// Where the copy is: the file's path, the blob's URL, or the command run.
    pub(crate) location: String,
    pub(crate) name: String,
    /// The gzip file's size.
    pub(crate) bytes: u64,
    /// The store copy's size before gzip.
    pub(crate) store_bytes: u64,
    pub(crate) sha256: String,
    pub(crate) took_ms: u64,
    /// Older copies removed under `backup.keep`.
    pub(crate) removed: Vec<String>,
    /// Why it failed, or a note (a prune that could not run) on a backup that landed.
    pub(crate) error: String,
    pub(crate) by: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Opts {
    /// A destination for this run only; empty reads `backup.to`.
    pub(crate) to: String,
    /// Copies to keep for this run only; `None` reads `backup.keep`.
    pub(crate) keep: Option<i64>,
    pub(crate) actor: String,
    pub(crate) now: String,
}

/// Takes one backup now and records it. Never panics on the way: every
/// failure is the outcome's `error`.
pub(crate) fn run_backup(s: &pulse_store::Store, o: Opts) -> Outcome {
    run_backup_with(s, o, &real_run)
}

pub(crate) fn run_backup_with(s: &pulse_store::Store, o: Opts, run: &Runner) -> Outcome {
    let started = Instant::now();
    let to_text = if o.to.trim().is_empty() {
        s.setting(TO, "")
    } else {
        o.to.clone()
    };
    let keep = o.keep.unwrap_or_else(|| s.setting_int(KEEP)).max(0) as usize;
    let subscription = s.setting(SUBSCRIPTION, "").trim().to_string();
    let mut out = Outcome {
        at: o.now.clone(),
        by: if o.actor.is_empty() {
            "daemon".into()
        } else {
            o.actor.clone()
        },
        to: to_text.trim().to_string(),
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let dest = Destination::parse(&to_text)?;
        out.to = dest.label();
        let host = host_name();
        let stamp = must_iso(&o.now).format("%Y%m%dT%H%M%SZ").to_string();
        let name = format!("pulse-{host}-{stamp}.sqlite.gz");
        let tmp = Path::new(&s.home).join("tmp").join("backup");
        std::fs::create_dir_all(&tmp).with_context(|| format!("make {}", tmp.display()))?;
        clear_stale(&tmp);
        let snap = tmp.join(format!("pulse-{host}-{stamp}.sqlite"));
        let gz = tmp.join(&name);
        let _cleanup = Cleanup(vec![snap.clone(), gz.clone()]);
        snapshot(s, &snap)?;
        out.store_bytes = std::fs::metadata(&snap).map(|m| m.len()).unwrap_or(0);
        let (bytes, sha) = compress(&snap, &gz)?;
        let _ = std::fs::remove_file(&snap);
        out.bytes = bytes;
        out.sha256 = sha;
        out.name = name.clone();
        out.location = deliver(&dest, &gz, &name, &subscription, run)?;
        match prune(&dest, &host, keep, &subscription, run) {
            Ok(removed) => out.removed = removed,
            Err(e) => {
                out.error = format!(
                    "the backup landed; older copies were not removed: {}",
                    crate::compute::redact_secrets(&crate::run::err_message(&e))
                )
            }
        }
        Ok(())
    })();
    out.took_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(()) => out.ok = true,
        Err(e) => {
            out.ok = false;
            out.error = crate::compute::redact_secrets(&crate::run::err_message(&e));
        }
    }
    record(s, &out);
    out
}

/// The daemon's turn: a backup when `backup.to` is set, `backup.every_hours`
/// is over 0 and the last one is older than that (an hour after a failure).
/// Returns the daemon's log line, none when there was nothing to do.
pub(crate) fn tend_backup(s: &pulse_store::Store, now: &str, actor: &str) -> Vec<String> {
    if !due(s, now) {
        return Vec::new();
    }
    let out = run_backup(
        s,
        Opts {
            actor: actor.to_string(),
            now: now.to_string(),
            ..Default::default()
        },
    );
    vec![line(&out)]
}

/// Whether the daemon's next backup is due at `now`.
pub(crate) fn due(s: &pulse_store::Store, now: &str) -> bool {
    next_due(s).is_some_and(|at| at.as_str() <= now)
}

/// When the daemon takes its next backup; `None` while it takes none (no
/// destination, or `backup.every_hours` 0).
pub(crate) fn next_due(s: &pulse_store::Store) -> Option<String> {
    if s.setting(TO, "").trim().is_empty() {
        return None;
    }
    let every = s.setting_int(EVERY_HOURS);
    if every <= 0 {
        return None;
    }
    Some(match last(s) {
        None => String::new(),
        Some(l) if l.ok => plus_hours(&l.at, every as f64),
        Some(l) => plus_hours(&l.at, RETRY_HOURS),
    })
}

/// The last outcome, from meta.
pub(crate) fn last(s: &pulse_store::Store) -> Option<Outcome> {
    match s.meta(META) {
        Ok((raw, true)) => serde_json::from_str(&raw).ok(),
        _ => None,
    }
}

/// What `pulse backup --status` and the hub show.
pub fn status(s: &pulse_store::Store) -> Value {
    let to = s.setting(TO, "");
    json!({
        "to": to.trim(),
        "every_hours": s.setting_int(EVERY_HOURS),
        "keep": s.setting_int(KEEP),
        "subscription": s.setting(SUBSCRIPTION, "").trim(),
        "last": last(s),
        "next_due": next_due(s),
    })
}

/// One line about an outcome, for the daemon's log and `pulse backup`.
pub(crate) fn line(o: &Outcome) -> String {
    if !o.ok {
        return format!("backup failed ({}): {}", o.to, o.error);
    }
    let mut text = format!(
        "backup: {} ({} before gzip) to {} in {}",
        crate::tmpspace::human_bytes(o.bytes),
        crate::tmpspace::human_bytes(o.store_bytes),
        o.location,
        seconds(o.took_ms)
    );
    if !o.removed.is_empty() {
        text.push_str(&format!(", {} older removed", o.removed.len()));
    }
    if !o.error.is_empty() {
        text.push_str("; ");
        text.push_str(&o.error);
    }
    text
}

/// `38s`, `2m05s`: how long an attempt took.
fn seconds(ms: u64) -> String {
    let secs = ms.div_ceil(1000);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

/// A consistent copy of the store at `path`, checked before it goes anywhere.
fn snapshot(s: &pulse_store::Store, path: &Path) -> Result<()> {
    let _ = std::fs::remove_file(path);
    s.conn()?
        .execute("VACUUM INTO ?", [path.display().to_string()])
        .context("copy the store (VACUUM INTO)")?;
    let copy = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("open the copy")?;
    let verdict: String = copy
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .context("check the copy")?;
    if verdict != "ok" {
        return Err(anyhow!("the copy did not pass quick_check: {verdict}"));
    }
    Ok(())
}

/// gzip `src` into `dst`; answers the gzip file's size and its sha256.
fn compress(src: &Path, dst: &Path) -> Result<(u64, String)> {
    let mut input = std::fs::File::open(src).with_context(|| format!("read {}", src.display()))?;
    let file = std::fs::File::create(dst).with_context(|| format!("write {}", dst.display()))?;
    let mut encoder =
        flate2::write::GzEncoder::new(Hashing::new(file), flate2::Compression::fast());
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        encoder.write_all(&buf[..n])?;
    }
    let hashing = encoder.finish()?;
    let (bytes, sha) = hashing.finish()?;
    Ok((bytes, sha))
}

/// Puts the file where the destination says; answers where it is.
fn deliver(
    dest: &Destination,
    file: &Path,
    name: &str,
    subscription: &str,
    run: &Runner,
) -> Result<String> {
    match dest {
        Destination::Dir(dir) => {
            if !dir.is_dir() {
                return Err(anyhow!(
                    "{} is not here (a drive not mounted, or a folder not made yet): the backup directory must exist already; pulse never makes it",
                    dir.display()
                ));
            }
            let part = dir.join(format!(".{name}.part"));
            let target = dir.join(name);
            std::fs::copy(file, &part).with_context(|| format!("copy to {}", part.display()))?;
            let want = std::fs::metadata(file)?.len();
            let got = std::fs::metadata(&part)?.len();
            if want != got {
                let _ = std::fs::remove_file(&part);
                return Err(anyhow!(
                    "the copy at {} is {got} bytes, the backup {want}",
                    part.display()
                ));
            }
            std::fs::rename(&part, &target)
                .with_context(|| format!("rename to {}", target.display()))?;
            Ok(target.display().to_string())
        }
        Destination::Azure {
            account,
            container,
            prefix,
        } => {
            let blob = blob_name(prefix, name);
            let args = vec![
                "storage".to_string(),
                "blob".into(),
                "upload".into(),
                "--account-name".into(),
                account.clone(),
                "--container-name".into(),
                container.clone(),
                "--name".into(),
                blob.clone(),
                "--file".into(),
                file.display().to_string(),
                "--overwrite".into(),
                "true".into(),
                "--output".into(),
                "none".into(),
            ];
            az_with_auth(account, subscription, &args, run)?;
            Ok(format!(
                "https://{account}.blob.core.windows.net/{container}/{blob}"
            ))
        }
        Destination::Command(cmd) => {
            let filled = cmd
                .replace(
                    "{file}",
                    &crate::util::shell_quote(&file.display().to_string()),
                )
                .replace("{name}", &crate::util::shell_quote(name));
            let (rc, stdout, stderr) = run(
                "sh",
                &["-c".to_string(), filled.clone()],
                &[],
                COMMAND_TIMEOUT,
            )?;
            if rc != 0 {
                return Err(anyhow!(
                    "`{}` exited {rc}: {}",
                    crate::run::squeeze(&filled, 160),
                    crate::run::squeeze(&format!("{stderr}\n{stdout}"), 400)
                ));
            }
            Ok(crate::run::squeeze(&filled, 200))
        }
    }
}

/// Removes this machine's copies beyond `keep` (newest kept); answers their names.
fn prune(
    dest: &Destination,
    host: &str,
    keep: usize,
    subscription: &str,
    run: &Runner,
) -> Result<Vec<String>> {
    if keep == 0 {
        return Ok(Vec::new());
    }
    let mine = |n: &str| n.starts_with(&format!("pulse-{host}-")) && n.ends_with(".sqlite.gz");
    match dest {
        Destination::Dir(dir) => {
            let mut names: Vec<String> = std::fs::read_dir(dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| mine(n))
                .collect();
            names.sort();
            let extra = names.len().saturating_sub(keep);
            let mut removed = Vec::new();
            for n in names.into_iter().take(extra) {
                std::fs::remove_file(dir.join(&n))
                    .with_context(|| format!("remove {}", dir.join(&n).display()))?;
                removed.push(n);
            }
            Ok(removed)
        }
        Destination::Azure {
            account,
            container,
            prefix,
        } => {
            let listed = az_with_auth(
                account,
                subscription,
                &[
                    "storage".to_string(),
                    "blob".into(),
                    "list".into(),
                    "--account-name".into(),
                    account.clone(),
                    "--container-name".into(),
                    container.clone(),
                    "--prefix".into(),
                    blob_name(prefix, &format!("pulse-{host}-")),
                    "--query".into(),
                    "[].name".into(),
                    "--output".into(),
                    "tsv".into(),
                ],
                run,
            )?;
            let mut names: Vec<String> = listed
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .filter(|l| mine(l.rsplit('/').next().unwrap_or(l)))
                .collect();
            names.sort();
            let extra = names.len().saturating_sub(keep);
            let mut removed = Vec::new();
            for n in names.into_iter().take(extra) {
                az_with_auth(
                    account,
                    subscription,
                    &[
                        "storage".to_string(),
                        "blob".into(),
                        "delete".into(),
                        "--account-name".into(),
                        account.clone(),
                        "--container-name".into(),
                        container.clone(),
                        "--name".into(),
                        n.clone(),
                        "--output".into(),
                        "none".into(),
                    ],
                    run,
                )?;
                removed.push(n);
            }
            Ok(removed)
        }
        Destination::Command(_) => Ok(Vec::new()),
    }
}

fn blob_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Runs one `az storage ...` call: with the account key when it can be read
/// (handed over in the environment), with the login otherwise. Answers stdout.
/// A `subscription` (`backup.subscription`) rides on every call as
/// `--subscription`, so an account outside the CLI's default subscription is
/// found for the key and the login's token is cut for the right tenant.
fn az_with_auth(
    account: &str,
    subscription: &str,
    args: &[String],
    run: &Runner,
) -> Result<String> {
    let base_env = vec![
        ("AZURE_CORE_NO_COLOR".to_string(), "true".to_string()),
        (
            "AZURE_CORE_ONLY_SHOW_ERRORS".to_string(),
            "true".to_string(),
        ),
    ];
    let args = with_subscription(args, subscription);
    let key = run(
        "az",
        &with_subscription(
            &[
                "storage".to_string(),
                "account".into(),
                "keys".into(),
                "list".into(),
                "--account-name".into(),
                account.to_string(),
                "--query".into(),
                "[0].value".into(),
                "--output".into(),
                "tsv".into(),
            ],
            subscription,
        ),
        &base_env,
        AZ_TIMEOUT,
    )
    .ok()
    .filter(|(rc, out, _)| *rc == 0 && !out.trim().is_empty())
    .map(|(_, out, _)| out.trim().to_string());
    let mut tried = Vec::new();
    if let Some(key) = key {
        let mut env = base_env.clone();
        env.push(("AZURE_STORAGE_KEY".into(), key));
        env.push(("AZURE_STORAGE_AUTH_MODE".into(), "key".into()));
        let (rc, stdout, stderr) = run("az", &args, &env, AZ_TIMEOUT)?;
        if rc == 0 {
            return Ok(stdout);
        }
        tried.push(format!(
            "with the account key: {}",
            az_text(&stdout, &stderr)
        ));
    } else {
        tried.push("the account key could not be read (az storage account keys list)".into());
    }
    let mut login = args.to_vec();
    login.push("--auth-mode".into());
    login.push("login".into());
    let (rc, stdout, stderr) = run("az", &login, &base_env, AZ_TIMEOUT)?;
    if rc == 0 {
        return Ok(stdout);
    }
    tried.push(format!("with the login: {}", az_text(&stdout, &stderr)));
    Err(anyhow!(
        "az {} failed - {}",
        args.iter().take(3).cloned().collect::<Vec<_>>().join(" "),
        tried.join("; ")
    ))
}

/// `args` plus `--subscription SUB` when one is set.
fn with_subscription(args: &[String], subscription: &str) -> Vec<String> {
    let mut out = args.to_vec();
    if !subscription.trim().is_empty() {
        out.push("--subscription".into());
        out.push(subscription.trim().to_string());
    }
    out
}

fn az_text(stdout: &str, stderr: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    crate::run::squeeze(text, 300)
}

/// Writes meta `backup` and the timeline event.
fn record(s: &pulse_store::Store, o: &Outcome) {
    let Ok(conn) = s.conn() else { return };
    exec1(
        &conn,
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        &[&META, &serde_json::to_string(o).unwrap_or_default()],
    );
    let subject = if o.name.is_empty() {
        o.to.clone()
    } else {
        o.name.clone()
    };
    event_at(
        &conn,
        &o.at,
        &o.by,
        if o.ok { EVENT_DONE } else { EVENT_FAILED },
        &subject,
        json!({
            "to": o.to,
            "location": o.location,
            "bytes": o.bytes,
            "store_bytes": o.store_bytes,
            "sha256": o.sha256,
            "took_ms": o.took_ms,
            "removed": o.removed,
            "error": o.error,
        }),
    );
}

/// This machine's name, as a file name part: `Manishs-MacBook-Pro-2`.
pub(crate) fn host_name() -> String {
    let raw = nix::unistd::gethostname()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let short = raw.split('.').next().unwrap_or_default();
    let clean: String = short
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() {
        "host".into()
    } else {
        clean
    }
}

/// Removes what an earlier attempt left in the temp dir.
fn clear_stale(tmp: &Path) {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

struct Cleanup(Vec<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// A writer that counts and hashes what passes through it.
struct Hashing<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}
impl<W: Write> Hashing<W> {
    fn new(inner: W) -> Self {
        Hashing {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }
    fn finish(mut self) -> Result<(u64, String)> {
        self.inner.flush()?;
        Ok((self.bytes, hex::encode(self.hasher.finalize())))
    }
}
impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The real runner: the program with this process's environment plus
/// `extra`, killed after `timeout`.
fn real_run(
    program: &str,
    args: &[String],
    extra: &[(String, String)],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for kv in crate::proc::environ() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!("{program} is not installed on this machine")
        } else {
            anyhow!("{program}: {e}")
        }
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let reader = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut text);
            }
            text
        })
    };
    let err_reader = |pipe: Option<std::process::ChildStderr>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut text);
            }
            text
        })
    };
    let out_thread = reader(stdout);
    let err_thread = err_reader(stderr);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{program} {} took longer than {:?} and was stopped",
                args.iter().take(3).cloned().collect::<Vec<_>>().join(" "),
                timeout
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_store};
    use std::sync::{Arc, Mutex};

    /// What a fake `az` saw: program, arguments, and the environment it was given.
    type Calls = Arc<Mutex<Vec<(String, Vec<String>, Vec<(String, String)>)>>>;
    /// What a fake shell saw: program and arguments.
    type Seen = Arc<Mutex<Vec<(String, Vec<String>)>>>;

    fn gunzip(path: &Path) -> Vec<u8> {
        let mut d = flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap());
        let mut out = Vec::new();
        d.read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn destinations_are_a_directory_azure_or_a_command() {
        assert_eq!(
            Destination::parse("/Volumes/Kingston/pulse").unwrap(),
            Destination::Dir(PathBuf::from("/Volumes/Kingston/pulse"))
        );
        assert_eq!(
            Destination::parse(" az://acct/pulse-backups/store/ ").unwrap(),
            Destination::Azure {
                account: "acct".into(),
                container: "pulse-backups".into(),
                prefix: "store".into()
            }
        );
        assert_eq!(
            Destination::parse("az://acct/pulse-backups")
                .unwrap()
                .label(),
            "az://acct/pulse-backups"
        );
        assert_eq!(
            Destination::parse("run: rclone copy {file} remote:pulse/").unwrap(),
            Destination::Command("rclone copy {file} remote:pulse/".into())
        );
        let home = crate::util::home_dir();
        assert_eq!(
            Destination::parse("~/Backups/pulse").unwrap(),
            Destination::Dir(PathBuf::from(home).join("Backups/pulse"))
        );
        for bad in ["", "az://acct", "run: echo hi", "relative/dir"] {
            let e = Destination::parse(bad).unwrap_err().to_string();
            assert!(e.contains(TO), "{bad}: {e}");
        }
    }

    #[test]
    fn a_backup_to_a_directory_is_a_checked_gzip_of_the_store_and_keeps_the_newest() {
        let s = new_store();
        let _ = s.set_setting(crate::util::HUMAN_AGENT, "lead.idle_hours", "7");
        let dir = tempfile::tempdir().unwrap();
        let host = host_name();
        // Two older copies and a stranger's: keep 2 removes the oldest of ours only.
        for stamp in ["20260101T000000Z", "20260102T000000Z"] {
            std::fs::write(
                dir.path().join(format!("pulse-{host}-{stamp}.sqlite.gz")),
                b"old",
            )
            .unwrap();
        }
        std::fs::write(
            dir.path()
                .join("pulse-other-host-20260103T000000Z.sqlite.gz"),
            b"x",
        )
        .unwrap();
        let out = run_backup_with(
            &s,
            Opts {
                to: dir.path().display().to_string(),
                keep: Some(2),
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &|_, _, _, _| panic!("a directory backup runs no program"),
        );
        assert!(out.ok, "{out:?}");
        assert_eq!(out.name, format!("pulse-{host}-20260906T100000Z.sqlite.gz"));
        assert_eq!(
            out.location,
            dir.path().join(&out.name).display().to_string()
        );
        assert_eq!(
            out.removed,
            vec![format!("pulse-{host}-20260101T000000Z.sqlite.gz")]
        );
        assert!(out.store_bytes > out.bytes, "{out:?}");
        let gz = std::fs::read(&out.location).unwrap();
        assert_eq!(gz.len() as u64, out.bytes);
        assert_eq!(hex::encode(Sha256::digest(&gz)), out.sha256);
        // The copy is a store with the setting in it.
        let raw = gunzip(Path::new(&out.location));
        let copy = dir.path().join("copy.sqlite");
        std::fs::write(&copy, raw).unwrap();
        let conn = rusqlite::Connection::open(&copy).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM settings WHERE name = 'lead.idle_hours'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "7");
        // Nothing of ours is left in the temp dir; the stranger's file stays.
        let tmp = Path::new(&s.home).join("tmp").join("backup");
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        assert!(
            dir.path()
                .join("pulse-other-host-20260103T000000Z.sqlite.gz")
                .exists()
        );
        // Recorded: meta and the event; the status says when the next is due.
        let last = last(&s).unwrap();
        assert_eq!(last, out);
        let conn = s.conn().unwrap();
        let ev = crate::util::must_one(
            &conn,
            "SELECT action, subject, detail FROM events WHERE action LIKE 'backup.%' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        assert_eq!(ev.str("action"), EVENT_DONE);
        assert_eq!(ev.str("subject"), out.name);
        assert!(ev.str("detail").contains(&out.sha256));
        let text = line(&out);
        assert!(text.starts_with("backup: "), "{text}");
        assert!(text.contains("1 older removed"), "{text}");
        assert_eq!(
            next_due(&s),
            None,
            "no destination setting, no daemon backups"
        );
        let _ = s.set_setting(
            crate::util::HUMAN_AGENT,
            TO,
            dir.path().display().to_string().as_str(),
        );
        assert_eq!(next_due(&s).as_deref(), Some("2026-09-07T10:00:00Z"));
        assert!(!due(&s, "2026-09-07T09:59:59Z"));
        assert!(due(&s, "2026-09-07T10:00:00Z"));
    }

    #[test]
    fn a_directory_that_is_not_there_is_refused_and_recorded() {
        let s = new_store();
        let out = run_backup_with(
            &s,
            Opts {
                to: "/Volumes/NotMounted/pulse".into(),
                keep: None,
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &|_, _, _, _| panic!("no program"),
        );
        assert!(!out.ok);
        assert!(out.error.contains("is not here"), "{out:?}");
        assert!(!Path::new("/Volumes/NotMounted").exists());
        let conn = s.conn().unwrap();
        let ev = crate::util::must_one(
            &conn,
            "SELECT action, subject FROM events WHERE action LIKE 'backup.%' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        assert_eq!(ev.str("action"), EVENT_FAILED);
        assert!(line(&out).starts_with("backup failed ("), "{}", line(&out));
        // The daemon retries an hour after a failure, not a day.
        let _ = s.set_setting(crate::util::HUMAN_AGENT, TO, "/Volumes/NotMounted/pulse");
        assert_eq!(next_due(&s).as_deref(), Some("2026-09-06T11:00:00Z"));
    }

    #[test]
    fn azure_uploads_with_the_account_key_in_the_environment_and_prunes_by_name() {
        let s = new_store();
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let host = host_name();
        let seen = calls.clone();
        let listed = format!(
            "store/pulse-{host}-20260101T000000Z.sqlite.gz\nstore/pulse-{host}-20260102T000000Z.sqlite.gz\nstore/pulse-{host}-20260906T100000Z.sqlite.gz\n"
        );
        let run = move |program: &str,
                        args: &[String],
                        env: &[(String, String)],
                        _: Duration|
              -> Result<(i32, String, String)> {
            seen.lock()
                .unwrap()
                .push((program.to_string(), args.to_vec(), env.to_vec()));
            let verb = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
            Ok(match verb.as_str() {
                "storage account keys list" => (0, "s3cr3tkey==\n".into(), String::new()),
                "storage blob upload --account-name" => (0, String::new(), String::new()),
                "storage blob list --account-name" => (0, listed.clone(), String::new()),
                "storage blob delete --account-name" => (0, String::new(), String::new()),
                other => panic!("unexpected az call: {other}"),
            })
        };
        let out = run_backup_with(
            &s,
            Opts {
                to: "az://acct/pulse-backups/store".into(),
                keep: Some(2),
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &run,
        );
        assert!(out.ok, "{out:?}");
        assert_eq!(
            out.location,
            format!(
                "https://acct.blob.core.windows.net/pulse-backups/store/pulse-{host}-20260906T100000Z.sqlite.gz"
            )
        );
        assert_eq!(
            out.removed,
            vec![format!("store/pulse-{host}-20260101T000000Z.sqlite.gz")]
        );
        let calls = calls.lock().unwrap();
        let upload = calls
            .iter()
            .find(|(_, a, _)| a.get(2).map(String::as_str) == Some("upload"))
            .expect("an upload");
        assert_eq!(upload.0, "az");
        assert!(
            upload
                .2
                .contains(&("AZURE_STORAGE_KEY".to_string(), "s3cr3tkey==".to_string())),
            "{:?}",
            upload.2
        );
        assert!(
            !upload.1.iter().any(|a| a.contains("s3cr3tkey")),
            "the key never travels as an argument"
        );
        assert!(!upload.1.contains(&"--auth-mode".to_string()));
        assert!(
            calls
                .iter()
                .all(|(_, a, _)| !a.contains(&"--subscription".to_string())),
            "no subscription set: az works in its default one"
        );
        let deletes = calls
            .iter()
            .filter(|(_, a, _)| a.get(2).map(String::as_str) == Some("delete"))
            .count();
        assert_eq!(deletes, 1);
        // Nothing secret in what was recorded.
        let (raw, _) = s.meta(META).unwrap();
        assert!(!raw.contains("s3cr3tkey"));
    }

    #[test]
    fn azure_calls_carry_backup_subscription_when_one_is_set() {
        let s = new_store();
        s.set_setting("owner", SUBSCRIPTION, " lamplitlabs-prod-de ")
            .unwrap();
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let host = host_name();
        let seen = calls.clone();
        let listed = format!(
            "store/pulse-{host}-20260101T000000Z.sqlite.gz\nstore/pulse-{host}-20260906T100000Z.sqlite.gz\n"
        );
        let run = move |program: &str,
                        args: &[String],
                        env: &[(String, String)],
                        _: Duration|
              -> Result<(i32, String, String)> {
            seen.lock()
                .unwrap()
                .push((program.to_string(), args.to_vec(), env.to_vec()));
            let verb = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
            Ok(match verb.as_str() {
                "storage account keys list" => (0, "s3cr3tkey==\n".into(), String::new()),
                "storage blob upload --account-name" => (0, String::new(), String::new()),
                "storage blob list --account-name" => (0, listed.clone(), String::new()),
                "storage blob delete --account-name" => (0, String::new(), String::new()),
                other => panic!("unexpected az call: {other}"),
            })
        };
        let out = run_backup_with(
            &s,
            Opts {
                to: "az://lamplitpulse/pulse-backups/store".into(),
                keep: Some(1),
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &run,
        );
        assert!(out.ok, "{out:?}");
        assert_eq!(out.removed.len(), 1);
        let calls = calls.lock().unwrap();
        // Three az_with_auth calls (upload, list, delete), each reading the key first: six
        // az calls, every one in the named subscription.
        assert_eq!(calls.len(), 6, "{calls:?}");
        for (program, args, _) in calls.iter() {
            assert_eq!(program, "az");
            let at = args
                .iter()
                .position(|a| a == "--subscription")
                .unwrap_or_else(|| panic!("--subscription missing in {args:?}"));
            assert_eq!(
                args.get(at + 1).map(String::as_str),
                Some("lamplitlabs-prod-de")
            );
            assert_eq!(
                args.iter().filter(|a| *a == "--subscription").count(),
                1,
                "{args:?}"
            );
        }
        assert_eq!(status(&s)["subscription"], "lamplitlabs-prod-de");
    }

    #[test]
    fn azure_falls_back_to_the_login_when_the_key_is_not_readable() {
        let s = new_store();
        let modes: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = modes.clone();
        let run = move |_: &str,
                        args: &[String],
                        _: &[(String, String)],
                        _: Duration|
              -> Result<(i32, String, String)> {
            let verb = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
            seen.lock().unwrap().push(args.to_vec());
            Ok(match verb.as_str() {
                "storage account keys list" => (
                    1,
                    String::new(),
                    "AuthorizationFailed: no keys for you".into(),
                ),
                "storage blob upload --account-name" | "storage blob list --account-name" => {
                    assert!(args.contains(&"--auth-mode".to_string()), "{args:?}");
                    (0, String::new(), String::new())
                }
                other => panic!("unexpected az call: {other}"),
            })
        };
        let out = run_backup_with(
            &s,
            Opts {
                to: "az://acct/pulse-backups".into(),
                keep: Some(3),
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &run,
        );
        assert!(out.ok, "{out:?}");
        assert!(out.location.ends_with(".sqlite.gz"), "{out:?}");
        assert!(out.removed.is_empty());
    }

    #[test]
    fn a_run_command_gets_the_file_and_the_name_quoted() {
        let s = new_store();
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let calls = seen.clone();
        let run = move |program: &str,
                        args: &[String],
                        _: &[(String, String)],
                        _: Duration|
              -> Result<(i32, String, String)> {
            calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            // The file exists while the command runs.
            let cmd = &args[1];
            let path = cmd.split_whitespace().nth(2).unwrap().trim_matches('\'');
            assert!(Path::new(path).exists(), "{cmd}");
            Ok((0, String::new(), String::new()))
        };
        let out = run_backup_with(
            &s,
            Opts {
                to: "run: rclone copy {file} remote:pulse/{name}".into(),
                keep: Some(2),
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &run,
        );
        assert!(out.ok, "{out:?}");
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sh");
        assert_eq!(calls[0].1[0], "-c");
        assert!(
            calls[0].1[1].starts_with("rclone copy "),
            "{}",
            calls[0].1[1]
        );
        assert!(
            calls[0].1[1].ends_with(&format!("remote:pulse/{}", out.name)),
            "{}",
            calls[0].1[1]
        );
        assert_eq!(out.location, crate::run::squeeze(&calls[0].1[1], 200));
        drop(calls);

        let failing = |_: &str, _: &[String], _: &[(String, String)], _: Duration| {
            Ok((7, String::new(), "remote: connection refused".into()))
        };
        let out = run_backup_with(
            &s,
            Opts {
                to: "run: rclone copy {file} remote:pulse/".into(),
                keep: None,
                actor: "owner".into(),
                now: TEST_NOW.into(),
            },
            &failing,
        );
        assert!(!out.ok);
        assert!(
            out.error.contains("exited 7") && out.error.contains("connection refused"),
            "{out:?}"
        );
    }

    #[test]
    fn tend_backup_waits_for_the_cadence_and_runs_when_due() {
        let s = new_store();
        assert!(
            tend_backup(&s, TEST_NOW, "daemon").is_empty(),
            "no destination, nothing"
        );
        let dir = tempfile::tempdir().unwrap();
        let _ = s.set_setting(
            crate::util::HUMAN_AGENT,
            TO,
            dir.path().display().to_string().as_str(),
        );
        let _ = s.set_setting(crate::util::HUMAN_AGENT, EVERY_HOURS, "0");
        assert!(
            tend_backup(&s, TEST_NOW, "daemon").is_empty(),
            "every_hours 0 leaves it to pulse backup"
        );
        let _ = s.set_setting(crate::util::HUMAN_AGENT, EVERY_HOURS, "12");
        let lines = tend_backup(&s, TEST_NOW, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("backup: "), "{lines:?}");
        assert!(tend_backup(&s, "2026-09-06T21:59:00Z", "daemon").is_empty());
        let lines = tend_backup(&s, "2026-09-06T22:00:00Z", "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        let st = status(&s);
        assert_eq!(st["every_hours"], 12);
        assert_eq!(st["last"]["ok"], true);
        assert_eq!(st["next_due"], "2026-09-07T10:00:00Z");
    }
}
