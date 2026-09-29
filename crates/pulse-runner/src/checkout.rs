use anyhow::anyhow;
use crossbeam_channel::Receiver;
use std::collections::BTreeMap;
use std::io;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const CHECKOUT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub(crate) struct CheckoutProbes {
    pub(crate) pending: BTreeMap<String, Receiver<anyhow::Result<()>>>,
    pub(crate) noted: BTreeMap<String, String>,
}

pub(crate) static CHECKOUT_HOLD: LazyLock<RwLock<BTreeMap<String, String>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Go: func readDirOnce(path string) error
pub(crate) fn read_dir_once(path: String) -> anyhow::Result<()> {
    let meta = std::fs::metadata(&path)?;
    if !meta.is_dir() {
        return Err(anyhow!("{path} is not a directory"));
    }
    let mut dir = std::fs::read_dir(path)?;
    match dir.next() {
        Some(Err(err)) if err.kind() != io::ErrorKind::UnexpectedEof => Err(err.into()),
        _ => Ok(()),
    }
}

/// Go: func macOSHint(path string) string
pub(crate) fn mac_os_hint(path: String) -> String {
    if std::env::consts::OS != "macos" {
        return String::new();
    }
    for folder in crate::service::PROTECTED_FOLDERS {
        if path.contains(&format!("/{folder}/")) {
            return format!(
                " (a macOS dialog is asking whether pulse may access your {folder} folder: answer Allow once; every pulse-signed build keeps the answer, a build that skipped signing asks again)"
            );
        }
    }
    String::new()
}

/// Go: func holdCheckouts(held map[string]string)
pub(crate) fn hold_checkouts(held: BTreeMap<String, String>) {
    *CHECKOUT_HOLD.write().expect("checkout hold poisoned") = held;
}

/// Go: func heldCheckouts() map[string]string
pub(crate) fn held_checkouts() -> BTreeMap<String, String> {
    CHECKOUT_HOLD
        .read()
        .expect("checkout hold poisoned")
        .clone()
}

impl CheckoutProbes {
    /// Go: func (p *checkoutProbes) ready(projects []row, timeout time.Duration) (ok []row, held map[string]string, lines []string)
    pub(crate) fn ready(
        &mut self,
        projects: Vec<crate::util::Row>,
        timeout: std::time::Duration,
    ) -> (Vec<crate::util::Row>, BTreeMap<String, String>, Vec<String>) {
        let mut ok = Vec::new();
        let mut held = BTreeMap::new();
        let mut lines = Vec::new();
        let start = std::time::Instant::now();
        for proj in projects {
            let path = proj.str("path");
            let slug = proj.str("slug");
            if !self.pending.contains_key(&path) {
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.pending.insert(path.clone(), rx);
                let probe_path = path.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(read_dir_once(probe_path));
                });
            }
            let wait = timeout.saturating_sub(start.elapsed());
            let recv = self
                .pending
                .get(&path)
                .expect("pending probe")
                .recv_timeout(wait);
            match recv {
                Ok(Ok(())) => {
                    self.pending.remove(&path);
                    self.noted.remove(&slug);
                    ok.push(proj);
                }
                Ok(Err(err)) => {
                    self.pending.remove(&path);
                    held.insert(slug, format!("cannot read {path}: {err}"));
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    held.insert(
                        slug,
                        format!(
                            "{path} has not answered for {}s{}",
                            timeout.as_secs(),
                            mac_os_hint(path.clone())
                        ),
                    );
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    self.pending.remove(&path);
                    held.insert(slug, format!("cannot read {path}: probe stopped"));
                }
            }
        }
        for (slug, reason) in &held {
            if self.noted.get(slug) != Some(reason) {
                self.noted.insert(slug.clone(), reason.clone());
                lines.push(format!("holding {slug} this loop: {reason}"));
            }
        }
        hold_checkouts(held.clone());
        (ok, held, lines)
    }
}
