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

