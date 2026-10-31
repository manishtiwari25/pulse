//! Adding a machine from the console: `POST /api/v1/compute/executors`.
//!
//! Three ways in, one route, told apart by `mode`:
//!
//! - `pair` (the default): the hub reaches `admin@host` over ssh and does
//!   what `pulse pair` does - creates the `pulse` user, installs the hands,
//!   registers and probes the executor. It takes minutes, so the request
//!   answers 202 at once and the pairing runs on a thread; `GET
//!   /api/v1/compute` lists it under `pairings` with its report as it
//!   grows, until the owner dismisses it
//!   (`POST /api/v1/compute/pairings/{name}/dismiss`).
//! - `register`: a machine someone prepared by hand is added as a drained
//!   executor (`pulse executor add`), probed right away when `probe` is
//!   true (`pulse executor probe`).
//! - `request`: the owner asks the cloud provider for a machine the way an
//!   agent does (`pulse compute request`), approved on the spot unless
//!   `approve` is false.
//!
//! Validation happens on the request thread, so a bad name, address or
//! label answers 400 and never starts a pairing. The pairing itself goes
//! through `pairer`, a function the tests replace.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pulse_runner::machines::{PairMachine, PairOutcome, RegisterMachine};
use serde::Serialize;
use serde_json::{Value, json};

use crate::additions::{
    ADDITION_WRITES, JsonMap, addition_failure, addition_method, addition_string,
};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, read_payload, write_error, write_json};
use rusqlite::types::Value as SqlValue;

/// One pairing the hub started, running or finished, until dismissed.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Pairing {
    pub name: String,
    pub admin: String,
    pub by: String,
    pub started: String,
    /// Empty while it runs.
    pub finished: String,
    /// `running`, `ok` or `failed`.
    pub status: String,
    pub ok: bool,
    pub dry_run: bool,
    pub lines: Vec<String>,
    pub changed: Vec<String>,
}

pub(crate) type Pairings = Arc<Mutex<BTreeMap<String, Pairing>>>;

/// Runs one pairing to its end; the real one is `pulse_runner::machines::pair_machine`.
pub(crate) type Pairer = Arc<
    dyn Fn(&pulse_store::Store, &PairMachine, &str) -> anyhow::Result<PairOutcome> + Send + Sync,
>;

pub(crate) fn real_pairer() -> Pairer {
    Arc::new(|s, m, actor| pulse_runner::machines::pair_machine(s, &s.now(), m, actor))
}

const RUNNING: &str = "running";

fn payload_int(payload: &JsonMap, key: &str, fallback: i64) -> Option<i64> {
    match payload.get(key) {
        None | Some(Value::Null) => Some(fallback),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.trunc() == *f).map(|f| f as i64)),
        Some(Value::String(t)) if t.trim().is_empty() => Some(fallback),
        Some(Value::String(t)) => t.trim().parse::<i64>().ok(),
        Some(_) => None,
    }
}

fn payload_bool(payload: &JsonMap, key: &str, fallback: bool) -> Option<bool> {
    match payload.get(key) {
        None | Some(Value::Null) => Some(fallback),
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::Number(n)) => n.as_i64().map(|n| n != 0),
        Some(Value::String(t)) => match t.trim() {
            "" => Some(fallback),
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        Some(_) => None,
    }
}

fn invalid(w: &mut ResponseWriter, key: &str, must: &str) {
    write_error(
        w,
        400,
        format!("Invalid {key}"),
        format!("{key} must be {must}."),
        format!("Send a JSON object with a valid {key}."),
    );
}

