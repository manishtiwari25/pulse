#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::{Result, anyhow};
use base64::Engine;
use regex::Regex;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
pub(crate) const HANDS_DEFAULT_WORKDIR: &str = "/srv/pulse";
static HANDS_JOB_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
static HANDS_BRANCH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._/-]+$").unwrap());
pub(crate) static HANDS_ENV_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(PULSE_[A-Z_]+|GIT_TERMINAL_PROMPT)=.*$").unwrap());
static HANDS_B64_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^b64 ([A-Za-z0-9+/=]+)$").unwrap());
static HANDS_UPLOAD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^git-upload-pack '([^'[:cntrl:]]+)'$").unwrap());
static HANDS_PINNED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^command="[^"]*pulse-hands[^"]*",([^ ]*,)?restrict"#).unwrap());
static HANDS_LOGGED_IN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(authTokens":\{"|loggedInUsers":\[\{")"#).unwrap());
#[derive(Debug, Clone)]
pub(crate) struct HandsError {
    pub(crate) msg: String,
}
impl std::fmt::Display for HandsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pulse-hands: {}", self.msg)
    }
}
impl std::error::Error for HandsError {}
/// Go: func handsDie(format string, args ...any) error
pub(crate) fn hands_die(msg: impl Into<String>) -> anyhow::Error {
    anyhow!(HandsError { msg: msg.into() })
}
#[derive(Debug, Clone)]
pub(crate) struct HandsExit {
    pub(crate) code: i32,
}
impl std::fmt::Display for HandsExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {}", self.code)
    }
}
impl std::error::Error for HandsExit {}
pub(crate) struct Hands {
    pub(crate) workdir: String,
    pub(crate) home: String,
    pub(crate) stdin: Box<dyn Read + Send>,
    pub(crate) stdout: Box<dyn Write + Send>,
    pub(crate) stderr: Box<dyn Write + Send>,
}
impl std::fmt::Debug for Hands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hands")
            .field("workdir", &self.workdir)
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}
/// Go: func handsAllowedWorker(rest []string) bool
pub(crate) fn hands_allowed_worker(rest: &[String]) -> bool {
    match rest
        .first()
        .map(|s| {
            std::path::Path::new(s)
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or(s)
                .to_string()
        })
        .as_deref()
    {
        Some("copilot") => true,
        Some("pulse") => rest.len() >= 3 && rest[1] == "harness" && rest[2] == "run",
        _ => false,
    }
}
/// Go: func newHands(stdin io.Reader, stdout, stderr io.Writer) *hands
pub(crate) fn new_hands<'a>(
    stdin: impl Read + Send + 'a,
    stdout: impl Write + Send + 'a,
    stderr: impl Write + Send + 'a,
) -> Hands {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let path = [
        std::path::Path::new(&home)
            .join(".local/bin")
            .display()
            .to_string(),
        std::path::Path::new(&home)
            .join(".cargo/bin")
            .display()
            .to_string(),
        std::path::Path::new(&home)
            .join(".bun/bin")
            .display()
            .to_string(),
        std::env::var("PATH").unwrap_or_default(),
    ]
    .join(":");
    unsafe {
        std::env::set_var("PATH", path);
    }
    Hands {
        workdir: std::env::var("PULSE_HANDS_WORKDIR")
            .unwrap_or_else(|_| HANDS_DEFAULT_WORKDIR.into()),
        home,
        stdin: erase_read_lifetime(Box::new(stdin)),
        stdout: erase_write_lifetime(Box::new(stdout)),
        stderr: erase_write_lifetime(Box::new(stderr)),
    }
}
fn erase_read_lifetime<'a>(r: Box<dyn Read + Send + 'a>) -> Box<dyn Read + Send> {
    // `Hands` is used synchronously by the command dispatcher; borrowed stdio writers outlive it.
    unsafe { std::mem::transmute::<Box<dyn Read + Send + 'a>, Box<dyn Read + Send>>(r) }
}
fn erase_write_lifetime<'a>(w: Box<dyn Write + Send + 'a>) -> Box<dyn Write + Send> {
    // `Hands` is used synchronously by the command dispatcher; borrowed stdio writers outlive it.
    unsafe { std::mem::transmute::<Box<dyn Write + Send + 'a>, Box<dyn Write + Send>>(w) }
}
impl Hands {
    /// Go: func (h *hands) jobDir(job string) (string, error)
    pub(crate) fn job_dir(&self, job: &str) -> Result<String> {
        if !HANDS_JOB_RE.is_match(job) || job == "." || job == ".." {
            return Err(hands_die("bad job id"));
        }
        Ok(format!("{}/{}", self.workdir, job))
    }
    /// Go: func (h *hands) cloneDir(job string) (string, error)
    pub(crate) fn clone_dir(&self, job: &str) -> Result<String> {
        let clone = format!("{}/clone", self.job_dir(job)?);
        if !Path::new(&clone).join(".git").is_dir() {
            return Err(hands_die(format!("no clone for job {job}")));
        }
        Ok(clone)
    }
    /// Go: func (h *hands) withTimeout(budget time.Duration, dir string, argv []string, extra map[string]string) int
    pub(crate) fn with_timeout(
        &mut self,
        budget: Duration,
        dir: &str,
        argv: &[String],
        extra: &std::collections::BTreeMap<String, String>,
    ) -> i32 {
        self.with_timeout_observed(budget, dir, argv, extra, None)
    }
    /// Go: func (h *hands) withTimeoutObserved(...)
    pub(crate) fn with_timeout_observed(
        &mut self,
        budget: Duration,
        dir: &str,
        argv: &[String],
        extra: &std::collections::BTreeMap<String, String>,
        observer: Option<&crate::proc::ProcessObserver>,
    ) -> i32 {
        if argv.is_empty() {
            return 127;
        }
        let mut env = crate::exec::worker_env_with_cli_path(
            &crate::util::strip_hook_git_env(
                &std::env::vars()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>(),
            ),
            extra,
        );
        env.retain(|kv| {
            !kv.starts_with("SSH_ORIGINAL_COMMAND=")
                && !kv.starts_with("SSH_CONNECTION=")
                && !kv.starts_with("SSH_CLIENT=")
        });
        let cancel = pulse_bus::CancellationToken::new();
        let grace = *HANDS_KILL_AFTER.read().unwrap();
        let out = crate::proc::run_group_observed(
            &cancel,
            argv,
            dir,
            &env,
            &[],
            budget + grace,
            observer,
        );
        match out {
            Ok(o) if o.timeout => crate::executor::REMOTE_EXIT_TIMEOUT,
            Ok(o) => {
                let _ = self.stdout.write_all(o.stdout.as_bytes());
                let _ = self.stderr.write_all(o.stderr.as_bytes());
                o.exit_code
            }
            Err(e) => {
                let _ = writeln!(self.stderr, "{}: {e}", argv[0]);
                127
            }
        }
    }
    /// Go: func (h *hands) probe() error
    pub(crate) fn probe(&mut self) -> Result<()> {
        let gitv = Command::new("git")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.split_whitespace().nth(2).map(ToOwned::to_owned))
            .unwrap_or_else(|| "missing".into());
        let mut copv = (PROBE_VERSION.read().unwrap())().unwrap_or_else(|_| "missing".into());
        let mut logged_in = "no".to_string();
        if copv != "missing" {
            let token_env = std::env::var("COPILOT_GITHUB_TOKEN").unwrap_or_default()
                + &std::env::var("GH_TOKEN").unwrap_or_default()
                + &std::env::var("GITHUB_TOKEN").unwrap_or_default();
            if !token_env.is_empty() {
                logged_in = "yes".into();
            } else if let Ok(data) =
                std::fs::read_to_string(Path::new(&self.home).join(".copilot/config.json"))
            {
                let flat = data.replace([' ', '\t', '\r', '\n'], "");
                if HANDS_LOGGED_IN_RE.is_match(&flat) {
                    logged_in = "yes".into();
                }
            }
        }
        let token_env = std::env::var("COPILOT_GITHUB_TOKEN").unwrap_or_default()
            + &std::env::var("GH_TOKEN").unwrap_or_default()
            + &std::env::var("GITHUB_TOKEN").unwrap_or_default();
        let mut harness = if !token_env.is_empty()
            || Path::new(&self.home)
                .join(".pulse/private/providers/copilot.json")
                .is_file()
        {
            "yes"
        } else {
            "no"
        }
        .to_string();
        let (mut pulsev, mut pulse_lookup) = match which::which("pulse") {
            Ok(p) => (
                p.canonicalize().unwrap_or(p).display().to_string(),
                String::new(),
            ),
            Err(_) => (
                "missing".into(),
                std::env::var("PATH")
                    .unwrap_or_default()
                    .replace(' ', "%20"),
            ),
        };
        if pulse_lookup.is_empty() && pulsev == "missing" {
            pulse_lookup = "-".into();
        }
        if std::env::var("PULSE_HANDS_ALLOW_ANY_WORKER").as_deref() == Ok("1") {
            if copv == "missing" {
                copv = "test".into();
            }
            logged_in = "yes".into();
            if pulsev == "missing" {
                pulsev = "test".into();
            }
            harness = "yes".into();
        }
        let mut jobs = Vec::new();
        let mut keys = Vec::new();
        let mut remotes = 0;
        for e in std::fs::read_dir(&self.workdir)
            .into_iter()
            .flatten()
            .flatten()
        {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = e.file_name().to_string_lossy().to_string();
                jobs.push(name.clone());
                if e.path().join("run.key").exists() {
                    keys.push(name.clone());
                }
                let clone = e.path().join("clone");
                if clone.join(".git").is_dir() {
                    let out = Command::new("git")
                        .args(["-C", &clone.display().to_string(), "remote"])
                        .env_clear()
                        .envs(crate::util::git_env(&[]).into_iter().filter_map(|kv| {
                            kv.split_once('=')
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                        }))
                        .output();
                    if out
                        .ok()
                        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
                        .unwrap_or(false)
                    {
                        remotes += 1;
                    }
                }
            }
        }
        let mut keys_ok = "no".to_string();
        if let Ok(data) =
            std::fs::read_to_string(Path::new(&self.home).join(".ssh/authorized_keys"))
        {
            let mut total = 0;
            let mut pinned = 0;
            for line in data
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                total += 1;
                if HANDS_PINNED_RE.is_match(line) {
                    pinned += 1;
                }
            }
            if total > 0 && total == pinned {
                keys_ok = "yes".into();
            }
        }
        match std::env::var("PULSE_HANDS_KEYS_OK") {
            Ok(v) if !v.is_empty() => {
                keys_ok = v;
            }
            _ => {}
        }
        let _ = std::fs::create_dir_all(&self.workdir);
        let free = Command::new("df")
            .args(["-Pm", &self.workdir])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .nth(1)
                    .and_then(|l| l.split_whitespace().nth(3).map(ToOwned::to_owned))
            })
            .unwrap_or_else(|| "0".into());
        // Go: user.Current() - the passwd entry of the real uid, not $USER.
        let user = nix::unistd::User::from_uid(nix::unistd::getuid())
            .ok()
            .flatten()
            .map_or_else(|| "?".to_string(), |u| u.name);
        let lookup = if pulse_lookup.is_empty() {
            String::new()
        } else {
            format!(" pulse_lookup={pulse_lookup}")
        };
        let dash = |v: Vec<String>| {
            if v.is_empty() {
                "-".into()
            } else {
                v.join(",")
            }
        };
        writeln!(
            self.stdout,
            "hands=ok user={user} workdir={} git={gitv} copilot={copv} logged_in={logged_in} harness={harness} pulse={pulsev}{lookup} remotes={remotes} keys_ok={keys_ok} disk_free_mb={free} jobs={} keys={} heartbeat=1 steps=1",
            self.workdir,
            dash(jobs),
            dash(keys)
        )?;
        Ok(())
    }
    /// Go: func (h *hands) prepare(job, branch, agent string) error
    pub(crate) fn prepare(&mut self, job: &str, branch: &str, agent: &str) -> Result<()> {
        if !HANDS_JOB_RE.is_match(agent) {
            return Err(hands_die("bad agent name"));
        }
        if !HANDS_BRANCH_RE.is_match(branch) {
            return Err(hands_die("bad branch"));
        }
        let dir = self.job_dir(job)?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| hands_die(e.to_string()))?;
        let bundle = format!("{dir}/source.bundle");
        let mut data = Vec::new();
        self.stdin.read_to_end(&mut data)?;
        std::fs::write(&bundle, data).map_err(|e| hands_die(e.to_string()))?;
        if !crate::util::git("", &["init", "-q", &format!("{dir}/verify")]).ok() {
            return Err(hands_die("cannot make the scratch repository"));
        }
        if !crate::util::git(&format!("{dir}/verify"), &["bundle", "verify", &bundle]).ok() {
            let _ = std::fs::remove_dir_all(format!("{dir}/verify"));
            return Err(hands_die("the bundle does not verify"));
        }
        let _ = std::fs::remove_dir_all(format!("{dir}/verify"));
        let clone = format!("{dir}/clone");
        let cl = if branch == "HEAD" {
            crate::util::git(
                "",
                &[
                    "-c",
                    "advice.detachedHead=false",
                    "clone",
                    "-q",
                    "--no-tags",
                    &bundle,
                    &clone,
                ],
            )
        } else {
            crate::util::git(
                "",
                &["clone", "-q", "--no-tags", "-b", branch, &bundle, &clone],
            )
        };
        if !cl.ok() {
            return Err(hands_die(cl.stderr.trim().to_string()));
        }
        let _ = std::fs::remove_file(bundle);
        let _ = crate::util::git(&clone, &["remote", "remove", "origin"]);
        let _ = crate::util::git(&clone, &["config", "user.name", agent]);
        let _ = crate::util::git(
            &clone,
            &["config", "user.email", &format!("{agent}@pulse.local")],
        );
        let _ = crate::util::git(&clone, &["config", "commit.gpgsign", "false"]);
        let _ = crate::util::git(&clone, &["config", "tag.gpgsign", "false"]);
        write!(
            self.stdout,
            "{}",
            crate::util::git(&clone, &["rev-parse", "HEAD"]).stdout
        )?;
        Ok(())
    }
    /// Go: func (h *hands) key(job, executor string) error
    pub(crate) fn key(&mut self, job: &str, executor: &str) -> Result<()> {
        if !HANDS_JOB_RE.is_match(executor) {
            return Err(hands_die("bad executor name"));
        }
        let dir = self.job_dir(job)?;
        let clone = self.clone_dir(job)?;
        let key = format!("{dir}/run.key");
        let mut data = Vec::new();
        self.stdin.read_to_end(&mut data)?;
        std::fs::write(&key, &data).map_err(|e| hands_die(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
        }
        if !String::from_utf8_lossy(&data).contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
            let _ = std::fs::remove_file(&key);
            return Err(hands_die("not an openssh private key"));
        }
        if !crate::util::file_exists(&key) {
            return Err(hands_die(format!(
                "the run's signing key {key} does not exist; refusing to configure user.signingkey (ADR-022)"
            )));
        }
        let agent = crate::util::git(&clone, &["config", "user.name"])
            .stdout
            .trim()
            .to_string();
        if !HANDS_JOB_RE.is_match(&agent) {
            return Err(hands_die("the clone has no agent identity (prepare first)"));
        }
        let hooks = format!("{clone}/.git/hooks");
        for args in [
            vec!["config", "gpg.format", "ssh"],
            vec!["config", "gpg.ssh.program", "ssh-keygen"],
            vec!["config", "user.signingkey", &key],
            vec!["config", "commit.gpgsign", "true"],
            vec!["config", "tag.gpgsign", "false"],
            vec!["config", "core.hooksPath", &hooks],
        ] {
            let _ = crate::util::git(&clone, &args);
        }
        std::fs::create_dir_all(&hooks)?;
        let hook = format!("{hooks}/prepare-commit-msg");
        std::fs::write(&hook, crate::keys::provenance_hook(&agent, job, executor))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
        }
        writeln!(self.stdout, "keyed")?;
        Ok(())
    }
    pub(crate) fn hook(&mut self, job: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        let mut data = Vec::new();
        self.stdin.read_to_end(&mut data)?;
        let hooks = format!("{clone}/.git/hooks");
        std::fs::create_dir_all(&hooks)?;
        let hook = format!("{hooks}/pre-commit");
        std::fs::write(&hook, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
        }
        writeln!(self.stdout, "hooked")?;
        Ok(())
    }
    pub(crate) fn brief(&mut self, job: &str) -> Result<()> {
        let dir = self.job_dir(job)?;
        if !Path::new(&dir).is_dir() {
            return Err(hands_die(format!("no job dir for {job} (prepare first)")));
        }
        let mut data = Vec::new();
        self.stdin.read_to_end(&mut data)?;
        std::fs::write(format!("{dir}/brief.md"), data)?;
        Ok(())
    }
    pub(crate) fn run(&mut self, job: &str, minutes: &str, rest: &[String]) -> Result<()> {
        let clone = self.clone_dir(job)?;
        let mut extra = BTreeMap::new();
        let mut i = 0;
        while i < rest.len() && rest[i] != "--" {
            if !HANDS_ENV_RE.is_match(&rest[i]) {
                return Err(hands_die(format!(
                    "env key not allowed: {}",
                    rest[i].split_once('=').map(|x| x.0).unwrap_or(&rest[i])
                )));
            }
            let (k, v) = rest[i].split_once('=').unwrap_or((&rest[i], ""));
            extra.insert(k.to_string(), v.to_string());
            i += 1;
        }
        if i >= rest.len() || rest[i] != "--" {
            return Err(hands_die("run needs -- before the worker command"));
        }
        let cmd = &rest[i + 1..];
        if cmd.is_empty() {
            return Err(hands_die("run needs a worker command"));
        }
        if std::env::var("PULSE_HANDS_ALLOW_ANY_WORKER").as_deref() != Ok("1")
            && !hands_allowed_worker(cmd)
        {
            return Err(hands_die(
                "only the copilot or pulse harness worker may run here",
            ));
        }
        let budget = hands_budget(minutes)?;
        if std::env::var("COPILOT_SUBAGENT_MAX_CONCURRENT").is_err() {
            extra.insert("COPILOT_SUBAGENT_MAX_CONCURRENT".into(), "256".into());
        }
        if std::env::var("COPILOT_SUBAGENT_MAX_DEPTH").is_err() {
            extra.insert("COPILOT_SUBAGENT_MAX_DEPTH".into(), "128".into());
        }
        Err(anyhow!(HandsExit {
            code: self.with_timeout_observed(budget, &clone, cmd, &extra, None)
        }))
    }
    pub(crate) fn result(&mut self, job: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        let data = std::fs::read(format!("{clone}/PULSE_RESULT.md"))
            .map_err(|_| anyhow!(HandsExit { code: 3 }))?;
        self.stdout.write_all(&data)?;
        Ok(())
    }
    pub(crate) fn count(&mut self, job: &str, base: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        if !Regex::new(r"^[0-9a-f]{40,64}$").unwrap().is_match(base) {
            return Err(hands_die("bad base"));
        }
        let r = crate::util::git(&clone, &["rev-list", "--count", &format!("{base}..HEAD")]);
        write!(self.stdout, "{}", r.stdout)?;
        if !r.ok() {
            write!(self.stderr, "{}", r.stderr)?;
            return Err(anyhow!(HandsExit { code: r.code }));
        }
        Ok(())
    }
    pub(crate) fn dirty(&mut self, job: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        write!(
            self.stdout,
            "{}",
            crate::util::git(&clone, &["status", "--porcelain", "--untracked-files=no"]).stdout
        )?;
        Ok(())
    }
    pub(crate) fn changed(&mut self, job: &str, base: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        if !Regex::new(r"^[0-9a-f]{40,64}$").unwrap().is_match(base) {
            return Err(hands_die("bad base"));
        }
        let r = crate::util::git(&clone, &["diff", "--name-only", &format!("{base}..HEAD")]);
        write!(self.stdout, "{}", r.stdout)?;
        if !r.ok() {
            write!(self.stderr, "{}", r.stderr)?;
            return Err(anyhow!(HandsExit { code: r.code }));
        }
        Ok(())
    }
    pub(crate) fn drop_result(&mut self, job: &str) -> Result<()> {
        let clone = self.clone_dir(job)?;
        if crate::util::git(&clone, &["ls-files", "--error-unmatch", "PULSE_RESULT.md"]).ok() {
            let _ = crate::util::git(&clone, &["rm", "-q", "--cached", "PULSE_RESULT.md"]);
            let _ = crate::util::git(
                &clone,
                &[
                    "commit",
                    "-q",
                    "-m",
                    "runner: drop PULSE_RESULT.md (never committed)",
                ],
            );
            writeln!(self.stdout, "dropped")?;
        } else {
            writeln!(self.stdout, "clean")?;
        }
        Ok(())
    }
    pub(crate) fn verify(&mut self, job: &str, minutes: &str, rest: &[String]) -> Result<()> {
        if rest.first().map(String::as_str) != Some("--") {
            return Err(hands_die("verify needs -- before the command"));
        }
        if rest.len() != 2 {
            return Err(hands_die("verify takes one shell command string"));
        }
        let clone = self.clone_dir(job)?;
        let budget = hands_budget(minutes)?;
        Err(anyhow!(HandsExit {
            code: self.with_timeout(
                budget,
                &clone,
                &["bash".into(), "-c".into(), rest[1].clone()],
                &BTreeMap::new()
            )
        }))
    }
    pub(crate) fn clean(&mut self, job: &str) -> Result<()> {
        std::fs::remove_dir_all(self.job_dir(job)?)?;
        Ok(())
    }
    pub(crate) fn upload_pack(&mut self, path: &str) -> Result<()> {
        let path = path.trim_matches('\'');
        if !path.starts_with(&(self.workdir.clone() + "/"))
            || !path.ends_with("/clone")
            || path
                .trim_start_matches(&(self.workdir.clone() + "/"))
                .matches('/')
                .count()
                != 1
        {
            return Err(hands_die(format!(
                "upload-pack path must be a job clone under {}",
                self.workdir
            )));
        }
        if path.contains("..") {
            return Err(hands_die("bad path"));
        }
        if !Path::new(path).join(".git").is_dir() {
            return Err(hands_die("no such clone"));
        }
        let mut child = Command::new("git")
            .args(["upload-pack", path])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let status = child.wait()?;
        if !status.success() {
            return Err(anyhow!(HandsExit {
                code: status.code().unwrap_or(1)
            }));
        }
        Ok(())
    }
    pub(crate) fn hands_main(&mut self, args: &[String]) -> Result<()> {
        let mut argv = args.to_vec();
        if let Ok(cmd) = std::env::var("SSH_ORIGINAL_COMMAND") {
            if cmd.is_empty() {
                // No forced command override.
            } else {
                if let Some(c) = HANDS_UPLOAD_RE.captures(&cmd) {
                    argv = vec!["git-upload-pack".into(), c[1].to_string()];
                } else if let Some(c) = HANDS_B64_RE.captures(&cmd) {
                    let raw = base64::engine::general_purpose::STANDARD.decode(&c[1]).map_err(|_| hands_die("refused: commands arrive as 'b64 ...' from pulse.py or as git-upload-pack"))?;
                    argv = String::from_utf8_lossy(&raw)
                        .split('\0')
                        .map(ToOwned::to_owned)
                        .collect();
                    while argv.last().map(|s| s.is_empty()).unwrap_or(false) {
                        argv.pop();
                    }
                    if argv.is_empty() {
                        return Err(hands_die("empty command"));
                    }
                } else {
                    return Err(hands_die(
                        "refused: commands arrive as 'b64 ...' from pulse.py or as git-upload-pack",
                    ));
                }
            }
        }
        if argv.is_empty() {
            return Err(hands_die(
                "no command (this key only runs pulse-hands verbs)",
            ));
        }
        let verb = argv[0].as_str();
        let rest = &argv[1..];
        let need = |n: usize, usage: &str| -> Result<()> {
            if rest.len() != n {
                Err(hands_die(usage))
            } else {
                Ok(())
            }
        };
        match verb {
            "probe" => self.probe(),
            "prepare" => {
                need(3, "prepare JOB BRANCH AGENT")?;
                self.prepare(&rest[0], &rest[1], &rest[2])
            }
            "key" => {
                need(2, "key JOB EXECUTOR")?;
                self.key(&rest[0], &rest[1])
            }
            "hook" => {
                need(1, "hook JOB")?;
                self.hook(&rest[0])
            }
            "brief" => {
                need(1, "brief JOB")?;
                self.brief(&rest[0])
            }
            "run" => {
                if rest.len() < 4 {
                    return Err(hands_die("run JOB MINUTES [K=V ...] -- CMD ..."));
                }
                self.run(&rest[0], &rest[1], &rest[2..])
            }
            "result" => {
                need(1, "result JOB")?;
                self.result(&rest[0])
            }
            "count" => {
                need(2, "count JOB BASE")?;
                self.count(&rest[0], &rest[1])
            }
            "dirty" => {
                need(1, "dirty JOB")?;
                self.dirty(&rest[0])
            }
            "changed" => {
                need(2, "changed JOB BASE")?;
                self.changed(&rest[0], &rest[1])
            }
            "drop-result" => {
                need(1, "drop-result JOB")?;
                self.drop_result(&rest[0])
            }
            "verify" => {
                if rest.len() < 4 {
                    return Err(hands_die("verify JOB MINUTES -- CMD"));
                }
                self.verify(&rest[0], &rest[1], &rest[2..])
            }
            "clean" => {
                need(1, "clean JOB")?;
                self.clean(&rest[0])
            }
            "git-upload-pack" => {
                need(1, "git-upload-pack PATH")?;
                self.upload_pack(&rest[0])
            }
            "heartbeat" | "steps" => Err(hands_die(format!("verb not allowed: {verb}"))),
            _ => Err(hands_die(format!("verb not allowed: {verb}"))),
        }
    }
}
/// Go: func handsBudget(budget string) (time.Duration, error)
pub(crate) fn hands_budget(budget: &str) -> Result<Duration> {
    let (n, secs) = if let Some(s) = budget.strip_suffix('s') {
        (
            s.parse::<u64>().map_err(|_| hands_die("bad minutes"))?,
            true,
        )
    } else {
        (
            budget
                .parse::<u64>()
                .map_err(|_| hands_die("bad minutes"))?,
            false,
        )
    };
    if n < 1 {
        return Err(hands_die("the budget must be at least one second"));
    }
    Ok(Duration::from_secs(if secs { n } else { n * 60 }))
}
pub(crate) static HANDS_KILL_AFTER: LazyLock<RwLock<Duration>> =
    LazyLock::new(|| RwLock::new(Duration::from_secs(30)));
