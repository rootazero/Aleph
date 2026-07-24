//! Lightweight team resolution for the subagent tool's `send_message` /
//! `read_inbox` faces: `ensure_team` maps a `team_name` to a team id,
//! creating the team on first use. (The former `register_teammate`
//! auto-registration was removed with the honest-trim of named spawns —
//! spawned sub-agents are not addressable teammates.)

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::store::TeamStore;
use crate::teams::types::NewTeam;

/// Manages teammate registration and lifecycle.
pub struct TeammateManager {
    team_store: Arc<dyn TeamStore>,
}

impl TeammateManager {
    pub fn new(team_store: Arc<dyn TeamStore>) -> Self {
        Self { team_store }
    }

    /// Ensure a team exists with the given name, creating it if necessary.
    /// The `parent_agent_id` becomes the team leader.
    /// Returns the team ID.
    pub async fn ensure_team(&self, team_name: &str, parent_agent_id: &str) -> Result<String> {
        if let Some(existing) = self.team_store.get_team_by_name(team_name).await? {
            return Ok(existing.id);
        }

        match self
            .team_store
            .create_team(NewTeam {
                name: team_name.to_string(),
                description: "Auto-created team for teammate collaboration".to_string(),
                leader_id: parent_agent_id.to_string(),
            })
            .await
        {
            Ok(team) => Ok(team.id),
            Err(e) => self
                .team_store
                .get_team_by_name(team_name)
                .await?
                .map(|t| t.id)
                .ok_or_else(|| crate::error::AlephError::Other {
                    message: format!("Failed to create or find team '{team_name}': {e}"),
                    suggestion: None,
                }),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::store::SqliteTeamStore;
    use rusqlite::Connection;

    async fn setup() -> TeammateManager {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate");
        TeammateManager::new(Arc::new(store))
    }

    #[tokio::test]
    async fn ensure_team_creates_new_team() {
        let mgr = setup().await;
        let team_id = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        assert!(!team_id.is_empty());
    }

    #[tokio::test]
    async fn ensure_team_returns_existing() {
        let mgr = setup().await;
        let id1 = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        let id2 = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        assert_eq!(id1, id2);
    }

}
