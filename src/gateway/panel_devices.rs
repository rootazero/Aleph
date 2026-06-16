//! Panel device tier store (G2/G3).
//!
//! Remote Panels connect over WebSocket and, unlike the local (loopback)
//! desktop App, are NOT implicitly trusted as operators. Each remote Panel
//! identifies itself with a stable `device_id` (a UUID the Panel generates once
//! and keeps in `localStorage`) and is persisted here with a Chat/Config tier:
//!
//! - **Chat** (default): converse + read. Config-mutating tools and RPC methods
//!   are refused (`method_authz::{tool,rpc}_requires_operator`).
//! - **Config**: the above plus Aleph's own configuration surface — granted
//!   explicitly by an operator (`devices.set_level`), either after the fact or
//!   the moment a new device pairs.
//!
//! Loopback connections never touch this store — they are always operator
//! (single-machine zero-config). The store is reached as a process-global
//! ([`global_store`]) by the WS connect handshake and cloned into the
//! `devices.*` RPC handlers, mirroring `SecurityStore`'s shared-handle pattern.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::gateway::inbound_router::ChannelPermissionLevel;

/// Error type for panel-device storage operations.
#[derive(Debug, thiserror::Error)]
pub enum PanelDeviceError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
}

/// A persisted remote Panel device and its authorization tier.
#[derive(Debug, Clone, Serialize)]
pub struct PanelDevice {
    /// Stable Panel-generated identifier (UUID kept in `localStorage`).
    pub device_id: String,
    /// Human label the Panel reports (e.g. "Chrome on MacBook"). May be empty.
    pub device_name: String,
    /// Authorization tier. Serializes as `"chat"` / `"config"`.
    pub tier: ChannelPermissionLevel,
    /// First time this device was seen (RFC 3339).
    pub first_seen_at: String,
    /// Most recent handshake (RFC 3339).
    pub last_seen_at: String,
    /// When Config tier was last granted, if ever (RFC 3339).
    pub granted_at: Option<String>,
    /// Who granted Config (operator device id / label), if recorded.
    pub granted_by: Option<String>,
}

/// `"chat"` / `"config"` label for a tier — the wire form used by the store
/// and the `devices.*` RPC, distinct from the connect-role string
/// (`"guest"` / `"operator"`) the gates key on.
#[must_use]
pub const fn tier_label(tier: ChannelPermissionLevel) -> &'static str {
    match tier {
        ChannelPermissionLevel::Config => "config",
        ChannelPermissionLevel::Chat => "chat",
    }
}

/// Parse a `"chat"` / `"config"` label (accepts the `"guest"` / `"operator"`
/// role aliases too). Unknown values fall back to the safe Chat default.
#[must_use]
pub fn tier_from_label(label: &str) -> ChannelPermissionLevel {
    match label {
        "config" | "operator" => ChannelPermissionLevel::Config,
        _ => ChannelPermissionLevel::Chat,
    }
}

/// Storage for remote Panel devices and their tiers.
#[async_trait]
pub trait PanelDeviceStore: Send + Sync {
    /// Record a handshake from `device_id`, inserting it at the Chat default if
    /// unknown. Returns `(tier, is_new)` — `is_new` is true only on first sight.
    async fn record_seen(
        &self,
        device_id: &str,
        device_name: &str,
    ) -> Result<(ChannelPermissionLevel, bool), PanelDeviceError>;

    /// Set a device's tier (operator action). Upserts so an operator may
    /// pre-authorize a device id before it first connects.
    async fn set_tier(
        &self,
        device_id: &str,
        tier: ChannelPermissionLevel,
        granted_by: Option<&str>,
    ) -> Result<(), PanelDeviceError>;

    /// All known devices, most-recently-seen first.
    async fn list(&self) -> Result<Vec<PanelDevice>, PanelDeviceError>;

    /// Forget a device. It re-registers at the Chat default on its next
    /// handshake, so this both revokes Config and clears stale entries.
    async fn remove(&self, device_id: &str) -> Result<(), PanelDeviceError>;
}

