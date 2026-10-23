use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use crate::api::Scores;

pub const BASELINE_CACHE_SETTINGS: &[&str] = &[
    "worker_model",
    "light_model",
    "provider",
    "harness.max_steps",
    "harness.step_timeout",
    "harness.commit_timeout",
    "harness.output_limit",
    "harness.max_cost",
    "harness.keep_observations",
    "minutes_per_job",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BaselineCacheEntry {
    pub project: String,
    pub commit: String,
    pub fixture_hash: String,
    pub score: f64,
    pub scores: Scores,
    pub run_id: String,
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BaselineCacheSummary {
    pub count: i64,
    pub commit: String,
    pub fixture_hash: String,
    pub score: f64,
    pub at: String,
}

fn quoted(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn push_kv(canonical: &mut String, key: &str, value: &str) -> Result<()> {
    writeln!(canonical, "{key}={}", quoted(value)?).expect("write to string");
    Ok(())
}

fn missing_baseline_table(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("no such table: evolution_baselines"))
}

fn ensure_baseline_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS evolution_baselines (
  project TEXT NOT NULL,
  "commit" TEXT NOT NULL,
  fixture_hash TEXT NOT NULL,
  score REAL NOT NULL,
  fixtures_json TEXT NOT NULL,
  run_id TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (project, "commit", fixture_hash)
);
CREATE INDEX IF NOT EXISTS evolution_baselines_project_at ON evolution_baselines (project, at)"#,
    )?;
    Ok(())
}

pub fn baseline_fixture_key(s: &pulse_store::Store, project: &str) -> Result<String> {
    let conn = s.conn()?;
    let mut canonical = String::new();
    let mut rows = conn.prepare(
        "SELECT id, request, scorer_cmd, weight, kind, COALESCE(metric, 0) FROM evolution_fixtures
        WHERE project = ? AND enabled = 1 ORDER BY id",
    )?;
    let mut fixtures = rows.query(params![project])?;
    let mut idx = 0;
    while let Some(row) = fixtures.next()? {
        idx += 1;
        let id: String = row.get(0)?;
        let request: String = row.get(1)?;
        let scorer_cmd: String = row.get(2)?;
        let weight: f64 = row.get(3)?;
        let kind: String = row.get(4)?;
        let metric = row.get::<_, i64>(5)? != 0;
        push_kv(&mut canonical, &format!("fixture.{idx}.id"), &id)?;
        push_kv(&mut canonical, &format!("fixture.{idx}.request"), &request)?;
        push_kv(
            &mut canonical,
            &format!("fixture.{idx}.scorer_cmd"),
            &scorer_cmd,
        )?;
        writeln!(canonical, "fixture.{idx}.weight={weight:.17}").expect("write to string");
        push_kv(&mut canonical, &format!("fixture.{idx}.kind"), &kind)?;
        writeln!(canonical, "fixture.{idx}.metric={metric}").expect("write to string");
    }

    writeln!(canonical, "fixtures.count={idx}").expect("write to string");
    let (mut heldout, heldout_state) = crate::private::load_private(s).unwrap_or_default();
    heldout.sort_by(|a, b| a.stem.cmp(&b.stem));
    writeln!(canonical, "heldout.count={}", heldout_state.count).expect("write to string");
    writeln!(canonical, "heldout.pending={}", heldout_state.pending).expect("write to string");
    writeln!(canonical, "heldout.generated={}", heldout_state.generated).expect("write to string");
    push_kv(&mut canonical, "heldout.problem", &heldout_state.problem)?;
    for (i, fixture) in heldout.iter().enumerate() {
        let idx = i + 1;
        push_kv(
            &mut canonical,
            &format!("heldout.{idx}.stem"),
            &fixture.stem,
        )?;
        push_kv(
            &mut canonical,
            &format!("heldout.{idx}.request"),
            &fixture.request,
        )?;
        push_kv(
            &mut canonical,
            &format!("heldout.{idx}.judge"),
            &fixture.judge,
        )?;
        let scorer = std::fs::read_to_string(&fixture.scorer).unwrap_or_default();
        push_kv(&mut canonical, &format!("heldout.{idx}.scorer"), &scorer)?;
    }
    for key in BASELINE_CACHE_SETTINGS {
        push_kv(
            &mut canonical,
            &format!("setting.{key}"),
            &s.setting(key, ""),
        )?;
    }
    Ok(canonical)
}

