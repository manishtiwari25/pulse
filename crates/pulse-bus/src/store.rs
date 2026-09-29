use crate::bus::{Bus, Event, Filter, default};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, bounded};
use rusqlite::OptionalExtension;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// DefaultInterval is how often tail polls when interval is zero.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

/// The stand-in for Go's context.Context: clones share one cancellation flag
/// (handles to the same context); `with_timeout`/`child` derive a new context
/// that is done when its parent is done, when it is cancelled itself, or when
/// its deadline passes.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    parent: Option<Arc<CancellationToken>>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: context.WithCancel(parent).
    pub fn child(&self) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: self.deadline,
            parent: Some(Arc::new(self.clone())),
        }
    }

    /// Go: context.WithTimeout(parent, d).
    pub fn with_timeout(&self, d: Duration) -> Self {
        self.with_deadline(Instant::now() + d)
    }

    /// Go: context.WithDeadline(parent, at); an earlier parent deadline wins.
    pub fn with_deadline(&self, at: Instant) -> Self {
        let mut c = self.child();
        c.deadline = Some(match self.deadline {
            Some(p) if p < at => p,
            _ => at,
        });
        c
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Go: ctx.Err() != nil.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
            || self.deadline_exceeded()
            || self.parent.as_ref().is_some_and(|p| p.is_cancelled())
    }

    /// Go: ctx.Deadline().
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Go: ctx.Err() == context.DeadlineExceeded.
    pub fn deadline_exceeded(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Time left until the deadline, if any (zero once it has passed).
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline.map(|d| d.saturating_duration_since(Instant::now()))
    }
}

pub fn emit(
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: impl Into<pulse_store::Detail>,
) -> Result<Event> {
    emit_on(Some(default()), s, actor, action, subject, detail)
}

