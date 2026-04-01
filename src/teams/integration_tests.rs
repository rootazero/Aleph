//! End-to-end integration tests for the three-layer team communication system.
//!
//! These tests exercise the full Explorer -> Critic -> Escalation flow using
//! real SQLite-backed stores (in-memory) for full integration coverage.

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
use crate::teams::roles::review::*;
use crate::teams::roles::types::*;
use crate::teams::sessions::store::*;
use crate::teams::sessions::types::*;
use crate::teams::store::{SqliteTeamStore, TeamStore};
use crate::teams::types::*;

// ---------------------------------------------------------------------------
// Test 1: Explorer-Critic Review Cycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_explorer_critic_review_cycle() {
    // --- Setup ---
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

    let team_id = "team-review";
    let task_id = "task-explore-1";
    let explorer_id = "explorer-1";
    let critic_id = "critic-1";

    // --- Step 1: Explorer submits Discovery artifact ---
    let discovery_v1 = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: explorer_id.into(),
            artifact_type: ArtifactType::Discovery,
            title: "Initial discovery: cache optimization".into(),
            content: "We can improve cache hit rates by 30% with LRU eviction.".into(),
            metadata: json!({"version": 1}),
        })
        .await
        .unwrap();

    assert_eq!(discovery_v1.artifact_type, ArtifactType::Discovery);

    // --- Step 2: System sends review request to Critic (Layer 1 auto-notification) ---
    let review_req = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: "system".into(),
            to: vec![critic_id.into()],
            cc: vec![],
            msg_type: MessageType::ReviewRequest,
            subject: "Review discovery: cache optimization".into(),
            content: format!("Please review artifact {}", discovery_v1.id),
            reply_to: None,
            attachments: vec![discovery_v1.id.clone()],
        })
        .await
        .unwrap();

    assert_eq!(review_req.msg_type, MessageType::ReviewRequest);
    assert_eq!(review_req.attachments.len(), 1);

    // --- Step 3: Critic reads inbox and finds review request ---
    let critic_inbox = inbox
        .read(critic_id, team_id, Some(&MessageType::ReviewRequest), true)
        .await
        .unwrap();

    assert_eq!(critic_inbox.len(), 1);
    assert_eq!(critic_inbox[0].msg_type, MessageType::ReviewRequest);
    assert_eq!(critic_inbox[0].attachments[0], discovery_v1.id);

    // --- Step 4: Critic reviews -- REJECT ---
    let config = TeamRoleConfig {
        role: AgentRole::Critic,
        prompt_template: String::new(),
        review_dimensions: vec![
            "correctness".into(),
            "completeness".into(),
            "feasibility".into(),
        ],
        min_score_threshold: 7,
        min_challenges: 3,
    };

    let review_v1 = ReviewScore {
        task_id: task_id.into(),
        artifact_id: discovery_v1.id.clone(),
        scores: vec![
            DimensionScore {
                dimension: "correctness".into(),
                score: 5,
                rationale: "Claims lack supporting data".into(),
            },
            DimensionScore {
                dimension: "completeness".into(),
                score: 4,
                rationale: "Missing edge cases".into(),
            },
            DimensionScore {
                dimension: "feasibility".into(),
                score: 6,
                rationale: "Feasible but risky".into(),
            },
        ],
        overall_pass: false,
        challenges: vec![
            Challenge {
                point: "No benchmark data provided".into(),
                severity: Severity::Critical,
                evidence: "The 30% improvement claim has no supporting measurements".into(),
            },
            Challenge {
                point: "LRU may not be optimal for our access patterns".into(),
                severity: Severity::Major,
                evidence: "Our workload is scan-heavy, not locality-heavy".into(),
            },
            Challenge {
                point: "Missing memory overhead analysis".into(),
                severity: Severity::Major,
                evidence: "No discussion of memory cost of maintaining LRU metadata".into(),
            },
        ],
        improvement_suggestions: vec!["Add benchmarks".into(), "Consider ARC or 2Q".into()],
        risks_if_accepted: vec!["Performance regression".into()],
    };

    // Validate: failing review with 3 challenges should be valid
    assert!(review_v1.validate(&config).is_ok());

    // Save review as artifact
    let review_artifact_v1 = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: critic_id.into(),
            artifact_type: ArtifactType::Review,
            title: "Review of cache optimization discovery (v1)".into(),
            content: serde_json::to_string(&review_v1).unwrap(),
            metadata: json!({"overall_pass": false, "version": 1}),
        })
        .await
        .unwrap();

    // --- Step 5: Challenge sent to Explorer ---
    let challenge_msg = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: critic_id.into(),
            to: vec![explorer_id.into()],
            cc: vec![],
            msg_type: MessageType::Challenge,
            subject: "Challenges to cache optimization discovery".into(),
            content: "Your discovery needs revision. See review artifact for details.".into(),
            reply_to: Some(review_req.id.clone()),
            attachments: vec![review_artifact_v1.id.clone()],
        })
        .await
        .unwrap();

    assert_eq!(challenge_msg.msg_type, MessageType::Challenge);

    // --- Step 6: Explorer reads challenges ---
    let explorer_inbox = inbox
        .read(explorer_id, team_id, Some(&MessageType::Challenge), true)
        .await
        .unwrap();

    assert_eq!(explorer_inbox.len(), 1);
    assert!(explorer_inbox[0].content.contains("revision"));

    // --- Step 7: Explorer revises and submits updated Discovery ---
    let discovery_v2 = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: explorer_id.into(),
            artifact_type: ArtifactType::Discovery,
            title: "Revised discovery: cache optimization with ARC".into(),
            content: "After benchmarking, ARC eviction improves hit rates by 25% with \
                      only 8KB additional memory overhead per cache instance."
                .into(),
            metadata: json!({"version": 2, "parent_artifact": discovery_v1.id}),
        })
        .await
        .unwrap();

    // --- Step 8: Critic reviews again -- PASS ---
    let review_v2 = ReviewScore {
        task_id: task_id.into(),
        artifact_id: discovery_v2.id.clone(),
        scores: vec![
            DimensionScore {
                dimension: "correctness".into(),
                score: 8,
                rationale: "Claims now backed by benchmark data".into(),
            },
            DimensionScore {
                dimension: "completeness".into(),
                score: 7,
                rationale: "Edge cases addressed, memory overhead documented".into(),
            },
            DimensionScore {
                dimension: "feasibility".into(),
                score: 9,
                rationale: "ARC is well-suited for scan-heavy workloads".into(),
            },
        ],
        overall_pass: true,
        challenges: vec![
            Challenge {
                point: "Benchmark only covers read-heavy scenario".into(),
                severity: Severity::Minor,
                evidence: "Write-heavy benchmarks not included".into(),
            },
            Challenge {
                point: "ARC implementation complexity".into(),
                severity: Severity::Minor,
                evidence: "ARC is more complex than LRU to implement".into(),
            },
            Challenge {
                point: "Memory overhead under high concurrency".into(),
                severity: Severity::Minor,
                evidence: "8KB is per-instance; need to verify at scale".into(),
            },
        ],
        improvement_suggestions: vec!["Add write-heavy benchmarks".into()],
        risks_if_accepted: vec![],
    };

    // Validate: passing review with all scores >= 7 and 3 challenges should be valid
    assert!(review_v2.validate(&config).is_ok());

    // Save passing review as artifact
    let _review_artifact_v2 = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: critic_id.into(),
            artifact_type: ArtifactType::Review,
            title: "Review of cache optimization discovery (v2)".into(),
            content: serde_json::to_string(&review_v2).unwrap(),
            metadata: json!({"overall_pass": true, "version": 2}),
        })
        .await
        .unwrap();

    // --- Step 9: Verify all artifacts ---
    let all_artifacts = artifact_store
        .get_artifacts_for_task(task_id)
        .await
        .unwrap();

    // 2 discoveries + 2 reviews = 4 artifacts
    assert_eq!(all_artifacts.len(), 4);

    let discoveries: Vec<_> = all_artifacts
        .iter()
        .filter(|a| a.artifact_type == ArtifactType::Discovery)
        .collect();
    assert_eq!(discoveries.len(), 2);

    let reviews: Vec<_> = all_artifacts
        .iter()
        .filter(|a| a.artifact_type == ArtifactType::Review)
        .collect();
    assert_eq!(reviews.len(), 2);

    // Verify events were logged (MessageSent + MessageRead events)
    let events = event_store.get_events(team_id, None, None).await.unwrap();
    assert!(
        events.len() >= 2,
        "Expected at least 2 events, got {}",
        events.len()
    );

    let sent_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageSent)
        .collect();
    assert!(
        sent_events.len() >= 2,
        "Expected at least 2 MessageSent events"
    );

    let read_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageRead)
        .collect();
    assert!(
        read_events.len() >= 2,
        "Expected at least 2 MessageRead events"
    );
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
        review_reject_threshold: 3,
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
            msg_type: MessageType::Discovery,
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
            msg_type: MessageType::Challenge,
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
    let team_id_str;

    // Create a team and add the agent as member
    let team = team_store
        .create_team(NewTeam {
            name: "Context Test Team".into(),
            description: "For testing context injection".into(),
            leader_id: "some-leader".into(),
        })
        .await
        .unwrap();
    team_id_str = team.id.clone();

    team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: agent_id.into(),
            role: "explorer".into(),
        })
        .await
        .unwrap();

    // Send messages to the agent
    msg_store
        .send_message(NewMessage {
            team_id: team.id.clone(),
            from_agent: "sender-1".into(),
            msg_type: MessageType::ReviewRequest,
            subject: "Please review".into(),
            content: "Review this artifact".into(),
            recipients: vec![Recipient {
                agent_id: agent_id.into(),
                role: RecipientRole::To,
            }],
            reply_to: None,
            attachments: vec![],
        })
        .await
        .unwrap();

    msg_store
        .send_message(NewMessage {
            team_id: team.id.clone(),
            from_agent: "sender-2".into(),
            msg_type: MessageType::Discovery,
            subject: "FYI: new finding".into(),
            content: "Just a heads up".into(),
            recipients: vec![Recipient {
                agent_id: agent_id.into(),
                role: RecipientRole::Cc,
            }],
            reply_to: None,
            attachments: vec![],
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
// Test 4: Review Score Validation Flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_review_score_validation_flow() {
    let config = TeamRoleConfig {
        role: AgentRole::Critic,
        prompt_template: String::new(),
        review_dimensions: vec!["correctness".into(), "completeness".into()],
        min_score_threshold: 7,
        min_challenges: 3,
    };

    let make_challenge = |point: &str| Challenge {
        point: point.into(),
        severity: Severity::Major,
        evidence: "evidence".into(),
    };

    let make_score = |dim: &str, score: u8| DimensionScore {
        dimension: dim.into(),
        score,
        rationale: "rationale".into(),
    };

    // --- Case 1: Reject with only 2 challenges (need 3) ---
    let review_2_challenges = ReviewScore {
        task_id: "task-1".into(),
        artifact_id: "art-1".into(),
        scores: vec![make_score("correctness", 8), make_score("completeness", 8)],
        overall_pass: true,
        challenges: vec![make_challenge("challenge 1"), make_challenge("challenge 2")],
        improvement_suggestions: vec![],
        risks_if_accepted: vec![],
    };

    let result = review_2_challenges.validate(&config);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors.iter().any(|e| e.contains("Minimum 3 challenges")));

    // --- Case 2: Reject pass=true with score 5 (below threshold 7) ---
    let review_low_score = ReviewScore {
        task_id: "task-1".into(),
        artifact_id: "art-1".into(),
        scores: vec![make_score("correctness", 5), make_score("completeness", 8)],
        overall_pass: true,
        challenges: vec![
            make_challenge("c1"),
            make_challenge("c2"),
            make_challenge("c3"),
        ],
        improvement_suggestions: vec![],
        risks_if_accepted: vec![],
    };

    let result = review_low_score.validate(&config);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors
        .iter()
        .any(|e| e.contains("correctness") && e.contains("5/10")));

    // --- Case 3: Valid passing review (3 challenges, all scores >= 7) ---
    let review_valid_pass = ReviewScore {
        task_id: "task-1".into(),
        artifact_id: "art-1".into(),
        scores: vec![make_score("correctness", 8), make_score("completeness", 9)],
        overall_pass: true,
        challenges: vec![
            make_challenge("c1"),
            make_challenge("c2"),
            make_challenge("c3"),
        ],
        improvement_suggestions: vec![],
        risks_if_accepted: vec![],
    };

    assert!(review_valid_pass.validate(&config).is_ok());

    // --- Case 4: Valid failing review (3 challenges, low scores, pass=false) ---
    let review_valid_fail = ReviewScore {
        task_id: "task-1".into(),
        artifact_id: "art-1".into(),
        scores: vec![make_score("correctness", 3), make_score("completeness", 2)],
        overall_pass: false,
        challenges: vec![
            make_challenge("major flaw 1"),
            make_challenge("major flaw 2"),
            make_challenge("major flaw 3"),
        ],
        improvement_suggestions: vec!["Rewrite everything".into()],
        risks_if_accepted: vec!["System failure".into()],
    };

    assert!(
        review_valid_fail.validate(&config).is_ok(),
        "Failing review with low scores and sufficient challenges should be valid"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Team Disband Cleanup
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
