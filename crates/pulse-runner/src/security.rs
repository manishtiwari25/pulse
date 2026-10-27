//! Dependabot alerts, the daemon's side: one agent keeps an eye on the
//! security alerts of every PULSE-managed repository and fixes them. Once
//! per `dependencies.alert_check_minutes` the daemon reads a project's open
//! alerts through the gh CLI and queues one `task` job per affected
//! manifest, briefed with the exact versions to adopt and the commands that
//! adopt them. The versions come from `release_age`: the newest patched
//! version that is at least `dependencies.min_release_age_days` old and
//! stays within the installed version's compatible range. A fix that needs
//! a major upgrade, an alert with no patched version, an ecosystem the
//! watcher does not read, or alerts switched off on GitHub go to the owner
//! as a finding, once. The watcher's memory per project is the `meta` row
//! `dependabot:<slug>`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::release_age::{
    self, ALERT_CHECK_MINUTES, Candidate, Ecosystem, HttpRegistry, Registry, SECURITY_UPDATES,
    cmp_versions, compat_key,
};
use crate::util::{event_at, must_rows, plus_hours, plus_minutes, trunc};

/// Reads a repository's open alerts: `owner/repo` -> the JSON body gh
/// printed, or the error text (gh's stderr) when it could not. (The
/// lifetime lets a test pass a closure that borrows its counters; a bare
/// `dyn Fn` alias would demand `'static`.)
pub(crate) type AlertFetch<'a> = dyn Fn(&str) -> Result<String, String> + 'a;

/// The source prefix of the jobs this pass queues: `security:<slug>:<manifest>:<alert numbers>`.
pub(crate) const SOURCE_PREFIX: &str = "security:";
/// Above the loop's improve jobs and a lead's sittings: a known vulnerability waits for nobody.
pub(crate) const JOB_PRIORITY: i32 = 90;
pub(crate) const JOB_TIER: i32 = 1;
pub(crate) const BOARD_AREA: &str = "security";
pub(crate) const BOARD_THREAD: &str = "dependabot";
const GH_TIMEOUT: Duration = Duration::from_secs(60);
/// A finished job's branch may still be landing this long; the same alert
/// set is not queued again meanwhile.
const QUIET_HOURS: f64 = 24.0;

/// What the watcher remembers about one project, in `meta`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Watch {
    #[serde(default)]
    checked_at: String,
    #[serde(default)]
    alerts: Vec<i64>,
    /// manifest -> the job last queued for it.
    #[serde(default)]
    jobs: BTreeMap<String, String>,
    /// manifest -> when its youngest patched version is old enough.
    #[serde(default)]
    waiting: BTreeMap<String, String>,
    #[serde(default)]
    disabled_told: bool,
    /// The findings already posted: `eco:<name>`, `unfixable:<number>`, `major:<name>:<version>`.
    #[serde(default)]
    told: Vec<String>,
}

fn meta_key(slug: &str) -> String {
    format!("dependabot:{slug}")
}

fn load_watch(s: &pulse_store::Store, slug: &str) -> Watch {
    match s.meta(&meta_key(slug)) {
        Ok((v, true)) if !v.is_empty() => serde_json::from_str(&v).unwrap_or_default(),
        _ => Watch::default(),
    }
}

fn save_watch(conn: &rusqlite::Connection, slug: &str, w: &Watch) {
    let _ = conn.execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![meta_key(slug), serde_json::to_string(w).unwrap_or_default()],
    );
}

/// `owner/repo` of a GitHub remote (`https://github.com/o/r.git`,
/// `git@github.com:o/r.git`, `ssh://git@github.com/o/r`); `None` for any other remote.
pub(crate) fn github_repo(remote: &str) -> Option<String> {
    let r = remote.trim();
    let rest = if let Some(rest) = r.strip_prefix("git@github.com:") {
        rest
    } else {
        let after_scheme = r.split_once("://").map(|(_, rest)| rest).unwrap_or(r);
        let after_scheme = after_scheme
            .split_once('@')
            .map(|(_, rest)| rest)
            .unwrap_or(after_scheme);
        after_scheme.strip_prefix("github.com/")?
    };
    let path = rest.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// One open Dependabot alert, the fields the watcher reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alert {
    pub(crate) number: i64,
    pub(crate) ecosystem: String,
    pub(crate) name: String,
    pub(crate) manifest: String,
    /// The version Dependabot saw in the manifest, when it is a plain version.
    pub(crate) installed: String,
    /// `dependency.relationship`: `Some(true)` for a direct dependency.
    pub(crate) direct: Option<bool>,
    pub(crate) range: String,
    pub(crate) first_patched: Option<String>,
    pub(crate) severity: String,
    pub(crate) ghsa: String,
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for p in path {
        cur = match cur.get(p) {
            Some(c) => c,
            None => return "",
        };
    }
    cur.as_str().unwrap_or("")
}

/// The alerts in gh's output: one JSON array, the concatenated arrays
/// `--paginate` prints for several pages, or `--slurp`'s array of arrays.
pub(crate) fn parse_alerts(body: &str) -> Result<Vec<Alert>, String> {
    let mut objects = Vec::new();
    let push = |v: Value, objects: &mut Vec<Value>| match v {
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Array(inner) => objects.extend(inner),
                    other => objects.push(other),
                }
            }
        }
        other => objects.push(other),
    };
    let stream = serde_json::Deserializer::from_str(body).into_iter::<Value>();
    for v in stream {
        let v = v.map_err(|e| format!("alerts JSON: {e}"))?;
        push(v, &mut objects);
    }
    let mut out = Vec::new();
    for o in objects {
        if !o.is_object() {
            return Err("alerts JSON: not an array of alerts".into());
        }
        if str_at(&o, &["state"]) != "open" {
            continue;
        }
        let Some(number) = o.get("number").and_then(Value::as_i64) else {
            continue;
        };
        let installed = str_at(&o, &["dependency", "version_requirement"]).trim();
        let installed = if is_plain_version(installed) {
            installed.to_string()
        } else {
            String::new()
        };
        let direct = match str_at(&o, &["dependency", "relationship"]) {
            "direct" => Some(true),
            "transitive" => Some(false),
            _ => None,
        };
        let first_patched = o
            .pointer("/security_vulnerability/first_patched_version/identifier")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        let mut severity = str_at(&o, &["security_advisory", "severity"]).to_string();
        if severity.is_empty() {
            severity = str_at(&o, &["security_vulnerability", "severity"]).to_string();
        }
        out.push(Alert {
            number,
            ecosystem: str_at(&o, &["dependency", "package", "ecosystem"]).to_string(),
            name: str_at(&o, &["dependency", "package", "name"]).to_string(),
            manifest: str_at(&o, &["dependency", "manifest_path"]).to_string(),
            installed,
            direct,
            range: str_at(&o, &["security_vulnerability", "vulnerable_version_range"]).to_string(),
            first_patched,
            severity: severity.to_ascii_lowercase(),
            ghsa: str_at(&o, &["security_advisory", "ghsa_id"]).to_string(),
        });
    }
    Ok(out)
}

