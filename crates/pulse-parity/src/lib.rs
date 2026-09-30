//! PULSE: the parity harness (port of Go package internal/parity).
//!
//! The Go port compared itself with the Python oracle; the Rust port compares
//! itself with the Go oracle. A [`Scenario`] is a scripted list of `pulse`
//! invocations; [`run_both`] plays it against the oracle and the Rust binary,
//! each in a fresh `PULSE_HOME` with the same fixed `PULSE_NOW` clock per step,
//! and returns every difference in stdout, stderr, exit code and a canonical
//! dump of the resulting SQLite store. Tests skip cleanly when no oracle is
//! available.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};
use regex::Regex;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

/// The clock every parity scenario starts at (the one the Go golden tests used).
pub const FIXED_NOW: &str = "2026-09-06T21:00:00Z";

pub const REWRITES: &[(&str, &str)] = &[
    (
        "no self-votes - docs/workflows/pulse.md)",
        "no self-votes - pulse guide)",
    ),
    (
        "docs/evolution/STOP is present: the loop and ratification are paused until a human removes it",
        "STOP is present: the loop and ratification are paused until the owner lifts it",
    ),
    (
        "passed: ratify per docs/workflows/amend.md",
        "passed: ratify with pulse amend apply",
    ),
    (
        "unchecked: no docs/evolution/constitution.md",
        "unchecked: no constitution record",
    ),
    (
        "(pulse pair; docs/workflows/executors.md)",
        "(pulse pair; pulse record show WF-executors)",
    ),
];

pub fn expected(oracle: &str) -> String {
    let mut out = oracle.to_string();
    for (from, to) in REWRITES {
        out = out.replace(from, to);
    }
    out
}

pub fn legacy_agent_listing(text: &str) -> String {
    let presence_column = Regex::new(
        r"(?m)^(\S+[ \t]+\S+[ \t]+\S+[ \t]+)presence[ \t]+(?:online|idle|offline|dead|unknown|legacy|pruned)[ \t]+",
    )
    .unwrap();
    let heartbeat_column = Regex::new(r"last heartbeat [^ \t\r\n]+  ").unwrap();
    let text = presence_column.replace_all(text, "$1");
    heartbeat_column.replace_all(&text, "").to_string()
}

/// DEFAULT_ORACLE is where the Go oracle binary (the last Go `pulse`, built from
/// 28f35b8) lives unless PULSE_GO_ORACLE says otherwise.
pub const DEFAULT_ORACLE: &str = "/tmp/pulse-oracle/pulse";

/// The Go oracle binary: `$PULSE_GO_ORACLE`, else [`DEFAULT_ORACLE`], else
/// `None` (tests skip).
pub fn oracle() -> Option<PathBuf> {
    let p = std::env::var("PULSE_GO_ORACLE")
        .ok()
        .filter(|p| !p.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_ORACLE), PathBuf::from);
    p.is_file().then_some(p)
}

/// One `pulse` invocation of a scenario.
#[derive(Debug, Clone)]
pub struct Step {
    /// Arguments after the binary name; `{home}` and `{project}` are substituted.
    pub args: Vec<String>,
    /// Data written to the process' stdin.
    pub stdin: Option<String>,
    /// Extra environment for this step (on top of PULSE_HOME/PULSE_NOW).
    pub env: BTreeMap<String, String>,
    /// Working directory: `None` = the scenario's project checkout.
    pub cwd: Option<PathBuf>,
    /// Seconds added to [`FIXED_NOW`] for this step's clock.
    pub clock_offset_secs: i64,
}

impl Step {
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Step {
            args: args.into_iter().map(|s| s.as_ref().to_string()).collect(),
            stdin: None,
            env: BTreeMap::new(),
            cwd: None,
            clock_offset_secs: 0,
        }
    }

    pub fn stdin(mut self, s: &str) -> Self {
        self.stdin = Some(s.to_string());
        self
    }

    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.env.insert(k.to_string(), v.to_string());
        self
    }

    pub fn at(mut self, offset_secs: i64) -> Self {
        self.clock_offset_secs = offset_secs;
        self
    }
}

/// A scripted list of steps.
#[derive(Debug, Clone, Default)]
pub struct Scenario {
    pub name: String,
    pub steps: Vec<Step>,
    /// Extra files written into the project checkout before the first step
    /// (path relative to the checkout -> contents).
    pub project_files: BTreeMap<String, String>,
}

