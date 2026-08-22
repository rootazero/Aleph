//! `users.*` Gateway RPC client.
//!
//! Backs the project-room roster UI (Task 8): resolving `owner_user_id` /
//! `member_ids` into display names, and picking new members from the set of
//! known principals. The wire shape is `aleph_protocol::users`, shared with
//! the server and the CLI.

use crate::context::DashboardState;

/// The shared wire shape, not a third hand-written copy of it.
///
/// This was a local DTO that "mirrors `gateway::handlers::users::UserView`" —
/// a mirror being exactly the arrangement where one side moves and the other
/// keeps reflecting yesterday. The alias keeps every existing `UserInfo`
/// reference in this crate working while making the fields a compile-time
/// contract with the server and the CLI.
pub use aleph_protocol::users::UserView as UserInfo;

pub struct UsersApi;

impl UsersApi {
    /// The caller's own principal record. `Ok(None)` (not an error) when no
    /// caller user is scoped — e.g. a loopback / unrestricted connection with
    /// no P1 identity attached.
    pub async fn me(state: &DashboardState) -> Result<Option<UserInfo>, String> {
        let result = state.rpc_call("users.me", serde_json::Value::Null).await?;
        let parsed: aleph_protocol::users::UserMeResult =
            serde_json::from_value(result).map_err(|e| e.to_string())?;
        Ok(parsed.user)
    }

    /// Every known principal — member-visible (the roster picker needs it).
    pub async fn list(state: &DashboardState) -> Result<Vec<UserInfo>, String> {
        let result = state
            .rpc_call("users.list", serde_json::Value::Null)
            .await?;
        let parsed: aleph_protocol::users::UserListResult =
            serde_json::from_value(result).map_err(|e| e.to_string())?;
        Ok(parsed.users)
    }
}
