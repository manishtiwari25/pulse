//! Release age: when a dependency version was published, and whether it is
//! old enough to adopt. The owner's rule (`dependencies.min_release_age_days`,
//! one setting for every project) is that a version younger than N days is
//! never taken, whatever asks for it: a security update waits for the patched
//! version to age, and a branch that bumps a lockfile is held until every
//! version it pins is that old. The dates come from each ecosystem's registry
//! and are cached in `meta` once known, since a publish date never changes.

// Consumed by the security watcher and the landing; the allowance goes with their integration.
#![allow(dead_code)]

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::Value;

pub(crate) const MIN_RELEASE_AGE_DAYS: &str = "dependencies.min_release_age_days";
pub(crate) const SECURITY_UPDATES: &str = "dependencies.security_updates";
pub(crate) const ALERT_CHECK_MINUTES: &str = "dependencies.alert_check_minutes";

const REGISTRY_TIMEOUT: Duration = Duration::from_secs(20);
const USER_AGENT: &str = "pulse (https://github.com/manishtiwari25/pulse)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Ecosystem {
    Npm,
    Cargo,
    PyPi,
    Go,
    RubyGems,
}

impl Ecosystem {
    /// The registry a lockfile's pins come from, by the lockfile's file name.
    pub(crate) fn from_lockfile(path: &str) -> Option<Self> {
        match path.rsplit('/').next().unwrap_or(path) {
            "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb" => {
                Some(Self::Npm)
            }
            "Cargo.lock" => Some(Self::Cargo),
            "uv.lock" | "poetry.lock" | "Pipfile.lock" => Some(Self::PyPi),
            "go.sum" => Some(Self::Go),
            "Gemfile.lock" => Some(Self::RubyGems),
            _ => None,
        }
    }

    /// The ecosystem names GitHub's Dependabot alerts use.
    pub(crate) fn from_dependabot(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "npm" => Some(Self::Npm),
            "cargo" | "rust" => Some(Self::Cargo),
            "pip" | "pypi" | "python" => Some(Self::PyPi),
            "go" | "gomod" | "go_modules" => Some(Self::Go),
            "rubygems" | "bundler" | "gem" => Some(Self::RubyGems),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Cargo => "crates.io",
            Self::PyPi => "PyPI",
            Self::Go => "Go proxy",
            Self::RubyGems => "RubyGems",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Cargo => "cargo",
            Self::PyPi => "pypi",
            Self::Go => "go",
            Self::RubyGems => "rubygems",
        }
    }
}

/// Where publish dates come from: the registries over HTTP for the daemon, a
/// map for tests.
pub(crate) trait Registry {
    /// Every released version of `name`, prereleases and yanked ones left out,
    /// in no particular order.
    fn versions(&self, eco: Ecosystem, name: &str) -> Result<Vec<String>, String>;
    /// When `version` of `name` was published, as `2026-10-14T17:43:55Z`;
    /// `None` when the registry does not list that version.
    fn published_at(
        &self,
        eco: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<Option<String>, String>;
}

/// A fixed set of releases, for tests: (ecosystem, name) -> [(version, published)].
#[derive(Default)]
pub(crate) struct MapRegistry(pub(crate) BTreeMap<(Ecosystem, String), Vec<(String, String)>>);

impl MapRegistry {
    pub(crate) fn with(mut self, eco: Ecosystem, name: &str, releases: &[(&str, &str)]) -> Self {
        self.0.insert(
            (eco, name.to_string()),
            releases
                .iter()
                .map(|(v, at)| (v.to_string(), at.to_string()))
                .collect(),
        );
        self
    }
}

impl Registry for MapRegistry {
    fn versions(&self, eco: Ecosystem, name: &str) -> Result<Vec<String>, String> {
        match self.0.get(&(eco, name.to_string())) {
            Some(r) => Ok(r.iter().map(|(v, _)| v.clone()).collect()),
            None => Err(format!("{name} is not on {}", eco.label())),
        }
    }
    fn published_at(
        &self,
        eco: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<Option<String>, String> {
        let releases = self
            .0
            .get(&(eco, name.to_string()))
            .ok_or_else(|| format!("{name} is not on {}", eco.label()))?;
        Ok(releases
            .iter()
            .find(|(v, _)| v == version)
            .map(|(_, at)| at.clone()))
    }
}

/// The public registries, one HTTP document per package per process.
#[derive(Default)]
pub(crate) struct HttpRegistry {
    docs: RefCell<BTreeMap<(Ecosystem, String), Result<BTreeMap<String, String>, String>>>,
}

static CLIENT: LazyLock<Option<reqwest::blocking::Client>> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REGISTRY_TIMEOUT)
        .build()
        .ok()
});