/// A refusal from the runner's own checks (a `pulse_cli::Error`, exit 2) is
/// the request's fault: 400 with its words. Anything else keeps the
/// additions' mapping.
fn machine_failure(w: &mut ResponseWriter, err: anyhow::Error, fix: &str) {
    if let Some(e) = err.downcast_ref::<pulse_cli::Error>()
        && e.code == 2
    {
        let why = if e.why.is_empty() {
            "The machine was not added.".to_string()
        } else {
            e.why.clone()
        };
        let fix = if e.fix.is_empty() {
            fix.to_string()
        } else {
            e.fix.clone()
        };
        write_error(w, 400, e.what.clone(), why, fix);
        return;
    }
    addition_failure(w, err.to_string());
}

impl Handler {
    /// `GET /api/v1/compute` carries these: newest first.
    pub(crate) fn pairings_json(&self) -> Vec<Value> {
        let mut all: Vec<Pairing> = self
            .pairings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        all.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| a.name.cmp(&b.name)));
        all.into_iter()
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .collect()
    }

    pub(crate) fn addition_machine_add(&self, w: &mut ResponseWriter, r: &Request) {
        if !addition_method(w, r, vec!["POST".into()])
            || !self.addition_available(w, r, vec!["executors".into()])
        {
            return;
        }
        let (payload, ok) = read_payload(w, r);
        if !ok {
            return;
        }
        let (mode, ok) = addition_string(w, &payload, "mode", false);
        if !ok {
            return;
        }
        match mode.trim() {
            "" | "pair" => self.machine_pair(w, r, &payload),
            "register" => self.machine_register(w, r, &payload),
            "request" => self.machine_request(w, r, &payload),
            _ => invalid(w, "mode", "pair, register or request"),
        }
    }

    fn machine_fields(
        &self,
        w: &mut ResponseWriter,
        payload: &JsonMap,
    ) -> Option<(String, String, i32, String, String)> {
        let (name, ok) = addition_string(w, payload, "name", true);
        if !ok {
            return None;
        }
        let (workdir, ok) = addition_string(w, payload, "workdir", false);
        if !ok {
            return None;
        }
        let Some(workers) = payload_int(payload, "workers", 1) else {
            invalid(w, "workers", "a whole number of workers, at least 1");
            return None;
        };
        if !(1..=64).contains(&workers) {
            invalid(w, "workers", "a whole number of workers, 1 to 64");
            return None;
        }
        let (projects, ok) = addition_string(w, payload, "projects", false);
        if !ok {
            return None;
        }
        let (labels, ok) = addition_string(w, payload, "labels", false);
        if !ok {
            return None;
        }
        Some((name, workdir, workers as i32, projects, labels))
    }

    fn executor_json(&self, r: &Request, name: &str) -> Value {
        match self.addition_rows(
            r,
            "SELECT * FROM executors WHERE name=?",
            vec![SqlValue::Text(name.to_string())],
        ) {
            Ok(mut rows) if !rows.is_empty() => {
                serde_json::to_value(rows.remove(0)).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        }
    }

    fn machine_register(&self, w: &mut ResponseWriter, r: &Request, payload: &JsonMap) {
        let Some((name, workdir, workers, projects, labels)) = self.machine_fields(w, payload)
        else {
            return;
        };
        let (mut address, ok) = addition_string(w, payload, "ssh", false);
        if !ok {
            return;
        }
        if address.trim().is_empty() {
            let (alt, ok) = addition_string(w, payload, "address", false);
            if !ok {
                return;
            }
            address = alt;
        }
        let Some(probe) = payload_bool(payload, "probe", false) else {
            invalid(w, "probe", "true or false");
            return;
        };
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let m = RegisterMachine {
            name,
            address,
            workdir,
            workers,
            projects,
            labels,
        };
        let added = {
            let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
            match pulse_runner::machines::register_machine(
                store,
                &store.now(),
                &m,
                pulse_store::HUMAN_AGENT,
            ) {
                Ok(a) => a,
                Err(e) => {
                    machine_failure(w, e, "Correct the machine's details and add it again.");
                    return;
                }
            }
        };
        // The probe reaches the machine over ssh and may take a minute; it
        // runs without the writes lock so other owners' writes go on.
        let probe_json = if probe {
            match pulse_runner::machines::probe_machine(
                store,
                &store.now(),
                &added.name,
                pulse_store::HUMAN_AGENT,
            ) {
                Ok(p) => serde_json::to_value(p).unwrap_or(Value::Null),
                Err(e) => {
                    json!({"ok": false, "status": added.status, "note": e.to_string(), "line": format!("{} probe did not run: {e}", added.name)})
                }
            }
        } else {
            Value::Null
        };
        let executor = self.executor_json(r, &added.name);
        write_json(
            w,
            201,
            json!({"executor": executor, "added": added, "probe": probe_json}),
        );
    }

    fn machine_pair(&self, w: &mut ResponseWriter, r: &Request, payload: &JsonMap) {
        let Some((name, workdir, workers, projects, labels)) = self.machine_fields(w, payload)
        else {
            return;
        };
        let (admin, ok) = addition_string(w, payload, "admin", false);
        if !ok {
            return;
        }
        let Some(dry_run) = payload_bool(payload, "dry_run", false) else {
            invalid(w, "dry_run", "true or false");
            return;
        };
        let m = PairMachine {
            name,
            admin,
            workdir,
            workers,
            projects,
            labels,
            dry_run,
        };
        let name = match pulse_runner::machines::pair_check(&m) {
            Ok(n) => n,
            Err(e) => {
                machine_failure(w, e, "Correct the machine's details and pair it again.");
                return;
            }
        };
        let Some(store) = self.store.clone() else {
            return;
        };
        let now = store.now();
        if !dry_run {
            let taken = self
                .addition_rows(
                    r,
                    "SELECT name FROM executors WHERE name=? AND status != 'removed'",
                    vec![SqlValue::Text(name.clone())],
                )
                .map(|rows| !rows.is_empty())
                .unwrap_or(false);
            if taken {
                write_error(
                    w,
                    409,
                    format!("Executor {name} already exists"),
                    "A machine with this name is already paired.".to_string(),
                    format!(
                        "Pick another name, or remove {name} first (its Remove in the machines list)."
                    ),
                );
                return;
            }
        }
        let pairing = {
            let mut all = self.pairings.lock().unwrap_or_else(|e| e.into_inner());
            if all.get(&name).is_some_and(|p| p.status == RUNNING) {
                write_error(
                    w,
                    409,
                    format!("Already pairing {name}"),
                    "A pairing with this name is still running.".to_string(),
                    "Wait for it to finish; its report lands in the machines list.".to_string(),
                );
                return;
            }
            let p = Pairing {
                name: name.clone(),
                admin: m.admin.trim().to_string(),
                by: pulse_store::HUMAN_AGENT.to_string(),
                started: now.clone(),
                status: RUNNING.to_string(),
                dry_run,
                lines: vec![format!("pairing executor {name}")],
                ..Default::default()
            };
            all.insert(name.clone(), p.clone());
            p
        };
        let pairings = self.pairings.clone();
        let pairer = self.pairer.clone();
        let thread_name = format!("pulse-pair-{name}");
        let spawned = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let result = pairer(&store, &m, pulse_store::HUMAN_AGENT);
                let finished = store.now();
                let mut all = pairings.lock().unwrap_or_else(|e| e.into_inner());
                let Some(p) = all.get_mut(&name) else {
                    return;
                };
                p.finished = finished;
                match result {
                    Ok(out) => {
                        p.ok = out.ok;
                        p.status = if out.ok { "ok" } else { "failed" }.to_string();
                        p.lines = out.lines;
                        p.changed = out.changed;
                    }
                    Err(e) => {
                        p.ok = false;
                        p.status = "failed".to_string();
                        p.lines.push(e.to_string());
                    }
                }
            });
        if let Err(e) = spawned {
            let mut all = self.pairings.lock().unwrap_or_else(|e| e.into_inner());
            all.remove(&pairing.name);
            addition_failure(w, format!("could not start the pairing: {e}"));
            return;
        }
        write_json(
            w,
            202,
            json!({"pairing": serde_json::to_value(&pairing).unwrap_or(Value::Null)}),
        );
    }

    fn machine_request(&self, w: &mut ResponseWriter, r: &Request, payload: &JsonMap) {
        if !self.addition_available(w, r, vec!["compute_requests".into()]) {
            return;
        }
        let (mut project, ok) = addition_string(w, payload, "project", self.project.is_empty());
        if !ok {
            return;
        }
        if project.is_empty() {
            project = self.project.clone();
        }
        if !self.project.is_empty() && project != self.project {
            write_error(
                w,
                400,
                "Invalid project".to_string(),
                "This hub serves one project.".to_string(),
                format!("Request the machine for {}.", self.project),
            );
            return;
        }
        let (size, ok) = addition_string(w, payload, "size", false);
        if !ok {
            return;
        }
        let Some(hours) = payload_int(payload, "hours", 4) else {
            invalid(w, "hours", "a whole number of hours, at least 1");
            return;
        };
        if !(1..=i32::MAX as i64).contains(&hours) {
            invalid(w, "hours", "a whole number of hours, at least 1");
            return;
        }
        let (reason, ok) = addition_string(w, payload, "reason", true);
        if !ok {
            return;
        }
        let Some(approve) = payload_bool(payload, "approve", true) else {
            invalid(w, "approve", "true or false");
            return;
        };
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let req = match pulse_tasks::request_compute(
            store,
            &project,
            pulse_store::HUMAN_AGENT,
            &size,
            hours as i32,
            &reason,
        ) {
            Ok(req) => req,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if approve
            && req.status == "pending"
            && let Err(e) = pulse_tasks::approve(
                store,
                &req.id,
                "asked from the console",
                pulse_store::HUMAN_AGENT,
            )
        {
            addition_failure(w, e.to_string());
            return;
        }
        let query = "SELECT id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at FROM compute_requests WHERE id=?";
        match self.addition_rows(r, query, vec![SqlValue::Text(req.id.clone())]) {
            Ok(mut rows) if !rows.is_empty() => {
                write_json(w, 201, json!({"request": rows.remove(0)}));
            }
            Ok(_) => addition_failure(w, "not found"),
            Err(e) => addition_failure(w, e.to_string()),
        }
    }

    pub(crate) fn addition_pairing_dismiss(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        name: String,
    ) {
        if !addition_method(w, r, vec!["POST".into()])
            || !self.addition_available(w, r, vec!["executors".into()])
        {
            return;
        }
        let (_payload, ok) = read_payload(w, r);
        if !ok {
            return;
        }
        let mut all = self.pairings.lock().unwrap_or_else(|e| e.into_inner());
        match all.get(&name) {
            None => write_error(
                w,
                404,
                format!("No pairing {name}"),
                "No pairing with this name is listed.".to_string(),
                "Refresh the machines list.".to_string(),
            ),
            Some(p) if p.status == RUNNING => write_error(
                w,
                409,
                format!("Still pairing {name}"),
                "A running pairing cannot be dismissed.".to_string(),
                "Wait for it to finish.".to_string(),
            ),
            Some(_) => {
                all.remove(&name);
                write_json(w, 200, json!({"dismissed": name}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http as http;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn settle(h: &Handler, name: &str) -> Pairing {
        let start = Instant::now();
        loop {
            let p = h
                .pairings
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .expect("pairing listed");
            if p.status != RUNNING {
                return p;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "pairing never finished: {p:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn machine_register_from_the_console() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let h = http::handler(&s);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"register","name":"Box One","ssh":"pulse@10.0.0.9","labels":"gpu, docker","workers":2,"projects":"one"}"#,
        );
        assert_eq!(status, 201, "register: {result:?}");
        assert_eq!(result["executor"]["name"], "box-one");
        assert_eq!(result["executor"]["status"], "drained");
        assert_eq!(result["executor"]["workers"], 2);
        assert_eq!(result["executor"]["projects"], "one");
        assert_eq!(result["executor"]["workdir"], "/srv/pulse");
        assert_eq!(result["added"]["name"], "box-one");
        assert!(
            result["added"]["line"]
                .as_str()
                .unwrap()
                .contains("drained until `pulse executor probe box-one` passes"),
            "{result:?}"
        );
        assert!(
            result["added"]["offers"].as_str().unwrap().contains("gpu"),
            "{result:?}"
        );
        assert_eq!(result["probe"], Value::Null);
        let (status, listed) = http::additions_request(h.clone(), "GET", "/api/v1/compute", "");
        assert_eq!(status, 200);
        let names: Vec<&str> = listed["executors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert_eq!(names, vec!["box-one"]);
        assert_eq!(listed["pairings"], json!([]));
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"register","name":"box-one","ssh":"pulse@10.0.0.9"}"#,
        );
        assert_eq!(status, 400, "twice: {result:?}");
        assert!(
            result["error"].as_str().unwrap().contains("already exists"),
            "{result:?}"
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"register","name":"box-two"}"#,
        );
        assert_eq!(status, 400, "no address: {result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("ssh address is needed"),
            "{result:?}"
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"register","name":"box-two","ssh":"pulse@10.0.0.10","labels":"GPU!"}"#,
        );
        assert_eq!(status, 400, "bad labels: {result:?}");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"register","name":"box-two","ssh":"pulse@10.0.0.10","workers":0}"#,
        );
        assert_eq!(status, 400, "zero workers: {result:?}");
        assert_eq!(result["error"], "Invalid workers");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"sideways","name":"box-two"}"#,
        );
        assert_eq!(status, 400);
        assert_eq!(result["error"], "Invalid mode");
        let (status, _) = http::additions_request(h, "GET", "/api/v1/compute/executors", "");
        assert_eq!(status, 405);
    }

    #[test]
    fn machine_pairing_runs_in_the_background_and_is_listed() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let mut h = http::handler(&s);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        h.pairer = Arc::new(move |s, m, actor| {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(actor, pulse_store::HUMAN_AGENT);
            assert_eq!(m.admin, "admin@10.0.0.5");
            assert_eq!(m.labels, "xcode");
            std::thread::sleep(Duration::from_millis(150));
            s.conn().unwrap().execute("INSERT INTO executors (name, address, workdir, workers, projects, status, created_at) VALUES ('mac-mini', 'pulse@mac-mini', '/srv/pulse', 1, '*', 'active', ?)", [s.now()]).unwrap();
            Ok(PairOutcome {
                ok: true,
                lines: vec![
                    "pairing executor mac-mini".into(),
                    "probe mac-mini ok".into(),
                ],
                changed: vec!["~/.ssh/config".into()],
            })
        });
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"name":"local","admin":"admin@10.0.0.5"}"#,
        );
        assert_eq!(status, 400, "bad name: {result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("Executor name must be"),
            "{result:?}"
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"name":"mac-mini"}"#,
        );
        assert_eq!(status, 400, "no admin: {result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("admin ssh address is needed"),
            "{result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "refused before the pairer");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"pair","name":"Mac Mini","admin":"admin@10.0.0.5","labels":"xcode"}"#,
        );
        assert_eq!(status, 202, "pair: {result:?}");
        assert_eq!(result["pairing"]["name"], "mac-mini");
        assert_eq!(result["pairing"]["status"], "running");
        assert_eq!(result["pairing"]["admin"], "admin@10.0.0.5");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"pair","name":"mac-mini","admin":"admin@10.0.0.5"}"#,
        );
        assert_eq!(status, 409, "twice while running: {result:?}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("Already pairing"),
            "{result:?}"
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/pairings/mac-mini/dismiss",
            "{}",
        );
        assert_eq!(status, 409, "dismiss while running: {result:?}");
        let p = settle(&h, "mac-mini");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(p.status, "ok");
        assert!(p.ok);
        assert!(!p.finished.is_empty());
        assert_eq!(
            p.lines,
            vec!["pairing executor mac-mini", "probe mac-mini ok"]
        );
        let (status, listed) = http::additions_request(h.clone(), "GET", "/api/v1/compute", "");
        assert_eq!(status, 200);
        assert_eq!(listed["pairings"][0]["name"], "mac-mini");
        assert_eq!(listed["pairings"][0]["status"], "ok");
        assert_eq!(listed["pairings"][0]["changed"][0], "~/.ssh/config");
        assert_eq!(listed["executors"][0]["name"], "mac-mini");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"pair","name":"mac-mini","admin":"admin@10.0.0.5"}"#,
        );
        assert_eq!(status, 409, "paired already: {result:?}");
        assert!(
            result["error"].as_str().unwrap().contains("already exists"),
            "{result:?}"
        );
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/pairings/mac-mini/dismiss",
            "{}",
        );
        assert_eq!(status, 200, "dismiss: {result:?}");
        assert_eq!(result["dismissed"], "mac-mini");
        let (status, listed) = http::additions_request(h.clone(), "GET", "/api/v1/compute", "");
        assert_eq!(status, 200);
        assert_eq!(listed["pairings"], json!([]));
        let (status, _) =
            http::additions_request(h, "POST", "/api/v1/compute/pairings/mac-mini/dismiss", "{}");
        assert_eq!(status, 404);
    }

    #[test]
    fn machine_pairing_failure_keeps_its_report() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let mut h = http::handler(&s);
        h.pairer = Arc::new(|_, _, _| anyhow::bail!("inspection failed: ssh: connect refused"));
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"name":"box","admin":"admin@10.0.0.6","dry_run":true}"#,
        );
        assert_eq!(status, 202, "pair: {result:?}");
        assert_eq!(result["pairing"]["dry_run"], true);
        let p = settle(&h, "box");
        assert_eq!(p.status, "failed");
        assert!(!p.ok);
        assert_eq!(
            p.lines,
            vec![
                "pairing executor box",
                "inspection failed: ssh: connect refused"
            ]
        );
        let (status, listed) = http::additions_request(h, "GET", "/api/v1/compute", "");
        assert_eq!(status, 200);
        assert_eq!(listed["pairings"][0]["status"], "failed");
        assert_eq!(listed["executors"], json!([]));
    }

    #[test]
    fn machine_request_from_the_console_is_approved_on_the_spot() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let h = http::handler(&s);
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"request","project":"one","size":"small","hours":3,"reason":"a long build"}"#,
        );
        assert_eq!(status, 201, "request: {result:?}");
        assert_eq!(result["request"]["project"], "one");
        assert_eq!(result["request"]["requested_by"], "human");
        assert_eq!(result["request"]["size"], "small");
        assert_eq!(result["request"]["hours"], 3);
        assert_eq!(result["request"]["status"], "approved");
        assert_eq!(result["request"]["decided_by"], "human");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"request","project":"one","size":"small","hours":2,"reason":"later","approve":false}"#,
        );
        assert_eq!(status, 201, "pending: {result:?}");
        assert_eq!(result["request"]["status"], "pending");
        let (status, result) = http::additions_request(
            h.clone(),
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"request","project":"one","size":"small","hours":2}"#,
        );
        assert_eq!(status, 400, "no reason: {result:?}");
        assert_eq!(result["error"], "Invalid reason");
        let (status, result) = http::additions_request(
            h,
            "POST",
            "/api/v1/compute/executors",
            r#"{"mode":"request","project":"nobody","size":"small","hours":2,"reason":"x"}"#,
        );
        assert!(
            status == 400 || status == 404,
            "no project: {status} {result:?}"
        );
    }
}
