//! Shared board snapshots for the read API.
//!
//! Every project route and every organization route used to rebuild the
//! whole board snapshot (all posts with bodies, the tally, the plan gates,
//! four git subprocesses) on each request; the console polls a dozen such
//! routes and refetches them on every bus event, so a single hub kept several
//! cores busy and held gigabytes of duplicate snapshots in flight. Here a
//! snapshot is built once and handed out as an `Arc` to every request until
//! the store rows it was built from change (a cheap watermark over the
//! tables a snapshot reads) or it reaches `Handler::org_cache_ttl`. Requests
//! for the same project queue behind one build instead of each running their
//! own, and at most `MAX_CONCURRENT_BUILDS` builds run at once hub-wide.

use crate::board::BoardData;
use crate::server::Handler;
use anyhow::{Result, anyhow};
use pulse_bus::CancellationToken;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Snapshot builds that may run at the same time across all requests: an
/// organization view with nine projects behind it must not take nine cores.
const MAX_CONCURRENT_BUILDS: usize = 3;

/// A failed build is retried sooner than a good snapshot expires.
const ERROR_TTL: Duration = Duration::from_secs(5);

/// Organization routes other than `org` carry clock-dependent fields (agent
/// presence, heartbeat ages, timeline buckets) that the watermark cannot see,
/// so their assembled bytes are kept for a short while only; `org` itself
/// keeps the full TTL it always had.
pub(crate) const ORG_ROUTE_BYTES_TTL: Duration = Duration::from_secs(10);

type Slot = Arc<Mutex<Option<Snapshot>>>;

struct Snapshot {
    data: Result<Arc<BoardData>, String>,
    mark: String,
    at: Instant,
}

pub(crate) struct CachedBytes {
    pub(crate) bytes: Vec<u8>,
    mark: String,
    at: Instant,
}

#[derive(Default)]
pub(crate) struct ViewCache {
    snapshots: Mutex<BTreeMap<(String, String), Slot>>,
    presence: Mutex<BTreeMap<String, (bool, Instant)>>,
    org: Mutex<BTreeMap<String, CachedBytes>>,
    builds: Mutex<usize>,
    build_done: Condvar,
}

struct BuildPermit<'a>(&'a ViewCache);

impl Drop for BuildPermit<'_> {
    fn drop(&mut self) {
        let mut n = self.0.builds.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        self.0.build_done.notify_one();
    }
}

impl ViewCache {
    fn slot(&self, slug: &str, area: &str) -> Slot {
        let mut slots = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            slots
                .entry((slug.to_string(), area.to_string()))
                .or_default(),
        )
    }

    fn build_permit(&self) -> BuildPermit<'_> {
        let mut n = self.builds.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= MAX_CONCURRENT_BUILDS {
            n = self.build_done.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        BuildPermit(self)
    }

    /// Cached response bytes for an organization route while the view set
    /// they were built from carries the same watermark and is younger than
    /// `ttl`.
    pub(crate) fn org_bytes(&self, route: &str, mark: &str, ttl: Duration) -> Option<Vec<u8>> {
        let org = self.org.lock().unwrap_or_else(|e| e.into_inner());
        org.get(route)
            .filter(|c| c.mark == mark && c.at.elapsed() < ttl)
            .map(|c| c.bytes.clone())
    }

    pub(crate) fn store_org_bytes(&self, route: &str, mark: &str, bytes: &[u8]) {
        let mut org = self.org.lock().unwrap_or_else(|e| e.into_inner());
        org.insert(
            route.to_string(),
            CachedBytes {
                bytes: bytes.to_vec(),
                mark: mark.to_string(),
                at: Instant::now(),
            },
        );
    }
}

