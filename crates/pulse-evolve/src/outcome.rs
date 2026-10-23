use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LoopWindow {
    pub since: String,
    pub until: String,
    pub worker_hours: f64,
    pub landed: i64,
    pub attempts: i64,
    pub landings_per_hour: f64,
    pub first_pass_pct: f64,
    pub lead_minutes_median: f64,
    pub owner_asks: i64,
    pub improver_landings: i64,
    pub self_updates: i64,
}

impl From<crate::report::ImproverWindow> for LoopWindow {
    fn from(w: crate::report::ImproverWindow) -> Self {
        Self {
            since: w.since,
            until: w.until,
            worker_hours: w.worker_hours,
            landed: w.landed,
            attempts: w.attempts,
            landings_per_hour: w.landings_per_hour,
            first_pass_pct: w.first_pass_pct,
            lead_minutes_median: w.lead_minutes_median,
            owner_asks: w.owner_asks,
            improver_landings: w.improver_landings,
            self_updates: w.self_updates,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Outcome {
    pub landed_at: String,
    pub hours: i64,
    pub before: LoopWindow,
    pub after: LoopWindow,
    pub landings_per_hour_delta_pct: Option<f64>,
    pub first_pass_delta_pct: f64,
    pub lead_minutes_delta: f64,
    pub owner_asks_per_landing_delta: f64,
}

pub fn outcome_for(s: &pulse_store::Store, landed_at: &str, hours: i64) -> Result<Outcome> {
    let hours = hours.max(1);
    let at = DateTime::parse_from_rfc3339(landed_at)?.with_timezone(&Utc);
    let before_since = iso(at - Duration::hours(hours));
    let landed_at = iso(at);
    let after_until = iso(at + Duration::hours(hours));
    let before: LoopWindow =
        crate::report::loop_window(s, None, &before_since, Some(&landed_at))?.into();
    let after: LoopWindow =
        crate::report::loop_window(s, None, &landed_at, Some(&after_until))?.into();
    Ok(Outcome {
        landed_at,
        hours,
        landings_per_hour_delta_pct: pct_delta(before.landings_per_hour, after.landings_per_hour),
        first_pass_delta_pct: after.first_pass_pct - before.first_pass_pct,
        lead_minutes_delta: after.lead_minutes_median - before.lead_minutes_median,
        owner_asks_per_landing_delta: asks_per_landing(&after) - asks_per_landing(&before),
        before,
        after,
    })
}

pub fn outcome_lines(outcome: &Outcome) -> Vec<String> {
    vec![
        format!(
            "Landings per worker-hour: {} before -> {} after{}.",
            per_hour(&outcome.before),
            per_hour(&outcome.after),
            pct_phrase(outcome.landings_per_hour_delta_pct)
        ),
        format!(
            "First pass: {} before -> {} after.",
            first_pass(&outcome.before),
            first_pass(&outcome.after)
        ),
        format!(
            "Lead time median: {} before -> {} after.",
            lead(&outcome.before),
            lead(&outcome.after)
        ),
        format!(
            "Owner asks per landing: {} before -> {} after.",
            asks(&outcome.before),
            asks(&outcome.after)
        ),
    ]
}

pub fn regressed(outcome: &Outcome) -> bool {
    outcome.before.attempts >= 10
        && outcome.after.attempts >= 10
        && outcome.before.landings_per_hour > 0.0
        && outcome.after.landings_per_hour < outcome.before.landings_per_hour * 0.8
}

fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn pct_delta(before: f64, after: f64) -> Option<f64> {
    if before <= 0.0 {
        None
    } else {
        Some(100.0 * (after - before) / before)
    }
}

pub fn format_delta_pct(delta: Option<f64>) -> String {
    delta.map(|d| format!(" ({d:+.0}%)")).unwrap_or_default()
}

fn pct_phrase(delta: Option<f64>) -> String {
    format_delta_pct(delta)
}

fn per_hour(w: &LoopWindow) -> String {
    if w.worker_hours <= 0.0 {
        "no finished runs".to_string()
    } else {
        format!("{:.2}", w.landings_per_hour)
    }
}

fn first_pass(w: &LoopWindow) -> String {
    if w.attempts <= 0 {
        "no landing attempts".to_string()
    } else {
        format!("{:.0}% of {} attempt(s)", w.first_pass_pct, w.attempts)
    }
}

fn lead(w: &LoopWindow) -> String {
    if w.landed <= 0 {
        "no lead time".to_string()
    } else {
        format!("{:.0} min", w.lead_minutes_median)
    }
}

fn asks(w: &LoopWindow) -> String {
    if w.landed <= 0 {
        "no landing".to_string()
    } else {
        format!("{:.2}", asks_per_landing(w))
    }
}

pub fn asks_per_landing(w: &LoopWindow) -> f64 {
    if w.landed <= 0 {
        0.0
    } else {
        w.owner_asks as f64 / w.landed as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(before: (i64, i64, f64), after: (i64, i64, f64)) -> Outcome {
        Outcome {
            before: LoopWindow {
                landed: before.0,
                attempts: before.1,
                worker_hours: before.2,
                landings_per_hour: before.0 as f64 / before.2,
                ..Default::default()
            },
            after: LoopWindow {
                landed: after.0,
                attempts: after.1,
                worker_hours: after.2,
                landings_per_hour: after.0 as f64 / after.2,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn regressed_requires_twenty_percent_drop_and_ten_attempts_each_side() {
        assert!(regressed(&outcome((10, 10, 10.0), (7, 10, 10.0))));
        assert!(!regressed(&outcome((10, 10, 10.0), (8, 10, 10.0))));
        assert!(!regressed(&outcome((9, 9, 10.0), (6, 10, 10.0))));
        assert!(!regressed(&outcome((10, 10, 10.0), (6, 9, 10.0))));
    }

    #[test]
    fn outcome_lines_name_empty_windows_plainly() {
        let outcome = Outcome {
            before: LoopWindow {
                landed: 2,
                attempts: 3,
                worker_hours: 4.0,
                landings_per_hour: 0.5,
                first_pass_pct: 200.0 / 3.0,
                lead_minutes_median: 42.0,
                owner_asks: 1,
                ..Default::default()
            },
            after: LoopWindow::default(),
            landings_per_hour_delta_pct: Some(-100.0),
            ..Default::default()
        };
        let lines = outcome_lines(&outcome).join("\n");
        assert!(
            lines.contains(
                "Landings per worker-hour: 0.50 before -> no finished runs after (-100%)"
            )
        );
        assert!(
            lines.contains("First pass: 67% of 3 attempt(s) before -> no landing attempts after.")
        );
        assert!(lines.contains("Lead time median: 42 min before -> no lead time after."));
        assert!(lines.contains("Owner asks per landing: 0.50 before -> no landing after."));
    }
}
