//! Panel API for team management (teams.* RPC calls)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::context::DashboardState;

// -- Types --

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: String,
    pub member_count: usize,
    pub task_count: usize,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamMember {
    pub team_id: String,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamTask {
    pub id: String,
    pub team_id: String,
    pub agent_id: String,
    pub subject: String,
    pub status: String,
    pub result: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamDetail {
    pub team: TeamSummary,
    pub members: Vec<TeamMember>,
    pub tasks: Vec<TeamTask>,
}

// -- API --

pub struct TeamsApi;

impl TeamsApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<TeamSummary>, String> {
        let result = state.rpc_call("teams.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, team_id: &str) -> Result<TeamDetail, String> {
        let result = state.rpc_call("teams.get", json!({"team_id": team_id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn disband(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state.rpc_call("teams.disband", json!({"team_id": team_id})).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state.rpc_call("teams.delete", json!({"team_id": team_id})).await?;
        Ok(())
    }

    pub async fn agent_teams(state: &DashboardState, agent_id: &str) -> Result<Vec<TeamSummary>, String> {
        let result = state.rpc_call("agents.teams", json!({"agent_id": agent_id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}