pub fn emit_on(
    b: Option<Arc<Bus>>,
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: impl Into<pulse_store::Detail>,
) -> Result<Event> {
    let text = detail_text(detail)?;
    let at = s.now();
    let conn = s.conn()?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        conn.execute(
            "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![at, actor, action, subject, text],
        )
        .with_context(|| format!("event {action}"))?;
        Ok::<i64, anyhow::Error>(conn.last_insert_rowid())
    })();
    match result {
        Ok(id) => {
            conn.execute_batch("COMMIT")?;
            let e = Event {
                id,
                at,
                actor: actor.to_string(),
                action: action.to_string(),
                subject: subject.to_string(),
                detail: text,
            };
            if let Some(b) = b {
                b.publish_once(e.clone());
            }
            Ok(e)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
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
    Ok(conn
        .query_row("SELECT MAX(seq) FROM events", [], |r| {
            r.get::<_, Option<i64>>(0)
        })?
        .unwrap_or(0))
}

pub fn since(
    s: &pulse_store::Store,
    since_id: i64,
    filter: Filter,
    limit: usize,
) -> Result<Vec<Event>> {
    let conn = s.conn()?;
    since_conn(&conn, since_id, &filter, limit)
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
        if !filter.match_event(&e) {
            continue;
        }
        out.push(e);
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
    out.reverse();
    Ok(out)
}

fn since_conn(
    conn: &rusqlite::Connection,
    since_id: i64,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT seq, at, actor, action, subject, detail FROM events WHERE seq > ? ORDER BY seq",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([since_id], scan_event)? {
        let e = row?;
        if !filter.match_event(&e) {
            continue;
        }
        out.push(e);
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
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

pub fn tail(
    cancel: CancellationToken,
    s: pulse_store::Store,
    since_id: i64,
    filter: Filter,
    interval: Duration,
) -> Receiver<Event> {
    let interval = if interval.is_zero() {
        DEFAULT_INTERVAL
    } else {
        interval
    };
    let (tx, rx) = bounded(64);
    thread::spawn(move || {
        let Ok(conn) = s.conn() else {
            return;
        };
        let mut last = since_id;
        if last < 0 {
            let id = conn
                .query_row("SELECT MAX(seq) FROM events", [], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .optional()
                .ok()
                .flatten()
                .flatten()
                .unwrap_or(0);
            last = id;
        }
        let mut version = -1_i64;
        while !cancel.is_cancelled() {
            let Ok(v) = conn.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)) else {
                return;
            };
            if v != version {
                version = v;
                let Ok(events) = since_conn(&conn, last, &Filter::default(), 0) else {
                    return;
                };
                for e in events {
                    last = e.id;
                    if filter.match_event(&e) && tx.send(e).is_err() {
                        return;
                    }
                }
            }
            let end = std::time::Instant::now() + interval;
            while !cancel.is_cancelled() && std::time::Instant::now() < end {
                thread::sleep(Duration::from_millis(5).min(interval));
            }
        }
    });
    rx
}

pub fn bridge(
    cancel: CancellationToken,
    s: pulse_store::Store,
    interval: Duration,
) -> Receiver<()> {
    bridge_on(cancel, default(), s, interval)
}

pub fn bridge_on(
    cancel: CancellationToken,
    b: Arc<Bus>,
    s: pulse_store::Store,
    interval: Duration,
) -> Receiver<()> {
    let (done_tx, done_rx) = bounded(1);
    let events = tail(cancel, s, -1, Filter::default(), interval);
    thread::spawn(move || {
        for e in events {
            b.publish_once(e);
        }
        let _ = done_tx.send(());
    });
    done_rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{new, reset_default};
    use serde_json::json;
    use std::path::Path;
    use tempfile::tempdir;

    fn temp_store() -> pulse_store::Store {
        let dir = tempdir().unwrap().keep();
        pulse_store::Store::create(Path::new(&dir).join(pulse_store::STORE_NAME)).unwrap();
        pulse_store::Store::open(pulse_store::Options {
            home: dir.display().to_string(),
            now: "2026-09-06T21:00:00Z".to_string(),
            ..pulse_store::Options::default()
        })
        .unwrap()
    }

    #[test]
    fn emit_writes_row_and_publishes() {
        let _guard = crate::test_global_guard();
        let s = temp_store();
        let b = Arc::new(new());
        let (ch, cancel) = b.subscribe(
            Filter {
                actions: vec!["task.*".to_string()],
                ..Filter::default()
            },
            8,
        );
        let e = emit_on(
            Some(Arc::clone(&b)),
            &s,
            "agent-a",
            "task.created",
            "TASK-001",
            pulse_store::Detail::json(json!({"project": "demo", "title": "x"})).unwrap(),
        )
        .unwrap();
        assert_ne!(e.id, 0);
        assert_eq!(e.at, "2026-09-06T21:00:00Z");
        assert_eq!(e.detail, r#"{"project": "demo", "title": "x"}"#);
        assert_eq!(ch.recv().unwrap(), e);
        let rows = since(
            &s,
            0,
            Filter {
                actions: vec!["task.created".to_string()],
                ..Filter::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(rows, vec![e.clone()]);
        assert_eq!(last_id(&s).unwrap(), e.id);
        let d = reset_default();
        let (ch2, cancel2) = d.subscribe(
            Filter {
                subject: "TASK-002".to_string(),
                ..Filter::default()
            },
            1,
        );
        emit(&s, "agent-a", "task.created", "TASK-002", None::<&str>).unwrap();
        assert_eq!(ch2.recv().unwrap().subject, "TASK-002");
        cancel();
        cancel2();
    }

    #[test]
    fn tail_sees_rows_from_another_connection() {
        let _guard = crate::test_global_guard();
        let dir = tempdir().unwrap();
        pulse_store::Store::create(dir.path().join(pulse_store::STORE_NAME)).unwrap();
        let reader = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        let writer = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            now: "2026-09-06T21:00:00Z".to_string(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        let cancel = CancellationToken::new();
        let ch = tail(
            cancel.clone(),
            reader,
            -1,
            Filter {
                actions: vec!["post".to_string(), "task.*".to_string()],
                ..Filter::default()
            },
            Duration::from_millis(20),
        );
        thread::sleep(Duration::from_millis(50));
        writer.event("x", "settings.set", "stop", "").unwrap();
        emit_on(
            None,
            &writer,
            "agent-b",
            "task.amended",
            "TASK-001",
            pulse_store::Detail::json(json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        writer
            .event(
                "agent-b",
                "post",
                "20260906T210000Z-agent-b",
                pulse_store::Detail::json(
                    json!({"kind": "finding", "project": "demo", "thread": "a/b"}),
                )
                .unwrap(),
            )
            .unwrap();
        let got = (0..2)
            .map(|_| ch.recv_timeout(Duration::from_secs(3)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(got[0].action, "task.amended");
        assert_eq!(got[1].action, "post");
        assert_eq!(got[1].id, got[0].id + 1);
        cancel.cancel();
        assert!(ch.recv_timeout(Duration::from_secs(2)).is_err());
    }

    #[test]
    fn tail_from_since() {
        let _guard = crate::test_global_guard();
        let s = temp_store();
        for _ in 0..3 {
            s.event("a", "post", "p", "").unwrap();
        }
        let cancel = CancellationToken::new();
        let ch = tail(
            cancel.clone(),
            s,
            1,
            Filter::default(),
            Duration::from_millis(10),
        );
        let ids = (0..2)
            .map(|_| ch.recv_timeout(Duration::from_secs(2)).unwrap().id)
            .collect::<Vec<_>>();
        assert!(ids[0] < ids[1], "{ids:?}");
        cancel.cancel();
    }

    #[test]
    fn latest_returns_the_newest_window_oldest_first() {
        let _guard = crate::test_global_guard();
        let s = temp_store();
        let mut ids = Vec::new();
        for (i, action) in [
            "task.created",
            "settings.set",
            "task.amended",
            "task.amended",
            "post",
        ]
        .into_iter()
        .enumerate()
        {
            let e = emit_on(
                None,
                &s,
                "a",
                action,
                "S",
                pulse_store::Detail::json(json!({ "i": i })).unwrap(),
            )
            .unwrap();
            ids.push(e.id);
        }
        let got = latest(
            &s,
            0,
            Filter {
                actions: vec!["task.*".to_string()],
                ..Filter::default()
            },
            2,
        )
        .unwrap();
        assert_eq!(
            got.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[2], ids[3]]
        );
        let got = latest(&s, ids[2], Filter::default(), 10).unwrap();
        assert_eq!(
            got.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[3], ids[4]]
        );
        assert!(
            latest(&s, ids[4], Filter::default(), 10)
                .unwrap()
                .is_empty()
        );
        let all = since(&s, 0, Filter::default(), 0).unwrap();
        let got = latest(&s, 0, Filter::default(), 0).unwrap();
        assert_eq!(got, all);
    }

    #[test]
    fn bridge_delivers_store_writes_once() {
        let _guard = crate::test_global_guard();
        let s = temp_store();
        let b = Arc::new(new());
        let cancel = CancellationToken::new();
        let done = bridge_on(
            cancel.clone(),
            Arc::clone(&b),
            s.clone(),
            Duration::from_millis(20),
        );
        thread::sleep(Duration::from_millis(60));
        let (ch, stop_sub) = b.subscribe(Filter::default(), 16);
        s.event(
            "demo-lead",
            "post.created",
            "20260906T210000Z-demo-lead",
            pulse_store::Detail::json(json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "compute.auto_approve", "1")
            .unwrap();
        emit_on(
            Some(Arc::clone(&b)),
            &s,
            "runner",
            "job.leased",
            "job-1",
            None::<&str>,
        )
        .unwrap();
        let got = (0..3)
            .map(|_| ch.recv_timeout(Duration::from_secs(3)).unwrap())
            .collect::<Vec<_>>();
        let mut actions = std::collections::BTreeMap::new();
        for e in got {
            assert_ne!(e.id, 0);
            *actions.entry(e.action).or_insert(0) += 1;
        }
        assert_eq!(actions.get("post.created"), Some(&1));
        assert_eq!(actions.get("setting"), Some(&1));
        assert_eq!(actions.get("job.leased"), Some(&1));
        thread::sleep(Duration::from_millis(120));
        assert!(ch.try_recv().is_err());
        assert_eq!(b.deduped(), 1);
        cancel.cancel();
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        stop_sub();
    }

    #[test]
    fn store_helpers_reach_default_bus() {
        let _guard = crate::test_global_guard();
        let b = reset_default();
        let s = temp_store();
        let (ch, cancel) = b.subscribe(
            Filter {
                actions: vec!["setting".to_string()],
                ..Filter::default()
            },
            8,
        );
        s.set_setting("human", "workers", "2").unwrap();
        let got = ch.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(got.action, "setting");
        assert_eq!(got.subject, "workers");
        assert_eq!(got.detail, "2");
        assert_ne!(got.id, 0);
        b.publish_once(got);
        assert!(ch.recv_timeout(Duration::from_millis(200)).is_err());
        cancel();
    }
}
