//! Device Storage
//!
//! Persistent storage for approved devices using SQLite.

use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};

/// An approved device that can connect to the Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedDevice {
    /// Unique device identifier
    pub device_id: String,
    /// Human-readable device name
    pub device_name: String,
    /// Device type (macos, ios, android, cli, web)
    pub device_type: Option<String>,
    /// When the device was approved (ISO 8601)
    pub approved_at: String,
    /// Last time the device connected (ISO 8601)
    pub last_seen_at: Option<String>,
    /// Permissions granted to this device
    pub permissions: Vec<String>,
}

impl ApprovedDevice {
    /// Create a new approved device
    pub fn new(device_id: String, device_name: String, device_type: Option<String>) -> Self {
        Self {
            device_id,
            device_name,
            device_type,
            approved_at: chrono::Utc::now().to_rfc3339(),
            last_seen_at: None,
            permissions: vec!["*".to_string()], // Full access by default
        }
    }
}

/// Persistent storage for approved devices
pub struct DeviceStore {
    conn: Mutex<Connection>,
}

impl DeviceStore {
    /// Open or create a device store at the specified path
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory device store (for testing)
    pub fn in_memory() -> SqliteResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialize the database schema
    fn init_schema(&self) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS approved_devices (
                device_id TEXT PRIMARY KEY,
                device_name TEXT NOT NULL,
                device_type TEXT,
                approved_at TEXT NOT NULL,
                last_seen_at TEXT,
                permissions TEXT NOT NULL
            )",
            [],
        )?;

        // Create index for faster lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_devices_approved_at ON approved_devices(approved_at)",
            [],
        )?;

        debug!("Device store schema initialized");
        Ok(())
    }

    /// Approve a new device
    pub fn approve_device(&self, device: &ApprovedDevice) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let permissions_json = serde_json::to_string(&device.permissions).unwrap_or_default();

        conn.execute(
            "INSERT OR REPLACE INTO approved_devices
             (device_id, device_name, device_type, approved_at, last_seen_at, permissions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                device.device_id,
                device.device_name,
                device.device_type,
                device.approved_at,
                device.last_seen_at,
                permissions_json,
            ],
        )?;

        info!(
            device_id = %device.device_id,
            device_name = %device.device_name,
            "Device approved"
        );
        Ok(())
    }

    /// Check if a device is approved
    pub fn is_approved(&self, device_id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM approved_devices WHERE device_id = ?1",
                params![device_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        count > 0
    }

    /// Get an approved device by ID
    pub fn get_device(&self, device_id: &str) -> Option<ApprovedDevice> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT device_id, device_name, device_type, approved_at, last_seen_at, permissions
             FROM approved_devices WHERE device_id = ?1",
            params![device_id],
            |row| {
                let permissions_json: String = row.get(5)?;
                let permissions: Vec<String> =
                    serde_json::from_str(&permissions_json).unwrap_or_default();

                Ok(ApprovedDevice {
                    device_id: row.get(0)?,
                    device_name: row.get(1)?,
                    device_type: row.get(2)?,
                    approved_at: row.get(3)?,
                    last_seen_at: row.get(4)?,
                    permissions,
                })
            },
        )
        .ok()
    }

    /// List all approved devices
    pub fn list_devices(&self) -> Vec<ApprovedDevice> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT device_id, device_name, device_type, approved_at, last_seen_at, permissions
             FROM approved_devices ORDER BY approved_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let devices = stmt
            .query_map([], |row| {
                let permissions_json: String = row.get(5)?;
                let permissions: Vec<String> =
                    serde_json::from_str(&permissions_json).unwrap_or_default();

                Ok(ApprovedDevice {
                    device_id: row.get(0)?,
                    device_name: row.get(1)?,
                    device_type: row.get(2)?,
                    approved_at: row.get(3)?,
                    last_seen_at: row.get(4)?,
                    permissions,
                })
            })
            .ok()
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        devices
    }

    /// Update the last_seen_at timestamp for a device
    pub fn update_last_seen(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE approved_devices SET last_seen_at = ?1 WHERE device_id = ?2",
            params![now, device_id],
        )?;
        Ok(())
    }

    /// Revoke a device's approval
    pub fn revoke_device(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM approved_devices WHERE device_id = ?1",
            params![device_id],
        )?;
        if rows > 0 {
            info!(device_id = %device_id, "Device revoked");
        }
        Ok(rows > 0)
    }

    /// Get the count of approved devices
    pub fn device_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM approved_devices", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    /// Update device permissions
    pub fn update_permissions(&self, device_id: &str, permissions: &[String]) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let permissions_json = serde_json::to_string(permissions).unwrap_or_default();
        let rows = conn.execute(
            "UPDATE approved_devices SET permissions = ?1 WHERE device_id = ?2",
            params![permissions_json, device_id],
        )?;
        Ok(rows > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approve_and_get_device() {
        let store = DeviceStore::in_memory().unwrap();
        let device = ApprovedDevice::new(
            "test-device-1".to_string(),
            "Test MacBook".to_string(),
            Some("macos".to_string()),
        );

        store.approve_device(&device).unwrap();

        assert!(store.is_approved("test-device-1"));
        assert!(!store.is_approved("unknown-device"));

        let retrieved = store.get_device("test-device-1").unwrap();
        assert_eq!(retrieved.device_name, "Test MacBook");
        assert_eq!(retrieved.device_type, Some("macos".to_string()));
    }

    #[test]
    fn test_list_devices() {
        let store = DeviceStore::in_memory().unwrap();

        store
            .approve_device(&ApprovedDevice::new(
                "device-1".to_string(),
                "Device 1".to_string(),
                None,
            ))
            .unwrap();

        store
            .approve_device(&ApprovedDevice::new(
                "device-2".to_string(),
                "Device 2".to_string(),
                None,
            ))
            .unwrap();

        let devices = store.list_devices();
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn test_revoke_device() {
        let store = DeviceStore::in_memory().unwrap();
        let device = ApprovedDevice::new("to-revoke".to_string(), "Revokable".to_string(), None);

        store.approve_device(&device).unwrap();
        assert!(store.is_approved("to-revoke"));

        let revoked = store.revoke_device("to-revoke").unwrap();
        assert!(revoked);
        assert!(!store.is_approved("to-revoke"));
    }

    #[test]
    fn test_update_last_seen() {
        let store = DeviceStore::in_memory().unwrap();
        let device = ApprovedDevice::new("seen-device".to_string(), "Seen".to_string(), None);

        store.approve_device(&device).unwrap();

        // Initially no last_seen
        let retrieved = store.get_device("seen-device").unwrap();
        assert!(retrieved.last_seen_at.is_none());

        // Update last seen
        store.update_last_seen("seen-device").unwrap();

        let updated = store.get_device("seen-device").unwrap();
        assert!(updated.last_seen_at.is_some());
    }

    #[test]
    fn test_update_permissions() {
        let store = DeviceStore::in_memory().unwrap();
        let device = ApprovedDevice::new("perm-device".to_string(), "Perms".to_string(), None);

        store.approve_device(&device).unwrap();

        // Update permissions
        store
            .update_permissions("perm-device", &["read".to_string(), "write".to_string()])
            .unwrap();

        let updated = store.get_device("perm-device").unwrap();
        assert_eq!(updated.permissions, vec!["read", "write"]);
    }
}
