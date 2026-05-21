//! Panel API for team management (teams.* RPC calls)

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// -- Types --

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: String,
    /// Present in list responses (TeamSummary), absent in get responses (Team).
    #[serde(default)]
    pub member_count: usize,
    /// Present in list responses (TeamSummary), absent in get responses (Team).
    #[serde(default)]
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
        // Server returns {"teams": [...]}, extract the inner array
        let teams_value = result.get("teams").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(teams_value).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, team_id: &str) -> Result<TeamDetail, String> {
        let result = state
            .rpc_call("teams.get", json!({"team_id": team_id}))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn disband(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.disband", json!({"team_id": team_id}))
            .await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.delete", json!({"team_id": team_id}))
            .await?;
        Ok(())
    }

    pub async fn agent_teams(
        state: &DashboardState,
        agent_id: &str,
    ) -> Result<Vec<TeamSummary>, String> {
        let result = state
            .rpc_call("agents.teams", json!({"agent_id": agent_id}))
            .await?;
        // Server returns {"teams": [...]}, extract the inner array
        let teams_value = result.get("teams").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(teams_value).map_err(|e| e.to_string())
    }
}

// =============================================================================
// CoordTask DTO + kanban API methods
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoordTaskDto {
    pub id: String,
    #[serde(default)]
    pub team_id: Option<String>,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    /// "pending" | "blocked" | "in_progress" | "completed" | "failed" | "cancelled"
    pub status: String,
    #[serde(default)]
    pub owner: Option<String>,
    /// "low" | "normal" | "high" | "critical"
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub created_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub completed_at: Option<u64>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub status: Option<String>,
    pub owner: Option<String>,
    pub result: Option<String>,
}

impl TeamsApi {
    pub async fn list_tasks(
        state: &DashboardState,
        team_id: &str,
        filter: TaskFilter,
    ) -> Result<Vec<CoordTaskDto>, String> {
        let mut params = serde_json::Map::new();
        params.insert("team_id".to_string(), Value::String(team_id.to_string()));
        if let Some(s) = filter.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = filter.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        let result = state
            .rpc_call("teams.list_tasks", Value::Object(params))
            .await?;
        let tasks_value = result.get("tasks").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(tasks_value).map_err(|e| e.to_string())
    }

    pub async fn update_task(
        state: &DashboardState,
        task_id: &str,
        patch: TaskPatch,
    ) -> Result<CoordTaskDto, String> {
        let mut params = serde_json::Map::new();
        params.insert("task_id".to_string(), Value::String(task_id.to_string()));
        if let Some(s) = patch.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = patch.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        if let Some(r) = patch.result {
            params.insert("result".to_string(), Value::String(r));
        }
        let result = state
            .rpc_call("teams.update_task", Value::Object(params))
            .await?;
        let task_value = result.get("task").cloned().unwrap_or(Value::Null);
        serde_json::from_value(task_value).map_err(|e| e.to_string())
    }
}
