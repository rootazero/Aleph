/// Device upsert data
pub struct DeviceUpsertData<'a> {
    pub device_id: &'a str,
    pub device_name: &'a str,
    pub device_type: Option<&'a str>,
    pub public_key: &'a [u8],
    pub fingerprint: &'a str,
    pub role: &'a str,
    pub scopes: &'a [String],
}

/// Device row from database
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub device_id: String,
    pub device_name: String,
    pub device_type: Option<String>,
    pub public_key: Vec<u8>,
    pub fingerprint: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl DeviceRow {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let scopes_json: String = row.get(6)?;
        let scopes = serde_json::from_str(&scopes_json).unwrap_or_default();

        Ok(Self {
            device_id: row.get(0)?,
            device_name: row.get(1)?,
            device_type: row.get(2)?,
            public_key: row.get(3)?,
            fingerprint: row.get(4)?,
            role: row.get(5)?,
            scopes,
            created_at: row.get(7)?,
            approved_at: row.get(8)?,
            last_seen_at: row.get(9)?,
            revoked_at: row.get(10)?,
        })
    }
}

/// Token row from database
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub token_id: String,
    pub device_id: String,
    pub token_hash: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    pub rotated_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl TokenRow {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let scopes_json: String = row.get(4)?;
        let scopes = serde_json::from_str(&scopes_json).unwrap_or_default();

        Ok(Self {
            token_id: row.get(0)?,
            device_id: row.get(1)?,
            token_hash: row.get(2)?,
            role: row.get(3)?,
            scopes,
            issued_at: row.get(5)?,
            expires_at: row.get(6)?,
            last_used_at: row.get(7)?,
            rotated_at: row.get(8)?,
            revoked_at: row.get(9)?,
        })
    }
}

/// Pairing request data
pub struct PairingRequestData<'a> {
    pub request_id: &'a str,
    pub code: &'a str,
    pub pairing_type: &'a str,
    pub device_name: Option<&'a str>,
    pub device_type: Option<&'a str>,
    pub public_key: Option<&'a [u8]>,
    pub channel: Option<&'a str>,
    pub sender_id: Option<&'a str>,
    pub remote_addr: Option<&'a str>,
    pub metadata: Option<&'a str>,
    /// Browser variant: display label e.g. "Safari on 192.168.1.5".
    pub origin_label: Option<&'a str>,
    /// Browser variant: full client `User-Agent` header.
    pub user_agent: Option<&'a str>,
    /// Browser variant: peer IP captured server-side.
    pub peer_ip: Option<&'a str>,
    pub expires_at: i64,
}

impl<'a> Default for PairingRequestData<'a> {
    fn default() -> Self {
        Self {
            request_id: "",
            code: "",
            pairing_type: "device",
            device_name: None,
            device_type: None,
            public_key: None,
            channel: None,
            sender_id: None,
            remote_addr: None,
            metadata: None,
            origin_label: None,
            user_agent: None,
            peer_ip: None,
            expires_at: 0,
        }
    }
}

/// Pairing request row from database
#[derive(Debug, Clone)]
pub struct PairingRequestRow {
    pub request_id: String,
    pub code: String,
    pub pairing_type: String,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub public_key: Option<Vec<u8>>,
    pub channel: Option<String>,
    pub sender_id: Option<String>,
    pub remote_addr: Option<String>,
    pub metadata: Option<String>,
    pub origin_label: Option<String>,
    pub user_agent: Option<String>,
    pub peer_ip: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl PairingRequestRow {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            request_id: row.get(0)?,
            code: row.get(1)?,
            pairing_type: row.get(2)?,
            device_name: row.get(3)?,
            device_type: row.get(4)?,
            public_key: row.get(5)?,
            channel: row.get(6)?,
            sender_id: row.get(7)?,
            remote_addr: row.get(8)?,
            metadata: row.get(9)?,
            created_at: row.get(10)?,
            expires_at: row.get(11)?,
            origin_label: row.get(12)?,
            user_agent: row.get(13)?,
            peer_ip: row.get(14)?,
        })
    }

    /// Calculate remaining seconds until expiry
    pub fn remaining_secs(&self) -> u64 {
        let now = super::current_timestamp_ms();
        if self.expires_at > now {
            ((self.expires_at - now) / 1000) as u64
        } else {
            0
        }
    }
}
