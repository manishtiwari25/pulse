//! In-process event bus for store events.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: String,
}

impl Event {
    pub fn project(&self) -> String {
        detail_field(&self.detail, "project")
    }
    pub fn thread(&self) -> String {
        detail_field(&self.detail, "thread")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub actions: Vec<String>,
    pub project: String,
    pub subject: String,
    pub area: String,
    pub thread: String,
}

impl Filter {
    pub fn match_event(&self, e: &Event) -> bool {
        if !self.actions.is_empty() && !self.actions.iter().any(|g| glob(g, &e.action)) {
            return false;
        }
        if !self.subject.is_empty() && !glob(&self.subject, &e.subject) {
            return false;
        }
        if !self.project.is_empty() && e.project() != self.project && e.subject != self.project {
            return false;
        }
        if !self.area.is_empty() || !self.thread.is_empty() {
            let thread = e.thread();
            if thread.is_empty() {
                return false;
            }
            if !self.area.is_empty()
                && !thread.starts_with(&(self.area.clone() + "/"))
                && thread != self.area
            {
                return false;
            }
            if !self.thread.is_empty()
                && thread != self.thread
                && !thread.ends_with(&format!("/{}", self.thread))
            {
                return false;
            }
        }
        true
    }
}

pub fn glob(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if !pattern.contains(['*', '?', '[']) {
        return name == pattern || name.starts_with(&(pattern.to_string() + "."));
    }
    glob_match(pattern.as_bytes(), name.as_bytes())
}

fn glob_match(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        b'*' => (0..=s.len()).any(|i| glob_match(&p[1..], &s[i..])),
        b'?' => !s.is_empty() && glob_match(&p[1..], &s[1..]),
        c => !s.is_empty() && c == s[0] && glob_match(&p[1..], &s[1..]),
    }
}

struct Subscriber {
    tx: Sender<Event>,
    filter: Filter,
}

pub struct Bus {
    subs: Mutex<HashMap<usize, Subscriber>>,
    next: Mutex<usize>,
    dropped: AtomicI64,
    seen: Mutex<(HashMap<String, ()>, VecDeque<String>)>,
    deduped: AtomicI64,
}

const SEEN_SIZE: usize = 4096;

pub fn new() -> Bus {
    Bus {
        subs: Mutex::new(HashMap::new()),
        next: Mutex::new(0),
        dropped: AtomicI64::new(0),
        seen: Mutex::new((HashMap::new(), VecDeque::new())),
        deduped: AtomicI64::new(0),
    }
}

impl Default for Bus {
    fn default() -> Self {
        new()
    }
}

impl Bus {
    pub fn subscribe(
        &self,
        filter: Filter,
        buffer: usize,
    ) -> (Receiver<Event>, impl FnOnce() + Send + 'static) {
        let cap = if buffer == 0 { 64 } else { buffer };
        let (tx, rx) = bounded(cap);
        let mut next = self.next.lock();
        let id = *next;
        *next += 1;
        drop(next);
        self.subs.lock().insert(id, Subscriber { tx, filter });
        let bus = default();
        let cancel = move || {
            bus.subs.lock().remove(&id);
        };
        (rx, cancel)
    }

    pub fn publish(&self, e: Event) {
        for s in self.subs.lock().values() {
            if !s.filter.match_event(&e) {
                continue;
            }
            if s.tx.try_send(e.clone()).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn dropped(&self) -> i64 {
        self.dropped.load(Ordering::Relaxed)
    }
    pub fn subscribers(&self) -> usize {
        self.subs.lock().len()
    }
    pub fn deduped(&self) -> i64 {
        self.deduped.load(Ordering::Relaxed)
    }

    pub fn publish_once(&self, e: Event) -> bool {
        if e.id != 0 {
            let key = format!("{}|{}|{}|{}", e.id, e.at, e.action, e.subject);
            let mut seen = self.seen.lock();
            if seen.0.contains_key(&key) {
                self.deduped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            if seen.1.len() >= SEEN_SIZE
                && let Some(old) = seen.1.pop_front()
            {
                seen.0.remove(&old);
            }
            seen.0.insert(key.clone(), ());
            seen.1.push_back(key);
        }
        self.publish(e);
        true
    }
}

static DEFAULT: OnceLock<Arc<Bus>> = OnceLock::new();

pub fn default() -> Arc<Bus> {
    DEFAULT.get_or_init(|| Arc::new(new())).clone()
}

pub fn reset_default() -> Arc<Bus> {
    default()
}

pub fn emit(
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: impl Into<pulse_store::Detail>,
) -> Result<Event> {
    emit_on(&default(), s, actor, action, subject, detail)
}

pub fn emit_on(
    b: &Bus,
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: impl Into<pulse_store::Detail>,
) -> Result<Event> {
    let text = detail.into().text()?;
    let tx = s.begin()?;
    tx.execute(
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![s.now(), actor, action, subject, text],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    let e = Event {
        id,
        at: s.now(),
        actor: actor.into(),
        action: action.into(),
        subject: subject.into(),
        detail: text,
    };
    b.publish_once(e.clone());
    Ok(e)
}

pub fn publish_after(events: &[Event]) {
    let b = default();
    for e in events {
        b.publish_once(e.clone());
    }
}

pub fn detail_text(detail: impl Into<pulse_store::Detail>) -> Result<String> {
    detail.into().text()
}

pub fn last_id(s: &pulse_store::Store) -> Result<i64> {
    let conn = s.conn()?;
    let id: Option<i64> = conn
        .query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))
        .ok();
    Ok(id.unwrap_or(0))
}

pub fn since(
    s: &pulse_store::Store,
    since_id: i64,
    filter: Filter,
    limit: usize,
) -> Result<Vec<Event>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT seq, at, actor, action, subject, detail FROM events WHERE seq > ? ORDER BY seq",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([since_id], scan_event)? {
        let e = row?;
        if filter.match_event(&e) {
            out.push(e);
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

pub fn latest(
    s: &pulse_store::Store,
    since_id: i64,
    filter: Filter,
    limit: usize,
) -> Result<Vec<Event>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT seq, at, actor, action, subject, detail FROM events WHERE seq > ? ORDER BY seq DESC",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([since_id], scan_event)? {
        let e = row?;
        if filter.match_event(&e) {
            out.push(e);
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
    }
    out.reverse();
    Ok(out)
}

fn scan_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: row.get(0)?,
        at: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        subject: row.get(4)?,
        detail: row.get(5)?,
    })
}

pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

pub const PRIVATE_VALUE: &str = "[owner-private]";

pub fn redact(mut e: Event) -> Event {
    if e.action == "setting" && e.subject == "evolve.heldout_dir" && !e.detail.is_empty() {
        e.detail = PRIVATE_VALUE.into();
    }
    e
}

pub fn redactor(_s: Option<&pulse_store::Store>) -> impl Fn(Event) -> Event {
    redact
}

fn detail_field(detail: &str, key: &str) -> String {
    if !detail.trim_start().starts_with('{') {
        return String::new();
    }
    serde_json::from_str::<Value>(detail)
        .ok()
        .and_then(|v| v.get(key).and_then(Value::as_str).map(ToString::to_string))
        .unwrap_or_default()
}

pub fn install_store_observer() {
    pulse_store::on_event(|r| {
        default().publish_once(Event {
            id: r.seq,
            at: r.at,
            actor: r.actor,
            action: r.action,
            subject: r.subject,
            detail: r.detail,
        });
    });
}