fn fetch_json(url: &str) -> Result<Value, String> {
    let client = CLIENT.as_ref().ok_or("no HTTP client")?;
    let resp = client.get(url).send().map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Ok(Value::Null);
    }
    if !status.is_success() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    resp.json::<Value>().map_err(|e| format!("{url}: {e}"))
}

fn fetch_text(url: &str) -> Result<Option<String>, String> {
    let client = CLIENT.as_ref().ok_or("no HTTP client")?;
    let resp = client.get(url).send().map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 404 || status.as_u16() == 410 {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    resp.text().map(Some).map_err(|e| format!("{url}: {e}"))
}

/// `2026-10-14T17:43:55.246Z`, `...+00:00` and the like, as the store writes times.
pub(crate) fn normalize_time(raw: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|t| crate::util::format_iso(t.with_timezone(&chrono::Utc)))
}

impl HttpRegistry {
    /// version -> published, the whole document of one package, fetched once.
    fn doc(&self, eco: Ecosystem, name: &str) -> Result<BTreeMap<String, String>, String> {
        if let Some(d) = self.docs.borrow().get(&(eco, name.to_string())) {
            return d.clone();
        }
        let fetched = self.fetch_doc(eco, name);
        self.docs
            .borrow_mut()
            .insert((eco, name.to_string()), fetched.clone());
        fetched
    }

    fn fetch_doc(&self, eco: Ecosystem, name: &str) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        match eco {
            Ecosystem::Npm => {
                let url = format!("https://registry.npmjs.org/{}", name.replace('/', "%2F"));
                let doc = fetch_json(&url)?;
                if doc.is_null() {
                    return Err(format!("{name} is not on npm"));
                }
                if let Some(times) = doc.get("time").and_then(Value::as_object) {
                    for (v, at) in times {
                        if v == "created" || v == "modified" || is_prerelease(v) {
                            continue;
                        }
                        if let Some(at) = at.as_str().and_then(normalize_time) {
                            out.insert(v.clone(), at);
                        }
                    }
                }
            }
            Ecosystem::Cargo => {
                let url = format!("https://crates.io/api/v1/crates/{name}/versions");
                let doc = fetch_json(&url)?;
                if doc.is_null() {
                    return Err(format!("{name} is not on crates.io"));
                }
                for v in doc
                    .get("versions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if v.get("yanked").and_then(Value::as_bool).unwrap_or(false) {
                        continue;
                    }
                    let (Some(num), Some(at)) = (
                        v.get("num").and_then(Value::as_str),
                        v.get("created_at").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    if is_prerelease(num) {
                        continue;
                    }
                    if let Some(at) = normalize_time(at) {
                        out.insert(num.to_string(), at);
                    }
                }
            }
            Ecosystem::PyPi => {
                let url = format!("https://pypi.org/pypi/{name}/json");
                let doc = fetch_json(&url)?;
                if doc.is_null() {
                    return Err(format!("{name} is not on PyPI"));
                }
                if let Some(releases) = doc.get("releases").and_then(Value::as_object) {
                    for (v, files) in releases {
                        if is_prerelease(v) {
                            continue;
                        }
                        let earliest = files
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|f| !f.get("yanked").and_then(Value::as_bool).unwrap_or(false))
                            .filter_map(|f| {
                                f.get("upload_time_iso_8601")
                                    .and_then(Value::as_str)
                                    .and_then(normalize_time)
                            })
                            .min();
                        if let Some(at) = earliest {
                            out.insert(v.clone(), at);
                        }
                    }
                }
            }
            Ecosystem::RubyGems => {
                let url = format!("https://rubygems.org/api/v1/versions/{name}.json");
                let doc = fetch_json(&url)?;
                if doc.is_null() {
                    return Err(format!("{name} is not on RubyGems"));
                }
                for v in doc.as_array().into_iter().flatten() {
                    if v.get("prerelease")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let (Some(num), Some(at)) = (
                        v.get("number").and_then(Value::as_str),
                        v.get("created_at").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    if is_prerelease(num) {
                        continue;
                    }
                    // Platform builds (x86_64-linux) share the version's first publish time.
                    if let Some(at) = normalize_time(at) {
                        let keep = out.get(num).is_none_or(|have| at < *have);
                        if keep {
                            out.insert(num.to_string(), at);
                        }
                    }
                }
            }
            Ecosystem::Go => {
                let module = go_escape(name);
                let list = fetch_text(&format!("https://proxy.golang.org/{module}/@v/list"))?
                    .ok_or_else(|| format!("{name} is not on the Go proxy"))?;
                for v in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    if is_prerelease(v) {
                        continue;
                    }
                    // Dates come one request per version; `published_at` fetches them on demand.
                    out.insert(v.to_string(), String::new());
                }
            }
        }
        Ok(out)
    }
}