/// SQLite-backed [`PanelDeviceStore`], mirroring `SqlitePairingStore`.
pub struct SqlitePanelDeviceStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqlitePanelDeviceStore {
    /// Open (or create) the store at `db_path`.
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self, PanelDeviceError> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(db_path.as_ref())?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory store (tests / probe wiring).
    pub fn in_memory() -> Result<Self, PanelDeviceError> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_schema(conn: &Connection) -> Result<(), PanelDeviceError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS panel_devices (
                device_id TEXT PRIMARY KEY,
                device_name TEXT NOT NULL DEFAULT '',
                tier TEXT NOT NULL DEFAULT 'chat',
                first_seen_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                granted_at TEXT,
                granted_by TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_panel_devices_last_seen
                ON panel_devices(last_seen_at);
            "#,
        )?;
        Ok(())
    }
}

#[async_trait]
impl PanelDeviceStore for SqlitePanelDeviceStore {
    async fn record_seen(
        &self,
        device_id: &str,
        device_name: &str,
    ) -> Result<(ChannelPermissionLevel, bool), PanelDeviceError> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();

        let existing: Option<String> = conn
            .query_row(
                "SELECT tier FROM panel_devices WHERE device_id = ?1",
                params![device_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(tier_label_str) = existing {
            // Refresh last_seen; keep a non-empty name current.
            if device_name.is_empty() {
                conn.execute(
                    "UPDATE panel_devices SET last_seen_at = ?2 WHERE device_id = ?1",
                    params![device_id, now],
                )?;
            } else {
                conn.execute(
                    "UPDATE panel_devices SET last_seen_at = ?2, device_name = ?3 WHERE device_id = ?1",
                    params![device_id, now, device_name],
                )?;
            }
            return Ok((tier_from_label(&tier_label_str), false));
        }

        conn.execute(
            "INSERT INTO panel_devices \
             (device_id, device_name, tier, first_seen_at, last_seen_at) \
             VALUES (?1, ?2, 'chat', ?3, ?3)",
            params![device_id, device_name, now],
        )?;
        info!("New remote Panel device paired at chat tier: {device_id}");
        Ok((ChannelPermissionLevel::Chat, true))
    }

    async fn set_tier(
        &self,
        device_id: &str,
        tier: ChannelPermissionLevel,
        granted_by: Option<&str>,
    ) -> Result<(), PanelDeviceError> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        let label = tier_label(tier);
        // Config grants stamp granted_at/by; dropping to chat clears them.
        let (granted_at, granted_by) = match tier {
            ChannelPermissionLevel::Config => (Some(now.clone()), granted_by.map(str::to_string)),
            ChannelPermissionLevel::Chat => (None, None),
        };
        conn.execute(
            "INSERT INTO panel_devices \
             (device_id, device_name, tier, first_seen_at, last_seen_at, granted_at, granted_by) \
             VALUES (?1, '', ?2, ?3, ?3, ?4, ?5) \
             ON CONFLICT(device_id) DO UPDATE SET \
             tier = ?2, granted_at = ?4, granted_by = ?5",
            params![device_id, label, now, granted_at, granted_by],
        )?;
        debug!("Panel device {device_id} tier set to {label}");
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PanelDevice>, PanelDeviceError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, tier, first_seen_at, last_seen_at, granted_at, granted_by \
             FROM panel_devices ORDER BY last_seen_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let tier_label_str: String = row.get(2)?;
            Ok(PanelDevice {
                device_id: row.get(0)?,
                device_name: row.get(1)?,
                tier: tier_from_label(&tier_label_str),
                first_seen_at: row.get(3)?,
                last_seen_at: row.get(4)?,
                granted_at: row.get(5)?,
                granted_by: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    async fn remove(&self, device_id: &str) -> Result<(), PanelDeviceError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM panel_devices WHERE device_id = ?1",
            params![device_id],
        )?;
        Ok(())
    }
}

/// Process-global panel-device store, installed once at boot. The WS connect
/// handshake reads it via [`resolve_tier`]; absent (tests / probe) it degrades
/// to the in-memory-free path where every remote falls to the Chat default.
static GLOBAL_STORE: OnceLock<Arc<dyn PanelDeviceStore>> = OnceLock::new();

/// Install the process-global store. Idempotent — a second call is ignored.
pub fn set_global_store(store: Arc<dyn PanelDeviceStore>) {
    let _ = GLOBAL_STORE.set(store);
}

/// The process-global store, or `None` before boot wiring runs.
#[must_use]
pub fn global_store() -> Option<Arc<dyn PanelDeviceStore>> {
    GLOBAL_STORE.get().cloned()
}

/// Resolve a connection's tier at the `connect` handshake.
///
/// - Loopback ⇒ `Config` (operator): the local desktop App stays zero-config.
/// - Remote with a known/registered `device_id` ⇒ its persisted tier
///   (default `Chat`); the boolean is `true` only when the device was just
///   seen for the first time (drives the pairing-time grant prompt).
/// - Remote without a `device_id`, or before the store is wired ⇒ `Chat`.
pub async fn resolve_tier(
    is_loopback: bool,
    device_id: Option<&str>,
    device_name: &str,
) -> (ChannelPermissionLevel, bool) {
    if is_loopback {
        return (ChannelPermissionLevel::Config, false);
    }
    match (device_id, global_store()) {
        (Some(id), Some(store)) if !id.is_empty() => store
            .record_seen(id, device_name)
            .await
            .unwrap_or((ChannelPermissionLevel::Chat, false)),
        _ => (ChannelPermissionLevel::Chat, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_remote_device_defaults_to_chat_and_is_new() {
        let store = SqlitePanelDeviceStore::in_memory().unwrap();
        let (tier, is_new) = store.record_seen("dev-1", "Panel A").await.unwrap();
        assert_eq!(tier, ChannelPermissionLevel::Chat);
        assert!(is_new);
        // Second sight is not new and stays chat.
        let (tier, is_new) = store.record_seen("dev-1", "Panel A").await.unwrap();
        assert_eq!(tier, ChannelPermissionLevel::Chat);
        assert!(!is_new);
    }

    #[tokio::test]
    async fn set_tier_grants_and_revokes_config() {
        let store = SqlitePanelDeviceStore::in_memory().unwrap();
        store.record_seen("dev-2", "Panel B").await.unwrap();
        store
            .set_tier("dev-2", ChannelPermissionLevel::Config, Some("operator-x"))
            .await
            .unwrap();
        let (tier, _) = store.record_seen("dev-2", "Panel B").await.unwrap();
        assert_eq!(tier, ChannelPermissionLevel::Config);

        let listed = store.list().await.unwrap();
        let dev = listed.iter().find(|d| d.device_id == "dev-2").unwrap();
        assert_eq!(dev.tier, ChannelPermissionLevel::Config);
        assert_eq!(dev.granted_by.as_deref(), Some("operator-x"));

        store
            .set_tier("dev-2", ChannelPermissionLevel::Chat, None)
            .await
            .unwrap();
        let (tier, _) = store.record_seen("dev-2", "Panel B").await.unwrap();
        assert_eq!(tier, ChannelPermissionLevel::Chat);
    }

    #[tokio::test]
    async fn remove_forgets_device() {
        let store = SqlitePanelDeviceStore::in_memory().unwrap();
        store.record_seen("dev-3", "").await.unwrap();
        store.remove("dev-3").await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
        // Re-seen ⇒ chat + new again.
        let (tier, is_new) = store.record_seen("dev-3", "").await.unwrap();
        assert_eq!(tier, ChannelPermissionLevel::Chat);
        assert!(is_new);
    }

    #[tokio::test]
    async fn loopback_resolves_operator_without_store() {
        let (tier, is_new) = resolve_tier(true, None, "").await;
        assert_eq!(tier, ChannelPermissionLevel::Config);
        assert!(!is_new);
    }

    #[tokio::test]
    async fn remote_without_store_falls_to_chat() {
        let (tier, _) = resolve_tier(false, Some("dev-x"), "Panel").await;
        assert_eq!(tier, ChannelPermissionLevel::Chat);
    }
}
