//! Idle-line dedupe for the daemon log.
//!
//! The dispatch loop says why it is idle on every tick; a starved fleet
//! therefore wrote the same `idle: no queued job` line hundreds of times an
//! hour, and the per-project idle notes re-logged whenever a countdown such as
//! `ballot in 2h` or `next after 17m` ticked down. [`IdleLog`] logs a reason
//! when it changes (countdowns normalised away) and otherwise one summary per
//! `SUMMARY_EVERY` naming how long it has held and how many ticks it covered.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// How often an unchanged idle reason is summarised.
pub(crate) const SUMMARY_EVERY: Duration = Duration::from_secs(60 * 60);

static DIGITS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\d+").expect("digits regex"));
static COUNTDOWN: LazyLock<regex::Regex> = LazyLock::new(|| {
    // `2h`, `17m`, `40s`, `2h30m`, `1h05m`, `23m16s`, `1h2m3s`, `3 minutes`,
    // `2 hours` after the words the idle notes use for a wait: in, after,
    // ago, for, since, with.
    regex::Regex::new(r"\b(\d+h(\d+m)?(\d+s)?|\d+m(\d+s)?|\d+s|\d+ (minute|hour|second)s?)\b")
        .expect("countdown regex")
});
static GAUGE: LazyLock<regex::Regex> = LazyLock::new(|| {
    // A measured amount that drifts while a hold stands - `4594 MB free`,
    // `118 MB` - not a count of things.
    regex::Regex::new(r"\b\d+(\.\d+)? ?(MB|GB|MiB|GiB|KB|KiB)\b").expect("gauge regex")
});

/// The reason with each countdown replaced by one `#`, so two notes that
/// differ only by the time left compare equal - also across a unit boundary
/// (`1h05m` then `59m`, `10m0s` then `56s`), where a per-digit replacement
/// kept re-logging the same hold as its countdown ran down. A gauge in
/// megabytes or gigabytes is normalised the same way: the disk-floor hold
/// once re-logged every tick because the free megabytes moved with each
/// install.
pub(crate) fn normalize_countdowns(reason: &str) -> String {
    let reason = GAUGE
        .replace_all(reason, |caps: &regex::Captures| {
            DIGITS.replace_all(&caps[0], "#").into_owned()
        })
        .into_owned();
    COUNTDOWN
        .replace_all(&reason, |caps: &regex::Captures| {
            if caps.get(5).is_some() {
                // `3 minutes` keeps its word; only the number goes.
                DIGITS.replace_all(&caps[0], "#").into_owned()
            } else {
                "#".to_string()
            }
        })
        .into_owned()
}

#[derive(Debug, Clone)]
struct Held {
    key: String,
    since: Instant,
    last_said: Instant,
    ticks: u32,
}

/// Dedupes repeated idle reasons per channel (the daemon itself or a project).
#[derive(Debug, Default)]
pub(crate) struct IdleLog {
    held: BTreeMap<String, Held>,
}

impl IdleLog {
    /// The line to log for `reason` on `channel` right now, if any: the
    /// reason itself when it is new or changed beyond its countdowns, a
    /// summary once per [`SUMMARY_EVERY`] while it holds, nothing otherwise.
    pub(crate) fn line(&mut self, channel: &str, reason: &str, now: Instant) -> Option<String> {
        let key = normalize_countdowns(reason);
        match self.held.get_mut(channel) {
            Some(h) if h.key == key => {
                h.ticks += 1;
                if now.duration_since(h.last_said) < SUMMARY_EVERY {
                    return None;
                }
                let held = now.duration_since(h.since);
                h.last_said = now;
                Some(format!(
                    "{reason} (unchanged for {}, {} ticks)",
                    pulse_cli::format_duration(held),
                    h.ticks
                ))
            }
            _ => {
                self.held.insert(
                    channel.to_string(),
                    Held {
                        key,
                        since: now,
                        last_said: now,
                        ticks: 1,
                    },
                );
                Some(reason.to_string())
            }
        }
    }