/// Upper-case letters in a Go module path are escaped as `!x` on the proxy.
fn go_escape(module: &str) -> String {
    let mut out = String::new();
    for c in module.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

impl Registry for HttpRegistry {
    fn versions(&self, eco: Ecosystem, name: &str) -> Result<Vec<String>, String> {
        Ok(self.doc(eco, name)?.keys().cloned().collect())
    }
    fn published_at(
        &self,
        eco: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<Option<String>, String> {
        let doc = self.doc(eco, name)?;
        let Some(at) = doc.get(version) else {
            return Ok(None);
        };
        if !at.is_empty() {
            return Ok(Some(at.clone()));
        }
        let url = format!(
            "https://proxy.golang.org/{}/@v/{version}.info",
            go_escape(name)
        );
        let Some(text) = fetch_text(&url)? else {
            return Ok(None);
        };
        let info: Value = serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))?;
        Ok(info
            .get("Time")
            .and_then(Value::as_str)
            .and_then(normalize_time))
    }
}

fn cache_key(eco: Ecosystem, name: &str, version: &str) -> String {
    format!("release_age:{}:{name}:{version}", eco.key())
}

/// When `version` of `name` was published, through the `meta` cache: the
/// registry is asked once per version per store.
pub(crate) fn published_at(
    s: &pulse_store::Store,
    reg: &dyn Registry,
    eco: Ecosystem,
    name: &str,
    version: &str,
) -> Result<Option<String>, String> {
    let key = cache_key(eco, name, version);
    if let Ok((v, true)) = s.meta(&key) {
        if !v.is_empty() {
            return Ok(Some(v));
        }
    }
    let at = reg.published_at(eco, name, version)?;
    if let (Some(at), Ok(conn)) = (&at, s.conn()) {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, at],
        );
    }
    Ok(at)
}

/// Days from `published_at` to `now`, fractional; negative for a clock ahead of the registry.
pub(crate) fn age_days(published_at: &str, now: &str) -> f64 {
    let a = crate::util::must_iso(published_at);
    let b = crate::util::must_iso(now);
    (b - a).num_seconds() as f64 / 86_400.0
}

/// The first moment `published_at` counts as `min_days` old.
pub(crate) fn eligible_at(published_at: &str, min_days: i32) -> String {
    crate::util::plus_hours(published_at, 24.0 * min_days.max(0) as f64)
}

/// The owner's minimum age, in days; 0 turns the rule off.
pub(crate) fn min_release_age_days(s: &pulse_store::Store) -> i32 {
    crate::util::setting_int(s, MIN_RELEASE_AGE_DAYS).max(0)
}

/// `1.2.3-rc.1`, `1.2.3rc1`, `v1.2.3-beta` and the like.
pub(crate) fn is_prerelease(version: &str) -> bool {
    let v = version.trim_start_matches('v');
    let v = v.split('+').next().unwrap_or(v);
    if v.contains('-') {
        return true;
    }
    let mut seen_digit = false;
    for c in v.chars() {
        if c.is_ascii_digit() {
            seen_digit = true;
        } else if c.is_ascii_alphabetic() && seen_digit {
            return true;
        }
    }
    false
}

fn numeric_parts(version: &str) -> (Vec<u64>, String) {
    let v = version.trim_start_matches('v');
    let v = v.split('+').next().unwrap_or(v);
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c.to_string(), p.to_string()),
        None => {
            let idx = v
                .char_indices()
                .find(|(i, c)| c.is_ascii_alphabetic() && *i > 0)
                .map(|(i, _)| i);
            match idx {
                Some(i) => (v[..i].to_string(), v[i..].to_string()),
                None => (v.to_string(), String::new()),
            }
        }
    };
    let nums = core
        .split('.')
        .map(|p| p.trim_end_matches(|c: char| !c.is_ascii_digit()))
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    (nums, pre)
}

