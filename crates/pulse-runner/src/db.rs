#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::Result;
use std::time::{Duration, Instant};

pub(crate) use crate::util::Querier;

pub(crate) struct ConnQ(pub(crate) pulse_store::PooledConn);
impl crate::util::Querier for ConnQ {
    fn conn(&self) -> &rusqlite::Connection {
        &self.0
    }
}

/// Go: func (q connQ) Query(query string, args ...any) (*sql.Rows, error)
/// Go: func (q connQ) QueryRow(query string, args ...any) *sql.Row
/// Go: func (q connQ) Exec(query string, args ...any) (sql.Result, error)
impl ConnQ {
    pub(crate) fn execute(
        &self,
        query: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> rusqlite::Result<usize> {
        self.conn().execute(query, args)
    }
}

/// Go: func immediate(s *store.Store, fn func(q querier) error) (err error)
pub(crate) fn immediate<F>(s: &pulse_store::Store, fn_: F) -> Result<()>
where
    F: FnOnce(&ConnQ) -> Result<()>,
{
    let conn = s.conn()?;
    let q = ConnQ(conn);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match q.conn().execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => break,
            Err(e) if is_busy(&e) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e.into()),
        }
    }
    match fn_(&q) {
        Ok(()) => {
            q.conn().execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = q.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Go: func isBusy(err error) bool
pub(crate) fn is_busy(err: &dyn std::error::Error) -> bool {
    let m = err.to_string();
    m.contains("SQLITE_BUSY")
        || m.contains("database is locked")
        || m.contains("(5)")
        || m.contains("(6)")
}
