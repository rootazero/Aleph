use crate::gateway::channel::ChannelError;
use crate::sync_primitives::Mutex;
use rusqlite::Connection;
use std::collections::HashSet;

const CACHE_LIMIT: usize = 10_000;
const RETENTION: i64 = 7 * 24 * 60 * 60;

/// SQLite is authoritative; the bounded hot cache is discarded on pruning.
pub struct EventDeduper {
    state: Mutex<DedupeState>,
}

struct DedupeState {
    conn: Connection,
    cache: HashSet<(String, String)>,
    last_prune: i64,
}

impl EventDeduper {
    pub fn new(path: &str) -> Result<Self, ChannelError> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(std::path::Path::new(path))
            .map_err(|e| ChannelError::Internal(format!("Failed to open dedupe database: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS matrix_seen_events (
            room_id TEXT NOT NULL, event_id TEXT NOT NULL, seen_at INTEGER NOT NULL,
            PRIMARY KEY (room_id, event_id))",
            [],
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to create dedupe table: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "DELETE FROM matrix_seen_events WHERE seen_at < ?1",
            [now - RETENTION],
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to prune dedupe table: {e}")))?;
        Ok(Self {
            state: Mutex::new(DedupeState {
                conn,
                cache: HashSet::new(),
                last_prune: now,
            }),
        })
    }

    /// Call on a blocking worker. One lock serializes cache, pruning and insertion.
    pub fn try_insert(&self, room_id: &str, event_id: &str) -> Result<bool, ChannelError> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| ChannelError::Internal(format!("Dedupe state poisoned: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        if now - state.last_prune >= 3600 {
            state
                .conn
                .execute(
                    "DELETE FROM matrix_seen_events WHERE seen_at < ?1",
                    [now - RETENTION],
                )
                .map_err(|e| {
                    ChannelError::Internal(format!("Failed to prune dedupe table: {e}"))
                })?;
            state.cache.clear();
            state.last_prune = now;
        }
        let key = (room_id.to_string(), event_id.to_string());
        if state.cache.contains(&key) {
            return Ok(false);
        }
        let exists: bool = state.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM matrix_seen_events WHERE room_id = ?1 AND event_id = ?2)",
            [&key.0, &key.1],
            |row| row.get(0),
        ).map_err(|e| ChannelError::Internal(format!("Failed to query dedupe table: {e}")))?;
        if exists {
            return Ok(false);
        }
        if state.cache.len() >= CACHE_LIMIT {
            state.cache.clear();
        }
        state.cache.insert(key);
        state.conn.execute("INSERT OR IGNORE INTO matrix_seen_events (room_id, event_id, seen_at) VALUES (?1, ?2, ?3)",
            [room_id, event_id, &now.to_string()])
            .map_err(|e| ChannelError::Internal(format!("Failed to record dedupe entry: {e}")))?;
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
        let deduper = EventDeduper::new(path).unwrap();
        assert!(deduper.try_insert("!r", "$e1").unwrap());
        drop(deduper);
        assert!(!EventDeduper::new(path)
            .unwrap()
            .try_insert("!r", "$e1")
            .unwrap());
    }

    #[test]
    fn bounded_cache_falls_back_to_database_and_prunes_at_runtime() {
        let deduper = EventDeduper::new(":memory:").unwrap();
        for i in 0..=CACHE_LIMIT {
            assert!(deduper.try_insert("!r", &format!("${i}")).unwrap());
        }
        assert!(deduper.state.lock().unwrap().cache.len() <= CACHE_LIMIT);
        assert!(!deduper.try_insert("!r", "$0").unwrap());
        {
            let mut state = deduper.state.lock().unwrap();
            state
                .conn
                .execute("UPDATE matrix_seen_events SET seen_at = 0", [])
                .unwrap();
            state.last_prune = 0;
        }
        assert!(deduper.try_insert("!r", "$0").unwrap());
        let state = deduper.state.lock().unwrap();
        assert_eq!(
            state
                .conn
                .query_row("SELECT COUNT(*) FROM matrix_seen_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