pub(crate) type ProbeVersionFn = fn() -> Result<String>;
pub(crate) static PROBE_VERSION: LazyLock<RwLock<ProbeVersionFn>> =
    LazyLock::new(|| RwLock::new(copilot_version));
/// Go: func copilotVersion() (string, error)
pub(crate) fn copilot_version() -> Result<String> {
    let path = which::which("copilot")?;
    let out = Command::new(path).arg("--version").output();
    let Ok(out) = out else {
        return Ok("present".into());
    };
    if !out.status.success() {
        return Ok("present".into());
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let v = Regex::new(r"[^0-9.]")
        .unwrap()
        .replace_all(&first, "")
        .trim_matches('.')
        .to_string();
    if v.is_empty() {
        Ok("present".into())
    } else {
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hands(workdir: &str, input: &str) -> Hands {
        Hands {
            workdir: workdir.into(),
            home: tempfile::tempdir().unwrap().path().display().to_string(),
            stdin: Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            stdout: Box::new(Vec::<u8>::new()),
            stderr: Box::new(Vec::<u8>::new()),
        }
    }

    #[test]
    fn hands_key_refuses_signing_key_without_key_file() {
        let _serial = crate::test_helpers::serial();
        if unsafe { libc::getuid() } == 0 {
            return;
        }
        let job = "job-20260918T000000Z-abcdef";
        for (name, stdin, readonly, want_err, want_key) in [
            (
                "write fails",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n",
                true,
                "permission denied",
                false,
            ),
            (
                "not a key",
                "junk",
                false,
                "not an openssh private key",
                false,
            ),
            (
                "ok",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n",
                false,
                "",
                true,
            ),
        ] {
            let workdir = tempfile::tempdir().unwrap();
            let dir = workdir.path().join(job);
            let clone = dir.join("clone");
            std::fs::create_dir_all(&clone).unwrap();
            crate::test_helpers::run(&clone, &["git", "init", "-q"]);
            crate::test_helpers::run(&clone, &["git", "config", "user.name", "aurora"]);
            if readonly {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
                }
            }
            let mut h = make_hands(&workdir.path().display().to_string(), stdin);
            let err = h
                .key(job, "exec")
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            if !want_err.is_empty() {
                assert!(err.to_lowercase().contains(want_err), "{name}: {err}");
            } else {
                assert!(err.is_empty(), "{name}: {err}");
            }
            if readonly {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
                }
            }
            let out = std::process::Command::new("git")
                .args(["config", "--local", "--get", "user.signingkey"])
                .current_dir(&clone)
                .env_clear()
                .envs(crate::util::git_env(&[]).into_iter().filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                }))
                .output()
                .unwrap();
            let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if want_key {
                assert_eq!(got, dir.join("run.key").display().to_string());
            } else {
                assert!(got.is_empty(), "{name}: user.signingkey={got}");
                assert!(!dir.join("run.key").exists(), "{name}: run.key left behind");
            }
        }
    }
}
