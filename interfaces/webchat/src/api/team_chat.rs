//! Team-chat RPC wrappers: send a requirement to a team (spawns the leader run)
//! and hydrate the durable thread (tasks + artifacts). Mirrors the rpc_call
//! pattern used across `api/*`.

use crate::context::DashboardState;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct TeamChatSendResponse {
    pub run_id: String,
}

/// One durable thread item from `teams.chat.thread`. Mirrors the backend
/// `ThreadItem` (gateway/handlers/teams.rs): tasks + artifacts.
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadItem {
    pub kind: String, // "task" | "artifact"
    pub agent_id: String,
    pub title: String,
    pub content: String,
    pub timestamp: i64,
    #[serde(default)]
    pub artifact_id: Option<String>,
}

pub struct TeamChatApi;

impl TeamChatApi {
    /// Hand the user's requirement to the team leader (spawns the orchestration run).
    pub async fn send(
        state: &DashboardState,
        team_id: &str,
        message: &str,
    ) -> Result<TeamChatSendResponse, String> {
        let result = state
            .rpc_call("teams.chat.send", json!({ "team_id": team_id, "message": message }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Hydrate the team's durable thread (tasks + artifacts), chronologically.
    pub async fn thread(state: &DashboardState, team_id: &str) -> Result<Vec<ThreadItem>, String> {
        let result = state
            .rpc_call("teams.chat.thread", json!({ "team_id": team_id }))
            .await?;
        let items = result.get("items").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_thread_item_with_artifact_id() {
        let json = r#"{
            "kind": "artifact",
            "agent_id": "a",
            "title": "t",
            "content": "c",
            "timestamp": 123,
            "artifact_id": "x"
        }"#;
        let item: ThreadItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.kind, "artifact");
        assert_eq!(item.agent_id, "a");
        assert_eq!(item.title, "t");
        assert_eq!(item.content, "c");
        assert_eq!(item.timestamp, 123);
        assert_eq!(item.artifact_id, Some("x".to_string()));
    }

    #[test]
    fn deserializes_thread_item_without_artifact_id() {
        let json = r#"{
            "kind": "task",
            "agent_id": "b",
            "title": "do work",
            "content": "details",
            "timestamp": 456
        }"#;
        let item: ThreadItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.kind, "task");
        assert_eq!(item.artifact_id, None);
    }
}
