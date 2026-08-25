//! Team-chat RPC wrappers: send a requirement to a team (spawns the leader run)
//! and hydrate the durable thread (tasks + artifacts). Mirrors the rpc_call
//! pattern used across `api/*`.

use crate::context::DashboardState;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct TeamChatSendResponse {
    /// `None` exactly when `observed` is `true`: the multi-human mention
    /// gate (spec §6.2) decided not to mint a fan-out run for this send, so
    /// there is no run to bind a Stop button to. `#[serde(default)]` on the
    /// other two fields, not on this one — an older core always sends a
    /// (non-null) `run_id` string, so this still deserializes unchanged
    /// against that shape.
    pub run_id: Option<String>,
    /// Whether the multi-human mention gate decided this message was
    /// observed rather than activated (persisted + broadcast, no run
    /// minted). `#[serde(default)]` — an older core never sends this field,
    /// and a missing field must read as "not observed" (byte-identical to
    /// the pre-P3 always-activates behavior), not fail the whole send.
    #[serde(default)]
    pub observed: bool,
    /// The persisted transcript row's id, if the user message was durably
    /// stored. `#[serde(default)]` for the same older-core compatibility —
    /// and `None` legitimately covers a persist failure the server already
    /// warned about (the turn still proceeds with no id to hand back).
    #[serde(default)]
    pub message_id: Option<String>,
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
    /// The human speaker's raw id, for a `kind == "user"` row (spec §6.2
    /// humanization). `None` for agent/system rows and for a Panel talking
    /// to an older core that never sent this field. Mirrors the live
    /// `team.<id>.message` event's `author_user_id` (Task 2).
    #[serde(default)]
    pub author_user_id: Option<String>,
    /// Resolved display name for `author_user_id`. `None` exactly when
    /// `author_user_id` is `None`.
    #[serde(default)]
    pub author_display_name: Option<String>,
}

fn default_history_kind() -> String {
    "agent".to_string()
}

pub struct TeamChatApi;

impl TeamChatApi {
    /// Hand the user's message to the team. Usually spawns the leader's
    /// orchestration run (`response.run_id` is `Some`); once a SECOND human
    /// has spoken in the thread, a message that does not @-mention a roster
    /// member or `@all`/`@everyone` is only observed — persisted and
    /// broadcast, `response.observed == true`, `response.run_id == None`
    /// (spec §6.2). Single-human threads always activate, unchanged.
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
    fn send_response_deserializes_observed_null_run_id() {
        // Observe mode (spec §6.2): `run_id` is JSON `null`, not absent —
        // Task 2's handler always emits the key.
        let j = r#"{"run_id": null, "observed": true, "message_id": "m1"}"#;
        let resp: TeamChatSendResponse = serde_json::from_str(j).unwrap();
        assert_eq!(resp.run_id, None);
        assert!(resp.observed);
        assert_eq!(resp.message_id, Some("m1".to_string()));
    }

    #[test]
    fn send_response_deserializes_activated_run() {
        let j = r#"{"run_id": "r1", "observed": false, "message_id": "m1"}"#;
        let resp: TeamChatSendResponse = serde_json::from_str(j).unwrap();
        assert_eq!(resp.run_id, Some("r1".to_string()));
        assert!(!resp.observed);
    }

    #[test]
    fn send_response_defaults_observed_and_message_id_for_older_core() {
        // Pre-P3 shape: only `run_id`, always a non-null string. Must still
        // deserialize — `observed` and `message_id` default rather than
        // failing the whole send.
        let j = r#"{"run_id": "r1"}"#;
        let resp: TeamChatSendResponse = serde_json::from_str(j).unwrap();
        assert_eq!(resp.run_id, Some("r1".to_string()));
        assert!(!resp.observed);
        assert_eq!(resp.message_id, None);
    }

    #[test]
    fn history_item_carries_author_fields_for_a_human_row() {
        let j = r#"{"from_agent":"user","content":"hi","msg_type":"message","kind":"user",
                    "created_at":1,"author_user_id":"u-alice","author_display_name":"Alice"}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.author_user_id, Some("u-alice".to_string()));
        assert_eq!(it.author_display_name, Some("Alice".to_string()));
    }

    #[test]
    fn history_item_defaults_author_fields_when_absent() {
        // Older core (pre-P3): no author fields on the wire at all.
        let j = r#"{"from_agent":"risk_analyst","content":"hi","msg_type":"message",
                    "kind":"agent","created_at":1}"#;
        let it: TeamMessageItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.author_user_id, None);
        assert_eq!(it.author_display_name, None);
    }

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