impl Scenario {
    pub fn new(name: &str) -> Self {
        Scenario {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    pub fn cmd<I, S>(self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.step(Step::new(args))
    }

    pub fn file(mut self, path: &str, contents: &str) -> Self {
        self.project_files
            .insert(path.to_string(), contents.to_string());
        self
    }
}

/// What one step produced on one side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// One side (oracle or Rust) of a played scenario.
#[derive(Debug)]
pub struct Side {
    pub label: &'static str,
    pub home: PathBuf,
    pub project: PathBuf,
    pub outcomes: Vec<Outcome>,
    pub dump: String,
}

/// A difference between the two sides.
#[derive(Debug, Clone)]
pub struct Diff {
    pub step: Option<usize>,
    pub what: String,
    pub oracle: String,
    pub rust: String,
}

impl std::fmt::Display for Diff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.step {
            Some(i) => writeln!(f, "step {i}: {}", self.what)?,
            None => writeln!(f, "{}", self.what)?,
        }
        writeln!(f, "--- oracle (go)\n{}", self.oracle)?;
        writeln!(f, "+++ rust\n{}", self.rust)
    }
}

/// Plays `scenario` on both binaries, one after the other in the SAME
/// directory, and returns the differences (empty = parity).
///
/// Sharing the directory is what makes paths comparable: the project slug and
/// name derive from the checkout's basename, events store absolute paths, and
/// commands truncate long detail columns, so masking alone cannot line two
/// differently named roots up.
pub fn run_both(scenario: &Scenario, oracle_bin: &Path, rust_bin: &Path) -> Result<Vec<Diff>> {
    let (go, rs) = run_sides(scenario, oracle_bin, rust_bin)?;
    Ok(compare(scenario, &go, &rs))
}

/// Plays `scenario` on both binaries and returns the two sides.
pub fn run_sides(scenario: &Scenario, oracle_bin: &Path, rust_bin: &Path) -> Result<(Side, Side)> {
    let root = tempfile::Builder::new().prefix("pulse-parity-").tempdir()?;
    let go = play_in(root.path(), "oracle", oracle_bin, scenario)?;
    let rs = play_in(root.path(), "rust", rust_bin, scenario)?;
    Ok((go, rs))
}

/// Plays `scenario` on one binary in a fresh temporary root; see [`play_in`].
/// The root is removed when the function returns, so only the captured
/// outcomes and store dump survive.
pub fn play(label: &'static str, bin: &Path, scenario: &Scenario) -> Result<Side> {
    let root = tempfile::Builder::new().prefix("pulse-parity-").tempdir()?;
    play_in(root.path(), label, bin, scenario)
}

/// Plays `scenario` on one binary with `<root>/home` as the PULSE home and a
/// fresh git checkout at `<root>/demo-project` as the project. Anything left
/// in `root` from an earlier play is removed first.
pub fn play_in(root: &Path, label: &'static str, bin: &Path, scenario: &Scenario) -> Result<Side> {
    // the project must live outside the home (the store refuses to sit inside a
    // checkout) and carry the SAME basename on both sides: the project slug and
    // name derive from it and appear in the output being compared
    let home = root.join("home");
    if home.exists() {
        std::fs::remove_dir_all(&home)?;
    }
    std::fs::create_dir_all(&home)?;
    let project = fresh_checkout(&root.join("demo-project"), &scenario.project_files)?;
    let mut outcomes = Vec::with_capacity(scenario.steps.len());
    for step in &scenario.steps {
        outcomes.push(run_step(bin, &home, &project, step)?);
    }
    let db = home.join("pulse.sqlite");
    let dump = if db.is_file() {
        dump_store(&db)?
    } else {
        String::from("<no store>")
    };
    Ok(Side {
        label,
        home,
        project,
        outcomes,
        dump,
    })
}

/// The one commit every parity checkout starts from; fixed identity and dates
/// so both sides see the same hash (`reflect` prints it).
const GIT_IDENTITY: [(&str, &str); 6] = [
    ("GIT_AUTHOR_NAME", "parity"),
    ("GIT_AUTHOR_EMAIL", "parity@example.com"),
    ("GIT_AUTHOR_DATE", "2026-09-06T20:00:00Z"),
    ("GIT_COMMITTER_NAME", "parity"),
    ("GIT_COMMITTER_EMAIL", "parity@example.com"),
    ("GIT_COMMITTER_DATE", "2026-09-06T20:00:00Z"),
];

