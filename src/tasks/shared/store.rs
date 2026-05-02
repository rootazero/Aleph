//! Shared SQLite database handle for the tasks subsystem.
//!
//! Provides a thin wrapper around a rusqlite connection with WAL mode
//! and busy timeout configured. Intended to be used by both cron and
//! future task types (heartbeat, etc.).

use rusqlite::Connection;
use std::path::Path;

pub struct TaskDatabase {
    conn: Connection,
}

impl TaskDatabase {
    /// Open (or create) a SQLite database at the given path.
    ///
    /// Enables WAL journal mode and a 5-second busy timeout.
    pub fn open(path: &Path) -> Result<Self, String> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create store directory: {e}"))?;
        }

        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| format!("failed to open database at {}: {e}", path.display()))?;

        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}