impl Handler {
    /// The board snapshot for `slug`, shared between requests while the store
    /// rows it reads are unchanged and it is younger than `org_cache_ttl`.
    /// `force` rebuilds regardless (the `refresh` route); a zero TTL disables
    /// sharing altogether. Concurrent requests for the same project wait for
    /// one build. A request whose client went away before its build started
    /// does not start one.
    pub(crate) fn snapshot_shared(
        &self,
        slug: &str,
        area: &str,
        force: bool,
        ctx: Option<&CancellationToken>,
    ) -> Result<Arc<BoardData>> {
        self.snapshot_marked(slug, area, force, ctx)
            .map(|(data, _)| data)
    }

    /// `snapshot_shared` together with the watermark the snapshot was built
    /// from (empty when sharing is disabled). The watermark is computed even
    /// when the build fails so callers can tell a stale error from a new one.
    pub(crate) fn snapshot_marked(
        &self,
        slug: &str,
        area: &str,
        force: bool,
        ctx: Option<&CancellationToken>,
    ) -> Result<(Arc<BoardData>, String)> {
        let ttl = self.org_cache_ttl;
        if ttl.is_zero() {
            return Ok((Arc::new(self.snapshot(slug, area)?), String::new()));
        }
        let mark = self.watermark(slug)?;
        let slot = self.views.slot(slug, area);
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        if !force
            && let Some(entry) = guard.as_ref()
            && entry.mark == mark
        {
            match &entry.data {
                Ok(data) if entry.at.elapsed() < ttl => return Ok((Arc::clone(data), mark)),
                Err(err) if entry.at.elapsed() < ERROR_TTL.min(ttl) => {
                    return Err(anyhow!("{err}"));
                }
                _ => {}
            }
        }
        if ctx.is_some_and(|c| c.is_cancelled()) {
            return Err(anyhow!(
                "the request was cancelled before the snapshot was built"
            ));
        }
        let _permit = self.views.build_permit();
        let data = self.snapshot(slug, area).map(Arc::new);
        *guard = Some(Snapshot {
            data: data.as_ref().map(Arc::clone).map_err(|e| e.to_string()),
            mark: mark.clone(),
            at: Instant::now(),
        });
        data.map(|data| (data, mark))
    }

    /// `board::present` remembered per path for the cache TTL: it spawns git.
    pub(crate) fn present_cached(&self, p: &pulse_store::Project) -> bool {
        let ttl = self.org_cache_ttl;
        if ttl.is_zero() {
            return crate::board::present(p);
        }
        {
            let presence = self
                .views
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((present, at)) = presence.get(&p.path)
                && at.elapsed() < ttl
            {
                return *present;
            }
        }
        let present = crate::board::present(p);
        self.views
            .presence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(p.path.clone(), (present, Instant::now()));
        present
    }

