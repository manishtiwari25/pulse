use crate::lists::join_counts;
use crate::register::{json_output, no_args, py_repr, store};
use crate::settings::agent_setting_value;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use pulse_cli::Context;
use pulse_cli::flags::FlagSet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCount {
    pub status: String,
    pub n: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusReport {
    pub store: String,
    pub schema_version: String,
    pub projects: Vec<pulse_store::Project>,
    pub agents: Vec<StatusCount>,
    pub posts: i64,
    pub jobs: Vec<StatusCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedules: Vec<StatusCount>,
    pub runs_today: i64,
    pub daily_cap: i64,
    pub runner: String,
    pub harness: String,
    pub daemon: String,
    pub bus: BusStatus,
    pub planner_cache: PlannerCacheStatus,
    pub executors: String,
    pub capacity: String,
    /// Bulk storage on another disk (pulse bulk); empty while local.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bulk: String,
    pub hygiene: String,
    pub planner: Vec<PlannerNote>,
    pub recovery: Vec<RecoveryNote>,
    pub landing: Vec<LandingNote>,
    pub settings: Vec<pulse_store::SettingRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusStatus {
    pub subscribers: i64,
    pub dropped: i64,
    pub deduped: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCacheStatus {
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerNote {
    pub project: String,
    pub at: String,
    pub note: String,
    pub stale: bool,
}

pub type RecoveryNote = PlannerNote;
pub type LandingNote = PlannerNote;

pub fn status_counts(s: &pulse_store::Store, query: &str) -> Result<Vec<StatusCount>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(query)?;
    let rows = stmt.query_map([], |r| {
        Ok(StatusCount {
            status: r.get(0)?,
            n: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn parse_utc(text: &str) -> Result<DateTime<Utc>> {
    let t = DateTime::parse_from_rfc3339(text).map_err(|_| {
        pulse_cli::errorf(
            2,
            &format!(
                "pulse: Not a UTC time like 2026-09-05T08:00:00Z: {}",
                py_repr(text)
            ),
            "",
            "",
        )
    })?;
    if t.offset().local_minus_utc() != 0 || text.len() != 20 || !text.ends_with('Z') {
        return Err(pulse_cli::errorf(
            2,
            &format!(
                "pulse: Not a UTC time like 2026-09-05T08:00:00Z: {}",
                py_repr(text)
            ),
            "",
            "",
        )
        .into());
    }
    Ok(t.with_timezone(&Utc))
}

pub fn setting_int(values: &BTreeMap<String, String>, name: &str) -> i64 {
    let raw = values
        .get(name)
        .cloned()
        .or_else(|| pulse_store::DEFAULT_SETTINGS.get(name).cloned())
        .unwrap_or_default();
    raw.trim().parse().unwrap_or_else(|_| {
        pulse_store::DEFAULT_SETTINGS
            .get(name)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    })
}

pub fn daily_cap(values: &BTreeMap<String, String>) -> i64 {
    let raw = values
        .get("jobs_per_day")
        .cloned()
        .or_else(|| pulse_store::DEFAULT_SETTINGS.get("jobs_per_day").cloned())
        .unwrap_or_default();
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "none" | "off" | "unlimited" => 0,
        _ => setting_int(values, "jobs_per_day").max(0),
    }
}

pub fn runner_stopped(s: &pulse_store::Store) -> Result<String> {
    let values = s.settings()?;
    if values.get("stop").is_some_and(|v| v == "1") {
        return Ok("the runner is stopped in the store (pulse start to resume)".into());
    }
    if let Some(brain) = s.brain()? {
        let stop = Path::new(&brain.path).join("docs/evolution/STOP");
        match std::fs::metadata(&stop) {
            Ok(_) => {
                return Ok(format!(
                    "docs/evolution/STOP is present in the brain ({}); a human removes it",
                    brain.path
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(String::new())
}

pub fn harness_line(s: &pulse_store::Store) -> String {
    let name = s.setting("provider", pulse_provider::DEFAULT);
    let mut label = pulse_provider::label_of(&name);
    if label.is_empty() {
        label = name.clone();
    }
    let cancel = pulse_provider::CancelToken::new();
    let p = match pulse_provider::open(
        &name,
        pulse_provider::Options {
            home: s.home.clone(),
            ..Default::default()
        },
    ) {
        Ok(p) => p,
        Err(err) => {
            return format!(
                "native · {label} · status unknown ({})",
                one_status_line(&err.to_string(), 200)
            );
        }
    };
    match p.status(&cancel) {
        Ok(login) if login.logged_in => {
            format!("native · {} · signed in as {}", p.label(), login.user)
        }
        Ok(login) if !login.detail.is_empty() => format!(
            "native · {} · not signed in - {}",
            p.label(),
            one_status_line(&login.detail, 240)
        ),
        Ok(_) => format!(
            "native · {} · not signed in - open Settings → Harness or run pulse harness login",
            p.label()
        ),
        Err(err) => format!(
            "native · {} · status unknown ({})",
            p.label(),
            one_status_line(&err.to_string(), 200)
        ),
    }
}

pub fn one_status_line(s: &str, limit: usize) -> String {
    let text = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() > limit {
        text[..text.floor_char_boundary(limit)].to_string()
    } else {
        text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonBeat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub at: String,
    pub loops: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<i64>,
    pub note: String,
    pub bus: BusStatus,
    pub planner_cache: PlannerCacheStatus,
}

pub fn read_daemon_beat(s: &pulse_store::Store) -> Result<(DaemonBeat, bool)> {
    let (raw, exists) = s.meta("daemon")?;
    if !exists {
        return Ok((DaemonBeat::default(), false));
    }
    Ok((serde_json::from_str(&raw)?, true))
}

pub fn daemon_interval_seconds(s: &pulse_store::Store) -> Result<i64> {
    let Ok((beat, _)) = read_daemon_beat(s) else {
        return Ok(pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64);
    };
    Ok(beat
        .interval
        .filter(|n| *n != 0)
        .unwrap_or(pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64))
}

pub fn daemon_status(s: &pulse_store::Store) -> Result<String> {
    let (beat, exists) = match read_daemon_beat(s) {
        Ok(v) => v,
        Err(_) => return Ok("unreadable heartbeat".into()),
    };
    if !exists {
        return Ok("never started".into());
    }
    let pid = beat
        .pid
        .map(|n| n.to_string())
        .unwrap_or_else(|| "None".into());
    let interval_text = beat
        .interval
        .map(|n| n.to_string())
        .unwrap_or_else(|| "None".into());
    if beat.note == "exited" {
        return Ok(format!("exited {} (pid {pid})", beat.at));
    }
    let at = parse_utc(&beat.at)?;
    let now = parse_utc(&s.now())?;
    let age = (now - at).num_seconds();
    let seconds = beat
        .interval
        .filter(|n| *n != 0)
        .unwrap_or(pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64);
    let state = if age > 2 * seconds + 60 {
        "silent"
    } else {
        "alive"
    };
    Ok(format!(
        "{state} pid {pid}, last loop {age}s ago, {} loop(s), interval {interval_text}s",
        beat.loops
    ))
}

pub fn meta_notes(s: &pulse_store::Store, key: &str) -> Result<Vec<PlannerNote>> {
    let (raw, exists) = s.meta(key)?;
    if !exists {
        return Ok(vec![]);
    }
    #[derive(Deserialize)]
    struct Stored {
        at: String,
        note: String,
    }
    let stored: BTreeMap<String, Stored> =
        serde_json::from_str(&raw).map_err(|e| anyhow!("read {key} notes: {e}"))?;
    let interval = daemon_interval_seconds(s)?;
    let now = parse_utc(&s.now())?;
    let mut out = Vec::with_capacity(stored.len());
    for (project, item) in stored {
        let at = parse_utc(&item.at)?;
        out.push(PlannerNote {
            project,
            at: item.at,
            note: item.note,
            stale: now - at > Duration::seconds(2 * interval),
        });
    }
    Ok(out)
}

pub fn planner_notes(s: &pulse_store::Store) -> Result<Vec<PlannerNote>> {
    meta_notes(s, "planner")
}
pub fn recovery_notes(s: &pulse_store::Store) -> Result<Vec<RecoveryNote>> {
    meta_notes(s, "recovery")
}
pub fn landing_notes(s: &pulse_store::Store) -> Result<Vec<LandingNote>> {
    meta_notes(s, "landing")
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CapacityNote {
    queued: i64,
    slots: i64,
    inflight: i64,
    starved_since: String,
    held: String,
}

fn read_capacity_note(s: &pulse_store::Store) -> Result<(CapacityNote, bool)> {
    let (raw, exists) = s.meta("capacity")?;
    if !exists {
        return Ok((CapacityNote::default(), false));
    }
    Ok((
        serde_json::from_str(&raw).map_err(|e| anyhow!("read capacity heartbeat: {e}"))?,
        true,
    ))
}

pub fn capacity_status(s: &pulse_store::Store) -> Result<String> {
    let (note, exists) = read_capacity_note(s)?;
    if !exists || note.queued == 0 {
        return Ok(String::new());
    }
    let mut line = format!(
        "{} job(s) queued, {} slot(s), {} in flight",
        note.queued, note.slots, note.inflight
    );
    if !note.held.is_empty() {
        line.push_str("; ");
        line.push_str(&note.held);
    }
    if !note.starved_since.is_empty() {
        let start = parse_utc(&note.starved_since)?;
        let now = parse_utc(&s.now())?;
        line.push_str(&format!(
            "; every slot busy for {} min - another machine would help (pulse pair; pulse record show WF-executors)",
            (now - start).num_minutes()
        ));
    }
    Ok(line)
}

/// The disk hold the daemon wrote with its last capacity note, or empty.
pub fn capacity_disk(s: &pulse_store::Store) -> Result<String> {
    let (raw, exists) = s.meta("capacity")?;
    if !exists || raw.is_empty() {
        return Ok(String::new());
    }
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    Ok(v.get("disk")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string())
}

/// The bulk-storage hold the daemon wrote with its last capacity note, or
/// empty: the volume carrying work, cache and worktrees is not mounted.
pub fn capacity_bulk(s: &pulse_store::Store) -> Result<String> {
    let (raw, exists) = s.meta("capacity")?;
    if !exists || raw.is_empty() {
        return Ok(String::new());
    }
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    Ok(v.get("bulk")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string())
}

pub fn capacity_hold(s: &pulse_store::Store) -> Result<String> {
    let (note, exists) = read_capacity_note(s)?;
    Ok(if exists { note.held } else { String::new() })
}

pub fn hygiene_status(s: &pulse_store::Store) -> Result<String> {
    let (raw, exists) = s.meta("hygiene")?;
    if !exists {
        return Ok(String::new());
    }
    #[derive(Deserialize)]
    struct Note {
        at: String,
        removed: BTreeMap<String, i64>,
        held: BTreeMap<String, i64>,
        bytes: i64,
        errors: i64,
        #[serde(default)]
        cargo_target: Vec<CargoTargetNote>,
    }
    #[derive(Deserialize)]
    struct CargoTargetNote {
        units: i64,
        incremental: i64,
        #[serde(default)]
        objects: i64,
        bytes: i64,
        #[serde(default)]
        skipped: String,
    }
    let note: Note = serde_json::from_str(&raw).map_err(|e| anyhow!("read hygiene note: {e}"))?;
    let counts = |m: &BTreeMap<String, i64>| {
        m.iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut line = format!("last sweep {}: ", note.at);
    if note.removed.is_empty() {
        line.push_str("nothing to remove");
    } else {
        line.push_str("removed ");
        line.push_str(&counts(&note.removed));
        if note.bytes > 0 {
            line.push_str(&format!(" ({} freed)", size_text(note.bytes)));
        }
    }
    if !note.held.is_empty() {
        line.push_str("; held ");
        line.push_str(&counts(&note.held));
    }
    if note.errors > 0 {
        line.push_str(&format!(
            "; {} error(s) - pulse sweep --dry-run shows them",
            note.errors
        ));
    }
    let cargo_units: i64 = note.cargo_target.iter().map(|x| x.units).sum();
    let cargo_incremental: i64 = note.cargo_target.iter().map(|x| x.incremental).sum();
    let cargo_objects: i64 = note.cargo_target.iter().map(|x| x.objects).sum();
    let cargo_bytes: i64 = note.cargo_target.iter().map(|x| x.bytes).sum();
    let cargo_skipped = note
        .cargo_target
        .iter()
        .filter(|x| !x.skipped.is_empty())
        .count();
    if cargo_bytes > 0 || cargo_units > 0 || cargo_incremental > 0 || cargo_objects > 0 {
        line.push_str(&format!(
            "; cargo target: freed {} ({} dead clone {}, {} stale incremental {}, {} stale debug {})",
            size_text(cargo_bytes),
            cargo_units,
            if cargo_units == 1 { "unit" } else { "units" },
            cargo_incremental,
            if cargo_incremental == 1 {
                "dir"
            } else {
                "dirs"
            },
            cargo_objects,
            if cargo_objects == 1 {
                "object"
            } else {
                "objects"
            },
        ));
    } else if !note.cargo_target.is_empty() && cargo_skipped == 0 {
        line.push_str("; cargo target: nothing to prune");
    } else if cargo_skipped > 0 {
        line.push_str(&format!(
            "; cargo target: skipped {cargo_skipped} busy or unsafe target(s)"
        ));
    }
    Ok(line)
}

pub fn size_text(n: i64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct UnfitMark {
    tool: String,
    at: String,
}

fn unfit_marks(s: &pulse_store::Store) -> Result<BTreeMap<String, BTreeMap<String, UnfitMark>>> {
    let (raw, exists) = s.meta("unfit")?;
    if !exists {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&raw).map_err(|e| anyhow!("read executor unfit marks: {e}"))
}

pub fn unfit_phrase(tool: &str) -> String {
    match tool {
        "services" => "needs services".into(),
        "hands outdated" => tool.into(),
        _ if tool.starts_with("setup:") => format!("cannot set up {}", &tool[6..]),
        _ => format!("lacks {}", tool.strip_prefix("tool:").unwrap_or(tool)),
    }
}

pub fn executors_line(s: &pulse_store::Store) -> Result<String> {
    let values = s.settings()?;
    let now = parse_utc(&s.now())?;
    let horizon = (now - Duration::minutes(2 * setting_int(&values, "minutes_per_job") + 15))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    #[derive(Clone)]
    struct Executor {
        name: String,
        status: String,
        workers: i64,
    }
    let mut executors = vec![Executor {
        name: "local".into(),
        status: "active".into(),
        workers: setting_int(&values, "workers").max(0),
    }];
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT name, status, workers FROM executors ORDER BY name")?;
    for row in stmt.query_map([], |r| {
        Ok(Executor {
            name: r.get(0)?,
            status: r.get(1)?,
            workers: r.get(2)?,
        })
    })? {
        executors.push(row?);
    }
    let marks = unfit_marks(s)?;
    let mut parts = Vec::new();
    for e in executors {
        let busy: i64 = conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?",
                (&e.name, &horizon),
                |r| r.get(0),
            )?;
        let mut part = format!("{} {} ({}/{} busy)", e.name, e.status, busy, e.workers);
        let mut unfit: BTreeMap<String, String> = BTreeMap::new();
        if let Some(mm) = marks.get(&e.name) {
            for (project, mark) in mm {
                if now - parse_utc(&mark.at)? < Duration::hours(24) {
                    unfit.insert(project.clone(), mark.tool.clone());
                }
            }
        }
        for (project, reason) in crate::recipes::recipe_blocks(s, &e.name)? {
            unfit.entry(project).or_insert(reason);
        }
        if !unfit.is_empty() {
            part.push_str("; ");
            part.push_str(
                &unfit
                    .iter()
                    .map(|(project, reason)| format!("{} for {}", unfit_phrase(reason), project))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        parts.push(part);
    }
    Ok(parts.join(", "))
}

pub fn status(s: &pulse_store::Store) -> Result<StatusReport> {
    let (schema_version, exists) = s.meta("schema_version")?;
    if !exists {
        return Err(anyhow!("store has no schema_version"));
    }
    let mut r = StatusReport {
        store: s.path.clone(),
        schema_version,
        projects: s.projects(true)?,
        agents: status_counts(s, "SELECT status, COUNT(*) FROM agents GROUP BY status")?,
        jobs: status_counts(s, "SELECT status, COUNT(*) FROM jobs GROUP BY status")?,
        schedules: status_counts(s, "SELECT status, COUNT(*) FROM schedules GROUP BY status")?,
        ..Default::default()
    };
    let conn = s.conn()?;
    r.posts = conn.query_row("SELECT COUNT(*) FROM posts", [], |row| row.get(0))?;
    let today = parse_utc(&s.now())?.format("%Y-%m-%d").to_string();
    r.runs_today = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE mode != 'dry-run' AND status != 'error'
		AND substr(started_at, 1, 10) = ?",
        [&today],
        |row| row.get(0),
    )?;
    r.settings = s.setting_rows()?;
    let mut values = BTreeMap::new();
    for setting in &mut r.settings {
        values.insert(setting.name.clone(), setting.value.clone());
        setting.value = agent_setting_value(&setting.name, &setting.value);
    }
    r.daily_cap = daily_cap(&values);
    r.runner = runner_stopped(s)?;
    if r.runner.is_empty() {
        r.runner = "running allowed".into();
    }
    r.harness = harness_line(s);
    r.daemon = daemon_status(s)?;
    if let Ok((beat, true)) = read_daemon_beat(s) {
        r.bus = beat.bus;
        r.planner_cache = beat.planner_cache;
    }
    r.executors = executors_line(s)?;
    r.capacity = capacity_status(s)?;
    r.bulk = pulse_runner::bulk::status_line(std::path::Path::new(&s.home));
    r.hygiene = hygiene_status(s)?;
    r.planner = planner_notes(s)?;
    r.recovery = recovery_notes(s)?;
    r.landing = landing_notes(s)?;
    Ok(r)
}

impl StatusReport {
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "store      {} (schema {})\nprojects   {} active\n",
            self.store,
            self.schema_version,
            self.projects.len()
        ));
        for p in &self.projects {
            let brain = if p.brain { "  (brain)" } else { "" };
            out.push_str(&format!("  {:<18} {}{}\n", p.slug, p.path, brain));
        }
        out.push_str(&format!(
            "agents     {}\nposts      {}\n",
            join_counts(&self.agents),
            self.posts
        ));
        let jobs = {
            let j = join_counts(&self.jobs);
            if j.is_empty() { "none".into() } else { j }
        };
        out.push_str(&format!("jobs       {jobs}\n"));
        if !self.schedules.is_empty() {
            out.push_str(&format!("schedules  {}\n", join_counts(&self.schedules)));
        }
        out.push_str(&format!("runs today {}", self.runs_today));
        if self.daily_cap > 0 {
            out.push_str(&format!(" of {} (jobs_per_day)\n", self.daily_cap));
        } else {
            out.push_str(" (no daily cap; jobs_per_day=0)\n");
        }
        out.push_str(&format!(
            "runner     {}\nharness    {}\ndaemon     {}\n",
            self.runner, self.harness, self.daemon
        ));
        if self.bus.dropped > 0 {
            out.push_str(&format!(
                "events     {} event(s) dropped for slow subscribers since the daemon started\n",
                self.bus.dropped
            ));
        }
        if self.planner_cache.hits > 0 || self.planner_cache.misses > 0 {
            out.push_str(&format!(
                "planner    cache {} hit(s), {} miss(es) since the daemon started\n",
                self.planner_cache.hits, self.planner_cache.misses
            ));
        }
        out.push_str(&format!("executors  {}\n", self.executors));
        if !self.capacity.is_empty() {
            out.push_str(&format!("capacity   {}\n", self.capacity));
        }
        if !self.bulk.is_empty() {
            out.push_str(&format!("bulk       {}\n", self.bulk));
        }
        if !self.hygiene.is_empty() {
            out.push_str(&format!("hygiene    {}\n", self.hygiene));
        }
        for (label, notes) in [
            ("planner", &self.planner),
            ("recovery", &self.recovery),
            ("landing", &self.landing),
        ] {
            for note in notes {
                let stale = if note.stale { " (stale)" } else { "" };
                out.push_str(&format!(
                    "{:<10} {}: {}{}\n",
                    label, note.project, note.note, stale
                ));
            }
        }
        out.push_str("settings   ");
        out.push_str(
            &self
                .settings
                .iter()
                .map(|s| format!("{}={}", s.name, setting_text_value(&s.name, &s.value)))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
        out
    }
}

/// The value a setting shows on the one-line `settings` row of the text
/// status. A project goal is a paragraph (or several) stored as JSON; printed
/// inline it swamps the row, so the text view names its source and points at
/// `pulse goal --project <slug>`. The JSON report keeps the full value.
fn setting_text_value(name: &str, value: &str) -> String {
    let Some(slug) = name.strip_prefix("project.goal.") else {
        return value.to_string();
    };
    let source = serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| v.get("source").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    format!("<{source} goal; pulse goal --project {slug}>")
}

pub fn status_text(s: &pulse_store::Store) -> Result<String> {
    Ok(status(s)?.text())
}

pub fn run_status(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    no_args(args)?;
    let report = status(store(c)?)?;
    if c.json {
        return json_output(c, &report);
    }
    c.printf(format_args!("{}", report.text()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn one_status_line_cuts_on_char_boundaries() {
        assert_eq!(one_status_line("a \n b", 80), "a b");
        assert_eq!(one_status_line("abcdef", 3), "abc");
        // A multi-byte char straddling the byte limit is dropped, not split.
        let s = "ab—cd";
        assert_eq!(one_status_line(s, 3), "ab");
        assert_eq!(one_status_line(s, 4), "ab");
        assert_eq!(one_status_line(s, 5), "ab—");
        assert_eq!(one_status_line("عميل عميل", 3), "ع");
    }

    #[test]
    fn daemon_interval_fallback() {
        for (name, raw, want) in [
            (
                "no-heartbeat",
                "",
                pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64,
            ),
            (
                "unreadable",
                "not json",
                pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64,
            ),
            (
                "missing-interval",
                "{}",
                pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64,
            ),
            (
                "null-interval",
                r#"{"interval":null}"#,
                pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64,
            ),
            (
                "zero-interval",
                r#"{"interval":0}"#,
                pulse_runner::DEFAULT_DAEMON_INTERVAL_SECONDS as i64,
            ),
            ("custom-interval", r#"{"interval":7}"#, 7),
        ] {
            let (_dir, s) = temporary_store();
            if !raw.is_empty() {
                s.conn()
                    .unwrap()
                    .execute("INSERT INTO meta (key, value) VALUES ('daemon', ?)", [raw])
                    .unwrap();
            }
            assert_eq!(daemon_interval_seconds(&s).unwrap(), want, "{name}");
        }
    }

    #[test]
    fn daemon_heartbeat_default_interval() {
        let (_dir, s) = temporary_store();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO meta(key,value) VALUES('daemon', '{\"at\":\"2026-09-06T18:48:59Z\"}')",
                [],
            )
            .unwrap();
        assert!(daemon_status(&s).unwrap().starts_with("silent "));
    }

    #[test]
    fn status_runner_counts_and_capacity() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO projects(slug,name,path,status,brain,added_at) VALUES ('active','Active','/missing-active','active',0,'2026-09-06T19:00:00Z'),('inactive','Inactive','/missing-inactive','inactive',0,'2026-09-06T19:00:00Z')",
        );
        for (id, mode, status, at) in [
            ("today", "real", "done", TEST_NOW),
            ("dry", "dry-run", "done", TEST_NOW),
            ("error", "real", "error", TEST_NOW),
            ("yesterday", "real", "done", "2026-09-05T23:59:59Z"),
            ("running", "real", "running", "2026-09-06T18:50:00Z"),
            ("stale-running", "real", "running", "2026-09-06T01:00:00Z"),
        ] {
            s.conn()
                .unwrap()
                .execute(
                    "INSERT INTO runs(id,job_id,agent,project,started_at,status,mode) VALUES (?,'job','worker','active',?,?,?)",
                    (id, at, status, mode),
                )
                .unwrap();
        }
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('capacity','{\"queued\":3,\"slots\":2,\"inflight\":2,\"starved_since\":\"2026-09-06T18:29:01Z\"}'),('daemon','{\"pid\":123,\"at\":\"2026-09-06T18:59:00Z\",\"loops\":4,\"interval\":30,\"note\":\"\"}')",
        );
        let report = status(&s).unwrap();
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].slug, "active");
        assert_eq!(report.runs_today, 3);
        assert_eq!(report.daily_cap, 2);
        assert_eq!(report.executors, "local active (1/1 busy)");
        assert_eq!(
            report.daemon,
            "alive pid 123, last loop 60s ago, 4 loop(s), interval 30s"
        );
        assert_eq!(
            report.capacity,
            "3 job(s) queued, 2 slot(s), 2 in flight; every slot busy for 30 min - another machine would help (pulse pair; pulse record show WF-executors)"
        );
        exec_sql(
            &s,
            "UPDATE meta SET value = '{\"queued\":3,\"slots\":0,\"inflight\":0,\"starved_since\":\"\",\"held\":\"workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)\"}' WHERE key = 'capacity'",
        );
        assert_eq!(
            capacity_status(&s).unwrap(),
            "3 job(s) queued, 0 slot(s), 0 in flight; workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)"
        );
        assert_eq!(
            capacity_hold(&s).unwrap(),
            "workers on this machine wait for a GitHub Copilot login: sign in with GitHub (Settings -> Harness)"
        );
        exec_sql(
            &s,
            "UPDATE meta SET value = '{\"queued\":3,\"slots\":2,\"inflight\":2,\"starved_since\":\"2026-09-06T18:29:01Z\"}' WHERE key = 'capacity'",
        );
        assert_eq!(capacity_hold(&s).unwrap(), "");
        assert_eq!(capacity_disk(&s).unwrap(), "");
        exec_sql(
            &s,
            "UPDATE meta SET value = '{\"queued\":3,\"slots\":2,\"inflight\":0,\"starved_since\":\"\",\"held\":\"\",\"disk\":\"disk: 252 MB free, needs 1024; 6 projects waiting\"}' WHERE key = 'capacity'",
        );
        assert_eq!(
            capacity_disk(&s).unwrap(),
            "disk: 252 MB free, needs 1024; 6 projects waiting"
        );
        // Back to the starved note so the CLI check below still sees it.
        exec_sql(
            &s,
            "UPDATE meta SET value = '{\"queued\":3,\"slots\":2,\"inflight\":2,\"starved_since\":\"2026-09-06T18:29:01Z\"}' WHERE key = 'capacity'",
        );
        assert_eq!(capacity_disk(&s).unwrap(), "");
        assert!(report.harness.contains("native · GitHub Copilot ·"));
        assert!(
            report
                .text()
                .contains(&format!("\nharness    {}\n", report.harness))
        );
        let as_json = invoke(Some(s.clone()), true, "", "status", &[]);
        let decoded: StatusReport = serde_json::from_str(&as_json.out).unwrap();
        assert_eq!(decoded.text(), report.text(), "{as_json:?}");
    }

    #[test]
    fn daemon_heartbeat_states() {
        let (_dir, s) = temporary_store();
        for (raw, want) in [
            ("not json", "unreadable heartbeat"),
            (
                r#"{"pid":42,"at":"2026-09-06T18:00:00Z","note":"exited"}"#,
                "exited 2026-09-06T18:00:00Z (pid 42)",
            ),
            (
                r#"{"at":"2026-09-06T19:00:00Z"}"#,
                "alive pid None, last loop 0s ago, 0 loop(s), interval Nones",
            ),
        ] {
            s.conn().unwrap().execute("INSERT INTO meta(key,value) VALUES ('daemon',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [raw]).unwrap();
            assert_eq!(daemon_status(&s).unwrap(), want);
        }
    }

    #[test]
    fn status_prints_dropped_bus_events() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('daemon','{\"pid\":123,\"at\":\"2026-09-06T18:59:00Z\",\"loops\":4,\"interval\":30,\"note\":\"\",\"bus\":{\"subscribers\":2,\"dropped\":7,\"deduped\":3}}')",
        );
        let report = status(&s).unwrap();
        assert_eq!(report.bus.subscribers, 2);
        assert_eq!(report.bus.dropped, 7);
        assert_eq!(report.bus.deduped, 3);
        assert!(
            report.text().contains(
                "events     7 event(s) dropped for slow subscribers since the daemon started\n"
            ),
            "dropped line missing:\n{}",
            report.text()
        );
    }

    #[test]
    fn planner_notes_text_and_stale() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('daemon','{\"pid\":123,\"at\":\"2026-09-06T18:59:00Z\",\"loops\":4,\"interval\":30,\"note\":\"\"}'), ('planner','{\"fresh\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"no suggestions; reflect due\"},\"old\":{\"at\":\"2026-09-06T18:58:59Z\",\"note\":\"no eligible suggestion (1 rejected)\"}}')",
        );
        let notes = planner_notes(&s).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].project, "fresh");
        assert!(!notes[0].stale);
        assert_eq!(notes[1].project, "old");
        assert!(notes[1].stale);
        let text = status(&s).unwrap().text();
        assert!(
            text.contains("planner    fresh: no suggestions; reflect due\n"),
            "fresh planner line missing:\n{text}"
        );
        assert!(
            text.contains("planner    old: no eligible suggestion (1 rejected) (stale)\n"),
            "stale planner line missing:\n{text}"
        );
    }

    #[test]
    fn hygiene_status_line() {
        let (_dir, s) = temporary_store();
        let mut report = status(&s).unwrap();
        assert_eq!(report.hygiene, "");
        assert!(!report.text().contains("hygiene"));
        exec_sql(
            &s,
            r#"INSERT INTO meta(key,value) VALUES ('hygiene','{"at":"2026-09-06T18:59:00Z","removed":{"clone":2,"scratch":5},"held":{"branch":20,"clone":8},"bytes":1503238553,"errors":1}')"#,
        );
        report = status(&s).unwrap();
        let want = "last sweep 2026-09-06T18:59:00Z: removed clone 2, scratch 5 (1.4 GB freed); held branch 20, clone 8; 1 error(s) - pulse sweep --dry-run shows them";
        assert_eq!(report.hygiene, want, "hygiene = want");
        assert!(
            report.text().contains(&format!("hygiene    {want}\n")),
            "hygiene line missing:\n{}",
            report.text()
        );
        exec_sql(
            &s,
            r#"UPDATE meta SET value = '{"at":"2026-09-06T19:09:00Z","removed":{},"held":{},"bytes":0,"errors":0}' WHERE key = 'hygiene'"#,
        );
        assert_eq!(
            hygiene_status(&s).unwrap(),
            "last sweep 2026-09-06T19:09:00Z: nothing to remove"
        );
    }

    #[test]
    fn unfit_marks_are_read_only() {
        let (_dir, s) = temporary_store();
        let raw = r#"{"local":{"fresh":{"tool":"tool:node@22","at":"2026-09-05T19:00:01Z"},"expired":{"tool":"uv","at":"2026-09-05T19:00:00Z"},"services":{"tool":"services","at":"2026-09-06T18:00:00Z"},"setup":{"tool":"setup:Gemfile.lock","at":"2026-09-06T18:30:00Z"}}}"#;
        s.conn()
            .unwrap()
            .execute("INSERT INTO meta(key,value) VALUES ('unfit',?)", [raw])
            .unwrap();
        let before = event_count(&s);
        assert_eq!(
            executors_line(&s).unwrap(),
            "local active (0/1 busy); lacks node@22 for fresh, needs services for services, cannot set up Gemfile.lock for setup"
        );
        assert_eq!(event_count(&s), before);
    }

    #[test]
    fn daily_cap_spellings() {
        let (_dir, s) = temporary_store();
        for value in [
            "none",
            "off",
            "unlimited",
            "0",
            "3",
            "invalid-old-value",
            "-2",
        ] {
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE settings SET value=? WHERE name='jobs_per_day'",
                    [value],
                )
                .unwrap();
            let report = status(&s).unwrap();
            if value == "3" {
                assert_eq!(report.daily_cap, 3);
            }
            if value != "3" {
                assert!(report.daily_cap >= 0);
            }
        }
    }

    #[test]
    fn recovery_notes_missing_key_is_empty_slice() {
        let (_dir, s) = temporary_store();
        let notes = recovery_notes(&s).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn recovery_notes_sorted_stale_and_malformed() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('daemon','{\"pid\":123,\"at\":\"2026-09-06T18:59:00Z\",\"loops\":4,\"interval\":30,\"note\":\"\"}'), ('recovery','{\"zeta\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"next retry at 2026-09-06T19:05:00Z\"},\"alpha\":{\"at\":\"2026-09-06T18:58:59Z\",\"note\":\"held (structural: missing npm)\"}}')",
        );
        let notes = recovery_notes(&s).unwrap();
        assert_eq!(notes[0].project, "alpha");
        assert_eq!(notes[0].note, "held (structural: missing npm)");
        assert!(notes[0].stale);
        assert_eq!(notes[1].project, "zeta");
        assert!(!notes[1].stale);
        s.conn()
            .unwrap()
            .execute("INSERT INTO meta(key,value) VALUES ('recovery','not json') ON CONFLICT(key) DO UPDATE SET value=excluded.value", [])
            .unwrap();
        assert!(
            recovery_notes(&s)
                .unwrap_err()
                .to_string()
                .contains("recovery notes")
        );
    }

    #[test]
    fn status_recovery_text_and_json() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('daemon',''), ('planner','{\"proj\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"planner note\"}}'),('recovery','{\"proj\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"3 failed in 7d: 1 run again, 1 re-planned (job-r), 1 held\"}}')",
        );
        let report = status(&s).unwrap();
        assert_eq!(report.recovery.len(), 1);
        let text = report.text();
        let planner = "planner    proj: planner note\n";
        let recovery =
            "recovery   proj: 3 failed in 7d: 1 run again, 1 re-planned (job-r), 1 held\n";
        let planner_at = text.find(planner);
        let recovery_at = text.find(recovery);
        assert!(
            planner_at.is_some() && recovery_at.is_some(),
            "planner/recovery lines missing:\n{text}"
        );
        assert!(
            recovery_at > planner_at,
            "recovery line should follow planner line:\n{text}"
        );
        assert!(
            serde_json::to_string(&report)
                .unwrap()
                .contains("\"recovery\":[")
        );
        let (_empty_dir, empty) = temporary_store();
        let empty_report = status(&empty).unwrap();
        assert!(empty_report.recovery.is_empty());
        assert!(
            serde_json::to_string(&empty_report)
                .unwrap()
                .contains("\"recovery\":[]")
        );
    }

    #[test]
    fn status_text_elides_project_goals() {
        let (_dir, s) = temporary_store();
        let goal = "{\"text\":\"Make zeta useful.\\n\\nSecond paragraph.\",\"source\":\"owner\"}";
        exec_sql(
            &s,
            &format!(
                "INSERT INTO settings(name,value,updated_at,updated_by) VALUES ('project.goal.zeta','{}','2026-09-06T18:59:00Z','owner')",
                goal.replace('\'', "''")
            ),
        );
        let report = status(&s).unwrap();
        let text = report.text();
        assert!(
            text.contains("project.goal.zeta=<owner goal; pulse goal --project zeta>"),
            "goal should be elided on the settings row:\n{text}"
        );
        assert!(!text.contains("Second paragraph"), "{text}");
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("Second paragraph"),
            "json keeps the full goal:\n{json}"
        );
        assert_eq!(
            setting_text_value("project.goal.q", "not json"),
            "<unknown goal; pulse goal --project q>"
        );
        assert_eq!(setting_text_value("quorum", "2"), "2");
    }

    #[test]
    fn status_landing_hold_text_and_json() {
        let (_dir, s) = temporary_store();
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('daemon','{\"pid\":123,\"at\":\"2026-09-06T18:59:00Z\",\"loops\":4,\"interval\":30,\"note\":\"\"}'),('recovery','{\"proj\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"1 held\"}}'),('landing','{\"zeta\":{\"at\":\"2026-09-06T18:59:31Z\",\"note\":\"2 branches waiting; deferred: the checkout of /w/zeta has uncommitted changes\"},\"alpha\":{\"at\":\"2026-09-06T18:58:59Z\",\"note\":\"1 branch waiting; locked: another landing holds /w/alpha/.git/pulse-land.lock\"}}')",
        );
        let report = status(&s).unwrap();
        assert_eq!(report.landing.len(), 2);
        assert_eq!(report.landing[0].project, "alpha");
        assert!(report.landing[0].stale);
        assert_eq!(report.landing[1].project, "zeta");
        assert!(!report.landing[1].stale);
        let text = report.text();
        let recovery = "recovery   proj: 1 held\n";
        let landing = "landing    zeta: 2 branches waiting; deferred: the checkout of /w/zeta has uncommitted changes\n";
        let recovery_at = text.find(recovery);
        let landing_at = text.find(landing);
        assert!(
            recovery_at.is_some() && landing_at.is_some() && landing_at > recovery_at,
            "recovery then landing lines expected:\n{text}"
        );
        assert!(text.contains("landing    alpha: 1 branch waiting; locked: another landing holds /w/alpha/.git/pulse-land.lock (stale)\n"), "stale landing line:\n{text}");
        exec_sql(
            &s,
            "INSERT INTO meta(key,value) VALUES ('landing','not json') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        );
        assert!(
            landing_notes(&s)
                .unwrap_err()
                .to_string()
                .contains("landing notes")
        );
        let (_empty_dir, empty) = temporary_store();
        let empty_report = status(&empty).unwrap();
        assert!(empty_report.landing.is_empty());
        assert!(!empty_report.text().contains("landing    "));
        assert!(
            empty_report.landing.is_empty(),
            "empty status landing: {:?}",
            empty_report.landing
        );
        assert!(
            serde_json::to_string(&empty_report)
                .unwrap()
                .contains("\"landing\":[]")
        );
    }
}
