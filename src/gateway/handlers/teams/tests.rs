//! Consolidated tests for the teams handler modules.

#![allow(unused_imports)]

use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::resilience::{AgentUsageTotal, StateDatabase};
use crate::sync_primitives::Arc;
use crate::teams::snapshots::{capture_snapshot, restore_snapshot, SqliteSnapshotStore};
use crate::teams::{NewTeam, NewTeamMember, TeamMemberKind, TeamStore};

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

pub use super::canvas::*;
pub use super::crud::*;
pub use super::snapshot::*;
pub use super::tasks::*;
pub use super::workflow::*;

mod tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;

    async fn coord_store() -> Arc<dyn CoordTaskStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        Arc::new(store)
    }

    /// A team store pre-seeded with the literal team ids a fixture addresses.
    ///
    /// Every task-facing handler now resolves its team through the ownership
    /// gate, so a team id that exists in no store is (correctly) 404 — these
    /// fixtures predate that and name their teams `"T"` / `"team-x"`.
    pub(super) async fn team_store_with(ids: &[&str]) -> Arc<dyn crate::teams::TeamStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        let store = crate::teams::store::SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate");
        for id in ids {
            store
                .insert_team_with_id(id, id)
                .await
                .expect("seed fixture team");
        }
        Arc::new(store)
    }

    fn create_req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.create_task".to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn create_task_trims_subject_and_returns_task() {
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({"team_id": "T", "subject": "  Ship it  ", "priority": "high"})),
            teams,
            store,
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        let task = &result["task"];
        assert_eq!(task["subject"], "Ship it");
        assert_eq!(task["team_id"], "T");
        assert_eq!(task["priority"], "high");
        assert_eq!(task["status"], "pending");
    }

    #[tokio::test]
    async fn create_task_rejects_blank_subject() {
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({"team_id": "T", "subject": "   "})),
            teams,
            store,
        )
        .await;

        let err = resp.error.expect("expected an error");
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn create_task_rejects_unknown_priority() {
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({"team_id": "T", "subject": "x", "priority": "urgent"})),
            teams,
            store,
        )
        .await;

        let err = resp.error.expect("expected an error");
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn create_task_with_owner_auto_injects_managed_by_dispatcher() {
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({
                "team_id": "T",
                "subject": "auto-dispatch",
                "owner": "worker-a",
            })),
            teams,
            store,
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let task = resp.result.expect("result")["task"].clone();
        assert_eq!(
        task["metadata"]["managed_by"], "dispatcher",
        "owner-bearing tasks must be flagged for the dispatcher loop or they silently never run; metadata={:?}",
        task["metadata"]
    );
    }

    #[tokio::test]
    async fn create_task_without_owner_skips_managed_by_marker() {
        // Orphan tasks (no owner yet) shouldn't auto-claim the dispatcher
        // namespace — they're parked until a leader assigns them.
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({ "team_id": "T", "subject": "orphan" })),
            teams,
            store,
        )
        .await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let task = resp.result.expect("result")["task"].clone();
        assert!(
            task["metadata"].get("managed_by").is_none(),
            "tasks without an owner must NOT carry the dispatcher marker; got {:?}",
            task["metadata"]
        );
    }

    #[tokio::test]
    async fn create_task_respects_caller_supplied_managed_by_override() {
        let store = coord_store().await;
        let teams = team_store_with(&["T"]).await;
        let resp = handle_create_task(
            create_req(json!({
                "team_id": "T",
                "subject": "manual",
                "owner": "worker-x",
                "metadata": {"managed_by": "team_delegate"},
            })),
            teams,
            store,
        )
        .await;
        assert!(resp.error.is_none());
        let task = resp.result.expect("result")["task"].clone();
        assert_eq!(
            task["metadata"]["managed_by"], "team_delegate",
            "caller-supplied managed_by must win over auto-injection"
        );
    }

    // -------------------------------------------------------------------------
    // handle_create tests
    // -------------------------------------------------------------------------

    async fn team_store() -> Arc<dyn crate::teams::TeamStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        let store = crate::teams::store::SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate");
        Arc::new(store)
    }

    fn test_event_bus() -> Arc<crate::gateway::event_bus::GatewayEventBus> {
        Arc::new(crate::gateway::event_bus::GatewayEventBus::new())
    }

    fn create_team_req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.create".to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn handle_create_persists_team_with_leader_and_members() {
        let store = team_store().await;
        let req = create_team_req(json!({
            "name": "ResearchSquad",
            "description": "ad-hoc",
            "leader_id": "agent-main",
            "members": [{"agent_id": "agent-alice", "role": "researcher"}]
        }));
        let resp = handle_create(req, store.clone(), test_event_bus()).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

        let team_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("team_id"))
            .and_then(|v| v.as_str())
            .expect("team_id in response")
            .to_string();

        let team = store.get_team(&team_id).await.unwrap().unwrap();
        assert_eq!(team.leader_id, "agent-main");

        let members = store.get_members(&team_id).await.unwrap();
        let ids: Vec<&str> = members.iter().map(|m| m.agent_id.as_str()).collect();
        assert!(ids.contains(&"agent-main"), "leader auto-enrolled");
        assert!(ids.contains(&"agent-alice"), "member enrolled");

        let leader_member = members.iter().find(|m| m.agent_id == "agent-main").unwrap();
        assert_eq!(leader_member.role, "leader", "leader has role=leader");
    }

    #[tokio::test]
    async fn handle_create_rejects_empty_name() {
        let store = team_store().await;
        let resp = handle_create(
            create_team_req(json!({"name": "", "leader_id": "a"})),
            store,
            test_event_bus(),
        )
        .await;
        let err = resp.error.expect("expected error");
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn handle_create_deduplicates_leader_in_members_list() {
        let store = team_store().await;
        let resp = handle_create(
            create_team_req(json!({
                "name": "Dup",
                "leader_id": "bot",
                "members": [{"agent_id": "bot", "role": "worker"}]
            })),
            store.clone(),
            test_event_bus(),
        )
        .await;
        assert!(resp.error.is_none());
        let team_id = resp
            .result
            .unwrap()
            .get("team_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let members = store.get_members(&team_id).await.unwrap();
        let bot_entries: Vec<_> = members.iter().filter(|m| m.agent_id == "bot").collect();
        assert_eq!(bot_entries.len(), 1, "leader enrolled exactly once");
    }

    #[tokio::test]
    async fn handle_create_with_auto_name_sets_flag() {
        let store = team_store().await;
        let req =
            create_team_req(json!({ "name": "新群聊", "leader_id": "main", "auto_name": true }));
        let resp = handle_create(req, Arc::clone(&store), test_event_bus()).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let team_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("team_id"))
            .and_then(|v| v.as_str())
            .expect("team_id in response")
            .to_string();
        assert!(store.take_auto_name_flag(&team_id).await.unwrap());
    }

    #[tokio::test]
    async fn handle_create_without_auto_name_leaves_flag_off() {
        let store = team_store().await;
        let req = create_team_req(json!({ "name": "My Team", "leader_id": "main" }));
        let resp = handle_create(req, Arc::clone(&store), test_event_bus()).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let team_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("team_id"))
            .and_then(|v| v.as_str())
            .expect("team_id in response")
            .to_string();
        assert!(!store.take_auto_name_flag(&team_id).await.unwrap());
    }

    // -------------------------------------------------------------------------
    // handle_chat_thread tests
    // -------------------------------------------------------------------------

    fn chat_thread_req(team_id: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.chat.thread".to_string(),
            params: Some(json!({ "team_id": team_id })),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn chat_thread_empty_store_returns_items_array() {
        // Verifies: "returns items array structure" + "None artifact_store degrades gracefully"
        let store = coord_store().await;
        let teams = team_store_with(&["team-x"]).await;
        let resp = handle_chat_thread(
            chat_thread_req("team-x"),
            teams,
            store,
            None, // no artifact store — must not error
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert!(
            result["items"].is_array(),
            "response must contain an 'items' array, got: {result:?}"
        );
        assert_eq!(
            result["items"].as_array().unwrap().len(),
            0,
            "empty store must yield empty items"
        );
    }

    #[tokio::test]
    async fn chat_thread_tasks_appear_as_items_sorted_by_timestamp() {
        use crate::agents::swarm::tasks::NewCoordTask;

        let store = coord_store().await;

        // Create two tasks; store assigns created_at=now_epoch() (seconds).
        // We cannot control the exact timestamp, but we can assert ordering is stable.
        store
            .create_task(NewCoordTask {
                team_id: Some("team-y".to_string()),
                subject: "First task".to_string(),
                description: "desc-1".to_string(),
                owner: Some("agent-a".to_string()),
                priority: crate::agents::swarm::tasks::Priority::default(),
                blocked_by: vec![],
                metadata: serde_json::Value::Object(Default::default()),
            })
            .await
            .expect("create first task");
        store
            .create_task(NewCoordTask {
                team_id: Some("team-y".to_string()),
                subject: "Second task".to_string(),
                description: "desc-2".to_string(),
                owner: None,
                priority: crate::agents::swarm::tasks::Priority::default(),
                blocked_by: vec![],
                metadata: serde_json::Value::Object(Default::default()),
            })
            .await
            .expect("create second task");

        let resp = handle_chat_thread(
            chat_thread_req("team-y"),
            team_store_with(&["team-y"]).await,
            store,
            None,
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let items = resp.result.expect("result")["items"].clone();
        let arr = items.as_array().expect("items is array");
        assert_eq!(arr.len(), 2, "two tasks should produce two items");

        // All are kind=task
        for item in arr {
            assert_eq!(item["kind"], "task");
        }

        // Items are sorted by timestamp (non-decreasing)
        let timestamps: Vec<i64> = arr
            .iter()
            .map(|i| i["timestamp"].as_i64().expect("timestamp is i64"))
            .collect();
        let mut sorted = timestamps.clone();
        sorted.sort();
        assert_eq!(timestamps, sorted, "items must be sorted by timestamp");
    }

    #[tokio::test]
    async fn chat_thread_includes_artifacts_when_store_present() {
        use crate::agents::swarm::tasks::NewCoordTask;
        use crate::teams::artifacts::{
            ArtifactStore, ArtifactType, NewArtifact, SqliteArtifactStore, TaskStatus,
        };

        let coord = coord_store().await;
        let artifacts: Arc<dyn ArtifactStore> =
            Arc::new(SqliteArtifactStore::new_in_memory().await);

        // Create one task and capture its id so the artifact can reference it.
        let task = coord
            .create_task(NewCoordTask {
                team_id: Some("team-z".to_string()),
                subject: "Build it".to_string(),
                description: "desc".to_string(),
                owner: Some("agent-builder".to_string()),
                priority: crate::agents::swarm::tasks::Priority::default(),
                blocked_by: vec![],
                metadata: serde_json::Value::Object(Default::default()),
            })
            .await
            .expect("create task");

        artifacts
            .create_artifact(NewArtifact {
                task_id: task.id.clone(),
                agent_id: "agent-author".to_string(),
                artifact_type: ArtifactType::Report,
                title: "Deliverable".to_string(),
                content: "# Done\n\nbody".to_string(),
                status: TaskStatus::Completed,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(Default::default()),
            })
            .await
            .expect("create artifact");

        let resp = handle_chat_thread(
            chat_thread_req("team-z"),
            team_store_with(&["team-z"]).await,
            coord,
            Some(artifacts),
        )
        .await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let items = resp.result.expect("result")["items"].clone();
        let arr = items.as_array().expect("items is array");

        // Both a task item and an artifact item should be present.
        let task_item = arr
            .iter()
            .find(|i| i["kind"] == "task")
            .expect("a task item must be present");
        assert_eq!(task_item["title"], "Build it");

        let artifact_item = arr
            .iter()
            .find(|i| i["kind"] == "artifact")
            .expect("an artifact item must be present");
        assert_eq!(artifact_item["agent_id"], "agent-author");
        assert_eq!(artifact_item["title"], "Deliverable");
        assert!(
            artifact_item["artifact_id"].is_string(),
            "artifact item must carry a Some(artifact_id), got: {artifact_item:?}"
        );

        // The merged list is sorted by timestamp (non-decreasing).
        let timestamps: Vec<i64> = arr
            .iter()
            .map(|i| i["timestamp"].as_i64().expect("timestamp is i64"))
            .collect();
        let mut sorted = timestamps.clone();
        sorted.sort();
        assert_eq!(
            timestamps, sorted,
            "merged items must be sorted by timestamp"
        );
    }

    // -------------------------------------------------------------------------
    // map_history tests
    // -------------------------------------------------------------------------

    #[test]
    fn map_history_sorts_chronologically_and_carries_fields() {
        use crate::teams::messages::types::{MessageType, TeamMessage};
        use chrono::{TimeZone, Utc};

        let t0 = Utc.timestamp_millis_opt(1_000_000).unwrap();
        let t1 = Utc.timestamp_millis_opt(2_000_000).unwrap();

        // Insert in reverse order to verify sort.
        let msgs = vec![
            TeamMessage {
                id: "m2".to_string(),
                team_id: "t1".to_string(),
                from_agent: "agent-b".to_string(),
                msg_type: MessageType::SystemNotification,
                subject: String::new(),
                content: "second".to_string(),
                recipients: vec![],
                reply_to: None,
                thread_id: None,
                attachments: vec![],
                created_at: t1,
                expires_at: None,
                author_user_id: None,
            },
            TeamMessage {
                id: "m1".to_string(),
                team_id: "t1".to_string(),
                from_agent: "agent-a".to_string(),
                msg_type: MessageType::Message,
                subject: String::new(),
                content: "first".to_string(),
                recipients: vec![],
                reply_to: None,
                thread_id: None,
                attachments: vec![],
                created_at: t0,
                expires_at: None,
                author_user_id: None,
            },
        ];

        let items = map_history(msgs);
        assert_eq!(items.len(), 2);

        // Chronological after sort: t0 first.
        assert_eq!(items[0].from_agent, "agent-a");
        assert_eq!(items[0].content, "first");
        assert_eq!(items[0].msg_type, "message");
        assert_eq!(items[0].created_at, t0.timestamp_millis());

        assert_eq!(items[1].from_agent, "agent-b");
        assert_eq!(items[1].content, "second");
        assert_eq!(items[1].msg_type, "system_notification");
        assert_eq!(items[1].created_at, t1.timestamp_millis());
    }

    /// Build a transcript row. `recipients` empty ⇒ broadcast to the group;
    /// non-empty ⇒ directed inbox traffic.
    fn history_msg(
        from_agent: &str,
        msg_type: crate::teams::messages::types::MessageType,
        to: &[&str],
        at_millis: i64,
    ) -> crate::teams::messages::types::TeamMessage {
        use crate::teams::messages::types::{Recipient, RecipientRole, TeamMessage};
        use chrono::{TimeZone, Utc};
        TeamMessage {
            id: format!("m-{from_agent}-{at_millis}"),
            team_id: "t1".to_string(),
            from_agent: from_agent.to_string(),
            msg_type,
            subject: String::new(),
            content: "body".to_string(),
            recipients: to
                .iter()
                .map(|a| Recipient {
                    agent_id: (*a).to_string(),
                    role: RecipientRole::To,
                })
                .collect(),
            reply_to: None,
            thread_id: None,
            attachments: vec![],
            created_at: Utc.timestamp_millis_opt(at_millis).unwrap(),
            expires_at: None,
            author_user_id: None,
        }
    }

    #[test]
    fn map_history_classifies_user_agent_and_system_rows() {
        use crate::teams::messages::types::MessageType;

        let items = map_history(vec![
            history_msg(
                crate::teams::broadcast::RESERVED_USER_HANDLE,
                MessageType::Message,
                &[],
                1,
            ),
            history_msg("risk_analyst", MessageType::Message, &[], 2),
            history_msg(
                crate::teams::broadcast::SYSTEM_HANDLE,
                MessageType::SystemNotification,
                &[],
                3,
            ),
        ]);

        let kinds: Vec<&str> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, vec!["user", "agent", "system"]);
    }

    #[test]
    fn map_history_drops_directed_inbox_traffic() {
        use crate::teams::messages::types::MessageType;

        // `team_messages` is a shared bus: the notifier's leader digests and the
        // router's escalation hints are addressed to one agent and were never
        // shown live. Replaying them turned a re-opened group chat into a
        // noisier conversation than the one the user had just been watching.
        let items = map_history(vec![
            history_msg(
                "team_dispatcher",
                MessageType::SystemNotification,
                &["leader"],
                1,
            ),
            history_msg("risk_analyst", MessageType::Message, &[], 2),
        ]);

        assert_eq!(items.len(), 1, "only the conversation row survives");
        assert_eq!(items[0].from_agent, "risk_analyst");
    }

    #[test]
    fn map_history_keeps_directed_conversation_rows() {
        use crate::teams::messages::types::MessageType;

        // An addressed *conversation* message (@mention reply) is still chat —
        // the filter keys on type-plus-recipients, not recipients alone, so an
        // agent-to-agent reply is not mistaken for inbox plumbing.
        let items = map_history(vec![history_msg(
            "risk_analyst",
            MessageType::Message,
            &["growth_analyst"],
            1,
        )]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "agent");
    }

    // -------------------------------------------------------------------------
    // handle_rename tests
    // -------------------------------------------------------------------------

    fn rename_req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.rename".to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn handle_rename_updates_name() {
        let store = team_store().await;
        let team = store
            .create_team(crate::teams::NewTeam {
                name: "Old".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        let req = rename_req(json!({ "team_id": team.id, "name": "Renamed" }));
        let resp = handle_rename(req, Arc::clone(&store), test_event_bus()).await;
        assert!(resp.error.is_none(), "rename should succeed: {resp:?}");

        let got = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed");
    }

    #[tokio::test]
    async fn handle_rename_rejects_blank_name() {
        let store = team_store().await;
        let req = rename_req(json!({ "team_id": "t", "name": "   " }));
        let resp = handle_rename(req, store, test_event_bus()).await;
        assert!(resp.error.is_some(), "blank name must be rejected");
    }

    /// Regression: disbanding a team must publish a `team.changed` frame. The
    /// group-chat sidebar and the teams tab both re-fetch on this topic; without
    /// the frame the two views drift apart (sidebar hides the disbanded team,
    /// teams tab keeps showing it as active until a manual refresh).
    #[tokio::test]
    async fn handle_disband_emits_team_changed() {
        use crate::gateway::events::{ChangeKind, GatewayEventFrame};

        let store = team_store().await;
        let team = store
            .create_team(crate::teams::NewTeam {
                name: "Doomed".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        let bus = test_event_bus();
        let mut rx = bus.subscribe_typed();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.disband".to_string(),
            params: Some(json!({ "team_id": team.id })),
            id: Some(json!(1)),
        };
        let resp = handle_disband(req, Arc::clone(&store), Arc::clone(&bus)).await;
        assert!(resp.error.is_none(), "disband should succeed: {resp:?}");

        match rx.try_recv() {
            Ok(GatewayEventFrame::TeamChanged { team_id, change }) => {
                assert_eq!(team_id, team.id);
                assert_eq!(change, ChangeKind::Updated);
            }
            other => panic!("expected a TeamChanged frame, got {other:?}"),
        }
    }
}

mod snapshot_handler_tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
    use crate::teams::{NewTeam, NewTeamMember, SqliteTeamStore};
    use rusqlite::Connection;

    async fn setup() -> (
        Arc<dyn TeamStore>,
        Arc<dyn CoordTaskStore>,
        Arc<SqliteSnapshotStore>,
        String,
    ) {
        let coord_conn = Connection::open_in_memory().unwrap();
        let coord = Arc::new(SqliteCoordTaskStore::new(coord_conn));
        coord.migrate().await.unwrap();
        let snap = Arc::new(SqliteSnapshotStore::new_from_shared(
            coord.connection_handle(),
        ));

        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "Alpha".into(),
                description: "rpc test".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        teams
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "leader".into(),
                role: "leader".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let teams_arc: Arc<dyn TeamStore> = teams;
        let coord_arc: Arc<dyn CoordTaskStore> = coord;
        (teams_arc, coord_arc, snap, team.id)
    }

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: Some(serde_json::Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn create_list_get_restore_delete_full_lifecycle() {
        let (teams, coord, snap, team_id) = setup().await;

        // create
        let resp = handle_snapshot_create(
            req(
                "teams.snapshot.create",
                json!({ "team_id": team_id, "tag": "v1", "note": "first" }),
            ),
            teams.clone(),
            coord.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "create error: {:?}", resp.error);
        let sid = resp.result.as_ref().unwrap()["snapshot_id"]
            .as_str()
            .unwrap()
            .to_string();

        // list
        let resp = handle_snapshot_list(
            req("teams.snapshot.list", json!({ "team_id": team_id })),
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        let arr = resp.result.as_ref().unwrap()["snapshots"]
            .as_array()
            .unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"].as_str().unwrap(), sid);

        // get
        let resp = handle_snapshot_get(
            req("teams.snapshot.get", json!({ "snapshot_id": sid })),
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        let payload = resp.result.as_ref().unwrap()["payload"].clone();
        assert_eq!(payload["team"]["id"].as_str().unwrap(), team_id);

        // restore — dry-run (apply omitted ⇒ default false)
        let resp = handle_snapshot_restore(
            req("teams.snapshot.restore", json!({ "snapshot_id": sid })),
            teams.clone(),
            coord.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert!(
            resp.result.as_ref().unwrap()["dry_run"].as_bool().unwrap(),
            "default apply must be false → dry_run true"
        );

        // delete
        let resp = handle_snapshot_delete(
            req("teams.snapshot.delete", json!({ "snapshot_id": sid })),
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert!(resp.result.as_ref().unwrap()["existed"].as_bool().unwrap());

        // delete again → existed:false (idempotent)
        let resp = handle_snapshot_delete(
            req("teams.snapshot.delete", json!({ "snapshot_id": sid })),
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert!(!resp.result.as_ref().unwrap()["existed"].as_bool().unwrap());

        // get after delete → not found
        let resp = handle_snapshot_get(
            req("teams.snapshot.get", json!({ "snapshot_id": sid })),
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn list_works_without_params() {
        let (teams, _coord, snap, _team_id) = setup().await;
        let resp = handle_snapshot_list(
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "teams.snapshot.list".into(),
                params: None,
                id: Some(serde_json::Value::Number(1.into())),
            },
            teams.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert!(resp.result.as_ref().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

mod usage_handler_tests {
    use super::*;
    use crate::resilience::{AgentTask, RiskLevel, StateDatabase, TaskTrace};
    use crate::teams::{NewTeam, NewTeamMember, SqliteTeamStore};
    use aleph_protocol::AgentTraceEvent;
    use rusqlite::Connection;

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: Some(serde_json::Value::Number(1.into())),
        }
    }

    async fn setup_team_with_usage() -> (Arc<dyn TeamStore>, Arc<StateDatabase>, String) {
        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "UsageTeam".into(),
                description: "usage rpc".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        for who in ["leader", "worker"] {
            teams
                .add_member(NewTeamMember {
                    team_id: team.id.clone(),
                    agent_id: who.into(),
                    role: who.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }

        let db = Arc::new(StateDatabase::in_memory().unwrap());
        // Seed task + provider_usage rows for leader & worker, plus one row
        // for an unrelated agent that must be filtered out.
        for (task, agent, input, ts) in [
            ("t1", "leader", 100u32, 1000i64),
            ("t1", "leader", 200, 1100),
            ("t1", "worker", 50, 1200),
            ("t2", "outsider", 999, 1300),
        ] {
            let _ = db
                .insert_agent_task(&AgentTask::new(task, "s", "coder", "x", RiskLevel::Low))
                .await;
            db.insert_trace(&TaskTrace {
                id: 0,
                task_id: task.into(),
                step_index: ts as u32,
                event: AgentTraceEvent::ProviderUsage {
                    agent_id: agent.into(),
                    input_tokens: input,
                    output_tokens: input / 2,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    thinking_tokens: None,
                },
                timestamp: ts,
            })
            .await
            .unwrap();
        }

        let teams_arc: Arc<dyn TeamStore> = teams;
        (teams_arc, db, team.id)
    }

    #[tokio::test]
    async fn usage_aggregates_members_and_excludes_outsiders() {
        let (teams, db, team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": team_id })),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "usage error: {:?}", resp.error);
        let result = resp.result.as_ref().unwrap();
        assert_eq!(result["member_count"].as_u64().unwrap(), 2);
        // 100 + 200 (leader) + 50 (worker) = 350; outsider's 999 must be ignored.
        assert_eq!(result["total"]["input_tokens"].as_u64().unwrap(), 350);
        // outputs = input/2 each → 50 + 100 + 25 = 175
        assert_eq!(result["total"]["output_tokens"].as_u64().unwrap(), 175);
        assert_eq!(result["total"]["call_count"].as_u64().unwrap(), 3);
        // Seed rows carry cache_read = None → 0, with non-zero input → ratio 0.0
        // (distinguishable from "no data" None).
        assert_eq!(result["total"]["cache_hit_ratio"].as_f64().unwrap(), 0.0);
        let per_agent = result["per_agent"].as_array().unwrap();
        assert_eq!(per_agent.len(), 2, "outsider must not appear");
    }

    /// Seed rows with real cache_read counts so the team-level cache_hit_ratio
    /// surfaces a non-zero number on the wire. Locks in the per-row vs. rollup
    /// denominator parity claimed by the comment above `handle_usage`.
    #[tokio::test]
    async fn usage_reports_cache_hit_ratio_when_cache_reads_present() {
        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "Cached".into(),
                description: "ratio test".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        teams
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "leader".into(),
                role: "leader".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let _ = db
            .insert_agent_task(&AgentTask::new("t1", "s", "coder", "x", RiskLevel::Low))
            .await;
        // Counters are DISJOINT for every provider — each adapter subtracts the
        // cached portion out of its protocol's prompt total before the usage is
        // recorded — so the ratio is cache_read / (input + cache_read)
        // = 80 / 180. This used to assert 0.8 on the theory that an
        // OpenAI/DeepSeek row carries cache_read *inside* input; no adapter can
        // produce that row, and asserting it locked in a rollup that
        // over-reported the hit rate by up to 2x in exactly the degraded regime
        // (see `AgentUsageTotal::cache_hit_ratio`).
        db.insert_trace(&TaskTrace {
            id: 0,
            task_id: "t1".into(),
            step_index: 0,
            event: AgentTraceEvent::ProviderUsage {
                agent_id: "leader".into(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: Some(80),
                cache_creation_tokens: None,
                thinking_tokens: None,
            },
            timestamp: 1000,
        })
        .await
        .unwrap();

        let teams_arc: Arc<dyn TeamStore> = teams;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": team.id })),
            teams_arc,
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let ratio = resp.result.as_ref().unwrap()["total"]["cache_hit_ratio"]
            .as_f64()
            .expect("ratio must be a number");
        assert!(
            (ratio - 80.0 / 180.0).abs() < 1e-9,
            "expected 0.444…, got {ratio}"
        );
    }

    #[tokio::test]
    async fn usage_honours_since_until_window() {
        let (teams, db, team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req(
                "teams.usage",
                json!({ "team_id": team_id, "since": 1050, "until": 1150 }),
            ),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        let total = resp.result.as_ref().unwrap()["total"].clone();
        // Only leader's 1100-stamped row sits inside [1050, 1150].
        assert_eq!(total["input_tokens"].as_u64().unwrap(), 200);
        assert_eq!(total["call_count"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn usage_returns_not_found_for_unknown_team() {
        let (teams, db, _team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": "ghost" })),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, RESOURCE_NOT_FOUND);
    }
}

/// `teams.chat.cancel`'s ownership gate.
///
/// The handler is addressed by a fan-out tree `run_id`, so the interesting
/// question is not "does the predicate return false" but "does a refused caller
/// reach the tracker at all". Both tests below assert the EFFECT on the tree:
/// the recording adapter counts every member run the walk aborts, and the probe
/// token counts the tree poison. A gate that returned `not_found` *after*
/// firing the poison would pass a response-shape assertion and still hand any
/// caller a one-shot kill switch on any fan-out in the process.
mod chat_cancel_gate_tests {
    use super::*;

    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    use crate::agents::background_tracker::{
        BackgroundAgentTracker, RunningRegistration, SpawnMeta,
    };
    use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
    use crate::gateway::context::GatewayContext;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus};
    use crate::gateway::inter_agent_policy::AgentToAgentPolicy;
    use crate::gateway::session_store::sqlite_backend::{
        SqliteSessionStore, SqliteSessionStoreConfig,
    };
    use crate::gateway::session_store::SessionStore;
    use crate::teams::broadcast::{team_of_fanout_run, GroupChatBroadcaster};
    // The seeded-team fixture is shared with the sibling suite rather than
    // re-derived: "a team that is in the store" has one definition here.
    use super::tests::team_store_with;

    /// Records every `cancel(run_id)` the tree walk issues. The walk is the
    /// only thing in this handler that reaches the execution engine, so an
    /// empty log means the caller never got past the gate.
    #[derive(Default)]
    struct RecordingCancelAdapter {
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingCancelAdapter {
        fn log(&self) -> Vec<String> {
            self.cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl ExecutionAdapter for RecordingCancelAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            self.cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(run_id.to_string());
            Ok(())
        }

        async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
            None
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    fn gateway_context(
        adapter: Arc<RecordingCancelAdapter>,
    ) -> (tempfile::TempDir, Arc<GatewayContext>) {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_store: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::new(SqliteSessionStoreConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .expect("session store"),
        );
        // The tempdir must outlive the store, so it goes back to the caller.
        // It used to be `mem::forget`-ed instead, which bought a one-line call
        // site at the price of one abandoned tree per test, forever.
        (
            temp,
            Arc::new(GatewayContext::new(
                session_store,
                Arc::new(AgentRegistry::new()),
                adapter,
                Arc::new(AgentToAgentPolicy::permissive()),
            )),
        )
    }

    fn cancel_req(run_id: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.chat.cancel".to_string(),
            params: Some(json!({ "run_id": run_id })),
            id: Some(json!(7)),
        }
    }

    /// A live fan-out tree plus one running member, wired exactly as
    /// `teams.chat.send` wires them.
    ///
    /// The returned `probe` is the tree's kill switch: re-registering the tree
    /// id with our own token replaces the one `register_fanout` minted, so
    /// `tracker.cancel(run_id)` — the poison step — becomes observable without
    /// adding a test-only accessor to production code. The index entry still
    /// comes from the real `register_fanout` write.
    fn live_fanout(
        team_id: &str,
    ) -> (
        String,
        String,
        CancellationToken,
        Vec<RunningRegistration>,
        crate::teams::broadcast::RegisteredFanout,
    ) {
        let run_id = format!("fanout-{}", uuid::Uuid::new_v4());
        let member_run_id = format!("member-{}", uuid::Uuid::new_v4());
        let fanout = GroupChatBroadcaster::register_fanout(run_id.clone(), team_id);

        let tracker = BackgroundAgentTracker::global();
        let probe = CancellationToken::new();
        let tree_probe = RunningRegistration::register(
            Arc::clone(&tracker),
            run_id.clone(),
            probe.clone(),
            "probe tree".to_string(),
            SpawnMeta {
                parent_id: None,
                depth: 0,
                root_session: run_id.clone(),
                model: None,
            },
        );
        let member = RunningRegistration::register(
            tracker,
            member_run_id.clone(),
            CancellationToken::new(),
            "probe member".to_string(),
            SpawnMeta {
                parent_id: Some(run_id.clone()),
                depth: 1,
                root_session: run_id.clone(),
                model: None,
            },
        );
        (
            run_id,
            member_run_id,
            probe,
            vec![tree_probe, member],
            fanout,
        )
    }

    /// The owner's own fan-out still cancels end to end — the gate must not
    /// have broken the feature it protects.
    #[tokio::test]
    async fn the_owning_caller_still_cancels_the_tree_and_its_members() {
        let teams = team_store_with(&["team-owned"]).await;
        let (run_id, member_run_id, probe, _regs, _fanout) = live_fanout("team-owned");
        let adapter = Arc::new(RecordingCancelAdapter::default());
        let (_scratch, ctx) = gateway_context(Arc::clone(&adapter));

        let resp = handle_chat_cancel(cancel_req(&run_id), ctx, teams).await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert_eq!(result["cancelled"], json!(true));
        assert_eq!(result["members_signalled"], json!(1));
        assert!(probe.is_cancelled(), "the tree poison must have fired");
        assert_eq!(
            adapter.log(),
            vec![member_run_id],
            "the in-flight member run must have been aborted through the engine"
        );
    }

    /// The gate. `team_store_with(&[])` has no row for the team this tree
    /// belongs to, which is exactly what `ScopedTeamStore` returns for another
    /// user's team — `Ok(None)`.
    ///
    /// Effect asserted on the tree, not on the reply: the poison must NOT have
    /// fired and the member walk must not have run. Move the gate below
    /// `tracker.cancel` and the response is still `not_found` while this test
    /// goes red, which is the whole reason it checks the token.
    #[tokio::test]
    async fn a_caller_who_cannot_see_the_team_cannot_touch_its_fanout() {
        let teams = team_store_with(&[]).await;
        let (run_id, _member_run_id, probe, _regs, _fanout) = live_fanout("team-foreign");
        let adapter = Arc::new(RecordingCancelAdapter::default());
        let (_scratch, ctx) = gateway_context(Arc::clone(&adapter));

        assert_eq!(
            team_of_fanout_run(&run_id).as_deref(),
            Some("team-foreign"),
            "fixture sanity: the run IS attributable, the team is just unreachable"
        );

        let resp = handle_chat_cancel(cancel_req(&run_id), ctx, teams).await;

        let err = resp.error.expect("expected a refusal");
        assert_eq!(err.code, RESOURCE_NOT_FOUND);
        assert!(
            !probe.is_cancelled(),
            "a refused caller must not poison the tree on their way out"
        );
        assert!(
            adapter.log().is_empty(),
            "a refused caller must not reach the member walk"
        );
    }

    /// No existence oracle: "someone else's tree" and "no such tree" must be
    /// byte-identical, or the refusal itself enumerates live fan-outs.
    #[tokio::test]
    async fn a_foreign_run_id_and_an_unknown_one_refuse_identically() {
        let (run_id, _member_run_id, _probe, _regs, _fanout) = live_fanout("team-foreign");
        let unknown = format!("fanout-{}", uuid::Uuid::new_v4());

        let (_foreign_scratch, foreign_ctx) =
            gateway_context(Arc::new(RecordingCancelAdapter::default()));
        let foreign =
            handle_chat_cancel(cancel_req(&run_id), foreign_ctx, team_store_with(&[]).await).await;
        let (_absent_scratch, absent_ctx) =
            gateway_context(Arc::new(RecordingCancelAdapter::default()));
        let absent =
            handle_chat_cancel(cancel_req(&unknown), absent_ctx, team_store_with(&[]).await).await;

        let foreign_bytes =
            serde_json::to_string(&foreign.error.expect("refusal")).expect("serialize");
        let absent_bytes =
            serde_json::to_string(&absent.error.expect("refusal")).expect("serialize");
        assert_eq!(
            foreign_bytes.replace(&run_id, "<ID>"),
            absent_bytes.replace(&unknown, "<ID>"),
            "the two refusals must differ only in the id the caller supplied"
        );
        assert!(
            !foreign_bytes.contains("team-foreign"),
            "the refusal must not name a team the caller cannot see"
        );
    }
}

/// Handler-level tests for the multi-user teamchat P3 author-stamping +
/// activation-gate + observe-mode work (`handle_chat_send`'s new
/// `author`/`observe` machinery in `canvas.rs`).
///
/// Teams here are created (not `insert_team_with_id`'d) so ownership is
/// stamped from the ambient caller — `gate_team` must admit whichever
/// `CALLER_USER` each test scopes the call under, which the unowned literal-id
/// fixture (`super::tests::team_store_with`) cannot provide.
mod chat_send_gate_tests {
    use super::*;

    use async_trait::async_trait;

    use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::context::GatewayContext;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus};
    use crate::gateway::inter_agent_policy::AgentToAgentPolicy;
    use crate::gateway::security::SecurityStore;
    use crate::gateway::session_store::sqlite_backend::{
        SqliteSessionStore, SqliteSessionStoreConfig,
    };
    use crate::gateway::session_store::SessionStore;
    use crate::teams::messages::{MessageStore, NewMessage, SqliteMessageStore};
    use crate::teams::store::SqliteTeamStore;
    use crate::teams::NewTeam;

    /// The spawned fan-out dispatch never resolves any real agent here (the
    /// registry is empty) — this adapter exists only so `GatewayContext`
    /// type-checks; its methods are never expected to be called by the
    /// assertions below (the handler's own JSON-RPC response is asserted
    /// synchronously, before the background spawn has any chance to run on
    /// the single-threaded `#[tokio::test]` runtime).
    struct NoopAdapter;

    #[async_trait]
    impl ExecutionAdapter for NoopAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, _run_id: &str) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
            None
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    fn gateway_context() -> (tempfile::TempDir, Arc<GatewayContext>) {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_store: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::new(SqliteSessionStoreConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .expect("session store"),
        );
        (
            temp,
            Arc::new(GatewayContext::new(
                session_store,
                Arc::new(AgentRegistry::new()),
                Arc::new(NoopAdapter),
                Arc::new(AgentToAgentPolicy::permissive()),
            )),
        )
    }

    /// A team led by "leader", owned by `owner` (stamped from the ambient
    /// caller at creation — `TeamStore::create_team`'s doc). Whichever caller
    /// a test scopes `handle_chat_send` under must equal `owner`, or
    /// `gate_team` 404s before any of the new machinery runs.
    async fn team_owned_by(owner: &str) -> (Arc<dyn crate::teams::TeamStore>, String) {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        let raw_store = SqliteTeamStore::new(conn);
        raw_store.migrate().await.expect("migrate");
        let store: Arc<dyn crate::teams::TeamStore> = Arc::new(raw_store);
        let team = CALLER_USER
            .scope(
                Some(owner.to_string()),
                store.create_team(NewTeam {
                    name: "T".to_string(),
                    description: String::new(),
                    leader_id: "leader".to_string(),
                }),
            )
            .await
            .expect("create team");
        (store, team.id)
    }

    async fn message_store() -> Arc<dyn MessageStore> {
        Arc::new(SqliteMessageStore::new_in_memory().await)
    }

    /// Seed one earlier human message from `author` — the transcript history
    /// `distinct_human_authors` reads back.
    async fn seed_human_message(msg_store: &Arc<dyn MessageStore>, team_id: &str, author: &str) {
        msg_store
            .send_message_with_ttl(
                NewMessage {
                    team_id: team_id.to_string(),
                    from_agent: crate::teams::broadcast::RESERVED_USER_HANDLE.to_string(),
                    msg_type: crate::teams::messages::MessageType::Message,
                    subject: String::new(),
                    content: format!("earlier message from {author}"),
                    recipients: Vec::new(),
                    reply_to: None,
                    attachments: Vec::new(),
                    author_user_id: Some(author.to_string()),
                },
                chrono::Duration::days(3650),
            )
            .await
            .expect("seed earlier human message");
    }

    fn send_req(team_id: &str, message: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.chat.send".to_string(),
            params: Some(json!({ "team_id": team_id, "message": message })),
            id: Some(json!(9)),
        }
    }

    /// Runs `handle_chat_send` with `caller` scoped as `CALLER_USER`, and all
    /// the optional dependencies the observe/auto-name/broadcaster paths
    /// don't need for these tests set to `None`.
    #[allow(clippy::too_many_arguments)]
    async fn send_as(
        caller: &str,
        store: Arc<dyn crate::teams::TeamStore>,
        msg_store: Arc<dyn MessageStore>,
        ctx: Arc<GatewayContext>,
        team_id: &str,
        message: &str,
    ) -> JsonRpcResponse {
        let security = Arc::new(SecurityStore::in_memory().expect("security store"));
        CALLER_USER
            .scope(
                Some(caller.to_string()),
                handle_chat_send(
                    send_req(team_id, message),
                    store,
                    Some(msg_store),
                    ctx,
                    None,
                    None,
                    None,
                    None,
                    crate::teams::broadcast::BroadcastConfig::default(),
                    security,
                ),
            )
            .await
    }

    /// Two distinct humans have spoken (alice earlier, bob now) and bob's
    /// message @-names nobody — spec §6.2's observe mode: persisted +
    /// broadcast, but no run minted.
    #[tokio::test]
    async fn multi_human_without_mention_observes_instead_of_dispatching() {
        let (store, team_id) = team_owned_by("u-bob").await;
        let msg_store = message_store().await;
        seed_human_message(&msg_store, &team_id, "u-alice").await;
        let (_scratch, ctx) = gateway_context();

        let resp = send_as(
            "u-bob",
            Arc::clone(&store),
            Arc::clone(&msg_store),
            ctx,
            &team_id,
            "我觉得可以",
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert!(
            result["run_id"].is_null(),
            "an observed message must not mint a run: {result:?}"
        );
        assert_eq!(result["observed"], serde_json::json!(true));
        assert!(
            result["message_id"].is_string(),
            "message_id must still be present: {result:?}"
        );

        // The new row carries bob's author_user_id.
        let rows = msg_store
            .list_team_messages(&team_id, 10)
            .await
            .expect("list transcript");
        let bobs_row = rows
            .iter()
            .find(|m| m.content == "我觉得可以")
            .expect("bob's message is in the transcript");
        assert_eq!(bobs_row.author_user_id, Some("u-bob".to_string()));

        // No fan-out tree was registered for this send: the response's
        // `run_id` is null, so there is no id under which one could exist —
        // `register_fanout` is the only mint point and this handler never
        // reached it for the observe branch.
    }

    /// A single human (alice, both times) has ever spoken in this team —
    /// activation must stay byte-identical to the pre-P3 behavior: every
    /// message activates (leader fallback, no @ needed).
    #[tokio::test]
    async fn single_human_path_is_byte_identical_to_before() {
        let (store, team_id) = team_owned_by("u-alice").await;
        let msg_store = message_store().await;
        seed_human_message(&msg_store, &team_id, "u-alice").await;
        let (_scratch, ctx) = gateway_context();

        let resp = send_as(
            "u-alice",
            Arc::clone(&store),
            Arc::clone(&msg_store),
            ctx,
            &team_id,
            "随便聊聊",
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert!(
            result["run_id"].is_string(),
            "a single-human thread must still activate (leader fallback): {result:?}"
        );
        assert_eq!(result["observed"], serde_json::json!(false));
        assert!(result["message_id"].is_string());
    }

    /// Two distinct humans (alice earlier, bob now), and bob's message
    /// explicitly @-names a roster member — activation still fires even
    /// though a second human is in the thread.
    #[tokio::test]
    async fn multi_human_with_mention_still_dispatches() {
        let (store, team_id) = team_owned_by("u-bob").await;
        store
            .add_member(crate::teams::NewTeamMember::for_agent(
                team_id.clone(),
                "coder",
                "member",
            ))
            .await
            .expect("add coder to roster");
        let msg_store = message_store().await;
        seed_human_message(&msg_store, &team_id, "u-alice").await;
        let (_scratch, ctx) = gateway_context();

        let resp = send_as(
            "u-bob",
            Arc::clone(&store),
            Arc::clone(&msg_store),
            ctx,
            &team_id,
            "@coder 实现它",
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert!(
            result["run_id"].is_string(),
            "an explicit @-mention must still activate in a multi-human thread: {result:?}"
        );
        assert_eq!(result["observed"], serde_json::json!(false));
    }
}

mod template_handler_tests {
    use super::*;

    #[tokio::test]
    async fn list_templates_returns_builtins() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "teams.list_templates".into(),
            params: None,
            id: Some(serde_json::Value::Number(1.into())),
        };
        let resp = handle_list_templates(req).await;
        assert!(resp.error.is_none(), "expected ok, got {:?}", resp.error);
        let entries = resp.result.expect("result")["templates"].clone();
        let names: Vec<String> = entries
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|v| v["name"].as_str().map(String::from))
            .collect();
        assert!(names.contains(&"software-dev".to_string()));
        assert!(names.contains(&"code-review".to_string()));
        assert!(names.contains(&"research-paper".to_string()));
        assert!(names.contains(&"strategy-room".to_string()));
    }
}
