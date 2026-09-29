use crate::records::numbered;
use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeadTimeRow {
    pub id: String,
    pub kind: String,
    pub project: String,
    pub tier: i64,
    pub status: String,
    pub delivery: String,
    pub proposed: String,
    pub delivered: String,
    pub days: Option<i64>,
    pub state: String,
    pub target: String,
    pub path: String,
}

pub fn lead_time(s: &pulse_store::Store, project: &str) -> Result<Vec<LeadTimeRow>> {
    let rows = s.list_records(pulse_store::RecordFilter {
        project: project.into(),
        ..pulse_store::RecordFilter::default()
    })?;
    let today = NaiveDate::parse_from_str(&s.now()[..10], "%Y-%m-%d")?;
    let mut out = Vec::new();
    for rec in rows {
        if !numbered(&rec.kind) {
            continue;
        }
        let mut row = LeadTimeRow {
            id: rec.id.clone(),
            kind: rec.kind.clone(),
            project: rec.project.clone(),
            tier: rec.tier,
            status: rec.status.clone(),
            delivery: rec.delivery.clone(),
            proposed: rec.date.clone(),
            delivered: rec.delivered.clone(),
            days: None,
            state: String::new(),
            target: String::new(),
            path: rec.path.clone(),
        };
        let proposed = NaiveDate::parse_from_str(&rec.date, "%Y-%m-%d");
        let delivered = NaiveDate::parse_from_str(&rec.delivered, "%Y-%m-%d");
        let retired = rec.delivery == "retired"
            || rec.status.starts_with("Superseded")
            || rec.status.starts_with("Deprecated");
        let closed = rec.delivery == "delivered" || rec.status.starts_with("Done");
        match (proposed, delivered) {
            (Err(_), _) => {
                row.state = "no-date".into();
                row.target = "add date:".into();
            }
            (Ok(p), Ok(d)) => {
                row.state = "delivered".into();
                row.target = "-".into();
                row.days = Some((d - p).num_days());
            }
            (Ok(_), Err(_)) if retired => {
                row.state = "retired".into();
                row.target = "retired".into();
            }
            (Ok(_), Err(_)) if closed => {
                row.state = "undated".into();
                row.target = "add delivered:".into();
            }
            (Ok(p), Err(_)) => {
                row.state = "open".into();
                row.target = "open".into();
                row.days = Some((today - p).num_days());
            }
        }
        let (_, rev) = s.get_record(&rec.id, 0)?;
        let rev = rev.expect("current revision exists");
        let limit = if rec.tier == 1 {
            Some(2)
        } else if rec.tier == 0 && rev.frontmatter.get("tier").is_some_and(|v| !v.is_null()) {
            Some(0)
        } else {
            None
        };
        if let (Some(limit), Some(days)) = (limit, row.days) {
            if row.state == "delivered" {
                row.target = if days > limit { "missed" } else { "met" }.into();
            } else if row.state == "open" && days > limit {
                row.target = "overdue".into();
            }
        }
        out.push(row);
    }
    Ok(out)
}

pub fn lead_days(r: &LeadTimeRow) -> String {
    if let Some(days) = r.days {
        days.to_string()
    } else if r.state == "retired" {
        "-".into()
    } else {
        "?".into()
    }
}

pub use lead_time as LeadTime;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    fn temp_store() -> TestStore {
        let _guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        TestStore { _home: home, store }
    }

    #[test]
    fn lead_time_legacy_states() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let cases = [
            (
                "met",
                "Done",
                "delivered",
                "2026-09-01",
                "2026-09-02",
                "delivered",
                "met",
                1,
                Some(1),
            ),
            (
                "missed",
                "Done",
                "delivered",
                "2026-09-01",
                "2026-09-04",
                "delivered",
                "missed",
                1,
                Some(3),
            ),
            (
                "overdue",
                "Draft",
                "",
                "2026-09-01",
                "",
                "open",
                "overdue",
                1,
                Some(5),
            ),
            (
                "retired",
                "Superseded (ADR-001)",
                "",
                "2026-09-01",
                "",
                "retired",
                "retired",
                2,
                None,
            ),
            (
                "undated",
                "Done",
                "",
                "2026-09-01",
                "",
                "undated",
                "add delivered:",
                2,
                None,
            ),
            (
                "no-date",
                "Draft",
                "",
                "",
                "",
                "no-date",
                "add date:",
                2,
                None,
            ),
            (
                "partial",
                "Accepted",
                "partially delivered",
                "2026-09-01",
                "2026-09-03",
                "delivered",
                "-",
                2,
                Some(2),
            ),
        ];
        for (i, tt) in cases.iter().enumerate() {
            let rec = pulse_store::Record {
                id: format!("WU-{:03}", i + 1),
                kind: "wu".into(),
                project: "brain".into(),
                number: (i + 1) as i64,
                slug: tt.0.into(),
                status: tt.1.into(),
                delivery: tt.2.into(),
                date: tt.3.into(),
                delivered: tt.4.into(),
                tier: tt.7,
                ..pulse_store::Record::default()
            };
            s.put_record(rec, [("tier".into(), json!(tt.7))].into(), "", "agent", "")
                .unwrap();
        }
        let rows = lead_time(&s, "brain").unwrap();
        assert_eq!(rows.len(), cases.len());
        for (got, want) in rows.iter().zip(cases) {
            assert_eq!(got.state, want.5, "{}", want.0);
            assert_eq!(got.target, want.6, "{}", want.0);
            assert_eq!(got.days, want.8, "{}", want.0);
        }
    }
}
