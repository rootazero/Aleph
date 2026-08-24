//! End-to-end integration tests for the team communication system.
//!
//! These tests exercise artifact creation, message routing, escalation,
//! context injection, and team cleanup using real SQLite-backed stores
//! (in-memory) for full integration coverage.

use chrono::Utc;
use serde_json::json;

use crate::sync_primitives::Arc;
use crate::teams::artifacts::*;
use crate::teams::context::*;
use crate::teams::events::*;
use crate::teams::messages::inbox::*;
use crate::teams::messages::router::*;
use crate::teams::messages::store::*;
use crate::teams::messages::types::*;
use crate::teams::sessions::store::*;
use crate::teams::sessions::types::*;
use crate::teams::store::{SqliteTeamStore, TeamStore};
use crate::teams::types::*;

// ---------------------------------------------------------------------------
// Test 1: Two-Agent Collaboration (artifact + message routing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_two_agent_collaboration() {
    let artifact_store = Arc::new(SqliteArtifactStore::new_in_memory().await);
    let msg_store: Arc<SqliteMessageStore> = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store: Arc<SqliteEventLogStore> =
        Arc::new(SqliteEventLogStore::new_in_memory().await);

    let router = MessageRouter::new(
        msg_store.clone(),
        event_store.clone(),
        EscalationRule::default(),
        None,
    );
    let inbox = Inbox::new(
        msg_store.clone() as Arc<dyn MessageStore>,
        event_store.clone(),
    );

    let team_id = "team-collab";
    let task_id = "task-1";
    let agent_a = "agent-a";
    let agent_b = "agent-b";

    // Agent A submits a report artifact
    let report = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: agent_a.into(),
            artifact_type: ArtifactType::Report,
            title: "Initial analysis".into(),
            content: "Cache optimization proposal.".into(),
            metadata: json!({"version": 1}),
            status: TaskStatus::Pending,
            blocked_by: vec![],
            assignee: None,
            priority: 0,
        })
        .await
        .unwrap();

    assert_eq!(report.artifact_type, ArtifactType::Report);

    // Agent A sends message to Agent B
    let msg1 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: agent_a.into(),
            to: vec![agent_b.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Please review".into(),
            content: format!("Please review artifact {}", report.id),
            reply_to: None,
            attachments: vec![report.id.clone()],
        })
        .await
        .unwrap();

    // Agent B reads inbox
    let b_inbox = inbox.read(agent_b, team_id, None, true).await.unwrap();
    assert_eq!(b_inbox.len(), 1);
    assert_eq!(b_inbox[0].attachments[0], report.id);

    // Agent B replies
    let _msg2 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: agent_b.into(),
            to: vec![agent_a.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Re: review".into(),
            content: "Needs benchmarks.".into(),
            reply_to: Some(msg1.id.clone()),
            attachments: vec![],
        })
        .await
        .unwrap();

    // Verify events
    let events = event_store.get_events(team_id, None, None).await.unwrap();
    let sent_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageSent)
        .collect();
    assert!(sent_events.len() >= 2);

    let read_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageRead)
        .collect();
    assert!(!read_events.is_empty());
}

