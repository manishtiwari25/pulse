//! Cancellation tokens with deadlines, shared by every crate that needs
//! cancellation (the store's connection waits, the bus, the runner).

use crossbeam_channel::{Receiver, bounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Why a token is done. The Display text is fixed - `context canceled` and
/// `context deadline exceeded` - because callers classify wrapped errors
/// and stored failure messages by matching on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextError {
    Canceled,
    DeadlineExceeded,
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ContextError::Canceled => "context canceled",
            ContextError::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

impl std::error::Error for ContextError {}

impl ContextError {
    pub fn is(err: &anyhow::Error, want: ContextError) -> bool {
        Self::of(err) == Some(want)
    }

    /// The context cause wrapped anywhere in `err`, if any.
    pub fn of(err: &anyhow::Error) -> Option<ContextError> {
        err.chain()
            .find_map(|e| e.downcast_ref::<ContextError>().copied())
    }
}

/// A cancellation token: clones share one cancellation flag (handles to the
/// same token); `with_timeout`/`child` derive a new token that is done when
/// its parent is done, when it is cancelled itself, or when its deadline
/// passes.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    parent: Option<Arc<CancellationToken>>,
    done: Arc<DoneSignal>,
}

/// The channel behind `done()`. Closing it (dropping the only sender) wakes
/// every receiver, now and forever: a disconnected channel stays ready;
/// descendants register here so a parent's cancel closes theirs too.
struct DoneSignal {
    tx: parking_lot::Mutex<Option<crossbeam_channel::Sender<()>>>,
    rx: Receiver<()>,
    waiters: parking_lot::Mutex<Vec<std::sync::Weak<DoneSignal>>>,
    timer: std::sync::OnceLock<()>,
}

impl Default for DoneSignal {
    fn default() -> Self {
        let (tx, rx) = bounded(1);
        Self {
            tx: parking_lot::Mutex::new(Some(tx)),
            rx,
            waiters: parking_lot::Mutex::new(Vec::new()),
            timer: std::sync::OnceLock::new(),
        }
    }
}

impl std::fmt::Debug for DoneSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoneSignal")
            .field("closed", &self.tx.lock().is_none())
            .finish()
    }
}

impl DoneSignal {
    fn close(&self) {
        self.tx.lock().take();
        let waiters = std::mem::take(&mut *self.waiters.lock());
        for w in waiters {
            if let Some(d) = w.upgrade() {
                d.close();
            }
        }
    }

    fn closed(&self) -> bool {
        self.tx.lock().is_none()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child(&self) -> Self {
        let done = Arc::new(DoneSignal::default());
        // Register with every ancestor so any of their cancels closes this one.
        let mut cur = Some(self);
        while let Some(t) = cur {
            let mut ws = t.done.waiters.lock();
            ws.retain(|w| w.strong_count() > 0);
            ws.push(Arc::downgrade(&done));
            cur = t.parent.as_deref();
        }
        let c = Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: self.deadline,
            parent: Some(Arc::new(self.clone())),
            done,
        };
        if self.is_cancelled() {
            c.done.close();
        }
        c
    }

    pub fn with_timeout(&self, d: Duration) -> Self {
        self.with_deadline(Instant::now() + d)
    }

    pub fn with_deadline(&self, at: Instant) -> Self {
        let mut c = self.child();
        c.deadline = Some(match self.deadline {
            Some(p) if p < at => p,
            _ => at,
        });
        c
    }

    pub fn cancel(&self) {
        // The first cause wins - cancelling after the deadline has already
        // passed leaves err() as DeadlineExceeded.
        if !self.deadline_exceeded() {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        self.done.close();
    }

    /// A receiver that is ready (disconnected) once the token is done and
    /// stays ready; every clone of it wakes, so it can sit in a
    /// `crossbeam_channel::select!` next to other channels.
    pub fn done(&self) -> Receiver<()> {
        if self.is_cancelled() {
            self.done.close();
        } else if let Some(at) = self.deadline
            && !self.done.closed()
        {
            let done = Arc::downgrade(&self.done);
            self.done.timer.get_or_init(|| {
                thread::Builder::new()
                    .name("ctx-deadline".into())
                    .spawn(move || {
                        loop {
                            let Some(strong) = done.upgrade() else { return };
                            if strong.closed() {
                                return;
                            }
                            let now = Instant::now();
                            if now >= at {
                                strong.close();
                                return;
                            }
                            drop(strong);
                            thread::sleep((at - now).min(Duration::from_millis(250)));
                        }
                    })
                    .expect("spawn ctx deadline timer");
            });
        }
        self.done.rx.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
            || self.deadline_exceeded()
            || self.parent.as_ref().is_some_and(|p| p.is_cancelled())
    }

    /// None while the token is live, otherwise the typed cause (whose
    /// Display text is `context canceled` or `context deadline exceeded`).
    pub fn err(&self) -> Option<ContextError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Some(ContextError::Canceled);
        }
        if self.deadline_exceeded() {
            return Some(ContextError::DeadlineExceeded);
        }
        self.parent.as_ref().and_then(|p| p.err())
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn deadline_exceeded(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Time left until the deadline, if any (zero once it has passed).
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }
}
