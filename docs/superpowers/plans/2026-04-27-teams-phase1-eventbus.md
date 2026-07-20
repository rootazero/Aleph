# Teams Phase 1: EventBus Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform teams module from manual audit logging to event-driven architecture using Aleph's existing EventBus infrastructure.

**Architecture:** Extend `AlephEvent` enum with Team variants, create `TeamEventLogger` as an `EventHandler` that auto-persists to SQLite, and migrate all `log_event` call sites to `bus.publish`. Fix N+1 query in message store.

**Tech Stack:** Rust, tokio, rusqlite, serde_json, async-trait

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/event/types.rs` | Modify | Add Team* event variants and data structs |
| `src/teams/events.rs` | Modify | Add `TeamEventLogger` EventHandler impl |
| `src/teams/messages/store.rs` | Modify | Fix N+1 query with JOIN |
| `src/teams/messages/router.rs` | Modify | Replace `log_event` with `bus.publish` |
| `src/teams/sessions/coordinator.rs` | Modify | Replace `log_event` with `bus.publish` |
| `src/teams/plans.rs` | Modify | Replace `log_event` with `bus.publish` |
| `src/teams/mod.rs` | Modify | Export new types |
| `src/teams/integration_tests.rs` | Modify | Update tests to verify event publishing |

---

### Task 1: Add Team Event Variants to AlephEvent

**Files:**
- Modify: `src/event/types.rs`
- Test: `src/event/types.rs` (existing test module)

- [ ] **Step 1: Define Team event data structs**

Add after line 156 (after `PartRemoved` variant):

```rust
// ============================================================================
// Team Event Types
// ============================================================================

/// Team message sent event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessageEvent {
    pub team_id: String,
    pub message_id: String,
    pub from_agent: String,
    pub to_agents: Vec<String>,
    pub subject: String,
    pub timestamp: i64,
}

/// Team message read event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessageReadEvent {
    pub team_id: String,
    pub message_id: String,
    pub reader_agent: String,
    pub timestamp: i64,
}

/// Team session lifecycle event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSessionEvent {
    pub team_id: String,
    pub session_id: String,
    pub trigger_agent: String,
    pub outcome: Option<String>,
}

/// Team plan submission/resolution event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamPlanEvent {
    pub team_id: String,
    pub artifact_id: String,
    pub submitter: String,
    pub leader: String,
}

/// Team plan resolved event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamPlanResolvedEvent {
    pub team_id: String,
    pub artifact_id: String,
    pub submitter: String,
    pub leader: String,
    pub approved: bool,
}

/// Team member joined/left event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemberEvent {
    pub team_id: String,
    pub agent_id: String,
    pub role: String,
}

/// Team task unblocked event (for Phase 2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTaskUnblockedEvent {
    pub team_id: String,
    pub task_id: String,
    pub unblocked_by: String,
}
```

- [ ] **Step 2: Extend AlephEvent enum**

Add variants to `AlephEvent` enum:

```rust
pub enum AlephEvent {
    // ... existing variants ...
    
