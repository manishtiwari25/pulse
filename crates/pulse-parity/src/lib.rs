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
        r"(?m)^(\S+[ \t]+\S+[ \t]+\S+[ \t]+)presence[ \t]+(?:online|idle|offline|unknown|legacy|pruned)[ \t]+",
    )
    .unwrap();
    let heartbeat_column = Regex::new(r"last heartbeat [^ \t\r\n]+  ").unwrap();
    let text = presence_column.replace_all(text, "$1");
    heartbeat_column.replace_all(&text, "").to_string()
}

/// The Go oracle binary: `$PULSE_GO_ORACLE`, else the coordinator's session
/// copy, else `None` (tests skip).
pub fn oracle() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PULSE_GO_ORACLE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let session = PathBuf::from(
        "/Users/work/.copilot/session-state/eb2b69f5-ae4f-4495-904e-534f0b2fbf3e/files/oracle/pulse",
    );
    session.is_file().then_some(session)
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
    pub home: tempfile::TempDir,
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

/// Plays `scenario` on both binaries and returns the differences (empty = parity).
pub fn run_both(scenario: &Scenario, oracle_bin: &Path, rust_bin: &Path) -> Result<Vec<Diff>> {
    let go = play("oracle", oracle_bin, scenario)?;
    let rs = play("rust", rust_bin, scenario)?;
    Ok(compare(scenario, &go, &rs))
}

/// Plays `scenario` on one binary in a fresh home, with a fresh git checkout
/// beside it as the project.
pub fn play(label: &'static str, bin: &Path, scenario: &Scenario) -> Result<Side> {
    let home = tempfile::Builder::new()
        .prefix(&format!("pulse-parity-{label}-"))
        .tempdir()?;
    let project = fresh_checkout(home.path(), &scenario.project_files)?;
    let mut outcomes = Vec::with_capacity(scenario.steps.len());
    for step in &scenario.steps {
        outcomes.push(run_step(bin, home.path(), &project, step)?);
    }
    let db = home.path().join("pulse.sqlite");
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

fn fresh_checkout(home: &Path, files: &BTreeMap<String, String>) -> Result<PathBuf> {
    // the project must live outside the home: the store refuses to sit inside a checkout
    let project = home.parent().unwrap_or(home).join(format!(
        "{}-project",
        home.file_name().unwrap_or_default().to_string_lossy()
    ));
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
        stdout: normalize(&String::from_utf8_lossy(&out.stdout), home, project),
        stderr: normalize(&String::from_utf8_lossy(&out.stderr), home, project),
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

/// `board.NewID`: `<prefix>-<yyyymmddThhmmssZ>-<6 random hex>` (job, fixture,
/// measure, fleet, cycle ids). The stamp comes from the fixed clock and stays;
/// only the random tail is masked.
fn mask_random_ids(s: &str) -> String {
    static RANDOM_ID: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\b([a-z]+-\d{8}T\d{6}Z)-[0-9a-f]{6}\b").unwrap());
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
        let mut st = conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid"))?;
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        out.push_str(&format!("== {t} ({}) ==\n", cols.join(", ")));
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(cols.len());
            for i in 0..cols.len() {
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
        mask_random_ids(&mask_versions(
            &side
                .dump
                .replace(&*side.project.to_string_lossy(), "<PROJECT>")
                .replace(&*side.home.path().to_string_lossy(), "<HOME>"),
        ))
    };
    let (go_dump, rs_dump) = (scrub(go), scrub(rs));
    if go_dump != rs_dump {
        let (o, r) = first_difference(&go_dump, &rs_dump);
        diffs.push(Diff {
            step: None,
            what: "store dump".to_string(),
            oracle: o,
            rust: r,
        });
    }
    diffs
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
    let diffs = run_both(scenario, &oracle_bin, rust_bin).expect("playing the scenario");
    if !diffs.is_empty() {
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
            mask_build_stamp("seed:pulse@28f35b8d1694 (pulse@dev) pulse@vm-seven pulse@28f35b8d16941"),
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