fn fresh_checkout(project: &Path, files: &BTreeMap<String, String>) -> Result<PathBuf> {
    let project = project.to_path_buf();
    if project.exists() {
        std::fs::remove_dir_all(&project)?;
    }
    std::fs::create_dir_all(&project)?;
    git(&project, ["init", "-q", "-b", "main"])?;
    git(&project, ["config", "user.email", "parity@example.com"])?;
    git(&project, ["config", "user.name", "parity"])?;
    git(&project, ["config", "commit.gpgsign", "false"])?;
    std::fs::write(project.join("README"), "parity\n")?;
    for (rel, contents) in files {
        let p = project.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, contents)?;
    }
    git(&project, ["add", "-A"])?;
    git(&project, ["commit", "-q", "-m", "parity: initial"])?;
    Ok(project)
}

fn git<I, S>(dir: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .envs(GIT_IDENTITY)
        .output()
        .context("running git")?;
    anyhow::ensure!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

fn run_step(bin: &Path, home: &Path, project: &Path, step: &Step) -> Result<Outcome> {
    let now = clock(step.clock_offset_secs);
    let args: Vec<String> = step
        .args
        .iter()
        .map(|a| {
            a.replace("{home}", &home.to_string_lossy())
                .replace("{project}", &project.to_string_lossy())
        })
        .collect();
    let mut cmd = Command::new(bin);
    cmd.args(&args)
        .current_dir(step.cwd.as_deref().unwrap_or(project))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("PULSE_HOME", home)
        .env("PULSE_NOW", &now)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &step.env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", bin.display()))?;
    if let Some(mut stdin) = child.stdin.take()
        && let Some(data) = &step.stdin
    {
        let _ = stdin.write_all(data.as_bytes());
    }
    let out = child.wait_with_output()?;
    Ok(Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: mask_exe(
            &normalize(&String::from_utf8_lossy(&out.stdout), home, project),
            bin,
        ),
        stderr: mask_exe(
            &normalize(&String::from_utf8_lossy(&out.stderr), home, project),
            bin,
        ),
    })
}

/// The scenario clock: [`FIXED_NOW`] plus `offset_secs`.
pub fn clock(offset_secs: i64) -> String {
    // 2026-09-06T21:00:00Z as a unix timestamp
    const BASE: i64 = 1_788_728_400;
    format_utc(BASE + offset_secs)
}

fn format_utc(ts: i64) -> String {
    // civil-from-days (Howard Hinnant); plenty for a test clock
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Replaces machine-specific text so the two sides compare: the home and
/// project paths, and the binary's own version.
pub fn normalize(s: &str, home: &Path, project: &Path) -> String {
    let out = s.replace(&*project.to_string_lossy(), "<PROJECT>");
    let out = out.replace(&*home.to_string_lossy(), "<HOME>");
    mask_random_ids(&mask_versions(&out))
}

/// `os.Executable()` shows up in installed hook commands and its directory in
/// service units (`WorkingDirectory`); each side prints its own binary, so
/// both become `<EXE>` / `<EXEDIR>`. The daemon banner's `pid N` is masked
/// too: two processes never share one.
fn mask_exe(s: &str, bin: &Path) -> String {
    let mut out = mask_pid(s);
    let mut names = vec![bin.to_path_buf()];
    if let Ok(real) = std::fs::canonicalize(bin)
        && real != bin
    {
        names.push(real);
    }
    for name in &names {
        out = out.replace(&*name.to_string_lossy(), "<EXE>");
    }
    for dir in names.iter().filter_map(|n| n.parent()) {
        out = out.replace(&*dir.to_string_lossy(), "<EXEDIR>");
    }
    out
}

fn mask_pid(s: &str) -> String {
    // `pid 123` in text and `"pid":123` in JSON (the daemon heartbeat in meta)
    static PID: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"\bpid \d+\b|(\\?"pid\\?":)\d+"#).unwrap());
    PID.replace_all(s, |c: &regex::Captures| match c.get(1) {
        Some(key) => format!("{}<PID>", key.as_str()),
        None => "pid <PID>".to_string(),
    })
    .into_owned()
}

/// A full 40-hex git object id: the build revision in the daemon heartbeat and
/// any commit of the per-side checkouts, none of which agree across sides.
fn mask_shas(s: &str) -> String {
    static SHA: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\b[0-9a-f]{40}\b").unwrap());
    SHA.replace_all(s, "<SHA>").into_owned()
}

/// The home's ed25519 signing key, generated fresh on each side.
fn mask_pubkeys(s: &str) -> String {
    static KEY: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"ssh-ed25519 AAAA[0-9A-Za-z+/=]+").unwrap());
    KEY.replace_all(s, "ssh-ed25519 <KEY>").into_owned()
}

