//! `users.*` Gateway RPC client.
//!
//! Backs the project-room roster UI (Task 8): resolving `owner_user_id` /
//! `member_ids` into display names, and picking new members from the set of
//! known principals. Mirrors `gateway::handlers::users::UserView` on the wire.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// Mirrors `gateway::handlers::users::UserView`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInfo {
    pub user_id: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
}

pub struct UsersApi;

impl UsersApi {
    /// The caller's own principal record. `Ok(None)` (not an error) when no
    /// caller user is scoped — e.g. a loopback / unrestricted connection with
    /// no P1 identity attached.
    pub async fn me(state: &DashboardState) -> Result<Option<UserInfo>, String> {
        let result = state.rpc_call("users.me", serde_json::Value::Null).await?;
        let user = result
            .get("user")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(user).map_err(|e| e.to_string())
    }

    /// Every known principal — member-visible (the roster picker needs it).
    pub async fn list(state: &DashboardState) -> Result<Vec<UserInfo>, String> {
        let result = state
            .rpc_call("users.list", serde_json::Value::Null)
            .await?;
        let arr = result
            .get("users")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(arr).map_err(|e| e.to_string())
    }
}
