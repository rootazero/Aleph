//! Users table — the principal registry for the one-server-one-org model.
//! See docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md §4.

use rusqlite::{params, OptionalExtension, Result as SqliteResult};

use super::{current_timestamp_ms, SecurityStore};

/// The implicit owner minted on first boot; adopts all pre-existing
/// single-user data so the single-machine experience is byte-identical.
pub const OWNER_USER_ID: &str = "u-owner";

pub(crate) const USERS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    user_id      TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'member',
    status       TEXT NOT NULL DEFAULT 'active',
    created_at   INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole {
    Admin,
    Member,
}

impl UserRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Active,
    Deactivated,
}

impl UserStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deactivated => "deactivated",
        }
    }
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "deactivated" => Some(Self::Deactivated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub user_id: String,
    pub display_name: String,
    pub role: UserRole,
    pub status: UserStatus,
    pub created_at: i64,
}

impl SecurityStore {
    pub fn create_user(
        &self,
        user_id: &str,
        display_name: &str,
        role: UserRole,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO users (user_id, display_name, role, status, created_at)
             VALUES (?1, ?2, ?3, 'active', ?4)",
            params![user_id, display_name, role.as_str(), current_timestamp_ms()],
        )?;
        crate::scope::directory::record(user_id, display_name);
        Ok(())
    }

    pub fn get_user(&self, user_id: &str) -> SqliteResult<Option<UserRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT user_id, display_name, role, status, created_at FROM users WHERE user_id = ?1",
            params![user_id],
            row_to_user,
        )
        .optional()
    }

    pub fn list_users(&self) -> SqliteResult<Vec<UserRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT user_id, display_name, role, status, created_at FROM users ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_user)?;
        rows.collect()
    }

    pub fn count_users(&self) -> SqliteResult<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
    }

    /// Partial update; `None` fields are left unchanged.
    pub fn update_user(
        &self,
        user_id: &str,
        display_name: Option<&str>,
        role: Option<UserRole>,
        status: Option<UserStatus>,
    ) -> SqliteResult<usize> {
        let rows = {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute(
                "UPDATE users SET
               display_name = COALESCE(?2, display_name),
               role         = COALESCE(?3, role),
               status       = COALESCE(?4, status)
             WHERE user_id = ?1",
                params![
                    user_id,
                    display_name,
                    role.map(UserRole::as_str),
                    status.map(UserStatus::as_str)
                ],
            )?
        };
        // Only on a real rename: `COALESCE` above leaves the column alone when
        // the caller passed `None`, and mirroring that here keeps the cache from
        // inventing a name a status-only update never touched.
        if let Some(name) = display_name {
            crate::scope::directory::record(user_id, name);
        }
        Ok(rows)
    }

    /// Idempotent first-boot bootstrap: if no users exist, mint the implicit
    /// owner (admin) and adopt every un-owned panel device. Cluster node rows
    /// (shared `devices` table, mine #3 in gateway/CLAUDE.md) are machines,
    /// not people — never adopted.
    pub fn ensure_bootstrap_owner(&self) -> SqliteResult<()> {
        if self.count_users()? == 0 {
            self.create_user(OWNER_USER_ID, "Owner", UserRole::Admin)?;
        }
        // Seed the render-time name cache. The per-write hooks above only see
        // writes made by THIS process, so without this a restarted server would
        // label every room message with a bare `u-…` id until someone happened
        // to be renamed. Runs on the boot path that is already guaranteed once.
        crate::scope::directory::hydrate(
            self.list_users()?
                .into_iter()
                .map(|u| (u.user_id, u.display_name)),
        );
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET user_id = ?1
             WHERE user_id IS NULL AND device_type = 'panel'",
            params![OWNER_USER_ID],
        )?;
        Ok(())
    }

    /// The linked user of a device row, if any.
    pub fn device_user(&self, device_id: &str) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT user_id FROM devices WHERE device_id = ?1",
            params![device_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map(Option::flatten)
    }

    /// Bind `user_id` to a device **only if it has no binding yet**.
    ///
    /// Used by the bootstrap-ticket exchange after `upsert_device` runs: an
    /// unbound ticket's first-time pairing (brand-new, still-NULL row)
    /// defaults to the owner, but an unbound re-pair of an already-owned
    /// device (owner preserved by `upsert_device`'s
    /// `COALESCE(excluded.user_id, devices.user_id)`) must never be silently
    /// reassigned — see `DeviceTokenManager::exchange_bootstrap_ticket`.
    pub fn set_device_user_if_unbound(&self, device_id: &str, user_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET user_id = ?1 WHERE device_id = ?2 AND user_id IS NULL",
            params![user_id, device_id],
        )?;
        Ok(())
    }

    /// Live (un-revoked) device ids bound to `user_id`.
    ///
    /// `devices` is the shared panel/node namespace (`src/gateway/CLAUDE.md`
    /// mine 3) — a cluster node's row is backfilled with `device_type = NULL`
    /// by `admit_node`, never a `user_id`, but the two call sites of this
    /// method (the live role re-stamp and the deactivation revoke-all) must
    /// not depend on that invariant holding forever. `PANEL_DEVICE_TYPE` is
    /// the sole predicate for "is this a panel device" anywhere in this
    /// codebase; this is that predicate applied here too.
    pub fn list_device_ids_for_user(&self, user_id: &str) -> SqliteResult<Vec<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id FROM devices \
             WHERE user_id = ?1 AND revoked_at IS NULL AND device_type = ?2",
        )?;
        let rows = stmt.query_map(
            params![user_id, crate::gateway::security::PANEL_DEVICE_TYPE],
            |r| r.get(0),
        )?;
        rows.collect()
    }

    #[cfg(test)]
    pub fn clear_device_user(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET user_id = NULL WHERE device_id = ?1",
            params![device_id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn set_device_user(&self, device_id: &str, user_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET user_id = ?1 WHERE device_id = ?2",
            params![user_id, device_id],
        )?;
        Ok(())
    }
}

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    let role_s: String = row.get(2)?;
    let status_s: String = row.get(3)?;
    Ok(UserRecord {
        user_id: row.get(0)?,
        display_name: row.get(1)?,
        // Fail-soft to Member on unknown text: a downgraded binary must never
        // promote an unknown role to admin (fail-closed on privilege).
        role: UserRole::from_str(&role_s).unwrap_or(UserRole::Member),
        status: UserStatus::from_str(&status_s).unwrap_or(UserStatus::Deactivated),
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::types::DeviceUpsertData;
    use crate::gateway::security::store::SecurityStore;

    /// Minimal `DeviceUpsertData` fixture, parameterized on `device_type` and
    /// `role` per the pre-existing/cluster-node distinction under test.
    fn device_fixture<'a>(
        device_id: &'a str,
        device_type: Option<&'a str>,
        role: &'a str,
    ) -> DeviceUpsertData<'a> {
        DeviceUpsertData {
            device_id,
            device_name: "Test Device",
            device_type,
            public_key: &[1u8; 32],
            fingerprint: device_id,
            role,
            scopes: &[],
            user_id: None,
        }
    }

    #[test]
    fn fresh_store_bootstraps_owner_admin() {
        let store = SecurityStore::in_memory().unwrap();
        // migrate() runs in in_memory(); ensure_bootstrap_owner is called at its end.
        let owner = store
            .get_user(OWNER_USER_ID)
            .unwrap()
            .expect("owner exists");
        assert_eq!(owner.role, UserRole::Admin);
        assert_eq!(owner.status, UserStatus::Active);
        assert_eq!(store.count_users().unwrap(), 1);
    }

    #[test]
    fn bootstrap_adopts_panel_devices_but_not_nodes() {
        let store = SecurityStore::in_memory().unwrap();
        // Panel device with no user (simulates pre-v14 row).
        store
            .upsert_device(&device_fixture("dev-panel", Some("panel"), "operator"))
            .unwrap();
        store.clear_device_user("dev-panel").unwrap(); // test helper, see Step 3
                                                       // Cluster node row: role='node', device_type NULL — must stay untouched.
        store
            .upsert_device(&device_fixture("node-1", None, "node"))
            .unwrap();
        store.clear_device_user("node-1").unwrap();

        store.ensure_bootstrap_owner().unwrap();

        assert_eq!(
            store.device_user("dev-panel").unwrap().as_deref(),
            Some(OWNER_USER_ID)
        );
        assert_eq!(store.device_user("node-1").unwrap(), None);
    }

    #[test]
    fn ensure_bootstrap_owner_is_idempotent_and_respects_existing_users() {
        let store = SecurityStore::in_memory().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        assert_eq!(store.count_users().unwrap(), 1);
        // Once a second user exists, re-running must not create anything.
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store.ensure_bootstrap_owner().unwrap();
        assert_eq!(store.count_users().unwrap(), 2);
    }

    #[test]
    fn update_user_changes_role_and_status() {
        let store = SecurityStore::in_memory().unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        store
            .update_user(
                "u-bob",
                None,
                Some(UserRole::Admin),
                Some(UserStatus::Deactivated),
            )
            .unwrap();
        let bob = store.get_user("u-bob").unwrap().unwrap();
        assert_eq!(bob.role, UserRole::Admin);
        assert_eq!(bob.status, UserStatus::Deactivated);
    }
}