fn is_plain_version(v: &str) -> bool {
    let v = v.trim_start_matches('v');
    !v.is_empty()
        && v.chars().next().is_some_and(|c| c.is_ascii_digit())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// Whether `version` lies in a Dependabot range such as `>= 3.0.0, < 3.1.9`
/// or `<= 16.4.0`; a clause the parser does not read counts as matching.
pub(crate) fn in_range(version: &str, range: &str) -> bool {
    use std::cmp::Ordering::*;
    for clause in range.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        let (op, v) = ["<=", ">=", "==", "<", ">", "="]
            .iter()
            .find_map(|op| clause.strip_prefix(op).map(|v| (*op, v.trim())))
            .unwrap_or(("=", clause));
        if v.is_empty() {
            continue;
        }
        let ord = cmp_versions(version, v);
        let holds = match op {
            "<" => ord == Less,
            "<=" => ord != Greater,
            ">" => ord == Greater,
            ">=" => ord != Less,
            _ => ord == Equal,
        };
        if !holds {
            return false;
        }
    }
    true
}

fn severity_rank(severity: &str) -> i32 {
    match severity {
        "critical" => 4,
        "high" => 3,
        "medium" | "moderate" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// The versions of `name` the project's lockfile pins, read through
/// `release_age::lockfile_pins`; the lockfile beside a manifest when the
/// alert names the manifest itself.
fn pinned_versions(root: &str, manifest: &str, name: &str) -> Vec<String> {
    let (dir, file) = match manifest.rsplit_once('/') {
        Some((d, f)) => (d, f),
        None => ("", manifest),
    };
    let lockfiles: Vec<&str> = match file {
        "package.json" => vec!["package-lock.json"],
        "Cargo.toml" => vec!["Cargo.lock"],
        "pyproject.toml" => vec!["uv.lock"],
        "Gemfile" => vec!["Gemfile.lock"],
        other => vec![other],
    };
    for lock in lockfiles {
        let rel = if dir.is_empty() {
            lock.to_string()
        } else {
            format!("{dir}/{lock}")
        };
        let path = std::path::Path::new(root).join(&rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(pins) = release_age::lockfile_pins(&rel, &text) {
            let mut versions: Vec<String> = pins
                .into_iter()
                .filter(|p| p.name == name)
                .map(|p| p.version)
                .collect();
            versions.sort_by(|a, b| cmp_versions(a, b));
            versions.dedup();
            return versions;
        }
    }
    Vec::new()
}

/// Whether `package.json` in `dir` lists `name` itself; `None` without a readable package.json.
fn npm_direct(root: &str, dir: &str, name: &str) -> Option<bool> {
    let path = std::path::Path::new(root).join(dir).join("package.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(
        [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ]
        .iter()
        .any(|k| {
            doc.get(k)
                .and_then(Value::as_object)
                .is_some_and(|m| m.contains_key(name))
        }),
    )
}

fn manifest_dir(manifest: &str) -> String {
    match manifest.rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d.to_string(),
        _ => ".".to_string(),
    }
}

/// One package a job moves.
#[derive(Debug, Clone)]
struct Fix {
    name: String,
    eco: Ecosystem,
    installed: Vec<String>,
    target: Candidate,
    direct: bool,
    /// (alert number, severity, GHSA id), by number.
    alerts: Vec<(i64, String, String)>,
}

impl Fix {
    fn numbers(&self) -> Vec<i64> {
        self.alerts.iter().map(|a| a.0).collect()
    }
    fn rank(&self) -> i32 {
        self.alerts
            .iter()
            .map(|a| severity_rank(&a.1))
            .max()
            .unwrap_or(0)
    }
}

fn numbers_text(numbers: &[i64]) -> String {
    numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn names_text(fixes: &[Fix]) -> String {
    fixes
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn job_source(slug: &str, manifest: &str, numbers: &[i64]) -> String {
    format!(
        "{SOURCE_PREFIX}{slug}:{manifest}:{}",
        numbers
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn job_title(fixes: &[Fix], manifest: &str, numbers: &[i64]) -> String {
    format!(
        "Security: update {} in {} (Dependabot {})",
        names_text(fixes),
        manifest_dir(manifest),
        numbers_text(numbers)
    )
}

/// The worker's instructions: the rule, the versions, the commands, nothing to read.
fn job_brief(
    repo: &str,
    manifest: &str,
    fixes: &[Fix],
    numbers: &[i64],
    min_days: i32,
    verify_cmd: &str,
) -> String {
    let dir = manifest_dir(manifest);
    let eco = fixes[0].eco;
    let mut b: Vec<String> = Vec::new();
    b.push(format!(
        "Dependabot reports {} {} against {manifest} of {repo}. The owner's standing rule: a dependency version is adopted only when it was published at least {min_days} days ago and stays within the installed version's compatible range (same major, same minor below 1.0). The versions below were chosen under that rule: adopt exactly these versions, nothing else. Change no other package beyond what the package manager must move with them, and never run a blanket update.",
        numbers.len(),
        crate::util::plural(numbers.len() as i32, "open alert"),
    ));
    b.push(String::new());
    b.push(format!("Packages ({}):", eco.label()));
    for f in fixes {
        let role = match (eco, f.direct) {
            (Ecosystem::Npm, true) => ", a direct dependency",
            (Ecosystem::Npm, false) => ", transitive",
            _ => "",
        };
        let alerts = f
            .alerts
            .iter()
            .map(|(n, sev, ghsa)| match (sev.is_empty(), ghsa.is_empty()) {
                (true, true) => format!("#{n}"),
                (false, true) => format!("#{n} ({sev})"),
                (true, false) => format!("#{n} ({ghsa})"),
                (false, false) => format!("#{n} ({sev}, {ghsa})"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        b.push(format!(
            "- {} {} -> {} (published {}{role}), fixes {alerts}",
            f.name,
            f.installed.join(" and "),
            f.target.version,
            f.target.published_at
        ));
    }
    b.push(String::new());
    b.push(format!("Commands, run in {dir}:"));
    match eco {
        Ecosystem::Npm => {
            let mut transitive = Vec::new();
            for f in fixes {
                if f.direct {
                    b.push(format!(
                        "  npm install --no-audit --no-fund {}@{}",
                        f.name, f.target.version
                    ));
                } else {
                    transitive.push(f);
                }
            }
            for f in &transitive {
                b.push(format!("  npm update --no-audit --no-fund {}", f.name));
            }
            b.push(format!(
                "  npm ls {}",
                fixes
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            if !transitive.is_empty() {
                let overrides = transitive
                    .iter()
                    .map(|f| format!("\"{}\": \"{}\"", f.name, f.target.version))
                    .collect::<Vec<_>>()
                    .join(", ");
                b.push(format!(
                    "If `npm ls` still shows a version other than the target for a transitive package, add an overrides entry to package.json - \"overrides\": {{{overrides}}} - merged into any overrides block already there, and run `npm install --no-audit --no-fund` again."
                ));
            }
        }
        Ecosystem::Cargo => {
            for f in fixes {
                if f.installed.len() > 1 {
                    for old in &f.installed {
                        b.push(format!(
                            "  cargo update -p {}@{old} --precise {}",
                            f.name, f.target.version
                        ));
                    }
                } else {
                    b.push(format!(
                        "  cargo update -p {} --precise {}",
                        f.name, f.target.version
                    ));
                }
            }
        }
        Ecosystem::PyPi => {
            let file = manifest.rsplit('/').next().unwrap_or(manifest);
            for f in fixes {
                let line = match file {
                    "uv.lock" => format!(
                        "  uv lock --upgrade-package {}=={}",
                        f.name, f.target.version
                    ),
                    "poetry.lock" => format!(
                        "  poetry update {} (then make sure poetry.lock pins {})",
                        f.name, f.target.version
                    ),
                    "Pipfile.lock" => format!(
                        "  pipenv update {} (then make sure Pipfile.lock pins {})",
                        f.name, f.target.version
                    ),
                    _ => format!(
                        "  edit the pin of {} in {file} to {}=={}",
                        f.name, f.name, f.target.version
                    ),
                };
                b.push(line);
            }
        }
        Ecosystem::Go => {
            for f in fixes {
                b.push(format!(
                    "  go get {}@{} && go mod tidy",
                    f.name, f.target.version
                ));
            }
        }
        Ecosystem::RubyGems => {
            for f in fixes {
                b.push(format!(
                    "  bundle update --conservative {} (then make sure Gemfile.lock pins {})",
                    f.name, f.target.version
                ));
            }
        }
    }
    b.push(String::new());
    b.push(
        "If the repository commits a build output of this package (for example a bundled web UI embedded in the binary), rebuild it with the project's build command and commit it too.".into(),
    );
    let check = if verify_cmd.is_empty() {
        "Run the project's check command".to_string()
    } else {
        format!("Run the project's check command (`{verify_cmd}`)")
    };
    b.push(format!(
        "{check}; commit everything as one commit titled `security: update {} (Dependabot {})`; report the changed (name, version) pairs in your SUMMARY line.",
        names_text(fixes),
        numbers_text(numbers)
    ));
    b.join("\n")
}

/// Runs `gh api` for the repository's open alerts, all pages.
fn gh_fetch(repo: &str) -> Result<String, String> {
    let argv = vec![
        "gh".to_string(),
        "api".to_string(),
        "--paginate".to_string(),
        format!("repos/{repo}/dependabot/alerts?state=open&per_page=100"),
    ];
    match crate::proc::run_group(
        &pulse_bus::CancellationToken::new(),
        &argv,
        "",
        &crate::proc::environ(),
        &[],
        GH_TIMEOUT,
    ) {
        Err(e) => Err(format!("gh api: {e}")),
        Ok(out) if out.timeout => Err(format!(
            "gh api did not answer within {}s",
            GH_TIMEOUT.as_secs()
        )),
        Ok(out) if out.exit_code != 0 => Err(format!(
            "gh api exited {}: {}",
            out.exit_code,
            trunc(out.stderr.trim(), 300)
        )),
        Ok(out) => Ok(out.stdout),
    }
}

enum Failure {
    Disabled,
    NotFound,
    Other,
}

fn classify(err: &str) -> Failure {
    let e = err.to_ascii_lowercase();
    if e.contains("dependabot alerts are disabled")
        || e.contains("disabled for this repository")
        || e.contains("http 403")
    {
        Failure::Disabled
    } else if e.contains("http 404") {
        Failure::NotFound
    } else {
        Failure::Other
    }
}

/// Posts a finding for the owner on the project's security thread.
fn tell_owner(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
    summary: String,
    body: String,
) -> Result<String, String> {
    crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.to_string(),
            area: BOARD_AREA.into(),
            thread: BOARD_THREAD.into(),
            summary: trunc(&summary, 236),
            body,
            for_owner: true,
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )
    .map(|p| p.str("id"))
    .map_err(|e| crate::run::err_message(&e))
}

/// The daemon's pass for one project: reads the project's open Dependabot
/// alerts when the interval is up and queues what the rule allows. Returns
/// the daemon's log lines, none when there was nothing to do.
pub(crate) fn tend_dependency_alerts(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    tend_dependency_alerts_with(s, slug, now, actor, &gh_fetch, &HttpRegistry::default())
}

pub(crate) fn tend_dependency_alerts_with(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
    fetch: &AlertFetch,
    reg: &dyn Registry,
) -> Vec<String> {
    if s.setting(SECURITY_UPDATES, "on") != "on" {
        return Vec::new();
    }
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let project = crate::board::project_by_slug(&conn, slug.to_string());
    if project.str("status") != "active" {
        return Vec::new();
    }
    let Some(repo) = github_repo(&project.str("remote")) else {
        return Vec::new();
    };
    let mut watch = load_watch(s, slug);
    let minutes = crate::util::setting_int(s, ALERT_CHECK_MINUTES).max(0);
    if !watch.checked_at.is_empty() && plus_minutes(&watch.checked_at, minutes).as_str() > now {
        return Vec::new();
    }
    let mut lines = Vec::new();
    match fetch(&repo) {
        Ok(body) => match parse_alerts(&body) {
            Ok(alerts) => {
                watch.disabled_told = false;
                let mut pass = Pass {
                    s,
                    conn: &conn,
                    slug,
                    repo: &repo,
                    root: project.str("path"),
                    verify_cmd: project.str("verify_cmd"),
                    now,
                    actor,
                    reg,
                    min_days: release_age::min_release_age_days(s),
                    watch: &mut watch,
                    lines: &mut lines,
                };
                pass.run(alerts);
            }
            Err(e) => lines.push(format!("Dependabot alerts of {repo}: {e}")),
        },
        Err(e) => match classify(&e) {
            Failure::Disabled => {
                if !watch.disabled_told {
                    let summary = format!(
                        "Dependabot alerts are off for {repo}: turn them on under Settings \u{2192} Code security so pulse can watch and fix them"
                    );
                    let body = format!(
                        "`gh api repos/{repo}/dependabot/alerts` answered: {}\n\nWith the alerts on, pulse reads them once per {} minutes (`{ALERT_CHECK_MINUTES}`) and queues one security update job per affected manifest, moving each vulnerable package to the newest patched version that is at least {} days old (`{}`). If the alerts are already on, the gh account pulse uses cannot read them: it needs the `security_events` scope or admin rights on the repository.\n\n`pulse settings set {SECURITY_UPDATES} off` stops the watcher instead.",
                        trunc(e.trim(), 300),
                        minutes,
                        release_age::min_release_age_days(s),
                        release_age::MIN_RELEASE_AGE_DAYS,
                    );
                    match tell_owner(s, slug, now, actor, summary, body) {
                        Ok(post) => {
                            watch.disabled_told = true;
                            event_at(
                                &conn,
                                now,
                                actor,
                                "security.disabled",
                                slug,
                                json!({"repo": repo, "post": post, "error": trunc(e.trim(), 300)}),
                            );
                            lines.push(format!(
                                "Dependabot alerts are off for {repo}; told the owner ({post})"
                            ));
                        }
                        Err(why) => lines.push(format!(
                            "Dependabot alerts are off for {repo}; the owner could not be told: {why}"
                        )),
                    }
                }
            }
            Failure::NotFound => {}
            Failure::Other => lines.push(format!(
                "Dependabot alerts of {repo}: {}",
                trunc(e.trim(), 300)
            )),
        },
    }
    watch.checked_at = now.to_string();
    save_watch(&conn, slug, &watch);
    lines
}

/// One run over a project's alerts.
struct Pass<'a> {
    s: &'a pulse_store::Store,
    conn: &'a rusqlite::Connection,
    slug: &'a str,
    repo: &'a str,
    root: String,
    verify_cmd: String,
    now: &'a str,
    actor: &'a str,
    reg: &'a dyn Registry,
    min_days: i32,
    watch: &'a mut Watch,
    lines: &'a mut Vec<String>,
}

impl Pass<'_> {
    fn run(&mut self, alerts: Vec<Alert>) {
        let numbers: BTreeSet<i64> = alerts.iter().map(|a| a.number).collect();
        let numbers: Vec<i64> = numbers.into_iter().collect();
        if numbers != self.watch.alerts {
            event_at(
                self.conn,
                self.now,
                self.actor,
                "security.alerts",
                self.slug,
                json!({"repo": self.repo, "count": numbers.len(), "numbers": numbers}),
            );
            self.watch.alerts = numbers.clone();
        }
        // Findings about alerts that are no longer open may be posted again
        // should the alerts come back.
        self.watch.told.retain(|t| {
            t.strip_prefix("unfixable:")
                .and_then(|n| n.parse::<i64>().ok())
                .is_none_or(|n| numbers.contains(&n))
        });
        let mut by_manifest: BTreeMap<String, Vec<Alert>> = BTreeMap::new();
        for a in alerts {
            by_manifest.entry(a.manifest.clone()).or_default().push(a);
        }
        let manifests: BTreeSet<String> = by_manifest.keys().cloned().collect();
        self.watch.waiting.retain(|m, _| manifests.contains(m));
        for (manifest, group) in by_manifest {
            self.manifest(&manifest, group);
        }
    }

    fn told(&mut self, key: &str) -> bool {
        if self.watch.told.iter().any(|t| t == key) {
            return true;
        }
        self.watch.told.push(key.to_string());
        false
    }

    fn tell(&mut self, summary: String, body: String) -> Option<String> {
        match tell_owner(self.s, self.slug, self.now, self.actor, summary, body) {
            Ok(post) => Some(post),
            Err(why) => {
                self.lines
                    .push(format!("{}: the owner could not be told: {why}", self.slug));
                None
            }
        }
    }

    fn manifest(&mut self, manifest: &str, alerts: Vec<Alert>) {
        let mut by_name: BTreeMap<String, Vec<Alert>> = BTreeMap::new();
        for a in alerts {
            by_name.entry(a.name.clone()).or_default().push(a);
        }
        let mut fixes: Vec<Fix> = Vec::new();
        // (name, floor version, published, eligible) per package still too young.
        let mut waiting: Vec<(String, String, String, String)> = Vec::new();
        for (name, alerts) in by_name {
            let Some(eco) = Ecosystem::from_dependabot(&alerts[0].ecosystem) else {
                self.unsupported(manifest, &alerts);
                continue;
            };
            let pinned = pinned_versions(&self.root, manifest, &name);
            // Fixable alerts grouped by the compatible range of the copy they hit.
            let mut lines: BTreeMap<(u64, u64), (Vec<String>, Vec<Alert>)> = BTreeMap::new();
            for a in alerts {
                let Some(patched) = a.first_patched.clone() else {
                    self.unfixable(manifest, &a);
                    continue;
                };
                let mut installed: Vec<String> = pinned
                    .iter()
                    .filter(|v| in_range(v, &a.range))
                    .cloned()
                    .collect();
                if installed.is_empty() {
                    installed = pinned.clone();
                }
                if installed.is_empty() {
                    installed = vec![if a.installed.is_empty() {
                        patched.clone()
                    } else {
                        a.installed.clone()
                    }];
                }
                let current = installed
                    .iter()
                    .max_by(|x, y| cmp_versions(x, y))
                    .cloned()
                    .unwrap_or_default();
                if compat_key(&patched) != compat_key(&current) {
                    self.major(manifest, &a, &current, &patched);
                    continue;
                }
                let entry = lines.entry(compat_key(&current)).or_default();
                for v in installed {
                    if !entry.0.contains(&v) {
                        entry.0.push(v);
                    }
                }
                entry.1.push(a);
            }
            for (_, (mut installed, alerts)) in lines {
                installed.sort_by(|a, b| cmp_versions(a, b));
                let current = installed.last().cloned().unwrap_or_default();
                let floor = alerts
                    .iter()
                    .filter_map(|a| a.first_patched.clone())
                    .max_by(|a, b| cmp_versions(a, b))
                    .unwrap_or_default();
                match release_age::newest_eligible(
                    self.s,
                    self.reg,
                    eco,
                    &name,
                    &current,
                    &floor,
                    self.now,
                    self.min_days,
                ) {
                    Ok(Some(target)) => {
                        let direct = match eco {
                            Ecosystem::Npm => {
                                npm_direct(&self.root, &manifest_dir(manifest), &name)
                                    .or(alerts[0].direct)
                                    .unwrap_or(false)
                            }
                            _ => true,
                        };
                        let mut hits: Vec<(i64, String, String)> = alerts
                            .iter()
                            .map(|a| (a.number, a.severity.clone(), a.ghsa.clone()))
                            .collect();
                        hits.sort();
                        fixes.push(Fix {
                            name: name.clone(),
                            eco,
                            installed,
                            target,
                            direct,
                            alerts: hits,
                        });
                    }
                    Ok(None) => {
                        match release_age::published_at(self.s, self.reg, eco, &name, &floor) {
                            Ok(Some(at)) => {
                                let eligible = release_age::eligible_at(&at, self.min_days);
                                self.lines.push(format!(
                                    "{}: {manifest} waits for {name} {floor} to be {} days old (from {eligible})",
                                    self.slug, self.min_days
                                ));
                                waiting.push((name.clone(), floor.clone(), at, eligible));
                            }
                            Ok(None) => self.lines.push(format!(
                                "{}: {manifest}: {} does not list {name} {floor} yet",
                                self.slug,
                                eco.label()
                            )),
                            Err(e) => {
                                self.lines
                                    .push(format!("{}: {manifest}: {name}: {e}", self.slug));
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        self.lines
                            .push(format!("{}: {manifest}: {name}: {e}", self.slug));
                        return;
                    }
                }
            }
        }
        if waiting.is_empty() {
            self.watch.waiting.remove(manifest);
        } else {
            let until = waiting
                .iter()
                .map(|w| w.3.clone())
                .max()
                .unwrap_or_default();
            if self.watch.waiting.get(manifest) != Some(&until) {
                event_at(
                    self.conn,
                    self.now,
                    self.actor,
                    "security.waiting",
                    self.slug,
                    json!({
                        "repo": self.repo,
                        "manifest": manifest,
                        "until": until,
                        "min_days": self.min_days,
                        "packages": waiting.iter().map(|(name, version, published_at, eligible_at)| json!({
                            "name": name, "version": version, "published_at": published_at, "eligible_at": eligible_at,
                        })).collect::<Vec<_>>(),
                    }),
                );
                self.watch.waiting.insert(manifest.to_string(), until);
            }
        }
        if !fixes.is_empty() {
            self.queue(manifest, fixes);
        }
    }

    fn unsupported(&mut self, manifest: &str, alerts: &[Alert]) {
        let eco = alerts[0].ecosystem.clone();
        if self.told(&format!("eco:{}", eco.to_ascii_lowercase())) {
            return;
        }
        let numbers: Vec<i64> = alerts.iter().map(|a| a.number).collect();
        let summary = format!(
            "Dependabot {} in {manifest} of {}: pulse does not update {eco} packages ({}), this one is yours",
            numbers_text(&numbers),
            self.repo,
            alerts[0].name
        );
        let body = format!(
            "The security watcher reads npm, Cargo, pip, Go and RubyGems alerts and queues their updates itself; {eco} it leaves to a human. Open alerts: {}.",
            numbers_text(&numbers)
        );
        if let Some(post) = self.tell(summary, body) {
            event_at(
                self.conn,
                self.now,
                self.actor,
                "security.unsupported",
                self.slug,
                json!({"repo": self.repo, "manifest": manifest, "ecosystem": eco, "numbers": numbers, "post": post}),
            );
            self.lines.push(format!(
                "{}: {manifest}: {eco} alerts are not pulse's to fix; told the owner ({post})",
                self.slug
            ));
        }
    }

    fn unfixable(&mut self, manifest: &str, a: &Alert) {
        if self.told(&format!("unfixable:{}", a.number)) {
            return;
        }
        let summary = format!(
            "Dependabot #{} ({}, {}): no patched version of {} exists yet; pulse keeps watching, replacing or dismissing it is your call",
            a.number, a.severity, a.ghsa, a.name
        );
        let body = format!(
            "{} in {manifest} of {} is vulnerable ({}) and the advisory names no fixed version. Pulse queues the update the moment one is published and at least {} days old.",
            a.name, self.repo, a.range, self.min_days
        );
        if let Some(post) = self.tell(summary, body) {
            event_at(
                self.conn,
                self.now,
                self.actor,
                "security.unfixable",
                self.slug,
                json!({"repo": self.repo, "manifest": manifest, "number": a.number, "package": a.name, "ghsa": a.ghsa, "post": post}),
            );
            self.lines.push(format!(
                "{}: {manifest}: #{} has no patched version; told the owner ({post})",
                self.slug, a.number
            ));
        }
    }

    fn major(&mut self, manifest: &str, a: &Alert, current: &str, patched: &str) {
        if self.told(&format!("major:{}:{patched}", a.name)) {
            return;
        }
        let summary = format!(
            "{} needs {patched} to fix #{}, a major upgrade from {current}: your call",
            a.name, a.number
        );
        let body = format!(
            "Dependabot #{} ({}, {}) in {manifest} of {}: the first patched version of {} is {patched}, outside the compatible range of the installed {current}. Pulse adopts patched versions within the installed major (minor below 1.0) on its own; a major upgrade is a Tier 2 change for a human. Upgrade it by hand or through an ADR, or dismiss the alert on GitHub.",
            a.number, a.severity, a.ghsa, self.repo, a.name
        );
        if let Some(post) = self.tell(summary, body) {
            event_at(
                self.conn,
                self.now,
                self.actor,
                "security.major",
                self.slug,
                json!({"repo": self.repo, "manifest": manifest, "number": a.number, "package": a.name, "from": current, "to": patched, "post": post}),
            );
            self.lines.push(format!(
                "{}: {manifest}: {} needs {patched} for #{}, a major upgrade; told the owner ({post})",
                self.slug, a.name, a.number
            ));
        }
    }

    fn queue(&mut self, manifest: &str, mut fixes: Vec<Fix>) {
        // Alerts a job of this manifest already covers - queued, running, or
        // done within the quiet period (its branch may still be landing) -
        // are not queued again; the job that stays takes the rest.
        let quiet_until = plus_hours(self.now, -QUIET_HOURS);
        let prefix = job_source(self.slug, manifest, &[]);
        let covered: BTreeSet<i64> = must_rows(
            self.conn,
            "SELECT source, status, finished_at FROM jobs WHERE project = ? AND source LIKE ?",
            &[&self.slug, &format!("{SOURCE_PREFIX}{}:%", self.slug)],
        )
        .iter()
        .filter(|j| j.str("source").starts_with(&prefix))
        .filter(|j| {
            matches!(
                j.str("status").as_str(),
                "queued" | "leased" | "running" | "blocked"
            ) || j.str("finished_at") >= quiet_until
        })
        .flat_map(|j| {
            j.str("source")[prefix.len()..]
                .split(',')
                .filter_map(|n| n.parse::<i64>().ok())
                .collect::<Vec<_>>()
        })
        .collect();
        fixes.retain(|f| f.numbers().iter().any(|n| !covered.contains(n)));
        if fixes.is_empty() {
            return;
        }
        fixes.sort_by(|a, b| b.rank().cmp(&a.rank()).then_with(|| a.name.cmp(&b.name)));
        let numbers: BTreeSet<i64> = fixes.iter().flat_map(Fix::numbers).collect();
        let numbers: Vec<i64> = numbers.into_iter().collect();
        let source = job_source(self.slug, manifest, &numbers);
        let title = job_title(&fixes, manifest, &numbers);
        let brief = job_brief(
            self.repo,
            manifest,
            &fixes,
            &numbers,
            self.min_days,
            &self.verify_cmd,
        );
        let (job, created) = match crate::jobs::add_job(
            self.s,
            self.slug.to_string(),
            "task".into(),
            title.clone(),
            brief,
            JOB_TIER,
            JOB_PRIORITY,
            source,
            self.actor.to_string(),
            true,
        ) {
            Ok(v) => v,
            Err(e) => {
                self.lines.push(format!(
                    "{}: {manifest}: {}",
                    self.slug,
                    crate::run::err_message(&e)
                ));
                return;
            }
        };
        let id = job.str("id");
        self.watch.jobs.insert(manifest.to_string(), id.clone());
        if !created {
            return;
        }
        event_at(
            self.conn,
            self.now,
            self.actor,
            "security.queued",
            &id,
            json!({
                "repo": self.repo,
                "slug": self.slug,
                "manifest": manifest,
                "numbers": numbers,
                "packages": fixes.iter().map(|f| json!({
                    "name": f.name,
                    "from": f.installed,
                    "to": f.target.version,
                    "published_at": f.target.published_at,
                    "alerts": f.numbers(),
                })).collect::<Vec<_>>(),
            }),
        );
        self.lines
            .push(format!("queued {id} for {}: {title}", self.slug));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_age::MapRegistry;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::{exec1, set_setting};
    use crate::util::Row;
    use std::cell::Cell;

    const NOW: &str = "2026-10-26T14:00:00Z";
    const REPO: &str = "manishtiwari25/pulse";
    const LOCK: &str = "web/hub/package-lock.json";

    fn setup() -> (TestStore, String) {
        let s = new_store();
        let (_, root) = new_repo(&s);
        exec1(
            &s,
            "UPDATE projects SET remote = 'https://github.com/manishtiwari25/pulse.git', verify_cmd = 'make check' WHERE slug = 'proj'",
            &[],
        );
        let hub = std::path::Path::new(&root).join("web/hub");
        std::fs::create_dir_all(&hub).unwrap();
        std::fs::write(
            hub.join("package-lock.json"),
            r#"{"name":"hub","lockfileVersion":3,"packages":{"":{"name":"hub","version":"0.1.0"},"node_modules/next":{"version":"16.3.8"},"node_modules/fast-uri":{"version":"3.1.8"},"node_modules/hono":{"version":"4.13.7"}}}"#,
        )
        .unwrap();
        std::fs::write(
            hub.join("package.json"),
            r#"{"name":"hub","dependencies":{"next":"16.3.8"},"devDependencies":{"vitest":"^3.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&root).join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"rustls\"\nversion = \"0.23.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        )
        .unwrap();
        (s, root)
    }

    fn registry() -> MapRegistry {
        MapRegistry::default()
            .with(
                Ecosystem::Npm,
                "next",
                &[
                    ("16.3.8", "2026-09-30T10:00:00Z"),
                    ("16.4.1", "2026-10-14T17:43:55Z"),
                    ("17.0.0", "2026-10-01T00:00:00Z"),
                ],
            )
            .with(
                Ecosystem::Npm,
                "fast-uri",
                &[
                    ("3.1.8", "2026-09-01T00:00:00Z"),
                    ("3.1.9", "2026-10-20T12:00:00Z"),
                ],
            )
            .with(
                Ecosystem::Npm,
                "hono",
                &[
                    ("4.13.7", "2026-09-10T00:00:00Z"),
                    ("4.13.11", "2026-10-15T00:00:00Z"),
                ],
            )
            .with(
                Ecosystem::Cargo,
                "rustls",
                &[
                    ("0.23.1", "2026-01-01T00:00:00Z"),
                    ("0.23.5", "2026-10-10T00:00:00Z"),
                ],
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn alert(
        number: i64,
        eco: &str,
        name: &str,
        manifest: &str,
        installed: &str,
        relationship: &str,
        range: &str,
        patched: Option<&str>,
        severity: &str,
    ) -> Value {
        json!({
            "number": number,
            "state": "open",
            "dependency": {
                "package": {"ecosystem": eco, "name": name},
                "manifest_path": manifest,
                "version_requirement": installed,
                "scope": "runtime",
                "relationship": relationship,
            },
            "security_advisory": {
                "ghsa_id": format!("GHSA-{number:04}-test"),
                "severity": severity,
            },
            "security_vulnerability": {
                "package": {"ecosystem": eco, "name": name},
                "severity": severity,
                "vulnerable_version_range": range,
                "first_patched_version": patched.map(|p| json!({"identifier": p})),
            },
        })
    }

    /// The live alert set of manishtiwari25/pulse on 2026-10-26, plus one cargo alert.
    fn live_alerts() -> Vec<Value> {
        let npm = |n, name, inst, rel, range, patched, sev| {
            alert(n, "npm", name, LOCK, inst, rel, range, Some(patched), sev)
        };
        vec![
            npm(
                26,
                "fast-uri",
                "3.1.8",
                "transitive",
                ">= 3.0.0, < 3.1.9",
                "3.1.9",
                "medium",
            ),
            npm(
                25,
                "fast-uri",
                "3.1.8",
                "transitive",
                ">= 3.0.0, < 3.1.9",
                "3.1.9",
                "low",
            ),
            npm(
                24,
                "fast-uri",
                "3.1.8",
                "transitive",
                ">= 3.1.3, < 3.1.9",
                "3.1.9",
                "medium",
            ),
            npm(
                23,
                "fast-uri",
                "3.1.8",
                "transitive",
                ">= 3.0.0, < 3.1.9",
                "3.1.9",
                "low",
            ),
            npm(
                20,
                "next",
                "16.3.8",
                "direct",
                ">= 16.0.0, <= 16.4.0",
                "16.4.1",
                "critical",
            ),
            npm(
                18,
                "next",
                "16.3.8",
                "direct",
                ">= 16.0.0, <= 16.4.0",
                "16.4.1",
                "critical",
            ),
            npm(
                17,
                "next",
                "16.3.8",
                "direct",
                ">= 16.0.0, <= 16.4.0",
                "16.4.1",
                "high",
            ),
            npm(
                16,
                "hono",
                "4.13.7",
                "transitive",
                ">= 4.12.4, < 4.13.11",
                "4.13.11",
                "medium",
            ),
            alert(
                3,
                "cargo",
                "rustls",
                "Cargo.lock",
                "0.23.1",
                "direct",
                "< 0.23.5",
                Some("0.23.5"),
                "high",
            ),
        ]
    }

    fn body(alerts: &[Value]) -> String {
        Value::Array(alerts.to_vec()).to_string()
    }

    fn jobs(s: &pulse_store::Store) -> Vec<Row> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM jobs WHERE source LIKE 'security:%' ORDER BY source",
            &[],
        )
    }

    fn owner_findings(s: &pulse_store::Store) -> Vec<Row> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT summary, body, extra, kind, area, thread FROM posts WHERE project = 'proj' AND area = 'security' ORDER BY id",
            &[],
        )
    }

    fn events(s: &pulse_store::Store, action: &str) -> Vec<Row> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT subject, detail FROM events WHERE action = ? ORDER BY seq",
            &[&action],
        )
    }

    fn watch(s: &pulse_store::Store) -> Watch {
        load_watch(s, "proj")
    }

    #[test]
    fn github_repos_come_from_https_and_ssh_remotes() {
        assert_eq!(
            github_repo("https://github.com/manishtiwari25/pulse.git").as_deref(),
            Some("manishtiwari25/pulse")
        );
        assert_eq!(
            github_repo("git@github.com:lamplitlabs/website.git").as_deref(),
            Some("lamplitlabs/website")
        );
        assert_eq!(
            github_repo("ssh://git@github.com/o/r/").as_deref(),
            Some("o/r")
        );
        assert_eq!(github_repo("https://gitlab.com/o/r.git"), None);
        assert_eq!(github_repo(""), None);
        assert_eq!(github_repo("https://github.com/o"), None);
    }

    #[test]
    fn alerts_parse_from_the_live_shape_and_from_paginated_output() {
        let live = r#"[{"number":26,"state":"open","dependency":{"package":{"ecosystem":"npm","name":"fast-uri"},"manifest_path":"web/hub/package-lock.json","version_requirement":"3.1.8","scope":"development","relationship":"transitive"},"security_advisory":{"ghsa_id":"GHSA-w74x-v6f9-j3xg","cve_id":"CVE-2026-110306","summary":"fast-uri vulnerable to URI scheme spoofing","severity":"medium"},"security_vulnerability":{"package":{"ecosystem":"npm","name":"fast-uri"},"severity":"medium","vulnerable_version_range":">= 3.0.0, < 3.1.9","first_patched_version":{"identifier":"3.1.9"}},"url":"https://api.github.com/repos/manishtiwari25/pulse/dependabot/alerts/26","created_at":"2026-10-26T05:16:08Z","fixed_at":null}]"#;
        let got = parse_alerts(live).unwrap();
        assert_eq!(
            got,
            vec![Alert {
                number: 26,
                ecosystem: "npm".into(),
                name: "fast-uri".into(),
                manifest: LOCK.into(),
                installed: "3.1.8".into(),
                direct: Some(false),
                range: ">= 3.0.0, < 3.1.9".into(),
                first_patched: Some("3.1.9".into()),
                severity: "medium".into(),
                ghsa: "GHSA-w74x-v6f9-j3xg".into(),
            }]
        );
        // Two pages, as `--paginate` prints them; a null first patched version; a closed alert.
        let page2 = json!([
            alert(30, "npm", "hono", LOCK, "^4.0.0", "transitive", "< 5.0.0", None, "low"),
            {"number": 31, "state": "fixed", "dependency": {"package": {"ecosystem": "npm", "name": "x"}}},
        ]);
        let got = parse_alerts(&format!("{live}{page2}")).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].number, 30);
        assert_eq!(got[1].first_patched, None);
        assert!(got[1].installed.is_empty(), "a range is not a version");
        // `--slurp` wraps the pages in one array.
        assert_eq!(parse_alerts(&format!("[{live},{page2}]")).unwrap().len(), 2);
        assert!(parse_alerts("not json").is_err());
        assert!(in_range("16.3.8", ">= 16.0.0, <= 16.4.0"));
        assert!(!in_range("16.4.1", ">= 16.0.0, <= 16.4.0"));
        assert!(in_range("3.1.8", ">= 3.1.3, < 3.1.9"));
        assert!(!in_range("3.1.2", ">= 3.1.3, < 3.1.9"));
        assert!(in_range("0.22.0", "< 0.23.5"));
        assert!(in_range("1.0.0", ""));
    }

    #[test]
    fn queues_one_job_per_manifest_with_the_exact_versions_and_commands() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        let reg = registry();
        let fetch = |repo: &str| {
            assert_eq!(repo, REPO);
            Ok(body(&live_alerts()))
        };
        let lines = tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        let js = jobs(&s);
        assert_eq!(js.len(), 2, "{lines:?}\n{js:?}");
        let (cargo, npm) = (&js[0], &js[1]);
        assert_eq!(
            npm.str("source"),
            "security:proj:web/hub/package-lock.json:16,17,18,20,23,24,25,26"
        );
        assert_eq!(
            npm.str("title"),
            "Security: update next, fast-uri, hono in web/hub (Dependabot #16, #17, #18, #20, #23, #24, #25, #26)"
        );
        assert_eq!(npm.int("priority"), JOB_PRIORITY);
        assert_eq!(npm.int("tier"), 1);
        assert_eq!(npm.str("kind"), "task");
        assert_eq!(npm.str("created_by"), "daemon");
        let brief = npm.str("brief");
        for want in [
            "adopt exactly these versions, nothing else",
            "at least 3 days ago",
            "- next 16.3.8 -> 16.4.1 (published 2026-10-14T17:43:55Z, a direct dependency), fixes #17 (high, GHSA-0017-test), #18 (critical, GHSA-0018-test), #20 (critical, GHSA-0020-test)",
            "- fast-uri 3.1.8 -> 3.1.9 (published 2026-10-20T12:00:00Z, transitive), fixes #23 (low, GHSA-0023-test), #24 (medium, GHSA-0024-test), #25 (low, GHSA-0025-test), #26 (medium, GHSA-0026-test)",
            "- hono 4.13.7 -> 4.13.11 (published 2026-10-15T00:00:00Z, transitive), fixes #16 (medium, GHSA-0016-test)",
            "Commands, run in web/hub:",
            "\n  npm install --no-audit --no-fund next@16.4.1\n",
            "\n  npm update --no-audit --no-fund fast-uri\n",
            "\n  npm update --no-audit --no-fund hono\n",
            "\n  npm ls next fast-uri hono\n",
            "\"overrides\": {\"fast-uri\": \"3.1.9\", \"hono\": \"4.13.11\"}",
            "rebuild it with the project's build command and commit it too",
            "Run the project's check command (`make check`); commit everything as one commit titled `security: update next, fast-uri, hono (Dependabot #16, #17, #18, #20, #23, #24, #25, #26)`",
        ] {
            assert!(brief.contains(want), "brief lacks {want:?}:\n{brief}");
        }
        assert!(
            !brief.contains("npm install --no-audit --no-fund fast-uri"),
            "a transitive package is not installed directly:\n{brief}"
        );
        assert_eq!(cargo.str("source"), "security:proj:Cargo.lock:3");
        assert_eq!(
            cargo.str("title"),
            "Security: update rustls in . (Dependabot #3)"
        );
        assert!(
            cargo
                .str("brief")
                .contains("Commands, run in .:\n  cargo update -p rustls --precise 0.23.5"),
            "{}",
            cargo.str("brief")
        );
        // Both are the owner's work in the lease order.
        for j in &js {
            let n = count(
                &s,
                &format!(
                    "SELECT COUNT(*) AS n FROM jobs j WHERE j.id = ? AND {}",
                    crate::jobs::OWNER_DRIVEN_SQL
                ),
                &[&j.str("id")],
            );
            assert_eq!(n, 1, "{} is not owner-driven", j.str("source"));
        }
        assert!(
            lines.iter().any(|l| l.contains(&format!(
                "queued {} for proj: Security: update next, fast-uri, hono in web/hub",
                npm.str("id")
            ))),
            "{lines:?}"
        );
        let w = watch(&s);
        assert_eq!(w.checked_at, NOW);
        assert_eq!(w.alerts, vec![3, 16, 17, 18, 20, 23, 24, 25, 26]);
        assert_eq!(w.jobs.get(LOCK), Some(&npm.str("id")));
        assert_eq!(w.jobs.get("Cargo.lock"), Some(&cargo.str("id")));
        assert!(w.waiting.is_empty());
        let alerts = events(&s, "security.alerts");
        assert_eq!(alerts.len(), 1);
        assert!(
            alerts[0].str("detail").contains("\"count\": 9"),
            "{alerts:?}"
        );
        let queued = events(&s, "security.queued");
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[1].str("subject"), npm.str("id"));
        assert!(
            queued[1].str("detail").contains("\"to\": \"16.4.1\""),
            "{}",
            queued[1].str("detail")
        );
        assert!(owner_findings(&s).is_empty());
    }

    #[test]
    fn a_check_inside_the_interval_is_skipped_and_the_same_alerts_queue_no_second_job() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        let reg = registry();
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(body(&live_alerts()))
        };
        tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        assert_eq!(calls.get(), 1);
        assert_eq!(jobs(&s).len(), 2);
        // Inside the hour: no fetch at all.
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 30), "daemon", &fetch, &reg);
        assert_eq!(calls.get(), 1, "{lines:?}");
        assert!(lines.is_empty(), "{lines:?}");
        // After it: the same alerts, the jobs still queued, nothing new.
        let later = plus_minutes(NOW, 61);
        let lines = tend_dependency_alerts_with(&s, "proj", &later, "daemon", &fetch, &reg);
        assert_eq!(calls.get(), 2);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(jobs(&s).len(), 2);
        assert_eq!(
            events(&s, "security.alerts").len(),
            1,
            "an unchanged set is not an event"
        );
        assert_eq!(watch(&s).checked_at, later);
        // A job done less than a day ago holds its source while the branch lands.
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ? WHERE source LIKE 'security:%'",
            &[&later],
        );
        tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 122), "daemon", &fetch, &reg);
        assert_eq!(jobs(&s).len(), 2);
        // A day later the alerts are still open: the job is queued again.
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_hours(NOW, 27.0), "daemon", &fetch, &reg);
        assert_eq!(jobs(&s).len(), 4, "{lines:?}");
    }

    #[test]
    fn a_patched_version_younger_than_the_rule_waits() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        // fast-uri 3.1.9 came out yesterday; next 16.4.1 is old enough.
        let reg = registry().with(
            Ecosystem::Npm,
            "fast-uri",
            &[
                ("3.1.8", "2026-09-01T00:00:00Z"),
                ("3.1.9", "2026-10-25T09:00:00Z"),
            ],
        );
        let alerts: Vec<Value> = live_alerts()
            .into_iter()
            .filter(|a| matches!(a["number"].as_i64(), Some(20 | 23)))
            .collect();
        let fetch = |_: &str| Ok(body(&alerts));
        let lines = tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        assert!(
            lines.iter().any(|l| l
                .contains("waits for fast-uri 3.1.9 to be 3 days old (from 2026-10-28T09:00:00Z)")),
            "{lines:?}"
        );
        let js = jobs(&s);
        assert_eq!(js.len(), 1, "{js:?}");
        assert_eq!(js[0].str("source"), format!("security:proj:{LOCK}:20"));
        assert_eq!(
            js[0].str("title"),
            "Security: update next in web/hub (Dependabot #20)"
        );
        assert!(!js[0].str("brief").contains("fast-uri"));
        let waiting = events(&s, "security.waiting");
        assert_eq!(waiting.len(), 1);
        assert!(
            waiting[0]
                .str("detail")
                .contains("\"until\": \"2026-10-28T09:00:00Z\""),
            "{waiting:?}"
        );
        assert_eq!(
            watch(&s).waiting.get(LOCK).map(String::as_str),
            Some("2026-10-28T09:00:00Z")
        );
        // The next check: same date, no second event.
        tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 61), "daemon", &fetch, &reg);
        assert_eq!(events(&s, "security.waiting").len(), 1);
        assert_eq!(jobs(&s).len(), 1);
        // With the rule off the young version is taken; next, still queued
        // under the first job, is not bumped a second time.
        set_setting(&s, release_age::MIN_RELEASE_AGE_DAYS, "0");
        let lines = tend_dependency_alerts_with(
            &s,
            "proj",
            &plus_minutes(NOW, 122),
            "daemon",
            &fetch,
            &reg,
        );
        let js = jobs(&s);
        assert_eq!(js.len(), 2, "{lines:?}");
        assert_eq!(js[1].str("source"), format!("security:proj:{LOCK}:23"));
        assert_eq!(
            js[1].str("title"),
            "Security: update fast-uri in web/hub (Dependabot #23)"
        );
        assert!(js[1].str("brief").contains("fast-uri 3.1.8 -> 3.1.9"));
        assert!(
            !js[1].str("brief").contains("next"),
            "{}",
            js[1].str("brief")
        );
        assert!(watch(&s).waiting.is_empty());
        // Once both jobs are long done and the alerts are still open, the
        // whole set is queued again as one job.
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ? WHERE source LIKE 'security:%'",
            &[&plus_minutes(NOW, 130)],
        );
        let later = plus_hours(NOW, 30.0);
        tend_dependency_alerts_with(&s, "proj", &later, "daemon", &fetch, &reg);
        let js = jobs(&s);
        assert_eq!(js.len(), 3, "{js:?}");
        assert_eq!(js[1].str("source"), format!("security:proj:{LOCK}:20,23"));
    }

    #[test]
    fn a_major_jump_and_an_unfixable_alert_go_to_the_owner_once() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        let reg = registry();
        let alerts = vec![
            alert(
                20,
                "npm",
                "next",
                LOCK,
                "16.3.8",
                "direct",
                ">= 16.0.0, < 17.0.0",
                Some("17.0.0"),
                "critical",
            ),
            alert(
                30,
                "npm",
                "hono",
                LOCK,
                "4.13.7",
                "transitive",
                "< 5.0.0",
                None,
                "low",
            ),
            alert(
                40,
                "maven",
                "org.apache:thing",
                "pom.xml",
                "1.0",
                "direct",
                "< 1.1",
                Some("1.1"),
                "high",
            ),
        ];
        let fetch = |_: &str| Ok(body(&alerts));
        let lines = tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        assert!(jobs(&s).is_empty(), "{lines:?}");
        let posts = owner_findings(&s);
        assert_eq!(posts.len(), 3, "{posts:?}");
        for p in &posts {
            assert_eq!(p.str("kind"), "finding");
            assert_eq!(p.str("thread"), BOARD_THREAD);
            assert!(p.str("extra").contains("for_owner"), "{p:?}");
            assert!(p.str("summary").chars().count() <= 240);
        }
        let summaries = posts.iter().map(|p| p.str("summary")).collect::<Vec<_>>();
        assert!(
            summaries.iter().any(
                |t| t == "next needs 17.0.0 to fix #20, a major upgrade from 16.3.8: your call"
            ),
            "{summaries:?}"
        );
        assert!(
            summaries.iter().any(|t| t.starts_with(
                "Dependabot #30 (low, GHSA-0030-test): no patched version of hono exists yet"
            )),
            "{summaries:?}"
        );
        assert!(
            summaries
                .iter()
                .any(|t| t.contains("pulse does not update maven packages")),
            "{summaries:?}"
        );
        assert_eq!(events(&s, "security.major").len(), 1);
        assert_eq!(events(&s, "security.unfixable").len(), 1);
        assert_eq!(events(&s, "security.unsupported").len(), 1);
        assert_eq!(lines.len(), 3, "{lines:?}");
        // Told once: the next check posts nothing.
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 61), "daemon", &fetch, &reg);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(owner_findings(&s).len(), 3);
        assert_eq!(events(&s, "security.major").len(), 1);
    }

    #[test]
    fn disabled_alerts_tell_the_owner_once() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        let reg = MapRegistry::default();
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Err("gh: Dependabot alerts are disabled for this repository. (HTTP 403)".to_string())
        };
        let lines = tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        let posts = owner_findings(&s);
        assert_eq!(posts.len(), 1, "{lines:?}");
        assert_eq!(
            posts[0].str("summary"),
            format!(
                "Dependabot alerts are off for {REPO}: turn them on under Settings \u{2192} Code security so pulse can watch and fix them"
            )
        );
        assert!(posts[0].str("extra").contains("for_owner"));
        assert_eq!(events(&s, "security.disabled").len(), 1);
        assert!(watch(&s).disabled_told);
        assert_eq!(lines.len(), 1, "{lines:?}");
        // Next interval: read again, told nothing more.
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 61), "daemon", &fetch, &reg);
        assert_eq!(calls.get(), 2);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(owner_findings(&s).len(), 1);
        assert_eq!(events(&s, "security.disabled").len(), 1);
        // A repository gh cannot find is skipped without a word; any other error is one line.
        let gone = |_: &str| Err("gh: Not Found (HTTP 404)".to_string());
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 122), "daemon", &gone, &reg);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(watch(&s).checked_at, plus_minutes(NOW, 122));
        let down = |_: &str| Err("gh api did not answer within 60s".to_string());
        let lines =
            tend_dependency_alerts_with(&s, "proj", &plus_minutes(NOW, 183), "daemon", &down, &reg);
        assert_eq!(
            lines,
            vec![format!(
                "Dependabot alerts of {REPO}: gh api did not answer within 60s"
            )]
        );
        assert_eq!(owner_findings(&s).len(), 1);
    }

    #[test]
    fn the_watch_is_off_when_the_setting_says_so_or_the_project_is_not_on_github() {
        let _serial = crate::test_helpers::serial();
        let (s, _root) = setup();
        let reg = registry();
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(body(&live_alerts()))
        };
        set_setting(&s, SECURITY_UPDATES, "off");
        assert!(tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg).is_empty());
        assert_eq!(calls.get(), 0);
        assert!(watch(&s).checked_at.is_empty());
        set_setting(&s, SECURITY_UPDATES, "on");
        exec1(
            &s,
            "UPDATE projects SET remote = 'https://gitlab.com/o/r.git' WHERE slug = 'proj'",
            &[],
        );
        assert!(tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg).is_empty());
        exec1(
            &s,
            "UPDATE projects SET remote = 'git@github.com:o/r.git', status = 'inactive' WHERE slug = 'proj'",
            &[],
        );
        assert!(tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg).is_empty());
        assert_eq!(calls.get(), 0);
        exec1(
            &s,
            "UPDATE projects SET status = 'active' WHERE slug = 'proj'",
            &[],
        );
        tend_dependency_alerts_with(&s, "proj", NOW, "daemon", &fetch, &reg);
        assert_eq!(calls.get(), 1);
        assert_eq!(jobs(&s).len(), 2);
    }
}
