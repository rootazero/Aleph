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

/// One replayed group-chat bubble from `teams.chat.history`.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamMessageItem {
    pub from_agent: String,
    pub content: String,
    pub msg_type: String,
    /// Server-derived render class: `"user"` | `"agent"` | `"system"`. Defaults
    /// to `"agent"` so a Panel talking to an older core still replays member
    /// replies correctly (it just loses the centered system-chip styling).
    #[serde(default = "default_history_kind")]
    pub kind: String,
    pub created_at: i64,
}

fn default_history_kind() -> String {
    "agent".to_string()
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
            .rpc_call(
                "teams.chat.send",
                json!({ "team_id": team_id, "message": message }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Stop an in-flight fan-out tree. `run_id` is the id [`Self::send`]
    /// returned (also carried by the `team.<id>.fanout` `started` event) — the
    /// group-chat analogue of `chat.abort`, which cannot reach a fan-out
    /// because the tree is not an `active_runs` entry of the single-agent
    /// engine.
    pub async fn cancel(state: &DashboardState, run_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.chat.cancel", json!({ "run_id": run_id }))
            .await
            .map(|_| ())
    }

    /// Replay the durable group-chat transcript as bubbles, chronologically.
    pub async fn history(
        state: &DashboardState,
        team_id: &str,
    ) -> Result<Vec<TeamMessageItem>, String> {
        let result = state
            .rpc_call("teams.chat.history", json!({ "team_id": team_id }))
            .await?;
        let items = result.get("items").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| e.to_string())
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
    fn deserializes_history_item() {
        let j = r#"{"from_agent":"risk_analyst","content":"hi","msg_type":"message",
                    "kind":"agent","created_at":123}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.from_agent, "risk_analyst");
        assert_eq!(it.kind, "agent");
        assert_eq!(it.created_at, 123);
    }

    #[test]
    fn history_item_without_kind_defaults_to_agent() {
        // Forward/backward compatibility: an older core omits `kind`; the
        // replay must still produce attributed member bubbles rather than
        // failing the whole hydrate on a missing field.
        let j =
            r#"{"from_agent":"risk_analyst","content":"hi","msg_type":"message","created_at":1}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.kind, "agent");
    }

    #[test]
    fn deserializes_system_history_item() {
        let j = r#"{"from_agent":"system","content":"depth cap","msg_type":"system_notification",
                    "kind":"system","created_at":9}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.kind, "system");
    }

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