/// `board.NewID`: `<prefix>-<yyyymmddThhmmssZ>-<6 random hex>` (job, fixture,
/// measure, fleet, cycle ids). The stamp comes from the fixed clock and stays;
/// only the random tail is masked.
fn mask_random_ids(s: &str) -> String {
    static RANDOM_ID: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        // thread names carry the id lowercased (`evolution/job-...t...z-...`)
        Regex::new(r"\b([a-z]+-\d{8}[Tt]\d{6}[Zz])-[0-9a-f]{6}\b").unwrap()
    });
    RANDOM_ID.replace_all(s, "$1-<rand>").into_owned()
}

fn mask_versions(s: &str) -> String {
    // v0.0.0-20260928222421-28f35b8d1694+dirty / dev-28f35b8d1694-dirty; `dev` alone stays
    let mut result = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let mut l = line.to_string();
        for token in line.split(|c: char| c.is_whitespace() || c == '"' || c == ',') {
            if token.starts_with("v0.0.0-") || (token.starts_with("dev-") && token.len() >= 16) {
                l = l.replace(token, "<VERSION>");
            }
        }
        result.push_str(&mask_build_stamp(&l));
    }
    result
}

/// `pulse@<12 hex>` (a stamped build) and `pulse@dev` (an unstamped one) name
/// the build that seeded a record; the two binaries never agree on it.
fn mask_build_stamp(line: &str) -> String {
    const TAG: &str = "pulse@";
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(TAG) {
        let (head, tail) = rest.split_at(at + TAG.len());
        out.push_str(head);
        let rev_len = tail
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_alphanumeric())
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        let rev = &tail[..rev_len];
        if rev == "dev" || (rev.len() == 12 && rev.chars().all(|c| c.is_ascii_hexdigit())) {
            out.push_str("<REV>");
        } else {
            out.push_str(rev);
        }
        rest = &tail[rev_len..];
    }
    out.push_str(rest);
    out
}

