//! Teammate lifecycle management for named sub-agents.
//!
//! Handles auto-creation of lightweight teams, member registration,
//! and cleanup when teammates complete their work.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::store::TeamStore;
use crate::teams::types::{NewTeam, NewTeamMember};

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
        // Check if team already exists by listing and filtering
        let teams = self.team_store.list_teams().await?;
        if let Some(existing) = teams.iter().find(|t| t.name == team_name) {
            return Ok(existing.id.clone());
        }

        // Create new team — may race with a concurrent call
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
            Err(_) => {
                // Race: another call created the team first. Retry lookup.
                let teams = self.team_store.list_teams().await?;
                teams
                    .iter()
                    .find(|t| t.name == team_name)
                    .map(|t| t.id.clone())
                    .ok_or_else(|| crate::error::AlephError::Other {
                        message: format!("Failed to create or find team '{}'", team_name),
                        suggestion: None,
                    })
            }
        }
    }

    /// Register a named agent as a member of a team.
    pub async fn register_teammate(
        &self,
        team_id: &str,
        agent_name: &str,
        role: &str,
    ) -> Result<()> {
        self.team_store
            .add_member(NewTeamMember {
                team_id: team_id.to_string(),
                agent_id: agent_name.to_string(),
                role: role.to_string(),
            })
            .await?;

        Ok(())
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

    #[tokio::test]
    async fn register_teammate_succeeds() {
        let mgr = setup().await;
        let team_id = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        mgr.register_teammate(&team_id, "researcher", "worker")
            .await
            .unwrap();
    }
}
