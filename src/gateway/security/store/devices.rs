use super::types::{DeviceRow, DeviceUpsertData};
use super::{current_timestamp_ms, SecurityStore};
use rusqlite::{params, Result as SqliteResult};

/// The column list every `devices` read shares, in the order
/// [`DeviceRow::from_row`] indexes them.
///
/// One constant because there are THREE `SELECT`s and ONE `from_row`: adding a
/// column to the projection of one of them makes `row.get(n)` on the others a
/// RUNTIME error, not a compile error, and the two that would break here are
/// `get_device_by_fingerprint` (every remote connect) and `get_device` (node
/// admission). Positional decoding across hand-copied column lists is a
/// contract with no compiler behind it.
const DEVICE_COLUMNS: &str = "device_id, device_name, device_type, public_key, fingerprint, role, \
                              scopes, created_at, approved_at, last_seen_at, revoked_at, user_id";

impl SecurityStore {
    /// Insert or update a device.
    ///
    /// Re-pairing clears `revoked_at`: a fresh pairing arrives only after a valid
    /// one-time bootstrap ticket (itself operator-gated), so re-adopting a
    /// previously-revoked `device_id` is by definition a live device again.
    /// Without clearing it, the ON CONFLICT path left the row `revoked_at`-stamped
    /// while `issue_device_token` minted a working token — the device then vanished
    /// from [`Self::list_devices`] (`WHERE revoked_at IS NULL`) and from
    /// `revoke_all_panel_devices`, leaving an un-listable, un-revocable operator
    /// token that even survived a shared-token rotation.
    pub fn upsert_device(&self, data: &DeviceUpsertData<'_>) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(data.scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize device scopes: {}", e);
            "[]".to_string()
        });

        // `user_id = COALESCE(excluded.user_id, devices.user_id)`: a `None`
        // binding (unbound re-pair) must leave an existing owner untouched —
        // mine-4 sibling of the `device_type`/`revoked_at` invariants below.
        conn.execute(
            r#"INSERT INTO devices
               (device_id, device_name, device_type, public_key, fingerprint, role, scopes, user_id, created_at, approved_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
               ON CONFLICT(device_id) DO UPDATE SET
                 device_name = excluded.device_name,
                 last_seen_at = ?9,
                 revoked_at = NULL,
                 user_id = COALESCE(excluded.user_id, devices.user_id)"#,
            params![data.device_id, data.device_name, data.device_type, data.public_key, data.fingerprint, data.role, scopes_json, data.user_id, now],
        )?;
        Ok(())
    }

    /// Get device by fingerprint
    pub fn get_device_by_fingerprint(&self, fingerprint: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE fingerprint = ?1"
        ))?;

        let result = stmt.query_row(params![fingerprint], DeviceRow::from_row);
        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Fetch a device by id, **including revoked ones**.
    ///
    /// [`Self::is_device_approved`] collapses "revoked" and "never existed" into
    /// the same `false`, and [`Self::list_devices`] hides revoked rows entirely.
    /// Node admission (`cluster::admit_node`) must tell those apart: a revoked
    /// node has to be refused on reconnect, while an unknown one is adopted.
    pub fn get_device(&self, device_id: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE device_id = ?1"
        ))?;
        match stmt.query_row(params![device_id], DeviceRow::from_row) {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if device is approved (not revoked)
    pub fn is_device_approved(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt =
            conn.prepare("SELECT 1 FROM devices WHERE device_id = ?1 AND revoked_at IS NULL")?;
        let exists: Result<i32, _> = stmt.query_row(params![device_id], |row| row.get(0));
        Ok(exists.is_ok())
    }

    /// List all active devices
    pub fn list_devices(&self) -> SqliteResult<Vec<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            &format!(
                "SELECT {DEVICE_COLUMNS} FROM devices WHERE revoked_at IS NULL ORDER BY created_at DESC"
            ),
        )?;

        let rows = stmt.query_map([], DeviceRow::from_row)?;
        rows.collect()
    }

    /// Update device `last_seen_at`
    pub fn touch_device(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET last_seen_at = ?1 WHERE device_id = ?2",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(())
    }

    /// Revoke a device
    pub fn revoke_device(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE devices SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(rows > 0)
    }
}