    /// A cheap signature of the store rows a project snapshot is built from:
    /// its posts, jobs, decisions, records and drafts, the evolution tables,
    /// the project row, the settings, and landing events. Anything that only
    /// changes on disk (git head, STOP files) is covered by the TTL instead.
    /// The events sequence as a whole is deliberately absent: `harness.step`
    /// events arrive every second or two and would defeat the cache.
    pub(crate) fn watermark(&self, slug: &str) -> Result<String> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let p = self.entry(slug)?;
        let conn = store.conn()?;
        // A 64-bit hash is plenty: a collision only keeps a snapshot until
        // its TTL, and the mark stays short enough to compare per request.
        let mut h = DefaultHasher::new();
        let mut mark = |part: &str| {
            h.write(part.as_bytes());
            h.write_u8(b'\n');
        };
        mark(&format!(
            "project:{}:{}:{}:{}:{}:{}:{}",
            p.slug, p.name, p.path, p.remote, p.brain, p.default_branch, p.status
        ));
        // The checkout root's mtime moves when a STOP file or the constitution
        // appears or goes; changes deeper in the tree wait for the TTL.
        let root_mtime = std::fs::metadata(&p.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        mark(&format!("root:{root_mtime}"));
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM posts WHERE project = ?",
            slug,
        )?);
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(rowid), 0), COALESCE(SUM(CASE status
                WHEN 'done' THEN 1 WHEN 'failed' THEN 2 WHEN 'cancelled' THEN 3
                WHEN 'queued' THEN 4 WHEN 'leased' THEN 5 WHEN 'blocked' THEN 6
                ELSE 7 END), 0), COALESCE(MAX(finished_at), ''), COALESCE(MAX(lease_until), ''),
                COALESCE(SUM(generation), 0), COALESCE(SUM(attempts), 0), COALESCE(MAX(lease_agent), '')
            FROM jobs WHERE project = ?",
            slug,
        )?);
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(decided_at), ''), COALESCE(SUM(LENGTH(result_post)), 0)
            FROM decisions WHERE project = ?",
            slug,
        )?);
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(updated_at), ''), COALESCE(SUM(current_rev), 0)
            FROM records WHERE project = ?",
            slug,
        )?);
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(at), '') FROM record_revisions
            WHERE record_id IN (SELECT id FROM records WHERE project = ?)",
            slug,
        )?);
        mark(&row_text(
            &conn,
            "SELECT COUNT(*), COALESCE(MAX(at), ''), COALESCE(SUM(base_rev), 0) FROM record_drafts
            WHERE record_id IN (SELECT id FROM records WHERE project = ?)",
            slug,
        )?);
        let tables = existing_tables(
            &conn,
            &[
                "evolution_archive",
                "evolution_fixtures",
                "evolution_baselines",
                "evolution_fleets",
                "evolution_cycle_leases",
            ],
        )?;
        for (table, query) in [
            (
                "evolution_archive",
                "SELECT COUNT(*), COALESCE(MAX(at), '') FROM evolution_archive WHERE project = ?",
            ),
            (
                "evolution_fixtures",
                "SELECT COUNT(*), COALESCE(MAX(at), ''), COALESCE(SUM(enabled), 0), COALESCE(SUM(metric), 0),
                    COALESCE(TOTAL(weight), 0), COALESCE(MAX(rowid), 0) FROM evolution_fixtures WHERE project = ?",
            ),
            (
                "evolution_baselines",
                "SELECT COUNT(*), COALESCE(MAX(at), '') FROM evolution_baselines WHERE project = ?",
            ),
            (
                "evolution_fleets",
                "SELECT COUNT(*), COALESCE(MAX(at), ''), COALESCE(MAX(cleaned_at), '') FROM evolution_fleets WHERE project = ?",
            ),
            (
                "evolution_cycle_leases",
                "SELECT COALESCE(MAX(owner), ''), COALESCE(MAX(expires_at), '') FROM evolution_cycle_leases WHERE project = ?",
            ),
        ] {
            if tables.contains(&table.to_string()) {
                mark(&row_text(&conn, query, slug)?);
            }
        }
        let landed: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE action >= 'land.' AND action < 'land/'",
            [],
            |r| r.get(0),
        )?;
        mark(&format!("landed:{landed}"));
        let mut stmt = conn.prepare("SELECT name, value FROM settings ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (name, value) = row?;
            mark(&format!("setting:{name}={value}"));
        }
        Ok(format!("{:016x}", h.finish()))
    }
}

/// The signature of a view set an organization response was built from:
/// every project with its presence, error and snapshot watermark.
pub(crate) fn org_mark(views: &[crate::read::ProjectView]) -> String {
    let mut h = DefaultHasher::new();
    for view in views {
        for part in [
            view.project.slug.as_str(),
            if view.present { "present" } else { "absent" },
            view.error.as_str(),
            view.mark.as_str(),
            if view.data.is_some() { "data" } else { "none" },
        ] {
            h.write(part.as_bytes());
            h.write_u8(0);
        }
        h.write_u8(b'\n');
    }
    format!("{:016x}", h.finish())
}

fn row_text(conn: &rusqlite::Connection, query: &str, slug: &str) -> Result<String> {
    let mut stmt = conn.prepare_cached(query)?;
    let text = stmt.query_row([slug], |r| {
        let mut parts = Vec::with_capacity(r.as_ref().column_count());
        for i in 0..r.as_ref().column_count() {
            let v = r.get_ref(i)?;
            parts.push(match v {
                rusqlite::types::ValueRef::Null => String::new(),
                rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                rusqlite::types::ValueRef::Real(f) => f.to_string(),
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                rusqlite::types::ValueRef::Blob(b) => format!("{} bytes", b.len()),
            });
        }
        Ok(parts.join("|"))
    })?;
    Ok(text)
}

