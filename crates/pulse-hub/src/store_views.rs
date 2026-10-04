use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
impl Handler {
    /// Go: func (h *Handler) metadata() (map[string]any, error)
    pub(crate) fn metadata(&self) -> anyhow::Result<JsonMap> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let (schema, _) = store.meta("schema_version")?;
        let (records, _) = store.meta("records_version")?;
        let info = std::fs::metadata(&store.path)?;
        let sequence: i64 =
            store
                .conn()?
                .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))?;
        let mtime = info
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(map_from_pairs(vec![
            ("store", json!(store.path)),
            ("schema_version", json!(schema)),
            ("records_version", json!(records)),
            ("binary_version", json!(pulse_cli::version())),
            ("generated_at", json!(store.now())),
            (
                "fingerprint",
                json!(format!("store:{mtime}:{}:{sequence}", info.len())),
            ),
            ("tool", json!(format!("pulse {}", pulse_cli::version()))),
            (
                "note",
                json!("computed from the store; no projection database"),
            ),
        ]))
    }
    /// Go: func (h *Handler) settings() (map[string]any, error)
    pub(crate) fn settings(&self) -> anyhow::Result<JsonMap> {
        let Some(store) = &self.store else {
            return Ok(map_from_pairs(vec![
                ("available", json!(false)),
                ("can_edit", json!(false)),
                ("settings", json!([])),
            ]));
        };
        let reports = pulse_inspect::settings_report(store)?;
        let mut rows = Vec::new();
        for report in reports {
            let mut row = crate::read::json_map(
                pulse_store::gojson::to_vec(&report)
                    .ok()
                    .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
                    .unwrap_or(Value::Null),
            )?;
            row.insert("secret".into(), json!(false));
            if pulse_bus::PRIVATE_SETTINGS.contains(&report.name.as_str()) {
                row.insert(
                    "value".into(),
                    json!(crate::privacy::hidden_setting_value(report.value)),
                );
                row.insert("default".into(), json!(""));
                row.insert("secret".into(), json!(true));
            } else if pulse_store::secret_shaped(&report.value) {
                row.insert("value".into(), json!(""));
                row.insert("default".into(), json!(""));
                row.insert("secret".into(), json!(true));
            }
            rows.push(Value::Object(row.into_iter().collect()));
        }
        Ok(map_from_pairs(vec![
            ("available", json!(true)),
            ("can_edit", json!(true)),
            ("settings", json!(rows)),
        ]))
    }
    /// Go: func (h *Handler) writeAPI(w http.ResponseWriter, r *http.Request, route string)
    pub(crate) fn write_api(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        route: String,
    ) {
        let Some(store) = &self.store else {
            crate::server::write_error(
                w,
                409,
                format!("store is not available; POST /api/v1/{route} requires the pulse store"),
                "No writable PULSE store is open.".into(),
                "Run pulse init, then restart the hub.".into(),
            );
            return;
        };
        let header = if route == "settings" {
            "X-Pulse-Settings"
        } else {
            "X-Pulse-Decide"
        };
        if r.header(header) != "yes" {
            crate::server::write_error(
                w,
                403,
                format!("missing {header}: yes"),
                "An explicit confirmation header is required.".into(),
                "Confirm the change in the hub.".into(),
            );
            return;
        }
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _guard = crate::additions::ADDITION_WRITES
            .lock()
            .expect("addition write lock poisoned");
        if route == "settings" {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let value = match payload.get("value") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => {
                    crate::server::write_error(
                        w,
                        400,
                        "value must be a string or a number".into(),
                        "This setting value has an unsupported JSON type.".into(),
                        "Send a string or number.".into(),
                    );
                    return;
                }
            };
            if let Err(err) = store.set_setting("human:dashboard", name, &value) {
                if err.downcast_ref::<pulse_store::InputError>().is_some() {
                    crate::server::write_error(
                        w,
                        400,
                        err.to_string(),
                        "The value does not satisfy this setting's rule.".into(),
                        "Use a value shown in GET /api/v1/settings.".into(),
                    );
                } else {
                    crate::server::write_error(
                        w,
                        500,
                        err.to_string(),
                        "The store could not save the setting.".into(),
                        "Check pulse status and the store's write permissions.".into(),
                    );
                }
                return;
            }
            match self.settings() {
                Ok(settings) => {
                    if let Some(row) =
                        settings
                            .get("settings")
                            .and_then(Value::as_array)
                            .and_then(|rows| {
                                rows.iter().find(|row| {
                                    row.get("name").and_then(Value::as_str) == Some(name)
                                })
                            })
                    {
                        crate::server::write_json(
                            w,
                            200,
                            json!(map_from_pairs(vec![
                                ("ok", json!(true)),
                                ("setting", row.clone())
                            ])),
                        );
                        return;
                    }
                    crate::server::write_error(
                        w,
                        503,
                        "The saved setting is unavailable".into(),
                        "The settings report did not contain the updated key.".into(),
                        "Refresh the settings page.".into(),
                    );
                }
                Err(err) => self.respond(w, Value::Null, err.to_string()),
            }
            return;
        }
        let id = payload
            .get("id")
            .or_else(|| payload.get("post_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let answer = payload
            .get("answer")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let words = payload
            .get("words")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        match pulse_board::decide(store, &id, &answer, &words, pulse_store::HUMAN_AGENT) {
            Ok(decision) => crate::server::write_json(
                w,
                200,
                json!(map_from_pairs(vec![
                    ("ok", json!(true)),
                    ("result_post", json!(decision.result.id)),
                    ("post_id", json!(id)),
                    ("answer", json!(answer)),
                ])),
            ),
            Err(err) => {
                if err
                    .downcast_ref::<pulse_cli::Error>()
                    .is_some_and(|e| e.code == 2)
                {
                    crate::server::write_error(
                        w,
                        400,
                        err.to_string(),
                        "The proposal could not be decided.".into(),
                        "Refresh the proposal and choose yes or no.".into(),
                    );
                } else {
                    crate::server::write_error(
                        w,
                        500,
                        err.to_string(),
                        "The store could not save the decision.".into(),
                        "Check pulse status and the store's write permissions.".into(),
                    );
                }
            }
        }
    }
    /// Go: func (h *Handler) executors(r *http.Request) ([]map[string]any, error)
    pub(crate) fn executors(&self, r: &crate::httpx::Request) -> anyhow::Result<Vec<JsonMap>> {
        let Some(store) = &self.store else {
            return Ok(vec![]);
        };
        let mut rows = self.addition_rows(r, "SELECT * FROM executors ORDER BY name", vec![])?;
        let local = map_from_pairs(vec![
            ("name", json!("local")),
            ("kind", json!("local")),
            ("address", json!("")),
            ("workdir", json!("")),
            (
                "workers",
                json!(std::cmp::max(0, store.setting_int("workers"))),
            ),
            ("projects", json!("*")),
            ("status", json!("active")),
            ("last_probe_at", json!("")),
            ("probe_note", json!("this machine")),
            ("errors", json!(0)),
            ("backoff_minutes", json!(0)),
            ("next_probe_at", json!("")),
            ("runs_done", json!(0)),
            ("runs_failed", json!(0)),
            ("created_at", json!("")),
            ("created_by", json!("")),
            ("running", json!(0)),
            (
                "free",
                json!(std::cmp::max(0, store.setting_int("workers"))),
            ),
        ]);
        rows.insert(0, local);
        let horizon = (crate::read::parse_dt(&store.now()).unwrap_or_else(go_zero_time)
            - chrono::Duration::minutes(2 * store.setting_int("minutes_per_job") + 15))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let conn = store.conn()?;
        for row in &mut rows {
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            let running: i64 = conn.query_row(
                "SELECT count(*) FROM runs WHERE status='running' AND executor=? AND started_at>?",
                rusqlite::params![name, horizon],
                |r| r.get(0),
            )?;
            row.insert("running".into(), json!(running));
            let mut free = 0;
            if row.get("status").and_then(Value::as_str) == Some("active") {
                free = std::cmp::max(
                    0,
                    num_value(row.get("workers").unwrap_or(&Value::Null)) - running,
                );
            }
            row.insert("free".into(), json!(free));
        }
        Ok(rows)
    }
    /// Go: func (h *Handler) runner(r *http.Request) (map[string]any, error)
    pub(crate) fn runner(&self, r: &crate::httpx::Request) -> anyhow::Result<JsonMap> {
        let Some(store) = &self.store else {
            return Ok(map_from_pairs(vec![("available", json!(false))]));
        };
        let executors = self.executors(r)?;
        let mut executor_rows = Vec::new();
        let (mut free, mut slots, mut active, mut running) = (0, 0, 0, 0);
        let mut any_project = false;
        for row in &executors {
            executor_rows.push(crate::board::pick(
                row,
                "name kind status address workers projects last_probe_at probe_note running free",
            ));
            running += num_value(row.get("running").unwrap_or(&Value::Null));
            if row.get("status").and_then(Value::as_str) == Some("active") {
                active += 1;
                free += num_value(row.get("free").unwrap_or(&Value::Null));
                slots += num_value(row.get("workers").unwrap_or(&Value::Null));
                any_project |= num_value(row.get("free").unwrap_or(&Value::Null)) > 0
                    && row.get("projects").and_then(Value::as_str) == Some("*");
            }
        }
        let workers = self.addition_rows(
            r,
            "SELECT name FROM agents WHERE status='active' AND kind='worker' ORDER BY name",
            vec![],
        )?;
        let leases = self.addition_rows(
            r,
            "SELECT DISTINCT lease_agent FROM jobs WHERE status='leased'",
            vec![],
        )?;
        let leased = leases
            .iter()
            .filter_map(|row| {
                row.get("lease_agent")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let idle = workers
            .iter()
            .filter_map(|row| row.get("name").and_then(Value::as_str))
            .filter(|name| !leased.contains(*name))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let counts = self.addition_rows(
            r,
            "SELECT status,count(*) AS n FROM jobs WHERE status IN ('queued','leased','blocked') GROUP BY status",
            vec![],
        )?;
        let mut jobs = map_from_pairs(vec![
            ("queued", json!(0)),
            ("leased", json!(0)),
            ("blocked", json!(0)),
            ("running", json!(running)),
            ("runs_today", json!(0)),
            ("cap", json!(0)),
        ]);
        for count in counts {
            if let Some(status) = count.get("status").and_then(Value::as_str) {
                jobs.insert(
                    status.to_string(),
                    count.get("n").cloned().unwrap_or(json!(0)),
                );
            }
        }
        let runs_today: i64 = store.conn()?.query_row(
            "SELECT count(*) FROM runs WHERE mode!='dry-run' AND status!='error' AND substr(started_at,1,10)=?",
            [store.now().chars().take(10).collect::<String>()],
            |r| r.get(0),
        )?;
        jobs.insert("runs_today".into(), json!(runs_today));
        jobs.insert(
            "cap".into(),
            json!(std::cmp::max(
                0,
                store
                    .setting("jobs_per_day", "")
                    .parse::<i64>()
                    .unwrap_or(0)
            )),
        );
        let text = pulse_inspect::daemon_status(store)?;
        let mut state = text.split_whitespace().next().unwrap_or("").to_string();
        if text.starts_with("never") {
            state = "never started".into();
        }
        let mut daemon = map_from_pairs(vec![("state", json!(state)), ("text", json!(text))]);
        if let Ok((raw, true)) = store.meta("daemon") {
            match serde_json::from_str::<Value>(&raw) {
                Ok(beat) => {
                    daemon.insert(
                        "pid".into(),
                        beat.get("pid").cloned().unwrap_or(Value::Null),
                    );
                    daemon.insert("at".into(), beat.get("at").cloned().unwrap_or(json!("")));
                    daemon.insert(
                        "loops".into(),
                        json!(num_value(beat.get("loops").unwrap_or(&Value::Null))),
                    );
                    daemon.insert(
                        "interval".into(),
                        json!(num_value(beat.get("interval").unwrap_or(&Value::Null))),
                    );
                    daemon.insert(
                        "note".into(),
                        json!(beat.get("note").and_then(Value::as_str).unwrap_or("")),
                    );
                    // The pending handover: since when, how many runs are in
                    // flight and which the new build waits for (daemon.rs
                    // `restart_beat`); absent when no restart is pending.
                    if let Some(restart) = beat.get("restart").filter(|r| r.is_object()) {
                        daemon.insert("restart".into(), restart.clone());
                    }
                }
                Err(_) => {
                    daemon.insert("state".into(), json!("unreadable"));
                    daemon.insert("text".into(), json!("unreadable heartbeat"));
                }
            }
        }
        let stopped = pulse_inspect::runner_stopped(store)?;
        let capacity = pulse_inspect::capacity_status(store)?;
        let hold = pulse_inspect::capacity_hold(store)?;
        let landing = pulse_inspect::landing_notes(store)?;
        let hygiene = pulse_inspect::hygiene_status(store).unwrap_or_default();
        Ok(map_from_pairs(vec![
            ("available", json!(true)),
            ("now", json!(store.now())),
            ("daemon", json!(daemon)),
            (
                "runner",
                json!(map_from_pairs(vec![
                    ("allowed", json!(stopped.is_empty())),
                    ("reason", json!(stopped))
                ])),
            ),
            (
                "hands",
                json!(map_from_pairs(vec![
                    ("free", json!(free)),
                    ("slots", json!(slots)),
                    ("executors_active", json!(active)),
                    ("executors", json!(executor_rows.len())),
                    ("any_project", json!(any_project))
                ])),
            ),
            ("executors", json!(executor_rows)),
            (
                "workers",
                json!(map_from_pairs(vec![
                    ("pool", json!(workers.len())),
                    ("idle", json!(idle.len())),
                    ("busy", json!(workers.len().saturating_sub(idle.len()))),
                    (
                        "target",
                        json!(std::cmp::max(1, store.setting_int("quorum") + 1))
                    ),
                    ("idle_names", json!(idle)),
                    (
                        "prune_after_days",
                        json!(store.setting_int("prune_after_days"))
                    )
                ])),
            ),
            ("jobs", json!(jobs)),
            ("capacity", json!(capacity)),
            ("hold", json!(hold)),
            ("landing", go_value(&landing)),
            ("hygiene", json!(hygiene)),
        ]))
    }
    /// Go: func (h *Handler) roster(views []projectView) ([]map[string]any, error)
    pub(crate) fn roster(
        &self,
        views: &[crate::read::ProjectView],
    ) -> anyhow::Result<Vec<JsonMap>> {
        let Some(store) = &self.store else {
            return Ok(vec![]);
        };
        let stored = store.agents("")?;
        let mut people = BTreeMap::<String, JsonMap>::new();
        for agent in stored {
            people.insert(agent.name.clone(), new_person(&agent));
        }
        let now = store.now();
        let presence = pulse_board::agent_presences(store, &now)?;
        let focuses = pulse_board::agent_focuses(store, &now)?;
        let reference = crate::read::parse_dt(&now).unwrap_or_else(go_zero_time);
        for view in views {
            let Some(d) = &view.data else { continue };
            // Go appends the post maps themselves; borrow them here.
            let mut by_agent = BTreeMap::<String, Vec<&JsonMap>>::new();
            for post in &d.posts {
                by_agent
                    .entry(str_field(post, "agent"))
                    .or_default()
                    .push(post);
            }
            for (name, mut posts) in by_agent {
                let row = people.entry(name.clone()).or_insert_with(|| {
                    let runner = posts
                        .iter()
                        .map(|p| str_field(p, "runner"))
                        .max()
                        .unwrap_or_default();
                    let model = posts
                        .iter()
                        .map(|p| str_field(p, "model"))
                        .max()
                        .unwrap_or_default();
                    let mut row = new_person(&pulse_store::Agent {
                        name: name.clone(),
                        kind: "legacy".into(),
                        status: "active".into(),
                        runner,
                        model,
                        ..Default::default()
                    });
                    // Go: row["names"] = []string{}
                    row.insert("names".into(), json!(Vec::<String>::new()));
                    row
                });
                crate::board::order(&mut posts, &["-date"]);
                let mut label = String::new();
                let mut home = String::new();
                for p in &posts {
                    if label.is_empty() {
                        label = str_field(p, "name");
                    }
                    if home.is_empty() {
                        home = str_field(p, "project");
                    }
                    let date = str_field(p, "date");
                    if date > str_field(row, "last_seen") {
                        row.insert("last_seen".into(), json!(date));
                    }
                    if str_field(row, "first_seen").is_empty()
                        || date < str_field(row, "first_seen")
                    {
                        row.insert("first_seen".into(), json!(date));
                    }
                    match str_field(p, "kind").as_str() {
                        "vote" | "claim" | "question" | "answer" | "suggestion" | "finding" => {
                            let key = format!("{}s", str_field(p, "kind"));
                            let n = num_value(row.get(&key).unwrap_or(&Value::Null)) + 1;
                            row.insert(key, json!(n));
                        }
                        _ => {}
                    }
                }
                if !label.is_empty() {
                    row.insert(
                        "names".into(),
                        json!(unique_append(strings(row.get("names")), label)),
                    );
                }
                if num_value(row.get("store_posts").unwrap_or(&Value::Null)) == 0 {
                    row.insert("posts".into(), json!(posts.len()));
                }
                let mut project = view.project.slug.clone();
                if project == pulse_store::COMMONS_SLUG && !home.is_empty() {
                    project.push_str(" (from ");
                    project.push_str(&home);
                    project.push(')');
                }
                row.insert(
                    "projects".into(),
                    json!(unique_append(strings(row.get("projects")), project)),
                );
            }
            for claim in &d.claims {
                if claim.get("status").and_then(Value::as_str) != Some("active") {
                    continue;
                }
                if let Some(person) = people.get_mut(&str_field(claim, "agent")) {
                    let mut row = crate::board::pick(claim, "thread_key summary expires id");
                    row.insert("slug".into(), json!(view.project.slug));
                    row.insert(
                        "base".into(),
                        json!(crate::read::base_for(&view.project.slug)),
                    );
                    let mut claims = person
                        .get("active_claims")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    claims.push(json!(row));
                    person.insert("active_claims".into(), json!(claims));
                }
            }
        }
        let mut out = Vec::new();
        for mut row in people.into_values() {
            let name = str_field(&row, "agent");
            let p = presence
                .get(&name)
                .cloned()
                .unwrap_or(pulse_board::AgentPresence {
                    presence: "legacy".into(),
                    presence_reason: "historical identity; no supervised execution".into(),
                    presence_checked_at: now.clone(),
                    ..Default::default()
                });
            row.insert("presence".into(), json!(p.presence));
            row.insert("presence_reason".into(), json!(p.presence_reason));
            row.insert("last_heartbeat_at".into(), json!(p.last_heartbeat_at));
            row.insert("heartbeat_source".into(), json!(p.heartbeat_source));
            row.insert(
                "heartbeat_age_seconds".into(),
                json!(p.heartbeat_age_seconds),
            );
            row.insert("dead_after_seconds".into(), json!(p.dead_after_seconds));
            row.insert("presence_session".into(), json!(p.presence_session));
            row.insert("presence_checked_at".into(), json!(p.presence_checked_at));
            row.insert("presence_expires_at".into(), json!(p.presence_expires_at));
            if let Some(f) = focuses.get(&name) {
                row.insert("current_job".into(), json!(f.job_id));
                row.insert("current_job_kind".into(), json!(f.job_kind));
                row.insert("current_job_title".into(), json!(f.job_title));
                row.insert("current_thread".into(), json!(f.thread));
                row.insert("executor".into(), json!(f.executor));
                row.insert("focus_since".into(), json!(f.since));
                row.insert("current_task".into(), json!(f.task));
                row.insert("current_task_title".into(), json!(f.task_title));
                row.insert("current_project".into(), json!(f.project));
                row.insert("current_source".into(), json!(f.source));
            }
            if str_field(&row, "first_seen").is_empty() {
                row.insert(
                    "first_seen".into(),
                    row.get("created_at").cloned().unwrap_or(json!("")),
                );
            }
            if str_field(&row, "last_seen").is_empty() {
                let last = if str_field(&row, "last_active_at").is_empty() {
                    str_field(&row, "created_at")
                } else {
                    str_field(&row, "last_active_at")
                };
                row.insert("last_seen".into(), json!(last));
            }
            let recent = if str_field(&row, "last_useful_at").is_empty() {
                str_field(&row, "last_seen")
            } else {
                str_field(&row, "last_useful_at")
            };
            let last = crate::read::parse_dt(&recent).unwrap_or(reference);
            let days = ((reference - last).num_milliseconds() as f64 / 86_400_000.0)
                .max(0.0)
                .mul_add(1000.0, 0.0)
                .round_ties_even()
                / 1000.0;
            row.insert("days_since".into(), json!(days));
            if row.get("presence").and_then(Value::as_str) == Some("online")
                && row
                    .get("active_claims")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty())
            {
                row.insert("status".into(), json!("working"));
            }
            out.push(row);
        }
        out.sort_by(|a, b| {
            status_rank(a)
                .cmp(&status_rank(b))
                .then_with(|| {
                    // Go compares the float64 days_since values.
                    let days = |row: &JsonMap| {
                        row.get("days_since").and_then(Value::as_f64).unwrap_or(0.0)
                    };
                    days(a).total_cmp(&days(b))
                })
                .then_with(|| str_field(a, "agent").cmp(&str_field(b, "agent")))
        });
        Ok(out)
    }
}
/// Go: func newPerson(agent store.Agent) map[string]any
pub(crate) fn new_person(agent: &pulse_store::Agent) -> JsonMap {
    let name = if agent.display.is_empty() {
        &agent.name
    } else {
        &agent.display
    };
    map_from_pairs(vec![
        ("agent", json!(agent.name)),
        ("name", json!(name)),
        (
            "names",
            if name.is_empty() {
                json!([])
            } else {
                json!([name])
            },
        ),
        ("kind", json!(agent.kind)),
        ("status", json!(agent.status)),
        ("agent_status", json!(agent.status)),
        ("runner", json!(agent.runner)),
        ("model", json!(agent.model)),
        ("session", json!("")),
        ("sessions", json!(agent.sessions)),
        ("session_count", json!(agent.sessions.len())),
        ("jobs_done", json!(agent.jobs_done)),
        ("jobs_failed", json!(agent.jobs_failed)),
        ("posts", json!(agent.posts)),
        ("store_posts", json!(agent.posts)),
        ("pruned_at", json!(agent.pruned_at)),
        ("pruned_reason", json!(agent.pruned_reason)),
        ("last_useful_at", json!(agent.last_useful_at)),
        ("last_active_at", json!(agent.last_active_at)),
        ("created_at", json!(agent.created_at)),
        ("legacy_id", json!(agent.legacy_id)),
        ("notes", json!(agent.notes)),
        ("votes", json!(0)),
        ("claims", json!(0)),
        ("questions", json!(0)),
        ("answers", json!(0)),
        ("suggestions", json!(0)),
        ("findings", json!(0)),
        ("first_seen", json!("")),
        ("last_seen", json!("")),
        ("active_claims", json!([])),
        ("projects", json!([])),
        ("current_job", json!("")),
        ("current_job_kind", json!("")),
        ("current_job_title", json!("")),
        ("current_thread", json!("")),
        ("executor", json!("")),
        ("focus_since", json!("")),
        ("current_task", json!("")),
        ("current_task_title", json!("")),
        ("current_project", json!("")),
        ("current_source", json!("")),
        ("days_since", json!(0.0)),
        ("approximate", json!(false)),
        ("name_shared", json!(false)),
        ("shared_with", json!([])),
    ])
}
/// Go: func uniqueAppend(values []string, value string) []string
pub(crate) fn unique_append(values: Vec<String>, value: String) -> Vec<serde_json::Value> {
    let mut out = values;
    if !out.iter().any(|old| old == &value) {
        out.push(value);
    }
    out.into_iter().map(Value::String).collect()
}

fn map_from_pairs(pairs: Vec<(&str, Value)>) -> JsonMap {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

fn str_field(row: &JsonMap, key: &str) -> String {
    row.get(key)
        .map(crate::board::str_value)
        .unwrap_or_default()
}

fn num_value(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

fn go_zero_time() -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("Go zero time is representable")
        .and_utc()
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(ToOwned::to_owned))
        .collect()
}

fn status_rank(row: &JsonMap) -> i32 {
    match str_field(row, "status").as_str() {
        "working" => 0,
        "active" => 1,
        "idle" => 2,
        "pruned" => 3,
        _ => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_metadata() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        let data = crate::test_helpers_f3::handler(&s).metadata().unwrap();
        let info = std::fs::metadata(&s.path).unwrap();
        let sequence: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap();
        let mtime = info
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        assert_eq!(
            data,
            map_from_pairs(vec![
                ("store", json!(s.path)),
                ("schema_version", json!("3")),
                ("records_version", json!(pulse_store::RECORDS_VERSION)),
                ("binary_version", json!(pulse_cli::version())),
                ("generated_at", json!(s.now())),
                (
                    "fingerprint",
                    json!(format!("store:{mtime}:{}:{sequence}", info.len()))
                ),
                ("tool", json!(format!("pulse {}", pulse_cli::version()))),
                (
                    "note",
                    json!("computed from the store; no projection database")
                ),
            ])
        );
        s.event("human", "metadata.changed", "", Value::Null)
            .unwrap();
        let updated = crate::test_helpers_f3::handler(&s).metadata().unwrap();
        assert_ne!(updated["fingerprint"], data["fingerprint"]);
    }
}