    /// Forgets `channel`, so its next reason logs again (the daemon left
    /// idle: a run started, or the project got work).
    pub(crate) fn clear(&mut self, channel: &str) {
        self.held.remove(channel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdowns_normalise_but_counts_do_not() {
        assert_eq!(
            normalize_countdowns(
                "3 eligible suggestions, 0 plannable; ballot in 2h; research: last research 47m ago, next after 13m; retry in 1h05m; wait 40s"
            ),
            "3 eligible suggestions, 0 plannable; ballot in #; research: last research # ago, next after #; retry in #; wait #"
        );
        assert_eq!(
            normalize_countdowns("job-20261001T1200Z-ab12 waits 3 minutes"),
            "job-20261001T1200Z-ab12 waits # minutes"
        );
        assert_eq!(normalize_countdowns("no queued job"), "no queued job");
        // format_duration writes minutes and seconds together; the restart
        // hold line once re-logged every tick because `23m16s` was no
        // countdown to this regex, and again when `10m0s` ran down to `56s`.
        assert_eq!(
            normalize_countdowns(
                "restarting: no new runs with 23m16s left of the drain; 1 in flight"
            ),
            "restarting: no new runs with # left of the drain; 1 in flight"
        );
        assert_eq!(
            normalize_countdowns("restarting: no new runs with 56s left of the drain; 1 in flight"),
            normalize_countdowns(
                "restarting: no new runs with 10m0s left of the drain; 1 in flight"
            )
        );
        assert_eq!(
            normalize_countdowns("one may take up to 1h2m3s"),
            "one may take up to #"
        );
        // The disk-floor hold re-logged every tick while installs moved the
        // free megabytes; the floor itself and the counts stay.
        assert_eq!(
            normalize_countdowns(
                "disk under floor: 4594 MB free on /Users/w/.pulse/work (daemon.min_free_mb 8192)"
            ),
            normalize_countdowns(
                "disk under floor: 4603 MB free on /Users/w/.pulse/work (daemon.min_free_mb 8192)"
            )
        );
        assert_eq!(
            normalize_countdowns(
                "disk headroom: room for 2 more local run(s) of up to 1600 MB above daemon.min_free_mb 1024; 1 setting up"
            ),
            "disk headroom: room for 2 more local run(s) of up to # MB above daemon.min_free_mb 1024; 1 setting up"
        );
        assert_ne!(
            normalize_countdowns(
                "disk headroom: room for 2 more local run(s) of up to 1600 MB above daemon.min_free_mb 1024; 1 setting up"
            ),
            normalize_countdowns(
                "disk headroom: room for 0 more local run(s) of up to 1600 MB above daemon.min_free_mb 1024; 2 setting up"
            )
        );
    }

    #[test]
    fn repeated_reason_logs_once_then_summarises_hourly() {
        let mut log = IdleLog::default();
        let t0 = Instant::now();
        assert_eq!(
            log.line("daemon", "no queued job", t0),
            Some("no queued job".into())
        );
        for i in 1..=5 {
            assert_eq!(
                log.line("daemon", "no queued job", t0 + Duration::from_secs(i * 60)),
                None,
                "tick {i} was logged again"
            );
        }
        let got = log
            .line("daemon", "no queued job", t0 + SUMMARY_EVERY)
            .expect("the hourly summary was not logged");
        assert!(
            got.starts_with("no queued job (unchanged for 1h") && got.ends_with("7 ticks)"),
            "{got}"
        );
        assert_eq!(
            log.line(
                "daemon",
                "no queued job",
                t0 + SUMMARY_EVERY + Duration::from_secs(60)
            ),
            None
        );
    }

    #[test]
    fn countdown_changes_stay_quiet_and_real_changes_log() {
        let mut log = IdleLog::default();
        let t0 = Instant::now();
        let first = "2 eligible suggestions, 0 plannable; ballot in 2h; next after 40m";
        assert!(log.line("proj", first, t0).is_some());
        assert_eq!(
            log.line(
                "proj",
                "2 eligible suggestions, 0 plannable; ballot in 1h; next after 25m",
                t0 + Duration::from_secs(900)
            ),
            None
        );
        let changed = "3 eligible suggestions, 1 plannable; ballot in 1h; next after 25m";
        assert_eq!(
            log.line("proj", changed, t0 + Duration::from_secs(960)),
            Some(changed.into())
        );
        // Channels are independent and a cleared channel logs again.
        assert!(log.line("other", changed, t0).is_some());
        log.clear("proj");
        assert_eq!(
            log.line("proj", changed, t0 + Duration::from_secs(1000)),
            Some(changed.into())
        );
    }
}