/// Dotted numeric order with a prerelease sorting below its release.
pub(crate) fn cmp_versions(a: &str, b: &str) -> Ordering {
    let (na, pa) = numeric_parts(a);
    let (nb, pb) = numeric_parts(b);
    let n = na.len().max(nb.len());
    for i in 0..n {
        let x = na.get(i).copied().unwrap_or(0);
        let y = nb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    match (pa.is_empty(), pb.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => pa.cmp(&pb),
    }
}

/// The part of a version a compatible update keeps: the major, and for a
/// `0.x` version the minor as well (the caret rule).
pub(crate) fn compat_key(version: &str) -> (u64, u64) {
    let (nums, _) = numeric_parts(version);
    let major = nums.first().copied().unwrap_or(0);
    if major == 0 {
        (0, nums.get(1).copied().unwrap_or(0))
    } else {
        (major, 0)
    }
}

/// A version to adopt with its publish time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) version: String,
    pub(crate) published_at: String,
}

/// The newest version of `name` compatible with `current` (same major, same
/// minor for `0.x`), at least `floor`, and published `min_days` or more before
/// `now`. `None` when nothing qualifies yet; `Err` when the registry could not
/// answer.
pub(crate) fn newest_eligible(
    s: &pulse_store::Store,
    reg: &dyn Registry,
    eco: Ecosystem,
    name: &str,
    current: &str,
    floor: &str,
    now: &str,
    min_days: i32,
) -> Result<Option<Candidate>, String> {
    let mut versions: Vec<String> = reg
        .versions(eco, name)?
        .into_iter()
        .filter(|v| !is_prerelease(v))
        .filter(|v| compat_key(v) == compat_key(current))
        .filter(|v| cmp_versions(v, floor) != Ordering::Less)
        .collect();
    versions.sort_by(|a, b| cmp_versions(b, a));
    for v in versions {
        let Some(at) = published_at(s, reg, eco, name, &v)? else {
            continue;
        };
        if age_days(&at, now) >= min_days as f64 {
            return Ok(Some(Candidate {
                version: v,
                published_at: at,
            }));
        }
    }
    Ok(None)
}

/// One pinned dependency of a lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pin {
    pub(crate) name: String,
    pub(crate) version: String,
}

/// The (name, version) pairs a lockfile pins, workspace members left out.
/// `None` for a lockfile format this module does not read.
pub(crate) fn lockfile_pins(path: &str, text: &str) -> Option<Vec<Pin>> {
    match path.rsplit('/').next().unwrap_or(path) {
        "Cargo.lock" => Some(toml_package_pins(text, |source| !source.is_empty())),
        // uv's workspace members carry `source = { editable = "." }` or `{ virtual = "." }`.
        "uv.lock" => Some(toml_package_pins(text, |source| {
            source.contains("registry")
        })),
        "package-lock.json" => package_lock_pins(text),
        "Gemfile.lock" => Some(gemfile_lock_pins(text)),
        _ => None,
    }
}