pub fn latest_baseline_scores(s: &pulse_store::Store, project: &str) -> Result<Option<Scores>> {
    let conn = s.conn()?;
    let fixtures_json: Option<String> = match conn
        .query_row(
            "SELECT fixtures_json FROM evolution_baselines WHERE project = ? ORDER BY at DESC LIMIT 1",
            params![project],
            |r| r.get(0),
        )
        .optional()
    {
        Ok(row) => row,
        Err(e) if missing_baseline_table(&e) => None,
        Err(e) => return Err(e.into()),
    };
    Ok(fixtures_json.and_then(|text| serde_json::from_str(&text).ok()))
}

pub fn baseline_fixture_hash(s: &pulse_store::Store, project: &str) -> Result<String> {
    let canonical = baseline_fixture_key(s, project)?;
    let digest = Sha256::digest(canonical.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("write to string");
    }
    Ok(out)
}

pub fn baseline_ttl_hours(s: &pulse_store::Store) -> i64 {
    s.setting("evolve.baseline_ttl_hours", "")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|v| *v >= 0)
        .unwrap_or(24)
}

pub fn cached_baseline(
    s: &pulse_store::Store,
    project: &str,
    commit: &str,
    fixture_hash: &str,
) -> Result<Option<BaselineCacheEntry>> {
    if commit.is_empty() || fixture_hash.is_empty() {
        return Ok(None);
    }
    let conn = s.conn()?;
    let row: Option<(f64, String, String, String)> = match conn.query_row(
        r#"SELECT score, fixtures_json, run_id, at FROM evolution_baselines
            WHERE project = ? AND "commit" = ? AND fixture_hash = ?"#,
        params![project, commit, fixture_hash],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ) {
        Ok(row) => Some(row),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) if missing_baseline_table(&e) => None,
        Err(e) => return Err(e.into()),
    };
    let Some((score, fixtures_json, run_id, at)) = row else {
        return Ok(None);
    };
    let ttl = baseline_ttl_hours(s);
    if ttl > 0 {
        let now = DateTime::parse_from_rfc3339(&s.now());
        let measured = DateTime::parse_from_rfc3339(&at);
        match (now, measured) {
            (Ok(now), Ok(measured))
                if now
                    .with_timezone(&Utc)
                    .signed_duration_since(measured.with_timezone(&Utc))
                    <= chrono::Duration::hours(ttl) => {}
            _ => return Ok(None),
        }
    }
    let mut scores: Scores = serde_json::from_str(&fixtures_json).unwrap_or_default();
    if scores.total == 0.0 && score != 0.0 {
        scores.total = score;
    }
    if scores.project.is_empty() {
        scores.project = project.to_string();
    }
    Ok(Some(BaselineCacheEntry {
        project: project.to_string(),
        commit: commit.to_string(),
        fixture_hash: fixture_hash.to_string(),
        score,
        scores,
        run_id,
        at,
    }))
}