    // Team events
    TeamMessageSent(TeamMessageEvent),
    TeamMessageRead(TeamMessageReadEvent),
    TeamSessionStarted(TeamSessionEvent),
    TeamSessionConcluded(TeamSessionEvent),
    TeamPlanSubmitted(TeamPlanEvent),
    TeamPlanResolved(TeamPlanResolvedEvent),
    TeamMemberAdded(TeamMemberEvent),
    TeamMemberRemoved(TeamMemberEvent),
    TeamTaskUnblocked(TeamTaskUnblockedEvent),
}
```

- [ ] **Step 3: Add EventType variants**

Add to `EventType` enum:

```rust
pub enum EventType {
    // ... existing ...
    TeamMessageSent,
    TeamMessageRead,
    TeamSessionStarted,
    TeamSessionConcluded,
    TeamPlanSubmitted,
    TeamPlanResolved,
    TeamMemberAdded,
    TeamMemberRemoved,
    TeamTaskUnblocked,
}
```

- [ ] **Step 4: Update event_type() match arms**

In `impl AlephEvent`, add:

```rust
pub fn event_type(&self) -> EventType {
    match self {
        // ... existing ...
        Self::TeamMessageSent(_) => EventType::TeamMessageSent,
        Self::TeamMessageRead(_) => EventType::TeamMessageRead,
        Self::TeamSessionStarted(_) => EventType::TeamSessionStarted,
        Self::TeamSessionConcluded(_) => EventType::TeamSessionConcluded,
        Self::TeamPlanSubmitted(_) => EventType::TeamPlanSubmitted,
        Self::TeamPlanResolved(_) => EventType::TeamPlanResolved,
        Self::TeamMemberAdded(_) => EventType::TeamMemberAdded,
        Self::TeamMemberRemoved(_) => EventType::TeamMemberRemoved,
        Self::TeamTaskUnblocked(_) => EventType::TeamTaskUnblocked,
    }
}
```

- [ ] **Step 5: Update name() match arms**

```rust
pub fn name(&self) -> &'static str {
    match self {
        // ... existing ...
        Self::TeamMessageSent(_) => "TeamMessageSent",
        Self::TeamMessageRead(_) => "TeamMessageRead",
        Self::TeamSessionStarted(_) => "TeamSessionStarted",
        Self::TeamSessionConcluded(_) => "TeamSessionConcluded",
        Self::TeamPlanSubmitted(_) => "TeamPlanSubmitted",
        Self::TeamPlanResolved(_) => "TeamPlanResolved",
        Self::TeamMemberAdded(_) => "TeamMemberAdded",
        Self::TeamMemberRemoved(_) => "TeamMemberRemoved",
        Self::TeamTaskUnblocked(_) => "TeamTaskUnblocked",
    }
}
```

- [ ] **Step 6: Write serialization test**

```rust
#[test]
fn test_team_event_serialization() {
    let event = AlephEvent::TeamMessageSent(TeamMessageEvent {
        team_id: "team-1".to_string(),
        message_id: "msg-1".to_string(),
        from_agent: "agent-a".to_string(),
        to_agents: vec!["agent-b".to_string()],
        subject: "Hello".to_string(),
        timestamp: 1000,
    });
    
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("team-1"));
    assert!(json.contains("agent-a"));
    
    let parsed: AlephEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_type(), EventType::TeamMessageSent);
}
```

- [ ] **Step 7: Run test**

```bash
cargo test -p alephcore event::types::tests::test_team_event_serialization --lib
```
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/event/types.rs
git commit -m "event: add Team* event variants for teams module"
```

---

### Task 2: Create TeamEventLogger EventHandler

**Files:**
- Modify: `src/teams/events.rs`
- Test: `src/teams/events.rs` (existing test module)

- [ ] **Step 1: Add EventHandler trait impl**

Add to `src/teams/events.rs` (after existing `SqliteEventLogStore` impl):

```rust
use crate::event::handler::{EventHandler, EventContext, HandlerError};
use crate::event::types::{AlephEvent, EventType};

/// EventHandler that persists team events to SQLite audit log.
pub struct TeamEventLogger {
    store: SqliteEventLogStore,
}

impl TeamEventLogger {
    pub fn new(store: SqliteEventLogStore) -> Self {
        Self { store }
    }
    
    fn convert_to_new_team_event(&self,
        event: &AlephEvent,
    ) -> Option<NewTeamEvent> {
        match event {
            AlephEvent::TeamMessageSent(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::MessageSent,
                agent_id: e.from_agent.clone(),
                payload: serde_json::json!({
                    "message_id": e.message_id,
                    "to_agents": e.to_agents,
                    "subject": e.subject,
                }),
            }),
            AlephEvent::TeamMessageRead(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::MessageRead,
                agent_id: e.reader_agent.clone(),
                payload: serde_json::json!({
                    "message_id": e.message_id,
                }),
            }),
            AlephEvent::TeamSessionStarted(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::SessionStarted,
                agent_id: e.trigger_agent.clone(),
                payload: serde_json::json!({
                    "session_id": e.session_id,
                }),
            }),
            AlephEvent::TeamSessionConcluded(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::SessionConcluded,
                agent_id: e.trigger_agent.clone(),
                payload: serde_json::json!({
                    "session_id": e.session_id,
                    "outcome": e.outcome,
                }),
            }),
            AlephEvent::TeamPlanSubmitted(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::PlanSubmitted,
                agent_id: e.submitter.clone(),
                payload: serde_json::json!({
                    "artifact_id": e.artifact_id,
                    "leader": e.leader,
                }),
            }),
            AlephEvent::TeamPlanResolved(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::PlanResolved,
                agent_id: e.leader.clone(),
                payload: serde_json::json!({
                    "artifact_id": e.artifact_id,
                    "submitter": e.submitter,
                    "approved": e.approved,
                }),
            }),
            AlephEvent::TeamMemberAdded(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::TaskCreated, // Reuse for member events
                agent_id: e.agent_id.clone(),
                payload: serde_json::json!({
                    "role": e.role,
                    "action": "added",
                }),
            }),
            AlephEvent::TeamMemberRemoved(e) => Some(NewTeamEvent {
                team_id: e.team_id.clone(),
                event_type: TeamEventType::TaskFailed, // Reuse for member events
                agent_id: e.agent_id.clone(),
                payload: serde_json::json!({
                    "role": e.role,
                    "action": "removed",
                }),
            }),
            _ => None,
        }
    }
}

#[async_trait]
impl EventHandler for TeamEventLogger {
    fn name(&self) -> &'static str {
        "TeamEventLogger"
    }
    
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamMessageSent,
            EventType::TeamMessageRead,
            EventType::TeamSessionStarted,
            EventType::TeamSessionConcluded,
            EventType::TeamPlanSubmitted,
            EventType::TeamPlanResolved,
            EventType::TeamMemberAdded,
            EventType::TeamMemberRemoved,
            EventType::TeamTaskUnblocked,
        ]
    }
    
    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        if let Some(team_event) = self.convert_to_new_team_event(event) {
            let _ = self.store.log_event(team_event).await
                .map_err(|e| HandlerError::Internal(e.to_string()))?;
        }
        Ok(vec![])
    }
}
```

