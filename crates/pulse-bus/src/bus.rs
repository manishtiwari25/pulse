use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockWriteGuard};

/// Event is one row of the events table (seq is ID).
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
    /// The project named in the detail JSON.
    pub fn project(&self) -> String {
        detail_field(&self.detail, "project")
    }

    /// The "area/thread" the detail names, if any.
    pub fn thread(&self) -> String {
        detail_field(&self.detail, "thread")
    }
}

/// Filter narrows a subscription; empty fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub actions: Vec<String>,
    pub project: String,
    pub subject: String,
    pub area: String,
    pub thread: String,
}

impl Filter {
    /// Match reports whether e passes f.
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

    pub fn matches(&self, e: &Event) -> bool {
        self.match_event(e)
    }
}

/// Matches a dotted action or subject against a pattern.
pub fn glob(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if !pattern.contains(['*', '?', '[']) {
        return name == pattern || name.starts_with(&(pattern.to_string() + "."));
    }
    match_component(pattern.as_bytes(), name.as_bytes())
}

fn match_component(pattern: &[u8], name: &[u8]) -> bool {
    if pattern.is_empty() {
        return name.is_empty();
    }
    match pattern[0] {
        b'*' => (0..=name.len()).any(|i| match_component(&pattern[1..], &name[i..])),
        b'?' => !name.is_empty() && name[0] != b'/' && match_component(&pattern[1..], &name[1..]),
        b'[' => match_class(pattern, name),
        c => !name.is_empty() && c == name[0] && match_component(&pattern[1..], &name[1..]),
    }
}

fn match_class(pattern: &[u8], name: &[u8]) -> bool {
    let Some(end) = pattern.iter().position(|&b| b == b']') else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    let class = &pattern[1..end];
    let neg = class.first().is_some_and(|b| *b == b'!' || *b == b'^');
    let class = if neg { &class[1..] } else { class };
    let mut ok = false;
    let mut i = 0;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == b'-' {
            ok |= class[i] <= name[0] && name[0] <= class[i + 2];
            i += 3;
        } else {
            ok |= class[i] == name[0];
            i += 1;
        }
    }
    if ok == neg {
        return false;
    }
    match_component(&pattern[end + 1..], &name[1..])
}

struct Subscriber {
    tx: Sender<Event>,
    rx: Receiver<Event>,
    filter: Filter,
}

/// Bus fans events out to subscribers.
pub struct Bus {
    subs: Mutex<HashMap<usize, Subscriber>>,
    next: Mutex<usize>,
    dropped: AtomicI64,
    seen: Mutex<Seen>,
    deduped: AtomicI64,
}

struct Seen {
    map: HashMap<String, ()>,
    ring: Vec<String>,
    pos: usize,
}

const SEEN_SIZE: usize = 4096;

/// Returns an empty bus.
pub fn new() -> Bus {
    Bus {
        subs: Mutex::new(HashMap::new()),
        next: Mutex::new(0),
        dropped: AtomicI64::new(0),
        seen: Mutex::new(Seen {
            map: HashMap::new(),
            ring: vec![String::new(); SEEN_SIZE],
            pos: 0,
        }),
        deduped: AtomicI64::new(0),
    }
}

impl Default for Bus {
    fn default() -> Self {
        new()
    }
}

impl Bus {
    /// Returns a receiver and an idempotent cancel function.
    pub fn subscribe(
        self: &Arc<Self>,
        filter: Filter,
        buffer: usize,
    ) -> (Receiver<Event>, impl Fn() + Send + Sync + 'static) {
        let cap = if buffer == 0 { 64 } else { buffer };
        let (tx, rx) = bounded(cap);
        let mut next = self.next.lock();
        let id = *next;
        *next += 1;
        drop(next);
        self.subs.lock().insert(
            id,
            Subscriber {
                tx,
                rx: rx.clone(),
                filter,
            },
        );
        let bus = Arc::clone(self);
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_done = Arc::clone(&done);
        let cancel = move || {
            if cancel_done.swap(true, Ordering::SeqCst) {
                return;
            }
            bus.subs.lock().remove(&id);
        };
        (rx, cancel)
    }