/// `[[package]]` tables with `name`, `version` and a `source` line that `is_dependency`
/// accepts: the shape Cargo.lock and uv.lock share.
fn toml_package_pins(text: &str, is_dependency: impl Fn(&str) -> bool) -> Vec<Pin> {
    let mut pins = Vec::new();
    let mut name = String::new();
    let mut version = String::new();
    let mut source = String::new();
    let mut in_package = false;
    let flush = |pins: &mut Vec<Pin>, name: &str, version: &str, source: &str| {
        if is_dependency(source) && !name.is_empty() && !version.is_empty() {
            pins.push(Pin {
                name: name.to_string(),
                version: version.to_string(),
            });
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("[[") {
            flush(&mut pins, &name, &version, &source);
            name.clear();
            version.clear();
            source.clear();
            in_package = line == "[[package]]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(v) = line.strip_prefix("name = ") {
            name = v.trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("version = ") {
            version = v.trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("source = ") {
            source = v.to_string();
        }
    }
    flush(&mut pins, &name, &version, &source);
    pins
}

/// The `specs:` entries of a Gemfile.lock: `    name (1.2.3)`, four spaces in; a
/// platform suffix (`1.16.5-x86_64-linux`) is dropped, the gem's version is the same.
fn gemfile_lock_pins(text: &str) -> Vec<Pin> {
    let mut pins = Vec::new();
    let mut in_specs = false;
    for line in text.lines() {
        if !line.starts_with(' ') {
            in_specs = false;
            continue;
        }
        if line.trim() == "specs:" {
            in_specs = true;
            continue;
        }
        if !in_specs || !line.starts_with("    ") || line.starts_with("      ") {
            continue;
        }
        let Some((name, rest)) = line.trim().split_once(" (") else {
            continue;
        };
        let Some(version) = rest.strip_suffix(')') else {
            continue;
        };
        let version = version.split('-').next().unwrap_or(version);
        if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        pins.push(Pin {
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    pins
}

fn package_lock_pins(text: &str) -> Option<Vec<Pin>> {
    let doc: Value = serde_json::from_str(text).ok()?;
    let packages = doc.get("packages")?.as_object()?;
    let mut pins = Vec::new();
    for (path, entry) in packages {
        if path.is_empty() || entry.get("link").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(name) = path.rsplit("node_modules/").next() else {
            continue;
        };
        if !path.contains("node_modules/") {
            continue;
        }
        let Some(version) = entry.get("version").and_then(Value::as_str) else {
            continue;
        };
        pins.push(Pin {
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    Some(pins)
}

/// A version `after` pins that `before` did not, with the version `before`
/// had for the same name when it had one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewPin {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) previous: Option<String>,
}

pub(crate) fn new_pins(before: &[Pin], after: &[Pin]) -> Vec<NewPin> {
    let mut had: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in before {
        had.entry(p.name.as_str())
            .or_default()
            .push(p.version.as_str());
    }
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for p in after {
        let versions = had.get(p.name.as_str());
        if versions.is_some_and(|vs| vs.contains(&p.version.as_str())) {
            continue;
        }
        if !seen.insert((p.name.clone(), p.version.clone())) {
            continue;
        }
        // The closest older version the lockfile had: the newest one below the new pin.
        let previous = versions.and_then(|vs| {
            vs.iter()
                .filter(|v| cmp_versions(v, &p.version) == Ordering::Less)
                .max_by(|a, b| cmp_versions(a, b))
                .or_else(|| vs.iter().max_by(|a, b| cmp_versions(a, b)))
                .map(|v| v.to_string())
        });
        out.push(NewPin {
            name: p.name.clone(),
            version: p.version.clone(),
            previous,
        });
    }
    out
}

/// What the age rule says about a set of new pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgeVerdict {
    /// Every new version is old enough and stays within its compatible range.
    Ready,
    /// The youngest new version has to age first; `eligible_at` is when.
    Wait {
        name: String,
        version: String,
        published_at: String,
        eligible_at: String,
    },
    /// An existing dependency moves to an incompatible version: a human's call.
    MajorJump {
        name: String,
        from: String,
        to: String,
    },
    /// The registry has no answer for a version, so the rule cannot be checked.
    Unknown {
        name: String,
        version: String,
        why: String,
    },
}

impl AgeVerdict {
    pub(crate) fn note(&self, min_days: i32) -> String {
        match self {
            Self::Ready => format!("every new version is at least {min_days} days old"),
            Self::Wait {
                name,
                version,
                published_at,
                eligible_at,
            } => format!(
                "{name} {version} was published {published_at}, younger than {min_days} days; it can land from {eligible_at}"
            ),
            Self::MajorJump { name, from, to } => {
                format!("{name} moves from {from} to {to}, past its compatible range")
            }
            Self::Unknown { name, version, why } => {
                format!("the publish date of {name} {version} is unknown: {why}")
            }
        }
    }
}

/// Checks `pins` against the rule: a compatibility jump first (no registry
/// needed), then the publish date of every new version; the `Wait` returned
/// names the version that becomes eligible last.
pub(crate) fn verdict(
    s: &pulse_store::Store,
    reg: &dyn Registry,
    eco: Ecosystem,
    pins: &[NewPin],
    now: &str,
    min_days: i32,
) -> AgeVerdict {
    for p in pins {
        if let Some(prev) = &p.previous {
            if compat_key(prev) != compat_key(&p.version) {
                return AgeVerdict::MajorJump {
                    name: p.name.clone(),
                    from: prev.clone(),
                    to: p.version.clone(),
                };
            }
        }
    }
    let mut wait: Option<AgeVerdict> = None;
    let mut wait_until = String::new();
    for p in pins {
        let at = match published_at(s, reg, eco, &p.name, &p.version) {
            Ok(Some(at)) => at,
            Ok(None) => {
                return AgeVerdict::Unknown {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    why: format!("{} does not list it", eco.label()),
                };
            }
            Err(why) => {
                return AgeVerdict::Unknown {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    why,
                };
            }
        };
        if age_days(&at, now) < min_days as f64 {
            let eligible = eligible_at(&at, min_days);
            if eligible > wait_until {
                wait_until = eligible.clone();
                wait = Some(AgeVerdict::Wait {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    published_at: at,
                    eligible_at: eligible,
                });
            }
        }
    }
    wait.unwrap_or(AgeVerdict::Ready)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-10-26T12:00:00Z";

    fn reg() -> MapRegistry {
        MapRegistry::default()
            .with(
                Ecosystem::Npm,
                "next",
                &[
                    ("16.3.8", "2026-09-30T10:00:00Z"),
                    ("16.4.0", "2026-10-07T10:00:00Z"),
                    ("16.4.1", "2026-10-14T17:43:55Z"),
                    ("16.4.2", "2026-10-25T09:00:00Z"),
                    ("16.5.0-canary.1", "2026-10-01T00:00:00Z"),
                    ("17.0.0", "2026-10-01T00:00:00Z"),
                ],
            )
            .with(
                Ecosystem::Cargo,
                "base64",
                &[
                    ("0.22.1", "2024-05-01T00:00:00Z"),
                    ("0.23.1", "2026-03-01T00:00:00Z"),
                ],
            )
    }

    #[test]
    fn versions_compare_numerically_with_prereleases_below() {
        assert_eq!(cmp_versions("16.4.1", "16.4.0"), Ordering::Greater);
        assert_eq!(cmp_versions("16.10.0", "16.9.9"), Ordering::Greater);
        assert_eq!(cmp_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(cmp_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(cmp_versions("1.2.3-rc.1", "1.2.3"), Ordering::Less);
        assert_eq!(cmp_versions("1.2.3rc1", "1.2.3"), Ordering::Less);
        assert!(is_prerelease("16.5.0-canary.1"));
        assert!(is_prerelease("2.0.0rc1"));
        assert!(!is_prerelease("v1.2.3"));
        assert!(!is_prerelease("1.2.3+build.5"));
        assert_eq!(compat_key("16.4.1"), (16, 0));
        assert_eq!(compat_key("0.23.1"), (0, 23));
    }

    #[test]
    fn ecosystems_come_from_lockfile_names_and_dependabot_names() {
        assert_eq!(
            Ecosystem::from_lockfile("web/hub/package-lock.json"),
            Some(Ecosystem::Npm)
        );
        assert_eq!(
            Ecosystem::from_lockfile("Cargo.lock"),
            Some(Ecosystem::Cargo)
        );
        assert_eq!(Ecosystem::from_lockfile("uv.lock"), Some(Ecosystem::PyPi));
        assert_eq!(
            Ecosystem::from_lockfile("Gemfile.lock"),
            Some(Ecosystem::RubyGems)
        );
        assert_eq!(Ecosystem::from_lockfile("Makefile"), None);
        assert_eq!(Ecosystem::from_dependabot("npm"), Some(Ecosystem::Npm));
        assert_eq!(Ecosystem::from_dependabot("pip"), Some(Ecosystem::PyPi));
        assert_eq!(
            Ecosystem::from_dependabot("rubygems"),
            Some(Ecosystem::RubyGems)
        );
        assert_eq!(Ecosystem::from_dependabot("maven"), None);
    }

    #[test]
    fn newest_eligible_skips_young_incompatible_and_prerelease_versions() {
        let s = crate::test_helpers::new_store();
        let r = reg();
        // 16.4.2 is one day old, 17.0.0 another major, the canary a prerelease: 16.4.1 wins.
        let got = newest_eligible(&s, &r, Ecosystem::Npm, "next", "16.3.8", "16.4.1", NOW, 3)
            .unwrap()
            .unwrap();
        assert_eq!(got.version, "16.4.1");
        assert_eq!(got.published_at, "2026-10-14T17:43:55Z");
        // With the rule off the newest compatible version is taken.
        let got = newest_eligible(&s, &r, Ecosystem::Npm, "next", "16.3.8", "16.4.1", NOW, 0)
            .unwrap()
            .unwrap();
        assert_eq!(got.version, "16.4.2");
        // A floor younger than the rule allows: nothing yet.
        let got =
            newest_eligible(&s, &r, Ecosystem::Npm, "next", "16.3.8", "16.4.2", NOW, 3).unwrap();
        assert!(got.is_none());
        // An unknown package is an error, not a silent empty answer.
        assert!(newest_eligible(&s, &r, Ecosystem::Npm, "nope", "1.0.0", "1.0.1", NOW, 3).is_err());
    }

    #[test]
    fn published_at_is_cached_in_meta() {
        let s = crate::test_helpers::new_store();
        let r = reg();
        let at = published_at(&s, &r, Ecosystem::Npm, "next", "16.4.1")
            .unwrap()
            .unwrap();
        assert_eq!(at, "2026-10-14T17:43:55Z");
        // An empty registry answers from the cache now.
        let empty = MapRegistry::default();
        let again = published_at(&s, &empty, Ecosystem::Npm, "next", "16.4.1")
            .unwrap()
            .unwrap();
        assert_eq!(again, at);
        assert!(published_at(&s, &empty, Ecosystem::Npm, "next", "16.4.0").is_err());
        assert_eq!(eligible_at(&at, 3), "2026-10-17T17:43:55Z");
        assert!((age_days(&at, NOW) - 11.76).abs() < 0.01);
    }

    #[test]
    fn cargo_lock_pins_leave_workspace_members_out() {
        let text = "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"base64\"\nversion = \"0.23.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"abc\"\n\n[[package]]\nname = \"pulse-hub\"\nversion = \"0.1.0\"\ndependencies = [\n \"base64\",\n]\n\n[[package]]\nname = \"regex\"\nversion = \"1.11.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        let pins = lockfile_pins("Cargo.lock", text).unwrap();
        assert_eq!(
            pins,
            vec![
                Pin {
                    name: "base64".into(),
                    version: "0.23.1".into()
                },
                Pin {
                    name: "regex".into(),
                    version: "1.11.1".into()
                },
            ]
        );
    }

    #[test]
    fn uv_lock_pins_leave_the_workspace_members_out() {
        let text = "version = 1\nrevision = 3\n\n[[package]]\nname = \"ai\"\nversion = \"0.1.0\"\nsource = { editable = \".\" }\ndependencies = [\n    { name = \"requests\" },\n]\n\n[[package]]\nname = \"requests\"\nversion = \"2.32.3\"\nsource = { registry = \"https://pypi.org/simple\" }\nsdist = { url = \"x\" }\n\n[[package]]\nname = \"tool\"\nversion = \"1.0.0\"\nsource = { virtual = \".\" }\n";
        let pins = lockfile_pins("uv.lock", text).unwrap();
        assert_eq!(
            pins,
            vec![Pin {
                name: "requests".into(),
                version: "2.32.3".into()
            }]
        );
    }

    #[test]
    fn gemfile_lock_pins_read_the_specs_and_drop_platforms() {
        let text = "GEM\n  remote: https://rubygems.org/\n  specs:\n    nokogiri (1.16.5-x86_64-linux)\n      racc (~> 1.4)\n    racc (1.8.1)\n    rack (3.0.9)\n\nPLATFORMS\n  x86_64-linux\n\nDEPENDENCIES\n  nokogiri\n  rack (>= 3)\n\nBUNDLED WITH\n   2.5.9\n";
        let pins = lockfile_pins("Gemfile.lock", text).unwrap();
        assert_eq!(
            pins,
            vec![
                Pin {
                    name: "nokogiri".into(),
                    version: "1.16.5".into()
                },
                Pin {
                    name: "racc".into(),
                    version: "1.8.1".into()
                },
                Pin {
                    name: "rack".into(),
                    version: "3.0.9".into()
                },
            ]
        );
    }

    #[test]
    fn package_lock_pins_name_nested_packages_and_skip_links() {
        let text = r#"{"name":"hub","lockfileVersion":3,"packages":{"":{"name":"hub","version":"0.1.0"},"node_modules/next":{"version":"16.3.8"},"node_modules/ajv/node_modules/fast-uri":{"version":"3.1.8"},"node_modules/local":{"resolved":"../local","link":true}}}"#;
        let pins = lockfile_pins("web/hub/package-lock.json", text).unwrap();
        assert_eq!(
            pins,
            vec![
                Pin {
                    name: "fast-uri".into(),
                    version: "3.1.8".into()
                },
                Pin {
                    name: "next".into(),
                    version: "16.3.8".into()
                },
            ]
        );
        assert!(lockfile_pins("pnpm-lock.yaml", "lockfileVersion: 9.0").is_none());
        assert!(lockfile_pins("package-lock.json", "{\"lockfileVersion\":1}").is_none());
    }

    #[test]
    fn new_pins_name_the_previous_version() {
        let before = vec![
            Pin {
                name: "next".into(),
                version: "16.3.8".into(),
            },
            Pin {
                name: "left".into(),
                version: "1.0.0".into(),
            },
        ];
        let after = vec![
            Pin {
                name: "next".into(),
                version: "16.4.1".into(),
            },
            Pin {
                name: "left".into(),
                version: "1.0.0".into(),
            },
            Pin {
                name: "hono".into(),
                version: "4.13.11".into(),
            },
        ];
        let got = new_pins(&before, &after);
        assert_eq!(
            got,
            vec![
                NewPin {
                    name: "next".into(),
                    version: "16.4.1".into(),
                    previous: Some("16.3.8".into())
                },
                NewPin {
                    name: "hono".into(),
                    version: "4.13.11".into(),
                    previous: None
                },
            ]
        );
        assert!(new_pins(&after, &after).is_empty());
    }

    #[test]
    fn verdict_waits_for_the_youngest_version_and_stops_at_a_major_jump() {
        let s = crate::test_helpers::new_store();
        let r = reg();
        let old = NewPin {
            name: "next".into(),
            version: "16.4.1".into(),
            previous: Some("16.3.8".into()),
        };
        let young = NewPin {
            name: "next".into(),
            version: "16.4.2".into(),
            previous: Some("16.4.1".into()),
        };
        assert_eq!(
            verdict(&s, &r, Ecosystem::Npm, &[old.clone()], NOW, 3),
            AgeVerdict::Ready
        );
        let v = verdict(
            &s,
            &r,
            Ecosystem::Npm,
            &[old.clone(), young.clone()],
            NOW,
            3,
        );
        assert_eq!(
            v,
            AgeVerdict::Wait {
                name: "next".into(),
                version: "16.4.2".into(),
                published_at: "2026-10-25T09:00:00Z".into(),
                eligible_at: "2026-10-28T09:00:00Z".into(),
            }
        );
        assert!(
            v.note(3).contains("can land from 2026-10-28T09:00:00Z"),
            "{}",
            v.note(3)
        );
        // With the rule off the young version is fine.
        assert_eq!(
            verdict(&s, &r, Ecosystem::Npm, &[young], NOW, 0),
            AgeVerdict::Ready
        );
        let jump = NewPin {
            name: "next".into(),
            version: "17.0.0".into(),
            previous: Some("16.3.8".into()),
        };
        assert_eq!(
            verdict(&s, &r, Ecosystem::Npm, &[old.clone(), jump], NOW, 3),
            AgeVerdict::MajorJump {
                name: "next".into(),
                from: "16.3.8".into(),
                to: "17.0.0".into()
            }
        );
        // A 0.x crate moving its minor is a jump too; a new name needs no previous version.
        let zero = NewPin {
            name: "base64".into(),
            version: "0.23.1".into(),
            previous: Some("0.22.1".into()),
        };
        assert!(matches!(
            verdict(&s, &r, Ecosystem::Cargo, &[zero], NOW, 3),
            AgeVerdict::MajorJump { .. }
        ));
        let added = NewPin {
            name: "base64".into(),
            version: "0.23.1".into(),
            previous: None,
        };
        assert_eq!(
            verdict(&s, &r, Ecosystem::Cargo, &[added], NOW, 3),
            AgeVerdict::Ready
        );
        let unknown = NewPin {
            name: "mystery".into(),
            version: "1.0.0".into(),
            previous: None,
        };
        assert!(matches!(
            verdict(&s, &r, Ecosystem::Npm, &[unknown], NOW, 3),
            AgeVerdict::Unknown { .. }
        ));
    }

    /// Asks the real registries; run by hand with `--ignored` when the network is there.
    #[test]
    #[ignore]
    fn live_registries_answer() {
        let r = HttpRegistry::default();
        for (eco, name, version) in [
            (Ecosystem::Npm, "next", "16.4.1"),
            (Ecosystem::Cargo, "base64", "0.22.1"),
            (Ecosystem::PyPi, "requests", "2.32.3"),
            (Ecosystem::Go, "golang.org/x/text", "v0.14.0"),
            (Ecosystem::RubyGems, "rack", "3.0.9"),
        ] {
            let versions = r.versions(eco, name).unwrap();
            assert!(
                versions.iter().any(|v| v == version),
                "{name}: {versions:?}"
            );
            let at = r.published_at(eco, name, version).unwrap().unwrap();
            assert!(
                at.ends_with('Z') && at.len() == 20,
                "{name} {version}: {at}"
            );
        }
        assert!(
            r.published_at(Ecosystem::Npm, "next", "0.0.0-nope")
                .unwrap()
                .is_none()
        );
        assert!(
            r.versions(Ecosystem::Npm, "pulse-no-such-package-zz")
                .is_err()
        );
    }

    #[test]
    fn registry_times_normalize_to_the_store_layout() {
        assert_eq!(
            normalize_time("2026-10-14T17:43:55.246Z").unwrap(),
            "2026-10-14T17:43:55Z"
        );
        assert_eq!(
            normalize_time("2026-10-14T17:43:55.246000+00:00").unwrap(),
            "2026-10-14T17:43:55Z"
        );
        assert_eq!(
            normalize_time("2026-10-14T19:43:55+02:00").unwrap(),
            "2026-10-14T17:43:55Z"
        );
        assert!(normalize_time("yesterday").is_none());
        assert_eq!(
            go_escape("github.com/BurntSushi/toml"),
            "github.com/!burnt!sushi/toml"
        );
    }
}