pub fn upsert_baseline(
    s: &pulse_store::Store,
    project: &str,
    commit: &str,
    fixture_hash: &str,
    scores: &Scores,
    run_id: &str,
) -> Result<()> {
    if commit.is_empty() || fixture_hash.is_empty() {
        return Ok(());
    }
    let fixtures_json = pulse_store::gojson::to_string(scores)?;
    let conn = s.conn()?;
    let insert = r#"INSERT INTO evolution_baselines(project, "commit", fixture_hash, score, fixtures_json, run_id, at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(project, "commit", fixture_hash) DO UPDATE SET
          score = excluded.score,
          fixtures_json = excluded.fixtures_json,
          run_id = excluded.run_id,
          at = excluded.at"#;
    match conn.execute(
        insert,
        params![
            project,
            commit,
            fixture_hash,
            scores.total,
            fixtures_json,
            run_id,
            s.now()
        ],
    ) {
        Ok(_) => {}
        Err(e) if missing_baseline_table(&e) => {
            ensure_baseline_table(&conn)?;
            conn.execute(
                insert,
                params![
                    project,
                    commit,
                    fixture_hash,
                    scores.total,
                    fixtures_json,
                    run_id,
                    s.now()
                ],
            )?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

pub fn baseline_cache_summary(
    s: &pulse_store::Store,
    project: &str,
) -> Result<BaselineCacheSummary> {
    let conn = s.conn()?;
    let count: i64 = match conn.query_row(
        "SELECT COUNT(*) FROM evolution_baselines WHERE project = ?",
        params![project],
        |r| r.get(0),
    ) {
        Ok(count) => count,
        Err(e) if missing_baseline_table(&e) => return Ok(BaselineCacheSummary::default()),
        Err(e) => return Err(e.into()),
    };
    let latest: Option<(String, String, f64, String)> = conn
        .query_row(
            r#"SELECT "commit", fixture_hash, score, at FROM evolution_baselines
            WHERE project = ? ORDER BY at DESC LIMIT 1"#,
            params![project],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((commit, fixture_hash, score, at)) = latest else {
        return Ok(BaselineCacheSummary {
            count,
            ..Default::default()
        });
    };
    Ok(BaselineCacheSummary {
        count,
        commit,
        fixture_hash,
        score,
        at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn baseline_fixture_hash_changes_with_inputs() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        fixture(&s, "demo", "cache-key", "printf 1", 1.0);
        let first = baseline_fixture_hash(&s, "demo").unwrap();
        let conn = s.conn().unwrap();
        conn.execute(
            "UPDATE evolution_fixtures SET request = request || ' changed' WHERE project = 'demo' AND id = (SELECT id FROM evolution_fixtures WHERE project = 'demo' ORDER BY id LIMIT 1)",
            [],
        )
        .unwrap();
        let fixture_changed = baseline_fixture_hash(&s, "demo").unwrap();
        assert_ne!(first, fixture_changed);
        conn.execute(
            "UPDATE evolution_fixtures SET request = REPLACE(request, ' changed', '') WHERE project = 'demo'",
            [],
        )
        .unwrap();
        s.set_setting("human", "worker_model", "changed-model")
            .unwrap();
        let model_changed = baseline_fixture_hash(&s, "demo").unwrap();
        assert_ne!(first, model_changed);
        s.set_setting("human", "worker_model", "").unwrap();
        conn.execute(
            "UPDATE evolution_fixtures SET metric = 1 WHERE project = 'demo'",
            [],
        )
        .unwrap();
        let metric_changed = baseline_fixture_hash(&s, "demo").unwrap();
        assert_ne!(first, metric_changed);
    }

    #[test]
    fn cached_baseline_respects_ttl() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        fixture(&s, "demo", "cache-key", "printf 1", 1.0);
        let hash = baseline_fixture_hash(&s, "demo").unwrap();
        let scores = Scores {
            project: "demo".into(),
            total: 0.7,
            ..Default::default()
        };
        upsert_baseline(&s, "demo", "abcdef", &hash, &scores, "run-one").unwrap();
        assert_eq!(
            cached_baseline(&s, "demo", "abcdef", &hash)
                .unwrap()
                .unwrap()
                .scores
                .total,
            0.7
        );
        let old = (DateTime::parse_from_rfc3339(&s.now())
            .unwrap()
            .with_timezone(&Utc)
            - chrono::Duration::hours(25))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE evolution_baselines SET at = ? WHERE project = 'demo'",
                params![old],
            )
            .unwrap();
        assert!(
            cached_baseline(&s, "demo", "abcdef", &hash)
                .unwrap()
                .is_none()
        );
        s.conn()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO settings(name, value, updated_at, updated_by) VALUES('evolve.baseline_ttl_hours', '0', ?, 'human')",
                params![s.now()],
            )
            .unwrap();
        assert!(
            cached_baseline(&s, "demo", "abcdef", &hash)
                .unwrap()
                .is_some()
        );
    }
}