// ---------------------------------------------------------------------------
// Test 2: Escalation to Collaborative Session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_escalation_to_collaborative_session() {
    // --- Setup ---
    let msg_store: Arc<SqliteMessageStore> = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store: Arc<SqliteEventLogStore> =
        Arc::new(SqliteEventLogStore::new_in_memory().await);
    let session_store = Arc::new(SqliteSessionStore::new_in_memory().await);

    let team_id = "team-escalate";
    let leader_id = "leader-1";
    let explorer_id = "explorer-1";
    let critic_id = "critic-1";

    let rules = EscalationRule {
        thread_message_threshold: 3,
        enabled: true,
    };

    let router = MessageRouter::new(
        msg_store.clone(),
        event_store.clone(),
        rules,
        Some(leader_id.into()),
    );

    // --- Step 1: Explorer sends first message (starts thread) ---
    let msg1 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: explorer_id.into(),
            to: vec![critic_id.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Cache optimization proposal".into(),
            content: "I propose we use LRU caching.".into(),
            reply_to: None,
            attachments: vec![],
        })
        .await
        .unwrap();

    let thread_id = msg1.thread_id.clone().unwrap();

    // --- Step 2: Back-and-forth replies ---
    let msg2 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: critic_id.into(),
            to: vec![explorer_id.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Re: Cache optimization proposal".into(),
            content: "LRU is not suitable for our workload.".into(),
            reply_to: Some(msg1.id.clone()),
            attachments: vec![],
        })
        .await
        .unwrap();

    // Third message exceeds threshold (3)
    let _msg3 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: explorer_id.into(),
            to: vec![critic_id.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Re: Cache optimization proposal".into(),
            content: "What about ARC instead?".into(),
            reply_to: Some(msg2.id.clone()),
            attachments: vec![],
        })
        .await
        .unwrap();

    // --- Step 3: Verify escalation notification to leader ---
    let leader_inbox = msg_store
        .read_inbox(leader_id, team_id, Some(&MessageType::SystemNotification))
        .await
        .unwrap();

    assert_eq!(leader_inbox.len(), 1);
    assert_eq!(leader_inbox[0].msg_type, MessageType::SystemNotification);
    assert!(leader_inbox[0].content.contains(&thread_id));
    assert!(leader_inbox[0].content.contains("collaborative session"));

    // --- Step 4: Leader starts a collaborative session ---
    let session = session_store
        .create_session(NewSession {
            team_id: team_id.into(),
            participants: vec![leader_id.into(), explorer_id.into(), critic_id.into()],
            topic: "Resolve cache strategy disagreement".into(),
            trigger: SessionTrigger::AutoEscalation {
                thread_id: thread_id.clone(),
                message_count: 3,
                rule: "thread_message_threshold".into(),
            },
            thread_id: Some(thread_id.clone()),
            max_rounds: 10,
        })
        .await
        .unwrap();

    assert_eq!(session.status, SessionStatus::Active);
    assert_eq!(session.participants.len(), 3);

    // Log session started event
    event_store
        .log_event(NewTeamEvent {
            team_id: team_id.into(),
            event_type: TeamEventType::SessionStarted,
            agent_id: leader_id.into(),
            payload: json!({"session_id": session.id, "topic": "cache strategy"}),
        })
        .await
        .unwrap();

    // --- Step 5: Participants exchange turns ---
    session_store
        .add_turn(
            &session.id,
            SessionTurn {
                agent_id: leader_id.into(),
                content: "Let's discuss the trade-offs between LRU and ARC.".into(),
                turn_number: 1,
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();

    session_store
        .add_turn(
            &session.id,
            SessionTurn {
                agent_id: explorer_id.into(),
                content: "ARC handles scan-heavy workloads better, with 25% improvement.".into(),
                turn_number: 2,
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();

    session_store
        .add_turn(
            &session.id,
            SessionTurn {
                agent_id: critic_id.into(),
                content: "I agree ARC is better. The benchmarks are convincing.".into(),
                turn_number: 3,
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();

    // --- Step 6: Session concludes ---
    let outcome = SessionOutcome {
        conclusion: "Team agrees to use ARC caching strategy.".into(),
        agreed_by: vec![leader_id.into(), explorer_id.into(), critic_id.into()],
        dissent: None,
    };

    session_store
        .conclude_session(&session.id, outcome)
        .await
        .unwrap();

    // Log session concluded event
    event_store
        .log_event(NewTeamEvent {
            team_id: team_id.into(),
            event_type: TeamEventType::SessionConcluded,
            agent_id: leader_id.into(),
            payload: json!({"session_id": session.id}),
        })
        .await
        .unwrap();

    // --- Step 7: Verify ---
    let fetched = session_store
        .get_session(&session.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.status, SessionStatus::Concluded);
    assert_eq!(fetched.transcript.len(), 3);
    assert_eq!(fetched.transcript[0].agent_id, leader_id);
    assert_eq!(fetched.transcript[1].agent_id, explorer_id);
    assert_eq!(fetched.transcript[2].agent_id, critic_id);

    let out = fetched.outcome.unwrap();
    assert_eq!(out.conclusion, "Team agrees to use ARC caching strategy.");
    assert_eq!(out.agreed_by.len(), 3);
    assert!(out.dissent.is_none());

    // Verify events include SessionStarted and SessionConcluded
    let events = event_store.get_events(team_id, None, None).await.unwrap();

    let session_events: Vec<_> = events
        .iter()
        .filter(|e| {
            e.event_type == TeamEventType::SessionStarted
                || e.event_type == TeamEventType::SessionConcluded
        })
        .collect();
    assert_eq!(session_events.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 3: Context Injection Shows Inbox Summary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_context_injection_shows_inbox_summary() {
    // --- Setup: create stores and a team ---
    let msg_store: Arc<SqliteMessageStore> = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store: Arc<SqliteEventLogStore> =
        Arc::new(SqliteEventLogStore::new_in_memory().await);
    let session_store: Arc<SqliteSessionStore> =
        Arc::new(SqliteSessionStore::new_in_memory().await);

    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
    let team_store = Arc::new(SqliteTeamStore::new(conn));
    team_store.migrate().await.unwrap();

    let agent_id = "agent-ctx-test";

    // Create a team and add the agent as member
    let team = team_store
        .create_team(NewTeam {
            name: "Context Test Team".into(),
            description: "For testing context injection".into(),
            leader_id: "some-leader".into(),
        })
        .await
        .unwrap();
    let team_id_str = team.id.clone();

    team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: agent_id.into(),
            role: "explorer".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Send messages to the agent
    msg_store
        .send_message(NewMessage {
            team_id: team.id.clone(),
            from_agent: "sender-1".into(),
            msg_type: MessageType::Message,
            subject: "Please review".into(),
            content: "Review this artifact".into(),
            recipients: vec![Recipient {
                agent_id: agent_id.into(),
                role: RecipientRole::To,
            }],
            reply_to: None,
            attachments: vec![],
            author_user_id: None,
        })
        .await
        .unwrap();

    msg_store
        .send_message(NewMessage {
            team_id: team.id.clone(),
            from_agent: "sender-2".into(),
            msg_type: MessageType::Message,
            subject: "FYI: new finding".into(),
            content: "Just a heads up".into(),
            recipients: vec![Recipient {
                agent_id: agent_id.into(),
                role: RecipientRole::Cc,
            }],
            reply_to: None,
            attachments: vec![],
            author_user_id: None,
        })
        .await
        .unwrap();

    // Create an active session the agent participates in
    let session = session_store
        .create_session(NewSession {
            team_id: team.id.clone(),
            participants: vec![agent_id.into(), "other-agent".into()],
            topic: "Active discussion".into(),
            trigger: SessionTrigger::Explicit {
                requested_by: "some-leader".into(),
            },
            thread_id: None,
            max_rounds: 10,
        })
        .await
        .unwrap();

    // --- Build the context provider ---
    let inbox = Arc::new(Inbox::new(
        msg_store.clone() as Arc<dyn MessageStore>,
        event_store.clone() as Arc<dyn EventLogStore>,
    ));

    let provider = TeamInboxContextProvider::new(inbox, team_store.clone() as Arc<dyn TeamStore>)
        .with_session_store(session_store.clone() as Arc<dyn SessionStore>);

    // --- Get inbox context ---
    let ctx = provider.get_inbox_context(agent_id).await;

    assert_eq!(ctx.unread_to, 1, "Expected 1 unread To message");
    assert_eq!(ctx.unread_cc, 1, "Expected 1 unread Cc message");
    assert_eq!(ctx.active_sessions.len(), 1);
    assert_eq!(ctx.active_sessions[0], session.id);

    // --- Verify injection text format ---
    let text = ctx.to_injection_text().unwrap();
    assert!(
        text.contains("1 unread messages requiring your action"),
        "Missing To count in: {text}"
    );
    assert!(
        text.contains("1 informational messages (cc)"),
        "Missing Cc count in: {text}"
    );
    assert!(
        text.contains("[Team Inbox]"),
        "Missing [Team Inbox] header in: {text}"
    );
    assert!(
        text.contains("inbox_read"),
        "Missing inbox_read instruction in: {text}"
    );
    assert!(
        text.contains("[Active Sessions]"),
        "Missing [Active Sessions] in: {text}"
    );
    assert!(text.contains(&session.id), "Missing session ID in: {text}");

    // --- Verify empty context for unknown agent ---
    let empty_ctx = provider.get_inbox_context("nonexistent-agent").await;
    assert_eq!(empty_ctx.unread_to, 0);
    assert_eq!(empty_ctx.unread_cc, 0);
    assert!(empty_ctx.to_injection_text().is_none());

    // Suppress unused variable warning
    let _ = team_id_str;
}

// ---------------------------------------------------------------------------
// Test 4: Team Disband Cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_team_disband_cleanup() {
    // --- Setup ---
    let msg_store: Arc<SqliteMessageStore> = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store: Arc<SqliteEventLogStore> =
        Arc::new(SqliteEventLogStore::new_in_memory().await);
    let session_store = Arc::new(SqliteSessionStore::new_in_memory().await);

    let team_id = "team-disband";

    // Send some messages
    msg_store
        .send_message(NewMessage {
            team_id: team_id.into(),
            from_agent: "agent-a".into(),
            msg_type: MessageType::Message,
            subject: "Hello".into(),
            content: "Test message 1".into(),
            recipients: vec![Recipient {
                agent_id: "agent-b".into(),
                role: RecipientRole::To,
            }],
            reply_to: None,
            attachments: vec![],
            author_user_id: None,
        })
        .await
        .unwrap();

    msg_store
        .send_message(NewMessage {
            team_id: team_id.into(),
            from_agent: "agent-b".into(),
            msg_type: MessageType::Message,
            subject: "Reply".into(),
            content: "Test message 2".into(),
            recipients: vec![Recipient {
                agent_id: "agent-a".into(),
                role: RecipientRole::To,
            }],
            reply_to: None,
            attachments: vec![],
            author_user_id: None,
        })
        .await
        .unwrap();

    // Create an active session
    let session = session_store
        .create_session(NewSession {
            team_id: team_id.into(),
            participants: vec!["agent-a".into(), "agent-b".into()],
            topic: "Active discussion".into(),
            trigger: SessionTrigger::Explicit {
                requested_by: "agent-a".into(),
            },
            thread_id: None,
            max_rounds: 10,
        })
        .await
        .unwrap();

    // Add a turn to make session more realistic
    session_store
        .add_turn(
            &session.id,
            SessionTurn {
                agent_id: "agent-a".into(),
                content: "Starting discussion".into(),
                turn_number: 1,
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();

    // Log some events
    event_store
        .log_event(NewTeamEvent {
            team_id: team_id.into(),
            event_type: TeamEventType::MessageSent,
            agent_id: "agent-a".into(),
            payload: json!({}),
        })
        .await
        .unwrap();

    event_store
        .log_event(NewTeamEvent {
            team_id: team_id.into(),
            event_type: TeamEventType::SessionStarted,
            agent_id: "agent-a".into(),
            payload: json!({}),
        })
        .await
        .unwrap();

    // Verify state before cleanup
    let inbox_b = msg_store
        .read_inbox("agent-b", team_id, None)
        .await
        .unwrap();
    assert_eq!(inbox_b.len(), 1, "agent-b should have 1 unread message");

    let active_sessions = session_store.list_active_sessions(team_id).await.unwrap();
    assert_eq!(active_sessions.len(), 1);

    let events_before = event_store.get_events(team_id, None, None).await.unwrap();
    assert_eq!(events_before.len(), 2);

    // --- Cleanup: Expire messages ---
    let expired = msg_store.expire_all_for_team(team_id).await.unwrap();
    assert_eq!(expired, 2);

    // --- Cleanup: Cancel sessions ---
    let cancelled = session_store.cancel_all_for_team(team_id).await.unwrap();
    assert_eq!(cancelled, 1);

    // --- Cleanup: Prune events (with zero duration = prune everything) ---
    let pruned = event_store
        .prune_events(team_id, chrono::Duration::zero())
        .await
        .unwrap();
    assert_eq!(pruned, 2);

    // --- Verify: No unread messages ---
    let inbox_b_after = msg_store
        .read_inbox("agent-b", team_id, None)
        .await
        .unwrap();
    assert!(
        inbox_b_after.is_empty(),
        "Inbox should be empty after expiration"
    );

    let inbox_a_after = msg_store
        .read_inbox("agent-a", team_id, None)
        .await
        .unwrap();
    assert!(
        inbox_a_after.is_empty(),
        "Inbox should be empty after expiration"
    );

    // --- Verify: No active sessions ---
    let active_after = session_store.list_active_sessions(team_id).await.unwrap();
    assert!(active_after.is_empty(), "No active sessions should remain");

    // Verify the session is now cancelled
    let fetched_session = session_store
        .get_session(&session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched_session.status, SessionStatus::Cancelled);

    // --- Verify: Events pruned ---
    let events_after = event_store.get_events(team_id, None, None).await.unwrap();
    assert!(events_after.is_empty(), "All events should be pruned");
}

// ---------------------------------------------------------------------------
// Test 5: Broadcast Pattern (query members → filter sender → send to all)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_broadcast_pattern() {
    // Test the broadcast pattern: query members, filter sender, send to all others
    let msg_store: Arc<SqliteMessageStore> = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store: Arc<SqliteEventLogStore> =
        Arc::new(SqliteEventLogStore::new_in_memory().await);
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
    let team_store = Arc::new(SqliteTeamStore::new(conn));
    team_store.migrate().await.unwrap();

    let router = MessageRouter::new(
        msg_store.clone(),
        event_store.clone(),
        EscalationRule::default(),
        None,
    );

    // Create team with 3 members
    let team = team_store
        .create_team(NewTeam {
            name: "Broadcast Test".into(),
            description: "".into(),
            leader_id: "leader".into(),
        })
        .await
        .unwrap();

    team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "worker-1".into(),
            role: "worker".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "worker-2".into(),
            role: "worker".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Simulate broadcast: get members, filter sender, send to all
    let members = team_store.get_members(&team.id).await.unwrap();
    let sender = "leader";
    let to: Vec<String> = members
        .into_iter()
        .map(|m| m.agent_id)
        .filter(|id| id != sender)
        .collect();

    assert_eq!(to.len(), 2);
    assert!(to.contains(&"worker-1".to_string()));
    assert!(to.contains(&"worker-2".to_string()));

    // Send to all filtered members
    let _msg = router
        .send(SendRequest {
            team_id: team.id.clone(),
            from_agent: sender.into(),
            to,
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Team announcement".into(),
            content: "All hands meeting tomorrow".into(),
            reply_to: None,
            attachments: vec![],
        })
        .await
        .unwrap();

    // Both workers should have the message
    let w1_inbox = msg_store
        .read_inbox("worker-1", &team.id, None)
        .await
        .unwrap();
    assert_eq!(w1_inbox.len(), 1);
    assert_eq!(w1_inbox[0].subject, "Team announcement");

    let w2_inbox = msg_store
        .read_inbox("worker-2", &team.id, None)
        .await
        .unwrap();
    assert_eq!(w2_inbox.len(), 1);

    // Leader should NOT have the message (was filtered out)
    let leader_inbox = msg_store
        .read_inbox("leader", &team.id, None)
        .await
        .unwrap();
    assert!(leader_inbox.is_empty());
}
