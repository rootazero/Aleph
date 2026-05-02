use crate::gateway::channel::ChannelError;
use crate::sync_primitives::Mutex;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;

/// Inbound event deduplicator backed by SQLite with an in-memory LRU cache.
pub struct EventDeduper {
    conn: Mutex<Connection>,
    cache: Mutex<HashSet<(String, String)>>,
}

impl EventDeduper {
    /// Open or create the dedupe database at the given path.
    pub fn new(path: &str) -> Result<Self, ChannelError> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(std::path::Path::new(path))
            .map_err(|e| ChannelError::Internal(format!("Failed to open dedupe database: {e}")))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS matrix_seen_events (
                room_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                seen_at INTEGER NOT NULL,
                PRIMARY KEY (room_id, event_id)
            )",
            [],
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to create dedupe table: {e}")))?;

        // Prune entries older than 7 days on startup
        let week_ago = chrono::Utc::now().timestamp() - 7 * 24 * 60 * 60;
        conn.execute(
            "DELETE FROM matrix_seen_events WHERE seen_at < ?1",
            [week_ago],
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to prune dedupe table: {e}")))?;

        // Warm the in-memory cache with recent entries (limit to 10000)
        let mut stmt = conn
            .prepare(
                "SELECT room_id, event_id FROM matrix_seen_events
                 ORDER BY seen_at DESC LIMIT 10000",
            )
            .map_err(|e| ChannelError::Internal(format!("Failed to prepare dedupe query: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| ChannelError::Internal(format!("Failed to query dedupe cache: {e}")))?;

        let mut cache = HashSet::new();
        for row in rows {
            let (room_id, event_id) =
                row.map_err(|e| ChannelError::Internal(format!("Failed to read dedupe row: {e}")))?;
            cache.insert((room_id, event_id));
        }
        drop(stmt);

        Ok(Self {
            conn: Mutex::new(conn),
            cache: Mutex::new(cache),
        })
    }

    /// Returns true if the event is new and was inserted.
    pub fn try_insert(&self, room_id: &str, event_id: &str) -> Result<bool, ChannelError> {
        let key = (room_id.to_string(), event_id.to_string());

        {
            let cache = self
                .cache
                .lock()
                .map_err(|e| ChannelError::Internal(format!("Dedupe cache poisoned: {e}")))?;
            if cache.contains(&key) {
                return Ok(false);
            }
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| ChannelError::Internal(format!("Dedupe connection poisoned: {e}")))?;

        let existing: Option<String> = conn
            .query_row(
                "SELECT event_id FROM matrix_seen_events WHERE room_id = ?1 AND event_id = ?2",
                [room_id, event_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| ChannelError::Internal(format!("Failed to query dedupe: {e}")))?;

        if existing.is_some() {
            return Ok(false);
        }

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO matrix_seen_events (room_id, event_id, seen_at) VALUES (?1, ?2, ?3)",
            [room_id, event_id, &now.to_string()],
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to insert dedupe: {e}")))?;

        drop(conn);

        let mut cache = self
            .cache
            .lock()
            .map_err(|e| ChannelError::Internal(format!("Dedupe cache poisoned: {e}")))?;
        cache.insert(key);

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedupe_basic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let deduper = EventDeduper::new(tmp.path().to_str().unwrap()).unwrap();

        assert!(deduper.try_insert("!r:example.com", "$e1").unwrap());
        assert!(!deduper.try_insert("!r:example.com", "$e1").unwrap());
        assert!(deduper.try_insert("!r:example.com", "$e2").unwrap());
        assert!(deduper.try_insert("!r2:example.com", "$e1").unwrap());
    }

    #[test]
    fn test_dedupe_persists_across_instances() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        {
            let deduper = EventDeduper::new(path).unwrap();
            assert!(deduper.try_insert("!r:example.com", "$e1").unwrap());
        }

        {
            let deduper = EventDeduper::new(path).unwrap();
            assert!(!deduper.try_insert("!r:example.com", "$e1").unwrap());
            assert!(deduper.try_insert("!r:example.com", "$e2").unwrap());
        }
    }
}
