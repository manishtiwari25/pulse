use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::tally::TallyResult;
use crate::thresholds::Thresholds;

const CACHE_TTL: Duration = Duration::from_secs(600);
const CACHE_MAX_ENTRIES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    store_path: String,
    project: String,
    area: String,
    th_hash: String,
}

// Go keeps a *Result in the cache and hands the same pointer to every caller;
// here callers own their copy, so the cache holds an Arc and the deep clone
// happens after the lock is released - the hub's parallel snapshots would
// otherwise queue on this one mutex for the duration of every copy.
#[derive(Clone)]
struct CacheValue {
    result: Arc<TallyResult>,
    version: String,
    at: Instant,
}

#[derive(Default)]
struct LruCache {
    entries: BTreeMap<CacheKey, CacheValue>,
    order: VecDeque<CacheKey>,
}

static TALLY_CACHE: LazyLock<Mutex<LruCache>> = LazyLock::new(|| Mutex::new(LruCache::default()));
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
}

pub fn cache_stats() -> CacheStats {
    CacheStats {
        hits: CACHE_HITS.load(Ordering::Relaxed),
        misses: CACHE_MISSES.load(Ordering::Relaxed),
    }
}

fn cache_ttl(s: &pulse_store::Store) -> Duration {
    let secs = s
        .setting_int("daemon.planner_cache_ttl_seconds")
        .max(CACHE_TTL.as_secs() as i64) as u64;
    Duration::from_secs(secs)
}

#[cfg(test)]
pub(crate) fn clear_cache_for_test() {
    *TALLY_CACHE.lock().expect("tally cache poisoned") = LruCache::default();
    CACHE_HITS.store(0, Ordering::Relaxed);
    CACHE_MISSES.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn cache_len_for_test() -> usize {
    TALLY_CACHE
        .lock()
        .expect("tally cache poisoned")
        .entries
        .len()
}

#[cfg(test)]
pub(crate) fn expire_cache_for_test() {
    let mut c = TALLY_CACHE.lock().expect("tally cache poisoned");
    for cv in c.entries.values_mut() {
        cv.at -= CACHE_TTL + Duration::from_secs(1);
    }
}

fn stable_hash<T: Serialize>(value: &T) -> String {
    let b = pulse_store::gojson::to_vec(value).unwrap_or_default();
    let h = Sha256::digest(b);
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn cache_key_for(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    mut th: Thresholds,
) -> CacheKey {
    if th.now.len() >= 16 {
        th.now = format!("{}:00Z", &th.now[..16]);
    }
    CacheKey {
        store_path: s.path.clone(),
        project: project.into(),
        area: area.into(),
        th_hash: stable_hash(&th),
    }
}

pub fn project_watermark(s: &pulse_store::Store, project: &str, area: &str) -> Result<String> {
    let conn = s.conn()?;
    let mut post_q =
        "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM posts WHERE project = ?".to_string();
    let (posts_count, posts_max): (i64, i64) = if area.is_empty() {
        conn.query_row(&post_q, [project], |r| Ok((r.get(0)?, r.get(1)?)))?
    } else {
        post_q.push_str(" AND area = ?");
        conn.query_row(&post_q, rusqlite::params![project, area], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
    };
    let (jobs_count, jobs_max, jobs_status, jobs_finished, jobs_lease_until): (
        i64,
        i64,
        i64,
        String,
        String,
    ) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(rowid), 0), COALESCE(SUM(CASE status
            WHEN 'done' THEN 1 WHEN 'failed' THEN 2 WHEN 'cancelled' THEN 3
            WHEN 'queued' THEN 4 WHEN 'leased' THEN 5 WHEN 'blocked' THEN 6
            ELSE 0 END), 0), COALESCE(MAX(finished_at), ''), COALESCE(MAX(lease_until), '')
        FROM jobs WHERE project = ?",
        [project],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let events_max: i64 =
        conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
    let (records_count, records_updated) =
        table_version(s, "records", "project", project, "updated_at");
    let (revs_count, revs_at) = record_revision_version(s, project);
    let settings_hash = settings_hash(s);
    let repo = s.project(project)?.map(|p| p.path).unwrap_or_default();
    let repo_mark = path_mtime_mark(&repo);
    Ok(format!(
        "e:{events_max}:p:{posts_count}:{posts_max}:j:{jobs_count}:{jobs_max}:{jobs_status}:{jobs_finished}:{jobs_lease_until}:rec:{records_count}:{records_updated}:rev:{revs_count}:{revs_at}:sh:{settings_hash}:r:{repo}:{repo_mark}"
    ))
}

fn path_mtime_mark(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format!("{}.{:09}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_default()
}

fn table_version(
    s: &pulse_store::Store,
    table: &str,
    where_col: &str,
    where_val: &str,
    max_col: &str,
) -> (i64, String) {
    let Ok(conn) = s.conn() else {
        return (0, String::new());
    };
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return (0, String::new());
    }
    let mut q = format!("SELECT COUNT(*), COALESCE(MAX({max_col}), '') FROM {table}");
    if where_col.is_empty() {
        conn.query_row(&q, [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((0, String::new()))
    } else {
        q.push_str(&format!(" WHERE {where_col} = ?"));
        conn.query_row(&q, [where_val], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((0, String::new()))
    }
}

fn settings_hash(s: &pulse_store::Store) -> String {
    let Ok(conn) = s.conn() else {
        return String::new();
    };
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'settings'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return String::new();
    }
    let Ok(mut stmt) = conn.prepare("SELECT name, value FROM settings ORDER BY name") else {
        return String::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
    else {
        return String::new();
    };
    let mut h = Sha256::new();
    for row in rows.flatten() {
        h.update(row.0.as_bytes());
        h.update(b"=");
        h.update(row.1.as_bytes());
        h.update(b"\n");
    }
    h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn record_revision_version(s: &pulse_store::Store, project: &str) -> (i64, String) {
    let Ok(conn) = s.conn() else {
        return (0, String::new());
    };
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'record_revisions'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return (0, String::new());
    }
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(at), '') FROM record_revisions
	      WHERE record_id IN (SELECT id FROM records WHERE project = ?)",
        [project],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((0, String::new()))
}

pub(crate) fn cached_compute<F>(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
    compute: F,
) -> Result<TallyResult>
where
    F: FnOnce() -> Result<TallyResult>,
{
    let key = cache_key_for(s, project, area, th);
    let version = project_watermark(s, project, area)?;
    let ttl = cache_ttl(s);
    let hit = {
        let mut c = TALLY_CACHE.lock().expect("tally cache poisoned");
        match c.entries.get(&key) {
            Some(cv) if cv.at.elapsed() < ttl && cv.version == version => {
                let result = Arc::clone(&cv.result);
                c.order.retain(|k| k != &key);
                c.order.push_front(key.clone());
                Some(result)
            }
            _ => None,
        }
    };
    if let Some(result) = hit {
        CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return Ok((*result).clone());
    }
    CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    let res = Arc::new(compute()?);
    {
        let mut c = TALLY_CACHE.lock().expect("tally cache poisoned");
        c.entries.insert(
            key.clone(),
            CacheValue {
                result: Arc::clone(&res),
                version,
                at: Instant::now(),
            },
        );
        c.order.retain(|k| k != &key);
        c.order.push_front(key);
        while c.entries.len() > CACHE_MAX_ENTRIES {
            if let Some(old) = c.order.pop_back() {
                c.entries.remove(&old);
            } else {
                break;
            }
        }
    }
    Ok(Arc::try_unwrap(res).unwrap_or_else(|shared| (*shared).clone()))
}