fn existing_tables(conn: &rusqlite::Connection, names: &[&str]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN (?, ?, ?, ?, ?)",
    )?;
    let mut args = names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    args.resize(5, String::new());
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
        r.get::<_, String>(0)
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_f3 as f3;

    fn repo() -> String {
        let dir = crate::test_helpers::hold_tempdir(tempfile::tempdir().expect("temp repo"));
        let dir = std::fs::canonicalize(dir).expect("canonical temp repo");
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["init", "-q", "-b", "main"])
            .output()
            .expect("git init");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir.display().to_string()
    }

    fn fixture() -> (f3::TestStore, Handler) {
        let s = f3::additions_store(false);
        f3::additions_project(&s, "cached");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET path=? WHERE slug=?",
                rusqlite::params![repo(), "cached"],
            )
            .unwrap();
        let h = f3::handler(&s);
        (s, h)
    }

    #[test]
    fn snapshot_is_shared_until_the_store_changes() {
        let _serial = crate::test_helpers::serial();
        let (s, h) = fixture();
        let a = h.snapshot_shared("cached", "", false, None).unwrap();
        let b = h.snapshot_shared("cached", "", false, None).unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "an unchanged store shares one snapshot"
        );
        let agent = pulse_board::join_agent(
            &s,
            pulse_board::JoinInput {
                name: "writer".into(),
                runner: "test".into(),
                model: "test".into(),
                session: "writer".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .0;
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "finding".into(),
                project: "cached".into(),
                area: "console".into(),
                thread: "cache".into(),
                agent_row: Some(agent),
                summary: "a new post".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let c = h.snapshot_shared("cached", "", false, None).unwrap();
        assert!(!Arc::ptr_eq(&a, &c), "a new post rebuilds the snapshot");
        assert_eq!(c.posts.len(), a.posts.len() + 1);
        let d = h.snapshot_shared("cached", "", true, None).unwrap();
        assert!(!Arc::ptr_eq(&c, &d), "refresh forces a rebuild");
    }

    #[test]
    fn settings_and_decisions_change_the_watermark() {
        let _serial = crate::test_helpers::serial();
        let (s, h) = fixture();
        let before = h.watermark("cached").unwrap();
        assert_eq!(before, h.watermark("cached").unwrap());
        s.conn()
            .unwrap()
            .execute("UPDATE settings SET value = '1' WHERE name = 'stop'", [])
            .unwrap();
        let stopped = h.watermark("cached").unwrap();
        assert_ne!(before, stopped);
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO decisions (post_id, project, answer, decided_at) VALUES ('p1', 'cached', 'yes', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        assert_ne!(stopped, h.watermark("cached").unwrap());
        // Harness steps are the bulk of the bus and must not invalidate it.
        let steady = h.watermark("cached").unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO events (at, actor, action, subject, detail) VALUES ('2026-01-01T00:00:00Z', 'x', 'harness.step', 'cached', '')",
                [],
            )
            .unwrap();
        assert_eq!(steady, h.watermark("cached").unwrap());
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO events (at, actor, action, subject, detail) VALUES ('2026-01-01T00:00:00Z', 'x', 'land.pushed', 'cached', '')",
                [],
            )
            .unwrap();
        assert_ne!(steady, h.watermark("cached").unwrap());
    }

    #[test]
    fn zero_ttl_disables_sharing() {
        let _serial = crate::test_helpers::serial();
        let (_s, mut h) = fixture();
        h.org_cache_ttl = Duration::ZERO;
        let a = h.snapshot_shared("cached", "", false, None).unwrap();
        let b = h.snapshot_shared("cached", "", false, None).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }
}
