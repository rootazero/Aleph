//! Presence Tracking for Connected Clients
//!
//! Tracks connected clients for multi-device awareness. Each connection
//! is represented by a `PresenceEntry` containing device metadata and
//! heartbeat timestamps. The `PresenceTracker` provides concurrent
//! read/write access via `DashMap`.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;
use std::sync::Arc;

/// A single connected client's presence information.
#[derive(Debug, Clone, Serialize)]
pub struct PresenceEntry {
    /// Unique connection identifier
    pub conn_id: String,
    /// Device identifier (from pairing), if available
    pub device_id: Option<String>,
    /// Human-readable device name
    pub device_name: String,
    /// Platform string (e.g. "macos", "ios", "web", "cli")
    pub platform: String,
    /// When the connection was established
    pub connected_at: DateTime<Utc>,
    /// Last heartbeat received from this connection
    pub last_heartbeat: DateTime<Utc>,
}

/// Concurrent presence tracker for all active Gateway connections.
///
/// Uses `DashMap` for lock-free concurrent access across async tasks.
#[derive(Clone)]
pub struct PresenceTracker {
    entries: Arc<DashMap<String, PresenceEntry>>,
}

impl PresenceTracker {
    /// Create a new empty presence tracker.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Insert or update a presence entry keyed by connection ID.
    pub fn upsert(&self, conn_id: String, entry: PresenceEntry) {
        self.entries.insert(conn_id, entry);
    }

    /// Remove a presence entry by connection ID, returning it if it existed.
    pub fn remove(&self, conn_id: &str) -> Option<PresenceEntry> {
        self.entries.remove(conn_id).map(|(_, entry)| entry)
    }

    /// Get a clone of the presence entry for a given connection ID.
    pub fn get(&self, conn_id: &str) -> Option<PresenceEntry> {
        self.entries.get(conn_id).map(|entry| entry.clone())
    }

    /// List all current presence entries.
    pub fn list(&self) -> Vec<PresenceEntry> {
        self.entries
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Update the last heartbeat timestamp for a connection.
    ///
    /// Returns `true` if the connection was found and updated, `false` otherwise.
    pub fn update_heartbeat(&self, conn_id: &str) -> bool {
        if let Some(mut entry) = self.entries.get_mut(conn_id) {
            entry.last_heartbeat = Utc::now();
            true
        } else {
            false
        }
    }

    /// Return the number of currently tracked connections.
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for PresenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(conn_id: &str, device_name: &str, platform: &str) -> PresenceEntry {
        let now = Utc::now();
        PresenceEntry {
            conn_id: conn_id.to_string(),
            device_id: None,
            device_name: device_name.to_string(),
            platform: platform.to_string(),
            connected_at: now,
            last_heartbeat: now,
        }
    }

    #[test]
    fn test_add_and_get_presence() {
        let tracker = PresenceTracker::new();
        let entry = make_entry("conn-1", "MacBook Pro", "macos");

        tracker.upsert("conn-1".to_string(), entry);

        let retrieved = tracker.get("conn-1").expect("entry should exist");
        assert_eq!(retrieved.conn_id, "conn-1");
        assert_eq!(retrieved.device_name, "MacBook Pro");
        assert_eq!(retrieved.platform, "macos");
        assert!(tracker.get("nonexistent").is_none());
        assert_eq!(tracker.count(), 1);
    }

    #[test]
    fn test_remove_presence() {
        let tracker = PresenceTracker::new();
        tracker.upsert("conn-1".to_string(), make_entry("conn-1", "iPhone", "ios"));

        assert_eq!(tracker.count(), 1);

        let removed = tracker.remove("conn-1").expect("should return removed entry");
        assert_eq!(removed.device_name, "iPhone");
        assert_eq!(tracker.count(), 0);

        // Removing again returns None
        assert!(tracker.remove("conn-1").is_none());
    }

    #[test]
    fn test_list_all_presence() {
        let tracker = PresenceTracker::new();
        tracker.upsert("conn-1".to_string(), make_entry("conn-1", "MacBook", "macos"));
        tracker.upsert("conn-2".to_string(), make_entry("conn-2", "iPhone", "ios"));
        tracker.upsert("conn-3".to_string(), make_entry("conn-3", "CLI", "cli"));

        let all = tracker.list();
        assert_eq!(all.len(), 3);

        // Verify all entries are present (order is not guaranteed with DashMap)
        let names: Vec<String> = all.iter().map(|e| e.device_name.clone()).collect();
        assert!(names.contains(&"MacBook".to_string()));
        assert!(names.contains(&"iPhone".to_string()));
        assert!(names.contains(&"CLI".to_string()));
    }

    #[test]
    fn test_update_heartbeat() {
        let tracker = PresenceTracker::new();
        let entry = make_entry("conn-1", "MacBook", "macos");
        let original_heartbeat = entry.last_heartbeat;

        tracker.upsert("conn-1".to_string(), entry);

        // Small sleep to ensure timestamp differs
        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(tracker.update_heartbeat("conn-1"));
        assert!(!tracker.update_heartbeat("nonexistent"));

        let updated = tracker.get("conn-1").expect("entry should exist");
        assert!(updated.last_heartbeat > original_heartbeat);
    }
}