    /// Delivers `e` to every matching subscriber without blocking.
    pub fn publish(&self, e: Event) {
        let subs = self
            .subs
            .lock()
            .values()
            .map(|s| (s.tx.clone(), s.rx.clone(), s.filter.clone()))
            .collect::<Vec<_>>();
        for (tx, rx, filter) in subs {
            if !filter.match_event(&e) {
                continue;
            }
            if tx.try_send(e.clone()).is_err() {
                if rx.try_recv().is_ok() {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                if tx.try_send(e.clone()).is_err() {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn dropped(&self) -> i64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn subscribers(&self) -> usize {
        self.subs.lock().len()
    }

    /// Publishes `e` unless an event with the same non-zero ID and content was seen.
    pub fn publish_once(&self, e: Event) -> bool {
        if e.id != 0 {
            let key = format!("{}|{}|{}|{}", e.id, e.at, e.action, e.subject);
            let mut seen = self.seen.lock();
            if seen.map.contains_key(&key) {
                self.deduped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            let pos = seen.pos;
            if !seen.ring[pos].is_empty() {
                let old = std::mem::take(&mut seen.ring[pos]);
                seen.map.remove(&old);
            }
            seen.ring[pos] = key.clone();
            seen.pos = (pos + 1) % SEEN_SIZE;
            seen.map.insert(key, ());
        }
        self.publish(e);
        true
    }

    pub fn deduped(&self) -> i64 {
        self.deduped.load(Ordering::Relaxed)
    }
}

static DEFAULT: OnceLock<Mutex<Arc<Bus>>> = OnceLock::new();
static STORE_OBSERVER: OnceLock<()> = OnceLock::new();

/// The process-wide bus that `emit` publishes to.
pub fn default() -> Arc<Bus> {
    install_store_observer();
    DEFAULT
        .get_or_init(|| Mutex::new(Arc::new(new())))
        .lock()
        .clone()
}

/// Replaces the process-wide bus with a fresh one and returns it.
/// Tests call it when they build a new store; it waits for any test that holds
/// [`exclusive_default`], so a parallel fixture never swaps the bus out from
/// under a subscriber.
pub fn reset_default() -> Arc<Bus> {
    let _shared = DEFAULT_TESTS.read().unwrap_or_else(|e| e.into_inner());
    reset_default_unlocked()
}

fn reset_default_unlocked() -> Arc<Bus> {
    install_store_observer();
    let bus = Arc::new(new());
    *DEFAULT.get_or_init(|| Mutex::new(Arc::clone(&bus))).lock() = Arc::clone(&bus);
    bus
}

static DEFAULT_TESTS: RwLock<()> = RwLock::new(());

/// Guard returned by [`exclusive_default`]; dropping it lets other fixtures reset the bus again.
pub struct ExclusiveDefault {
    _guard: RwLockWriteGuard<'static, ()>,
}

/// exclusive_default gives a test that subscribes to the default bus a fresh
/// bus that no other test can reset while the guard lives: tests run in
/// parallel threads, their fixtures reset the default bus, and a subscriber
/// that lost the bus it subscribed to would wait forever. Call it AFTER the
/// store fixture (fixtures call `reset_default`, which blocks while the guard
/// is held).
pub fn exclusive_default() -> (Arc<Bus>, ExclusiveDefault) {
    let guard = DEFAULT_TESTS.write().unwrap_or_else(|e| e.into_inner());
    (reset_default_unlocked(), ExclusiveDefault { _guard: guard })
}

pub fn install_store_observer() {
    STORE_OBSERVER.get_or_init(|| {
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
    });
}

/// Pulls one string field out of a JSON object detail.
pub fn detail_field(detail: &str, key: &str) -> String {
    if !detail.trim_start().starts_with('{') {
        return String::new();
    }
    serde_json::from_str::<Value>(detail)
        .ok()
        .and_then(|v| v.get(key).and_then(Value::as_str).map(ToString::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn ev(id: i64) -> Event {
        Event {
            id,
            ..Event::default()
        }
    }

    #[test]
    fn fan_out_one_publisher_many_subscribers() {
        let _serial = crate::test_helpers::serial();
        let b = Arc::new(new());
        const SUBS: usize = 5;
        const EVENTS: i64 = 200;
        let mut counts = vec![0usize; SUBS];
        let mut cancels = Vec::new();
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for i in 0..SUBS {
                let (ch, c) = b.subscribe(Filter::default(), EVENTS as usize);
                cancels.push(c);
                handles.push(scope.spawn(move || {
                    let mut n = 0usize;
                    for _ in ch {
                        n += 1;
                    }
                    (i, n)
                }));
            }
            for i in 0..EVENTS {
                b.publish(Event {
                    id: i,
                    action: "post".to_string(),
                    ..Event::default()
                });
            }
            for c in cancels {
                c();
            }
            for handle in handles {
                let (i, n) = handle.join().unwrap();
                counts[i] = n;
            }
        });
        for (i, n) in counts.into_iter().enumerate() {
            assert_eq!(
                n, EVENTS as usize,
                "subscriber {i} got {n} events, want {EVENTS}"
            );
        }
        if b.dropped() != 0 {
            panic!("dropped {}", b.dropped());
        }
        if b.subscribers() != 0 {
            panic!("subscribers left: {}", b.subscribers());
        }
    }

    #[test]
    fn slow_subscriber_drops_oldest_never_blocks() {
        let _serial = crate::test_helpers::serial();
        let b = Arc::new(new());
        let (ch, cancel) = b.subscribe(Filter::default(), 3);
        let p = Arc::clone(&b);
        let done = thread::spawn(move || {
            for i in 1..=10 {
                p.publish(ev(i));
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done.is_finished() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if !done.is_finished() {
            panic!("publisher blocked on a slow subscriber");
        }
        done.join().unwrap();
        let mut got = Vec::new();
        while got.len() < 3 {
            got.push(ch.recv().unwrap().id);
        }
        if got[0] != 8 || got[1] != 9 || got[2] != 10 {
            panic!("kept {got:?}, want the newest three [8 9 10]");
        }
        if b.dropped() != 7 {
            panic!("dropped {}, want 7", b.dropped());
        }
        cancel();
    }

    #[test]
    fn cancel_stops_delivery_and_is_idempotent() {
        let _serial = crate::test_helpers::serial();
        let b = Arc::new(new());
        let (ch, cancel) = b.subscribe(Filter::default(), 4);
        b.publish(ev(1));
        cancel();
        cancel();
        b.publish(ev(2));
        let ids = ch.iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn filter_match() {
        let _serial = crate::test_helpers::serial();
        let post = Event {
            action: "post".to_string(),
            subject: "20260906T210000Z-a".to_string(),
            detail: r#"{"kind": "finding", "project": "demo", "thread": "runner/compute"}"#
                .to_string(),
            ..Event::default()
        };
        let cases = vec![
            (Filter::default(), true),
            (
                Filter {
                    actions: vec!["post".to_string()],
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    actions: vec!["task.*".to_string()],
                    ..Filter::default()
                },
                false,
            ),
            (
                Filter {
                    actions: vec!["task.*".to_string(), "post".to_string()],
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    project: "demo".to_string(),
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    project: "other".to_string(),
                    ..Filter::default()
                },
                false,
            ),
            (
                Filter {
                    area: "runner".to_string(),
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    area: "board".to_string(),
                    ..Filter::default()
                },
                false,
            ),
            (
                Filter {
                    thread: "runner/compute".to_string(),
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    thread: "compute".to_string(),
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    thread: "runner/other".to_string(),
                    ..Filter::default()
                },
                false,
            ),
            (
                Filter {
                    subject: "2026*".to_string(),
                    ..Filter::default()
                },
                true,
            ),
            (
                Filter {
                    subject: "TASK-*".to_string(),
                    ..Filter::default()
                },
                false,
            ),
        ];
        for (filter, want) in cases {
            assert_eq!(filter.match_event(&post), want, "{filter:?}");
        }
        assert!(
            Filter {
                project: "demo".to_string(),
                ..Filter::default()
            }
            .match_event(&Event {
                action: "project.removed".to_string(),
                subject: "demo".to_string(),
                detail: "free text".to_string(),
                ..Event::default()
            })
        );
        for (pattern, name, want) in [
            ("post", "post", true),
            ("post", "post.question", true),
            ("post", "post-question", false),
            ("post.question", "post.question", true),
            ("post.question", "post.handoff", false),
            ("task", "task.amended", true),
            ("task.*", "task.amended", true),
            ("task.*", "task", false),
            ("post.*", "task.amended", false),
            ("*", "anything", true),
        ] {
            assert_eq!(glob(pattern, name), want, "{pattern} {name}");
        }
    }

    #[test]
    fn publish_once_dedups_by_id() {
        let _serial = crate::test_helpers::serial();
        let b = Arc::new(new());
        let (ch, cancel) = b.subscribe(Filter::default(), 8);
        assert!(b.publish_once(Event {
            id: 7,
            action: "a".to_string(),
            ..Event::default()
        }));
        assert!(!b.publish_once(Event {
            id: 7,
            action: "a".to_string(),
            ..Event::default()
        }));
        assert!(b.publish_once(Event {
            action: "synthetic".to_string(),
            ..Event::default()
        }));
        assert!(b.publish_once(Event {
            action: "synthetic".to_string(),
            ..Event::default()
        }));
        assert_eq!(b.deduped(), 1);
        let mut got = 0;
        while ch.recv_timeout(Duration::from_millis(200)).is_ok() {
            got += 1;
        }
        assert_eq!(got, 3);
        for i in 100..100 + SEEN_SIZE as i64 {
            b.publish_once(ev(i));
        }
        assert!(b.publish_once(ev(7)));
        assert!(b.publish_once(Event {
            id: 7,
            action: "b".to_string(),
            ..Event::default()
        }));
        cancel();
    }
}
