use rusqlite::{params, Result as SqliteResult};
use super::{SecurityStore, current_timestamp_ms};
use super::types::*;

impl SecurityStore {
    /// Insert a pairing request
    pub fn insert_pairing_request(
        &self,
        data: &PairingRequestData<'_>,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        conn.execute(
            "INSERT INTO pairing_requests
             (request_id, code, pairing_type, device_name, device_type, public_key, channel, sender_id, remote_addr, metadata, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(request_id) DO UPDATE SET
               code = excluded.code,
               expires_at = excluded.expires_at",
            params![
                data.request_id,
                data.code,
                data.pairing_type,
                data.device_name,
                data.device_type,
                data.public_key,
                data.channel,
                data.sender_id,
                data.remote_addr,
                data.metadata,
                now,
                data.expires_at,
            ],
        )?;
        Ok(())
    }

    /// Get pairing request by code
    pub fn get_pairing_request(
        &self,
        code: &str,
    ) -> SqliteResult<Option<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key, channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests WHERE code = ?1 AND expires_at > ?2",
        )?;

        let result = stmt.query_row(params![code, current_timestamp_ms()], PairingRequestRow::from_row);
        match result {
            Ok(req) => Ok(Some(req)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete a pairing request
    pub fn delete_pairing_request(
        &self,
        code: &str,
    ) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "DELETE FROM pairing_requests WHERE code = ?1",
            params![code],
        )?;
        Ok(rows > 0)
    }

    /// List pending pairing requests
    pub fn list_pairing_requests(
        &self,
    ) -> SqliteResult<Vec<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key, channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests WHERE expires_at > ?1 ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map(params![current_timestamp_ms()], PairingRequestRow::from_row)?;
        rows.collect()
    }

    /// Count pending pairing requests
    pub fn count_pairing_requests(
        &self,
    ) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM pairing_requests WHERE expires_at > ?1",
        )?;
        let count: i64 = stmt.query_row(params![current_timestamp_ms()], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Delete expired pairing requests
    pub fn delete_expired_pairing_requests(
        &self,
    ) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "DELETE FROM pairing_requests WHERE expires_at <= ?1",
            params![current_timestamp_ms()],
        )?;
        Ok(rows as u64)
    }
}
