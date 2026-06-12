//! Database handle, error type, and migration runner.

use rusqlite::Connection;
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

/// Embedded initial migration (spec §4). Applied on open.
const MIGRATION_0001: &str = include_str!("../../../migrations/0001_initial.sql");

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Domain(#[from] divan_core::DivanError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("integrity: {0}")]
    Integrity(String),
}

/// Shared SQLite handle. Cheap to clone (`Arc`).
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Open (or create) a database at `path`, set WAL, and run migrations.
    pub fn open(path: &std::path::Path) -> DbResult<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory database (tests).
    pub fn open_in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> DbResult<Self> {
        // WAL improves concurrent read/write for the daemon (spec §3.1).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(MIGRATION_0001)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Lock the connection. Poison is treated as a fatal integrity error.
    pub(crate) fn lock(&self) -> DbResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| DbError::Integrity("connection mutex poisoned".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_creates_schema() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN \
                 ('agents','tasks','task_deps','messages','artifacts','subscriptions',\
                  'trace_events','file_touches')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 8, "all 8 tables created");
    }

    #[test]
    fn summary_check_constraint_enforced_at_db() {
        // Spec §4: CHECK(length(summary) <= 400) — DB-level enforcement (K4).
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let long = "x".repeat(401);
        let res = conn.execute(
            "INSERT INTO messages (id, from_agent, kind, summary, created_at) \
             VALUES ('m1','a','status',?1, 0)",
            [&long],
        );
        assert!(
            res.is_err(),
            "401-char summary must be rejected by DB CHECK"
        );
    }
}