- [ ] **Step 2: Write integration test**

```rust
#[tokio::test]
async fn test_team_event_logger_persists_events() {
    use crate::event::types::*;
    
    let store = SqliteEventLogStore::new_in_memory().await;
    let logger = TeamEventLogger::new(store);
    let bus = EventBus::new();
    let ctx = EventContext::new(bus.clone());
    
    // Register logger as handler
    let mut registry = EventHandlerRegistry::new();
    registry.register(Arc::new(logger));
    let handles = registry.start(ctx.clone()).await;
    
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    // Publish a team event
    bus.publish(AlephEvent::TeamMessageSent(TeamMessageEvent {
        team_id: "team-1".to_string(),
        message_id: "msg-1".to_string(),
        from_agent: "agent-a".to_string(),
        to_agents: vec!["agent-b".to_string()],
        subject: "Test".to_string(),
        timestamp: 1000,
    })).await;
    
    tokio::time::sleep(Duration::from_millis(50)).await;
    
    // Verify event was logged
    let events = logger.store.get_events("team-1", None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, TeamEventType::MessageSent);
    
    registry.stop(&ctx);
    for h in handles {
        let _ = tokio::time::timeout(Duration::from_millis(100), h).await;
    }
}
```

- [ ] **Step 3: Run test**

```bash
cargo test -p alephcore teams::events::tests::test_team_event_logger_persists_events --lib
```
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/teams/events.rs
git commit -m "teams(events): add TeamEventLogger EventHandler for auto-persistence"
```

---

### Task 3: Fix N+1 Query in MessageStore

**Files:**
- Modify: `src/teams/messages/store.rs`
- Test: `src/teams/messages/store.rs` (existing test module)

- [ ] **Step 1: Add optimized read_inbox method**

Add new method using JOIN:

```rust
pub async fn read_inbox_optimized(
    &self,
    agent_id: &str,
    team_id: &str,
    msg_type: Option<&MessageType>,
) -> Result<Vec<TeamMessage>> {
    let conn = self.conn.lock().await;
    
    let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(mt) = msg_type {
        (
            r#"
            SELECT 
                m.id, m.team_id, m.from_agent, m.msg_type, m.subject, m.content,
                m.thread_id, m.reply_to, m.attachments, m.created_at,
                COALESCE(GROUP_CONCAT(r.agent_id || ':' || r.role), '') as recipients_str
            FROM team_messages m
            LEFT JOIN team_message_recipients r ON m.id = r.message_id
            WHERE m.team_id = ?1
              AND EXISTS (
                  SELECT 1 FROM team_message_recipients r2
                  WHERE r2.message_id = m.id AND r2.agent_id = ?2
              )
              AND m.msg_type = ?3
            GROUP BY m.id
            ORDER BY m.created_at DESC
            "#,
            vec![
                Box::new(team_id.to_owned()),
                Box::new(agent_id.to_owned()),
                Box::new(mt.as_str().to_owned()),
            ],
        )
    } else {
        (
            r#"
            SELECT 
                m.id, m.team_id, m.from_agent, m.msg_type, m.subject, m.content,
                m.thread_id, m.reply_to, m.attachments, m.created_at,
                COALESCE(GROUP_CONCAT(r.agent_id || ':' || r.role), '') as recipients_str
            FROM team_messages m
            LEFT JOIN team_message_recipients r ON m.id = r.message_id
            WHERE m.team_id = ?1
              AND EXISTS (
                  SELECT 1 FROM team_message_recipients r2
                  WHERE r2.message_id = m.id AND r2.agent_id = ?2
              )
            GROUP BY m.id
            ORDER BY m.created_at DESC
            "#,
            vec![
                Box::new(team_id.to_owned()),
                Box::new(agent_id.to_owned()),
            ],
        )
    };
    
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();
    
    let mut stmt = conn.prepare(sql).map_err(db_err)?;
    let messages = stmt
        .query_map(param_refs.as_slice(), |row| {
            let recipients_str: String = row.get(10)?;
            let recipients = if recipients_str.is_empty() {
                vec![]
            } else {
                recipients_str.split(',')
                    .filter_map(|s| {
                        let parts: Vec<&str> = s.split(':').collect();
                        if parts.len() == 2 {
                            Some(Recipient {
                                agent_id: parts[0].to_string(),
                                role: match parts[1] {
                                    "to" => RecipientRole::To,
                                    "cc" => RecipientRole::Cc,
                                    _ => RecipientRole::To,
                                },
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            
            let attachments_str: String = row.get(8)?;
            let attachments = if attachments_str.is_empty() {
                vec![]
            } else {
                attachments_str.split(',').map(String::from).collect()
            };
            
            Ok(TeamMessage {
                id: row.get(0)?,
                team_id: row.get(1)?,
                from_agent: row.get(2)?,
                msg_type: MessageType::from_stored(&row.get::<String>(3)?),
                subject: row.get(4)?,
                content: row.get(5)?,
                thread_id: row.get(6)?,
                reply_to: row.get(7)?,
                attachments,
                recipients,
                created_at: DateTime::parse_from_rfc3339(&row.get::<String>(9)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })
        .map_err(db_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_err)?;
    
    Ok(messages)
}
```

- [ ] **Step 2: Write performance comparison test**

```rust
#[tokio::test]
async fn test_optimized_inbox_matches_original() {
    let store = SqliteMessageStore::new_in_memory().await;
    
    // Seed with 50 messages
    for i in 0..50 {
        let _ = store.send_message(NewMessage {
            team_id: "team-1".to_string(),
            from_agent: "agent-a".to_string(),
            msg_type: MessageType::Message,
            subject: format!("Msg {i}"),
            content: "content".to_string(),
            recipients: vec![
                Recipient { agent_id: "agent-b".to_string(), role: RecipientRole::To },
            ],
            reply_to: None,
            attachments: vec![],
        }).await.unwrap();
    }
    
    // Both methods should return same results
    let original = store.read_inbox("agent-b", "team-1", None).await.unwrap();
    let optimized = store.read_inbox_optimized("agent-b", "team-1", None).await.unwrap();
    
    assert_eq!(original.len(), optimized.len());
    assert_eq!(original.len(), 50);
    
    // Verify content matches
    for (orig, opt) in original.iter().zip(optimized.iter()) {
        assert_eq!(orig.id, opt.id);
        assert_eq!(orig.subject, opt.subject);
        assert_eq!(orig.recipients.len(), opt.recipients.len());
    }
}
```

- [ ] **Step 3: Run test**

```bash
cargo test -p alephcore teams::messages::store::tests::test_optimized_inbox_matches_original --lib
```
Expected: PASS

- [ ] **Step 4: Swap method names**

Rename `read_inbox` to `read_inbox_legacy` and `read_inbox_optimized` to `read_inbox`.

- [ ] **Step 5: Commit**

```bash
git add src/teams/messages/store.rs
git commit -m "teams(messages): fix N+1 query in read_inbox with JOIN optimization"
```

---

### Task 4: Migrate MessageRouter to EventBus

**Files:**
- Modify: `src/teams/messages/router.rs`
- Modify: `src/teams/plans.rs`
- Modify: `src/teams/sessions/coordinator.rs`

- [ ] **Step 1: Update MessageRouter to accept EventBus**

```rust
pub struct MessageRouter {
    msg_store: Arc<dyn MessageStore>,
    event_store: Arc<dyn EventLogStore>, // Keep for backward compat during transition
    escalation_rules: EscalationRule,
    leader_id: Option<String>,
    bus: Option<EventBus>, // NEW
}

impl MessageRouter {
    pub fn with_bus(mut self, bus: EventBus) -> Self {
        self.bus = Some(bus);
        self
    }
}
```

- [ ] **Step 2: Replace log_event with publish in send()**

```rust
pub async fn send(&self, 
    req: SendRequest,
    // Add event_bus parameter or use self.bus
) -> Result<TeamMessage> {
    // ... existing code ...
    
    // 3. Publish event instead of logging
    if let Some(ref bus) = self.bus {
        let to_agents: Vec<String> = msg.recipients.iter()
            .map(|r| r.agent_id.clone())
            .collect();
        
        bus.publish(AlephEvent::TeamMessageSent(TeamMessageEvent {
            team_id: team_id.clone(),
            message_id: msg.id.clone(),
            from_agent: from_agent.clone(),
            to_agents,
            subject: msg.subject.clone(),
            timestamp: msg.created_at.timestamp_millis(),
        })).await;
    }
    
    // ... rest of existing code ...
}
```

- [ ] **Step 3: Update PlanManager to use EventBus**

```rust
pub struct PlanManager {
    msg_router: Arc<MessageRouter>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
    bus: Option<EventBus>, // NEW
}

// In submit_plan(), replace log_event with:
if let Some(ref bus) = self.bus {
    bus.publish(AlephEvent::TeamPlanSubmitted(TeamPlanEvent {
        team_id: team_id.to_string(),
        artifact_id: artifact.id.clone(),
        submitter: from_agent.to_string(),
        leader: leader_id.to_string(),
    })).await;
}
```

- [ ] **Step 4: Update SessionCoordinator**

Similar pattern for `start_session` and `finalize`.

- [ ] **Step 5: Run integration tests**

```bash
cargo test -p alephcore teams::integration_tests --lib
```
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/teams/messages/router.rs src/teams/plans.rs src/teams/sessions/coordinator.rs
git commit -m "teams: migrate log_event calls to EventBus publish"
```

---

### Task 5: Update Module Exports and Integration Tests

**Files:**
- Modify: `src/teams/mod.rs`
- Modify: `src/teams/integration_tests.rs`

- [ ] **Step 1: Export new types**

```rust
pub use events::{TeamEventLogger, EventLogStore, SqliteEventLogStore};
pub use kanban::{KanbanBoard, SqliteKanbanBoard, KanbanColumns, TaskStatus};
```

- [ ] **Step 2: Update integration tests to verify EventBus flow**

Add test that verifies events are published and received:

```rust
#[tokio::test]
async fn test_team_events_flow_through_bus() {
    let bus = EventBus::new();
    let ctx = EventContext::new(bus.clone());
    
    // Setup stores and router with bus
    let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
    let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
    let logger = TeamEventLogger::new(event_store);
    
    let mut registry = EventHandlerRegistry::new();
    registry.register(Arc::new(logger));
    let handles = registry.start(ctx.clone()).await;
    
    let router = Arc::new(MessageRouter::new(
        msg_store,
        event_store.clone(),
        EscalationRule::default(),
        None,
    ).with_bus(bus.clone()));
    
    // Send message
    let msg = router.send(SendRequest {
        team_id: "team-1".to_string(),
        from_agent: "agent-a".to_string(),
        to: vec!["agent-b".to_string()],
        cc: vec![],
        msg_type: MessageType::Message,
        subject: "Test".to_string(),
        content: "Hello".to_string(),
        reply_to: None,
        attachments: vec![],
    }).await.unwrap();
    
    tokio::time::sleep(Duration::from_millis(50)).await;
    
    // Verify event was logged
    let events = event_store.get_events("team-1", None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, TeamEventType::MessageSent);
    
    registry.stop(&ctx);
    for h in handles {
        let _ = tokio::time::timeout(Duration::from_millis(100), h).await;
    }
}
```

- [ ] **Step 3: Run all tests**

```bash
cargo test -p alephcore teams --lib
cargo clippy -p alephcore -- -D warnings
```
Expected: All PASS, no warnings

- [ ] **Step 4: Commit**

```bash
git add src/teams/mod.rs src/teams/integration_tests.rs
git commit -m "teams: update exports and integration tests for EventBus"
```

---

## Self-Review Checklist

- [ ] Spec coverage: All Phase 1 requirements (EventBus integration, N+1 fix, EventHandler) have tasks
- [ ] Placeholder scan: No TBD, TODO, or vague descriptions
- [ ] Type consistency: TeamMessageEvent, TeamPlanEvent, etc. match spec exactly
- [ ] Backward compat: Existing EventLogStore trait preserved, only usage pattern changes
- [ ] Test coverage: Each task has tests, integration test verifies end-to-end flow