/// A canonical text dump of the schema and every user table of the store,
/// rows in rowid order. Both sides run with a fixed clock, so any difference
/// is a real one.
pub fn dump_store(path: &Path) -> Result<String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut out = String::new();
    {
        let mut st = conn.prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?;
        let rows = st.query_map([], |r| {
            Ok(format!(
                "{} {} {}\n{}\n",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })?;
        out.push_str("== schema ==\n");
        for r in rows {
            out.push_str(&r?);
        }
    }
    let tables: Vec<String> = {
        let mut st = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        st.query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    for t in tables {
        // meta is a key/value table whose rows the daemon writes from several
        // threads (Go: goroutines), so its rowid order is not meaningful
        let order = if t == "meta" { "key" } else { "rowid" };
        let mut st = conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY {order}"))?;
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        out.push_str(&format!("== {t} ({}) ==\n", cols.join(", ")));
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(cols.len());
            for (i, col) in cols.iter().enumerate() {
                // a run's wall-clock duration is measured, not played
                if t == "runs" && col == "seconds" {
                    cells.push("<SECONDS>".to_string());
                    continue;
                }
                let cell = match row.get_ref(i)? {
                    ValueRef::Null => "NULL".to_string(),
                    ValueRef::Integer(n) => n.to_string(),
                    ValueRef::Real(f) => format!("{f}"),
                    ValueRef::Text(b) => format!("{:?}", String::from_utf8_lossy(b)),
                    ValueRef::Blob(b) => format!("blob[{}]", b.len()),
                };
                cells.push(cell);
            }
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
    }
    Ok(out)
}

/// Compares two played sides.
pub fn compare(scenario: &Scenario, go: &Side, rs: &Side) -> Vec<Diff> {
    let mut diffs = Vec::new();
    for (i, step) in scenario.steps.iter().enumerate() {
        let (a, b) = (&go.outcomes[i], &rs.outcomes[i]);
        let label = format!("pulse {}", step.args.join(" "));
        if a.code != b.code {
            diffs.push(Diff {
                step: Some(i),
                what: format!("{label}: exit code"),
                oracle: a.code.to_string(),
                rust: b.code.to_string(),
            });
        }
        if a.stdout != b.stdout {
            let (o, r) = first_difference(&a.stdout, &b.stdout);
            diffs.push(Diff {
                step: Some(i),
                what: format!("{label}: stdout"),
                oracle: o,
                rust: r,
            });
        }
        if a.stderr != b.stderr {
            let (o, r) = first_difference(&a.stderr, &b.stderr);
            diffs.push(Diff {
                step: Some(i),
                what: format!("{label}: stderr"),
                oracle: o,
                rust: r,
            });
        }
    }
    let scrub = |side: &Side| {
        mask_pubkeys(&mask_shas(&mask_pid(&mask_random_ids(&mask_versions(
            &side
                .dump
                .replace(&*side.project.to_string_lossy(), "<PROJECT>")
                .replace(&*side.home.to_string_lossy(), "<HOME>"),
        )))))
    };
    let (go_dump, rs_dump) = (scrub(go), scrub(rs));
    if go_dump != rs_dump {
        for (k, (o, r)) in line_hunks(&go_dump, &rs_dump).into_iter().enumerate() {
            diffs.push(Diff {
                step: None,
                what: format!("store dump (hunk {})", k + 1),
                oracle: o,
                rust: r,
            });
        }
    }
    diffs
}

/// Every differing hunk of two texts as (only-in-a, only-in-b) line groups,
/// computed from a longest-common-subsequence alignment of the lines so one
/// extra row does not turn the rest of the dump into a difference.
pub fn line_hunks(a: &str, b: &str) -> Vec<(String, String)> {
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();
    let (n, m) = (la.len(), lb.len());
    // lcs[i][j] = length of the LCS of la[i..] and lb[j..]
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if la[i] == lb[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    let mut flush = |only_a: &mut Vec<String>, only_b: &mut Vec<String>| {
        if !only_a.is_empty() || !only_b.is_empty() {
            hunks.push((only_a.join("\n"), only_b.join("\n")));
            only_a.clear();
            only_b.clear();
        }
    };
    while i < n || j < m {
        if i < n && j < m && la[i] == lb[j] {
            flush(&mut only_a, &mut only_b);
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            only_b.push(format!("{:>5}: {}", j + 1, lb[j]));
            j += 1;
        } else {
            only_a.push(format!("{:>5}: {}", i + 1, la[i]));
            i += 1;
        }
    }
    flush(&mut only_a, &mut only_b);
    hunks
}

/// The first differing lines of two texts with a little context.
pub fn first_difference(a: &str, b: &str) -> (String, String) {
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();
    let n = la.len().max(lb.len());
    for i in 0..n {
        if la.get(i) != lb.get(i) {
            let lo = i.saturating_sub(2);
            let ctx = |v: &[&str]| {
                v.iter()
                    .skip(lo)
                    .take(8)
                    .enumerate()
                    .map(|(k, l)| format!("{:>5}: {l}", lo + k + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return (ctx(&la), ctx(&lb));
        }
    }
    (String::new(), String::new())
}

/// Asserts parity for `scenario`, or skips (returns) when no oracle exists.
pub fn assert_parity(scenario: &Scenario, rust_bin: &Path) {
    let Some(oracle_bin) = oracle() else {
        eprintln!(
            "parity: no Go oracle (set PULSE_GO_ORACLE); skipping {}",
            scenario.name
        );
        return;
    };
    let (go, rs) = run_sides(scenario, &oracle_bin, rust_bin).expect("playing the scenario");
    let diffs = compare(scenario, &go, &rs);
    if !diffs.is_empty() {
        // PULSE_PARITY_DUMP_DIR=<dir> keeps both store dumps of a differing scenario.
        if let Ok(dir) = std::env::var("PULSE_PARITY_DUMP_DIR") {
            for side in [&go, &rs] {
                let path = Path::new(&dir).join(format!("{}-{}.dump", scenario.name, side.label));
                let _ = std::fs::write(path, &side.dump);
            }
        }
        let mut msg = format!(
            "parity: {} difference(s) in {}\n",
            diffs.len(),
            scenario.name
        );
        for d in &diffs {
            msg.push_str(&d.to_string());
        }
        panic!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_fixed_now() {
        assert_eq!(clock(0), FIXED_NOW);
        assert_eq!(clock(61), "2026-09-06T21:01:01Z");
        assert_eq!(clock(3 * 3600), "2026-09-07T00:00:00Z");
        assert_eq!(clock(-21 * 3600), "2026-09-06T00:00:00Z");
    }

    #[test]
    fn normalize_masks_random_id_tails_only() {
        let s = "job-20260906T210000Z-3fa9c1 fixture-20260906T210000Z-00ab12 keep-20260906T210000Z";
        assert_eq!(
            mask_random_ids(s),
            "job-20260906T210000Z-<rand> fixture-20260906T210000Z-<rand> keep-20260906T210000Z"
        );
    }

    #[test]
    fn normalize_masks_build_stamps() {
        assert_eq!(
            mask_build_stamp(
                "seed:pulse@28f35b8d1694 (pulse@dev) pulse@vm-seven pulse@28f35b8d16941"
            ),
            "seed:pulse@<REV> (pulse@<REV>) pulse@vm-seven pulse@28f35b8d16941"
        );
    }

    #[test]
    fn normalize_replaces_paths_and_versions() {
        let home = Path::new("/tmp/h");
        let project = Path::new("/tmp/h-project");
        let s = "store /tmp/h/pulse.sqlite in /tmp/h-project version v0.0.0-20260928222421-28f35b8d1694+dirty\n";
        assert_eq!(
            normalize(s, home, project),
            "store <HOME>/pulse.sqlite in <PROJECT> version <VERSION>\n"
        );
        assert_eq!(
            normalize("dev-28f35b8d1694-dirty\n", home, project),
            "<VERSION>\n"
        );
        assert_eq!(normalize("dev\n", home, project), "dev\n");
    }

    #[test]
    fn first_difference_points_at_the_line() {
        let (a, b) = first_difference("a\nb\nc\n", "a\nB\nc\n");
        assert!(a.contains("2: b"), "{a}");
        assert!(b.contains("2: B"), "{b}");
        assert_eq!(
            first_difference("same", "same"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn masks_pids_shas_and_json_pids() {
        assert_eq!(mask_pid("daemon pid 123 up"), "daemon pid <PID> up");
        assert_eq!(
            mask_pid(r#"{\"at\":\"x\",\"pid\":29017,\"loops\":1}"#),
            r#"{\"at\":\"x\",\"pid\":<PID>,\"loops\":1}"#
        );
        assert_eq!(mask_pid(r#"{"pid":7}"#), r#"{"pid":<PID>}"#);
        assert_eq!(
            mask_shas("rev 28f35b8d16945430794532b2da4aea450f8700a4 short 28f35b8d1694"),
            "rev <SHA> short 28f35b8d1694"
        );
        assert_eq!(
            mask_pubkeys(r#"| "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAvJ+yy/92D=""#),
            r#"| "ssh-ed25519 <KEY>""#
        );
    }

    #[test]
    fn line_hunks_align_around_an_inserted_line() {
        let hunks = line_hunks("a\nb\nc\nd\n", "a\nb\nX\nc\nD\n");
        assert_eq!(hunks.len(), 2, "{hunks:?}");
        assert_eq!(hunks[0], (String::new(), "    3: X".to_string()));
        assert_eq!(hunks[1], ("    4: d".to_string(), "    5: D".to_string()));
        assert!(line_hunks("same\n", "same\n").is_empty());
    }

    #[test]
    fn dump_store_lists_tables_in_rowid_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE t (a INTEGER, b TEXT); INSERT INTO t VALUES (2, 'y'); INSERT INTO t VALUES (1, 'x');",
            )
            .unwrap();
        }
        let d = dump_store(&path).unwrap();
        assert!(d.starts_with("== schema ==\n"), "{d}");
        assert!(d.contains("== t (a, b) ==\n2 | \"y\"\n1 | \"x\"\n"), "{d}");
    }

    #[test]
    fn oracle_plays_a_scenario_when_available() {
        let Some(bin) = oracle() else {
            eprintln!("no oracle; skipping");
            return;
        };
        let sc = Scenario::new("smoke").cmd(["version"]).cmd(["nosuch"]);
        let side = play("oracle", &bin, &sc).unwrap();
        assert_eq!(side.outcomes[0].code, 0);
        assert_eq!(side.outcomes[0].stdout, "<VERSION>\n");
        assert_eq!(side.outcomes[1].code, 2);
        assert!(side.outcomes[1].stderr.contains("unknown command"));
        assert_eq!(side.dump, "<no store>");
    }
}
