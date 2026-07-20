# Teams 模块重构：剔除过度设计，聚焦基础设施

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove hardcoded Explorer/Critic role system from teams module, simplify role to plain strings, add Lifecycle and Plan approval infrastructure.

**Architecture:** Delete `src/teams/roles/` module entirely. Simplify MessageType/ArtifactType/TeamEventType enums by removing role-specific variants and adding infrastructure variants. Add `lifecycle.rs` and `plans.rs` as thin wrappers over existing MessageRouter + ArtifactStore. Update tools accordingly.

**Tech Stack:** Rust, async-trait, SQLite (rusqlite), serde, schemars, chrono, tokio

**Spec:** `docs/superpowers/specs/2026-04-08-teams-refactor-design.md`

---

### Task 1: Delete roles/ module and update teams/mod.rs

**Files:**
- Delete: `src/teams/roles/types.rs`
- Delete: `src/teams/roles/prompts.rs`
- Delete: `src/teams/roles/review.rs`
- Delete: `src/teams/roles/mod.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Delete the roles directory**

```bash
rm -rf src/teams/roles/
```

- [ ] **Step 2: Update src/teams/mod.rs — remove roles, add lifecycle and plans**

Replace the full content of `src/teams/mod.rs` with:

```rust
//! Team management module.
//!
//! Provides types and a SQLite-backed store for managing teams of agents,
//! team membership, per-team task tracking, lifecycle management, and plan approval.

pub mod artifacts;
pub mod context;
pub mod events;
pub mod lifecycle;
pub mod messages;
pub mod plans;
pub mod sessions;
pub mod store;
pub mod types;

#[cfg(test)]
pub mod integration_tests;

pub use store::{SqliteTeamStore, TeamStore};
pub use types::{NewTeam, NewTeamMember, Team, TeamId, TeamMember, TeamStatus, TeamSummary};
```

- [ ] **Step 3: Verify the module compiles (expect errors — downstream files still reference roles)**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: Errors referencing `teams::roles` from other files — this is expected and will be fixed in subsequent tasks.

---

### Task 2: Clean up MessageType enum

**Files:**
- Modify: `src/teams/messages/types.rs`

- [ ] **Step 1: Update the MessageType enum**

In `src/teams/messages/types.rs`, replace the `MessageType` enum and its impl block:

```rust
/// Categorizes messages for filtering and TTL computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Message,
    SystemNotification,
    Idle,
    PlanApprovalRequest,
    PlanApproved,
    PlanRejected,
    ShutdownRequest,
    ShutdownApproved,
    ShutdownRejected,
    Custom(String),
}

impl MessageType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Message => "message",
            Self::SystemNotification => "system_notification",
            Self::Idle => "idle",
            Self::PlanApprovalRequest => "plan_approval_request",
            Self::PlanApproved => "plan_approved",
            Self::PlanRejected => "plan_rejected",
            Self::ShutdownRequest => "shutdown_request",
            Self::ShutdownApproved => "shutdown_approved",
            Self::ShutdownRejected => "shutdown_rejected",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "message" => Self::Message,
            "system_notification" => Self::SystemNotification,
            "idle" => Self::Idle,
            "plan_approval_request" => Self::PlanApprovalRequest,
            "plan_approved" => Self::PlanApproved,
            "plan_rejected" => Self::PlanRejected,
            "shutdown_request" => Self::ShutdownRequest,
            "shutdown_approved" => Self::ShutdownApproved,
            "shutdown_rejected" => Self::ShutdownRejected,
            other => Self::Custom(other.to_string()),
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/teams/messages/types.rs
git commit -m "teams: simplify MessageType enum — remove role-specific variants, add lifecycle + Custom"
```

---

### Task 3: Clean up ArtifactType enum

**Files:**
- Modify: `src/teams/artifacts.rs`

- [ ] **Step 1: Update ArtifactType enum and its impl block**

In `src/teams/artifacts.rs`, replace the `ArtifactType` enum and its impl:

```rust
/// The kind of artifact produced by an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Report,
    Code,
    Plan,
    Custom(String),
}

impl ArtifactType {
    /// Canonical string representation for storage.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Report => "report",
            Self::Code => "code",
            Self::Plan => "plan",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Reconstruct from a stored string value.
    pub fn from_stored(s: &str) -> Self {
        match s {
            "report" => Self::Report,
            "code" => Self::Code,
            "plan" => Self::Plan,
            other => Self::Custom(other.to_string()),
        }
    }
}
```

- [ ] **Step 2: Update the test_custom_artifact_type test at the bottom of artifacts.rs**

The existing tests should still work. The `test_create_and_read_artifact` test uses `ArtifactType::Report` which is kept. The `test_custom_artifact_type` test uses `ArtifactType::Custom` which is kept. No test changes needed.

- [ ] **Step 3: Commit**

```bash
git add src/teams/artifacts.rs
git commit -m "teams: simplify ArtifactType enum — remove role-specific variants, add Plan"
```

---

### Task 4: Clean up TeamEventType enum

**Files:**
- Modify: `src/teams/events.rs`

- [ ] **Step 1: Update TeamEventType enum — remove ReviewScoreSubmitted, add lifecycle + plan events**

In `src/teams/events.rs`, replace the `TeamEventType` enum and its impl:

```rust
/// Categorizes the kind of activity that occurred within a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamEventType {
    MessageSent,
    MessageRead,
    TaskCreated,
    TaskCompleted,
    TaskFailed,
    ArtifactSubmitted,
    SessionStarted,
    SessionConcluded,
    DigestGenerated,
    ShutdownRequested,
    ShutdownResolved,
    PlanSubmitted,
    PlanResolved,
}

impl TeamEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MessageSent => "message_sent",
            Self::MessageRead => "message_read",
            Self::TaskCreated => "task_created",
            Self::TaskCompleted => "task_completed",
            Self::TaskFailed => "task_failed",
            Self::ArtifactSubmitted => "artifact_submitted",
            Self::SessionStarted => "session_started",
            Self::SessionConcluded => "session_concluded",
            Self::DigestGenerated => "digest_generated",
            Self::ShutdownRequested => "shutdown_requested",
            Self::ShutdownResolved => "shutdown_resolved",
            Self::PlanSubmitted => "plan_submitted",
            Self::PlanResolved => "plan_resolved",
        }
    }

    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "message_sent" => Some(Self::MessageSent),
            "message_read" => Some(Self::MessageRead),
            "task_created" => Some(Self::TaskCreated),
            "task_completed" => Some(Self::TaskCompleted),
            "task_failed" => Some(Self::TaskFailed),
            "artifact_submitted" => Some(Self::ArtifactSubmitted),
            "session_started" => Some(Self::SessionStarted),
            "session_concluded" => Some(Self::SessionConcluded),
            "digest_generated" => Some(Self::DigestGenerated),
            "shutdown_requested" => Some(Self::ShutdownRequested),
            "shutdown_resolved" => Some(Self::ShutdownResolved),
            "plan_submitted" => Some(Self::PlanSubmitted),
            "plan_resolved" => Some(Self::PlanResolved),
            // Backwards compat: old event types map to None (ignored)
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/teams/events.rs
git commit -m "teams: update TeamEventType — remove ReviewScoreSubmitted, add lifecycle + plan events"
```

---

### Task 5: Update team_create tool — remove AgentRole dependency

**Files:**
- Modify: `src/builtin_tools/team/create.rs`

- [ ] **Step 1: Remove AgentRole imports**

In `src/builtin_tools/team/create.rs`, replace line 14:

```rust
use crate::teams::roles::{role_prompt_template, AgentRole};
```

with nothing (delete the line entirely).

- [ ] **Step 2: Simplify CreateAgentSpec — remove AgentRole field**

Replace the `role` field in `CreateAgentSpec` (around line 42):

```rust
    /// Team role for this agent (free-form string, e.g. "researcher", "analyst").
    /// When set to "leader" or "worker", the corresponding built-in prompt
    /// template is automatically appended to the agent's system prompt.
    #[serde(default)]
    pub role: Option<String>,
```

- [ ] **Step 3: Simplify MemberSpec — remove AgentRole field**

Remove the `agent_role` field from `MemberSpec` (delete lines 66-70 containing `pub agent_role: Option<AgentRole>`).

- [ ] **Step 4: Add built-in prompt loading helper**

Add this helper function at the top of the impl block (after the `new` method):

```rust
    /// Returns the built-in prompt template for "leader" or "worker" roles.
    /// Returns None for all other roles (user provides their own prompt).
    fn builtin_role_prompt(role: &str) -> Option<&'static str> {
        const LEADER_PROMPT: &str = include_str!("../../agents/prompts/team_leader.md");
        const WORKER_PROMPT: &str = include_str!("../../agents/prompts/team_worker.md");

        match role {
            "leader" => Some(LEADER_PROMPT),
            "worker" => Some(WORKER_PROMPT),
            _ => None,
        }
    }
```

- [ ] **Step 5: Simplify resolve_member — use string role**

Replace the `resolve_member` method body. Remove the `effective_role` calculation that uses `AgentRole::from_stored`. Instead:

```rust
    async fn resolve_member(&self, spec: &MemberSpec) -> Result<String> {
        if let Some(ref agent_id) = spec.agent_id {
            // Verify the existing agent is present in the runtime registry
            let instance = self.registry.get(agent_id).await.ok_or_else(|| {
                AlephError::other(format!("Agent '{}' not found in registry", agent_id))
            })?;

            // Inject role prompt for existing agents by appending to SOUL.md
            if !spec.role.is_empty() {
                if let Some(template) = Self::builtin_role_prompt(&spec.role) {
                    Self::append_role_prompt_to_soul(instance.agent_dir(), template).await;
                }
            }

            return Ok(agent_id.clone());
        }

        if let Some(ref create_spec) = spec.create {
            let leader_model = self
                .registry
                .get(&self.current_agent_id)
                .await
                .map(|inst| inst.config().model.clone())
                .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
            return self
                .create_inline_agent(create_spec, &spec.role, &leader_model)
                .await;
        }

        Err(AlephError::other(
            "MemberSpec must specify either agent_id or create",
        ))
    }
```

- [ ] **Step 6: Simplify create_inline_agent — use string role**

Change the signature from `role: Option<&AgentRole>` to `role: &str`:

```rust
    async fn create_inline_agent(
        &self,
        spec: &CreateAgentSpec,
        role: &str,
        leader_model: &str,
    ) -> Result<String> {
```

Update the role template resolution inside the method. Replace the `effective_role` and `role_template` logic with:

```rust
        // Determine the effective role: prefer spec.role if set, fall back to the member role
        let effective_role = spec.role.as_deref().unwrap_or(role);

        // Build the combined system prompt: user profile + role template
        let role_template = if effective_role.is_empty() {
            None
        } else {
            Self::builtin_role_prompt(effective_role)
        };
```

The rest of `create_inline_agent` (combined_prompt matching, writing SOUL.md, creating AgentInstance) stays the same.

- [ ] **Step 7: Update the call method — use builtin_role_prompt for leader**

In the `call` method, replace line 384:

```rust
            if let Some(template) = role_prompt_template(&AgentRole::Leader) {
```

with:

```rust
            if let Some(template) = Self::builtin_role_prompt("leader") {
```

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Should compile (or show only downstream errors from review_score tool)

- [ ] **Step 9: Commit**

```bash
git add src/builtin_tools/team/create.rs
git commit -m "teams: remove AgentRole from team_create — use plain string roles"
```

---

### Task 6: Delete review_score tool and unregister from builder

**Files:**
- Delete: `src/builtin_tools/team/review_score.rs`
- Modify: `src/builtin_tools/team/mod.rs`
- Modify: `src/executor/builtin_registry/registry.rs:197-198`
- Modify: `src/executor/builtin_registry/builder.rs:608-643`

- [ ] **Step 1: Delete review_score.rs**

```bash
rm src/builtin_tools/team/review_score.rs
```

- [ ] **Step 2: Update src/builtin_tools/team/mod.rs**

Remove line 8 (`pub mod review_score;`) and line 24 (`pub use review_score::{...};`).

The updated file:

```rust
//! Team management tools.

mod create;
mod delegate;
mod disband;
pub mod inbox_read;
pub mod message_send;
pub mod session_collaborate;
pub mod session_read;
pub mod session_turn;
mod status;
pub mod task_read_artifact;
pub mod task_submit;
mod team_digest;

pub use create::{
    CreateAgentSpec, EnrolledMember, MemberSpec, TeamCreateArgs, TeamCreateOutput, TeamCreateTool,
};
pub use delegate::{DelegateStatus, TeamDelegateArgs, TeamDelegateOutput, TeamDelegateTool};
pub use disband::{TeamDisbandArgs, TeamDisbandOutput, TeamDisbandTool};
pub use inbox_read::{InboxReadArgs, InboxReadOutput, InboxReadTool};
pub use message_send::{MessageSendArgs, MessageSendOutput, MessageSendTool};
pub use session_collaborate::{
    SessionCollaborateArgs, SessionCollaborateOutput, SessionCollaborateTool,
};
pub use session_read::{SessionReadArgs, SessionReadOutput, SessionReadTool};
pub use session_turn::{SessionTurnArgs, SessionTurnOutput, SessionTurnTool};
pub use status::{MemberInfo, TaskInfo, TeamStatusArgs, TeamStatusOutput, TeamStatusTool};
pub use task_read_artifact::{TaskReadArtifactArgs, TaskReadArtifactOutput, TaskReadArtifactTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
pub use team_digest::{TeamDigestArgs, TeamDigestOutput, TeamDigestTool};
```

- [ ] **Step 3: Remove review_score_tool from registry struct**

In `src/executor/builtin_registry/registry.rs`, delete line 197-198:

```rust
    /// Review score tool (optional — requires ArtifactStore + EventLogStore + MessageRouter)
    pub(crate) review_score_tool: Option<crate::builtin_tools::team::ReviewScoreTool>,
```

- [ ] **Step 4: Remove review_score registration from builder**

In `src/executor/builtin_registry/builder.rs`, delete the entire block from line 608 ("Add review_score tool") through line 643 (the closing `};`). Also remove the `review_score_tool` field from the `BuiltinToolRegistry` struct construction at the end of the build method.

- [ ] **Step 5: Search for any remaining references to ReviewScoreTool**

```bash
grep -rn "review_score\|ReviewScore" src/ --include="*.rs" | grep -v integration_tests | grep -v "target/"
```

Fix any remaining references. Check `src/executor/builtin_registry/definitions.rs` for tool definition entries to remove.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Should compile (integration_tests will be fixed next)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "teams: delete review_score tool and unregister from builder"
```

---

### Task 7: Rewrite integration tests

**Files:**
- Modify: `src/teams/integration_tests.rs`

The existing tests heavily use Explorer-Critic role patterns and ReviewScore types. We need to:
1. Rewrite Test 1 (Explorer-Critic) → generic two-agent collaboration test
2. Keep Test 2 (Escalation) but update MessageType variants
3. Keep Test 3 (Context Injection) but update MessageType variants
4. Delete Test 4 (ReviewScore validation) entirely
5. Keep Test 5 (Team Disband) unchanged

- [ ] **Step 1: Update imports**

Replace the imports at the top of `integration_tests.rs`:

```rust
//! End-to-end integration tests for the three-layer team communication system.
//!
//! These tests exercise the full message routing, escalation, and session flow
//! using real SQLite-backed stores (in-memory) for full integration coverage.

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
```

- [ ] **Step 2: Rewrite Test 1 — generic agent collaboration**

Replace `test_explorer_critic_review_cycle` with a test that exercises artifact creation, message routing, and inbox reading without role-specific types:

```rust
// ---------------------------------------------------------------------------
// Test 1: Two-Agent Collaboration via Messages and Artifacts
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
            title: "Initial analysis: cache optimization".into(),
            content: "We can improve cache hit rates by 30% with LRU eviction.".into(),
            metadata: json!({"version": 1}),
        })
        .await
        .unwrap();

    assert_eq!(report.artifact_type, ArtifactType::Report);

    // Agent A sends message to Agent B referencing the artifact
    let msg1 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: agent_a.into(),
            to: vec![agent_b.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Please review: cache optimization".into(),
            content: format!("Please review artifact {}", report.id),
            reply_to: None,
            attachments: vec![report.id.clone()],
        })
        .await
        .unwrap();

    assert_eq!(msg1.msg_type, MessageType::Message);
    assert_eq!(msg1.attachments.len(), 1);

    // Agent B reads inbox
    let b_inbox = inbox
        .read(agent_b, team_id, None, true)
        .await
        .unwrap();

    assert_eq!(b_inbox.len(), 1);
    assert_eq!(b_inbox[0].attachments[0], report.id);

    // Agent B replies with feedback
    let msg2 = router
        .send(SendRequest {
            team_id: team_id.into(),
            from_agent: agent_b.into(),
            to: vec![agent_a.into()],
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Re: cache optimization".into(),
            content: "Needs benchmarks. Consider ARC instead of LRU.".into(),
            reply_to: Some(msg1.id.clone()),
            attachments: vec![],
        })
        .await
        .unwrap();

    assert!(msg2.thread_id.is_some());

    // Agent A submits revised report
    let report_v2 = artifact_store
        .create_artifact(NewArtifact {
            task_id: task_id.into(),
            agent_id: agent_a.into(),
            artifact_type: ArtifactType::Report,
            title: "Revised: cache optimization with ARC".into(),
            content: "ARC eviction improves hit rates by 25%.".into(),
            metadata: json!({"version": 2, "parent_artifact": report.id}),
        })
        .await
        .unwrap();

    // Verify all artifacts for the task
    let all_artifacts = artifact_store
        .get_artifacts_for_task(task_id)
        .await
        .unwrap();
    assert_eq!(all_artifacts.len(), 2);

    // Verify events were logged
    let events = event_store.get_events(team_id, None, None).await.unwrap();
    assert!(events.len() >= 2);

    let sent_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageSent)
        .collect();
    assert!(sent_events.len() >= 2);

    let read_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == TeamEventType::MessageRead)
        .collect();
    assert!(read_events.len() >= 1);

    // Suppress unused variable warning
    let _ = report_v2;
}
```

- [ ] **Step 3: Update Test 2 — fix MessageType variants**

In `test_escalation_to_collaborative_session`, change `MessageType::Discovery` (line 354) to `MessageType::Message` and `MessageType::Challenge` (line 373) to `MessageType::Message`. These are just messages between agents now.

- [ ] **Step 4: Update Test 3 — fix MessageType variants**

In `test_context_injection_shows_inbox_summary`:
- Change `MessageType::ReviewRequest` (line 578) to `MessageType::Message`
- Change `MessageType::Discovery` (line 593) to `MessageType::Message`
- Update the inbox filter on line 88 from `Some(&MessageType::ReviewRequest)` to `None` (or `Some(&MessageType::Message)`)

- [ ] **Step 5: Delete Test 4 entirely**

Remove the entire `test_review_score_validation_flow` test (lines 673-773).

- [ ] **Step 6: Verify tests pass**

Run: `cargo test -p alephcore --lib teams::integration_tests -- --nocapture 2>&1 | tail -20`
Expected: All 4 remaining tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/teams/integration_tests.rs
git commit -m "teams: rewrite integration tests — remove role-specific patterns"
```

---

### Task 8: Update prompt templates

**Files:**
- Delete: `src/agents/prompts/team_explorer.md`
- Delete: `src/agents/prompts/team_critic.md`
- Modify: `src/agents/prompts/team_leader.md`
- Modify: `src/agents/prompts/team_worker.md`

- [ ] **Step 1: Delete explorer and critic prompts**

```bash
rm src/agents/prompts/team_explorer.md src/agents/prompts/team_critic.md
```

- [ ] **Step 2: Update team_leader.md**

Write the updated leader prompt focusing on infrastructure tools:

```markdown
# Team Leader

You are the leader of this team. Your responsibilities:

## Core Duties
- Break down the team objective into tasks and delegate via `team_delegate`
- Monitor progress via `team_status` and `team_digest`
- Coordinate members via `message_send`

## Plan Approval
- Members submit plans via `plan_submit` before executing complex tasks
- Review plans and approve (`plan_approve`) or reject (`plan_reject`) with feedback
- Ensure plans align with the team objective before approval

## Lifecycle Management
- Members may request shutdown via `shutdown_request` when their work is complete
- Approve (`shutdown_respond` with approved=true) when the member's contributions are sufficient
- Reject with reason if more work is needed

## Communication
- Use `inbox_read` to check messages from team members
- Use `message_send` to provide guidance, feedback, or new instructions
- When discussions stall (escalation notification), start a collaborative session via `session_collaborate`

## Escalation Protocol
- If you receive a SystemNotification about a thread exceeding the message threshold, consider starting a `session_collaborate` to resolve the discussion with all relevant participants

## Session Management
- Use `session_collaborate` to start multi-agent discussions
- Use `session_turn` to contribute to active sessions
- Use `session_read` to review session transcripts

## Quality
- Review artifacts submitted via `task_read_artifact`
- Provide constructive feedback through messages
- Synthesize final deliverables from team outputs
```

- [ ] **Step 3: Update team_worker.md**

Write the updated worker prompt:

```markdown
# Team Worker

You are a member of a team, working under the team leader's direction.

## Core Workflow
1. Receive tasks via messages — check `inbox_read` regularly
2. For complex tasks, submit a plan first via `plan_submit` and wait for leader approval
3. Execute the task and submit results via `task_submit`
4. Respond to feedback from the leader or other team members

## Plan Submission
- Before starting complex work, submit a plan via `plan_submit` describing your approach
- Wait for `plan_approved` or `plan_rejected` message before proceeding
- If rejected, revise your plan based on the leader's feedback and resubmit

## Communication
- Use `inbox_read` to check for new messages and task assignments
- Use `message_send` to ask questions, report progress, or share findings
- Respond promptly to messages addressed to you (To recipients)

## Task Completion
- Submit deliverables via `task_submit` with clear, well-structured content
- If your work is complete and no more tasks are pending, send an `idle` message to the leader via `message_send` with msg_type "idle"

## Shutdown
- When all your assigned work is done, request shutdown via `shutdown_request`
- Wait for leader approval before considering yourself done

## Collaboration
- If invited to a collaborative session, participate via `session_turn`
- Use `session_read` to review session context before contributing
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "teams: delete explorer/critic prompts, update leader/worker prompts for new tools"
```

---

### Task 9: Create lifecycle.rs

**Files:**
- Create: `src/teams/lifecycle.rs`

- [ ] **Step 1: Write the LifecycleManager**

```rust
//! Lifecycle management for team agents.
//!
//! Provides shutdown request/approval and idle notification protocols
//! built on top of the existing message routing system.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::{MessageType, TeamMessage};

/// Manages agent lifecycle within a team — shutdown and idle protocols.
///
/// This is a thin wrapper over [`MessageRouter`] that standardises the
/// message patterns for lifecycle events and logs them to the event store.
pub struct LifecycleManager {
    msg_router: Arc<MessageRouter>,
    event_store: Arc<dyn EventLogStore>,
}

impl LifecycleManager {
    pub fn new(
        msg_router: Arc<MessageRouter>,
        event_store: Arc<dyn EventLogStore>,
    ) -> Self {
        Self {
            msg_router,
            event_store,
        }
    }

    /// Agent requests to shut down — sends `ShutdownRequest` to the leader.
    pub async fn request_shutdown(
        &self,
        team_id: &str,
        from_agent: &str,
        leader_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: from_agent.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownRequest,
                subject: format!("Shutdown request from {from_agent}"),
                content: reason.to_string(),
                reply_to: None,
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownRequested,
                agent_id: from_agent.to_string(),
                payload: serde_json::json!({
                    "message_id": msg.id,
                    "reason": reason,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader approves a shutdown request.
    pub async fn approve_shutdown(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        request_msg_id: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownApproved,
                subject: "Shutdown approved".to_string(),
                content: format!("Your shutdown request has been approved."),
                reply_to: Some(request_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": true,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader rejects a shutdown request.
    pub async fn reject_shutdown(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        request_msg_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownRejected,
                subject: "Shutdown rejected".to_string(),
                content: reason.to_string(),
                reply_to: Some(request_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": false,
                    "reason": reason,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Agent reports idle status to the leader.
    pub async fn send_idle(
        &self,
        team_id: &str,
        agent_id: &str,
        leader_id: &str,
        last_task: Option<&str>,
    ) -> Result<TeamMessage> {
        let content = match last_task {
            Some(task) => format!("Idle. Last completed task: {task}"),
            None => "Idle. No tasks completed.".to_string(),
        };

        self.msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: agent_id.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::Idle,
                subject: format!("{agent_id} is idle"),
                content,
                reply_to: None,
                attachments: vec![],
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::SqliteMessageStore;

    async fn make_lifecycle() -> (LifecycleManager, Arc<SqliteMessageStore>) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);

        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        ));

        let lm = LifecycleManager::new(router, event_store);
        (lm, msg_store)
    }

    #[tokio::test]
    async fn test_shutdown_request_sends_message_to_leader() {
        let (lm, msg_store) = make_lifecycle().await;

        let msg = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "All tasks done")
            .await
            .unwrap();

        assert_eq!(msg.msg_type, MessageType::ShutdownRequest);
        assert_eq!(msg.from_agent, "worker-1");

        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::ShutdownRequest))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_approve_shutdown() {
        let (lm, msg_store) = make_lifecycle().await;

        let req = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "Done")
            .await
            .unwrap();

        let approval = lm
            .approve_shutdown("team-1", "leader-1", "worker-1", &req.id)
            .await
            .unwrap();

        assert_eq!(approval.msg_type, MessageType::ShutdownApproved);

        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::ShutdownApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_reject_shutdown() {
        let (lm, msg_store) = make_lifecycle().await;

        let req = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "Done")
            .await
            .unwrap();

        let rejection = lm
            .reject_shutdown("team-1", "leader-1", "worker-1", &req.id, "More work needed")
            .await
            .unwrap();

        assert_eq!(rejection.msg_type, MessageType::ShutdownRejected);

        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::ShutdownRejected))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_send_idle() {
        let (lm, msg_store) = make_lifecycle().await;

        let msg = lm
            .send_idle("team-1", "worker-1", "leader-1", Some("task-42"))
            .await
            .unwrap();

        assert_eq!(msg.msg_type, MessageType::Idle);
        assert!(msg.content.contains("task-42"));

        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::Idle))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }
}
```

- [ ] **Step 2: Verify tests pass**

Run: `cargo test -p alephcore --lib teams::lifecycle -- --nocapture`
Expected: All 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/teams/lifecycle.rs
git commit -m "teams: add LifecycleManager — shutdown/idle protocols over MessageRouter"
```

---

### Task 10: Create plans.rs

**Files:**
- Create: `src/teams/plans.rs`

- [ ] **Step 1: Write the PlanManager**

```rust
//! Plan approval workflow for team agents.
//!
//! Members submit plans as artifacts and request leader approval via messages.
//! The leader reviews and approves or rejects through the existing message system.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, TaskArtifact};
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::{MessageType, TeamMessage};

/// Result of submitting a plan — contains both the stored artifact and the
/// approval request message.
#[derive(Debug, Clone)]
pub struct PlanSubmission {
    pub artifact: TaskArtifact,
    pub message: TeamMessage,
}

/// Manages plan submission and approval within a team.
///
/// Plans are stored as [`ArtifactType::Plan`] artifacts linked to a task.
/// Approval flow uses `PlanApprovalRequest` → `PlanApproved` / `PlanRejected`
/// messages through the existing [`MessageRouter`].
pub struct PlanManager {
    msg_router: Arc<MessageRouter>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
}

impl PlanManager {
    pub fn new(
        msg_router: Arc<MessageRouter>,
        artifact_store: Arc<dyn ArtifactStore>,
        event_store: Arc<dyn EventLogStore>,
    ) -> Self {
        Self {
            msg_router,
            artifact_store,
            event_store,
        }
    }

    /// Submit a plan for leader approval.
    ///
    /// 1. Stores the plan content as a `Plan` artifact linked to `task_id`
    /// 2. Sends a `PlanApprovalRequest` message to the leader with the artifact ID
    pub async fn submit_plan(
        &self,
        team_id: &str,
        from_agent: &str,
        leader_id: &str,
        title: &str,
        content: &str,
        task_id: &str,
    ) -> Result<PlanSubmission> {
        // 1. Store plan as artifact
        let artifact = self
            .artifact_store
            .create_artifact(NewArtifact {
                task_id: task_id.to_string(),
                agent_id: from_agent.to_string(),
                artifact_type: ArtifactType::Plan,
                title: title.to_string(),
                content: content.to_string(),
                metadata: serde_json::json!({}),
            })
            .await?;

        // 2. Send approval request to leader
        let message = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: from_agent.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanApprovalRequest,
                subject: format!("Plan approval: {title}"),
                content: format!(
                    "Please review and approve/reject the plan.\n\
                     Task: {task_id}\n\
                     Artifact: {}",
                    artifact.id
                ),
                reply_to: None,
                attachments: vec![artifact.id.clone()],
            })
            .await?;

        // 3. Log event
        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanSubmitted,
                agent_id: from_agent.to_string(),
                payload: serde_json::json!({
                    "artifact_id": artifact.id,
                    "message_id": message.id,
                    "task_id": task_id,
                }),
            })
            .await;

        Ok(PlanSubmission { artifact, message })
    }

    /// Leader approves a submitted plan.
    pub async fn approve_plan(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        plan_msg_id: &str,
        feedback: &str,
    ) -> Result<TeamMessage> {
        let content = if feedback.is_empty() {
            "Plan approved.".to_string()
        } else {
            format!("Plan approved.\n\nFeedback: {feedback}")
        };

        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanApproved,
                subject: "Plan approved".to_string(),
                content,
                reply_to: Some(plan_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": true,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader rejects a submitted plan.
    pub async fn reject_plan(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        plan_msg_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanRejected,
                subject: "Plan rejected".to_string(),
                content: reason.to_string(),
                reply_to: Some(plan_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": false,
                    "reason": reason,
                }),
            })
            .await;

        Ok(msg)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::artifacts::SqliteArtifactStore;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::SqliteMessageStore;

    async fn make_plan_manager() -> (PlanManager, Arc<SqliteMessageStore>, Arc<SqliteArtifactStore>)
    {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let artifact_store = Arc::new(SqliteArtifactStore::new_in_memory().await);

        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        ));

        let pm = PlanManager::new(router, artifact_store.clone(), event_store);
        (pm, msg_store, artifact_store)
    }

    #[tokio::test]
    async fn test_submit_plan_creates_artifact_and_message() {
        let (pm, msg_store, artifact_store) = make_plan_manager().await;

        let submission = pm
            .submit_plan(
                "team-1",
                "worker-1",
                "leader-1",
                "Cache optimization plan",
                "# Plan\n\n1. Benchmark current performance\n2. Implement ARC\n3. Verify improvement",
                "task-1",
            )
            .await
            .unwrap();

        // Verify artifact
        assert_eq!(submission.artifact.artifact_type, ArtifactType::Plan);
        assert_eq!(submission.artifact.task_id, "task-1");
        assert!(submission.artifact.content.contains("Benchmark"));

        // Verify message
        assert_eq!(submission.message.msg_type, MessageType::PlanApprovalRequest);
        assert_eq!(submission.message.attachments.len(), 1);
        assert_eq!(submission.message.attachments[0], submission.artifact.id);

        // Verify leader inbox
        let inbox = msg_store
            .read_inbox(
                "leader-1",
                "team-1",
                Some(&MessageType::PlanApprovalRequest),
            )
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);

        // Verify artifact is retrievable
        let stored = artifact_store
            .get_artifact(&submission.artifact.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.artifact_type, ArtifactType::Plan);
    }

    #[tokio::test]
    async fn test_approve_plan() {
        let (pm, msg_store, _) = make_plan_manager().await;

        let submission = pm
            .submit_plan("team-1", "worker-1", "leader-1", "My plan", "Details", "task-1")
            .await
            .unwrap();

        let approval = pm
            .approve_plan(
                "team-1",
                "leader-1",
                "worker-1",
                &submission.message.id,
                "Looks good, proceed",
            )
            .await
            .unwrap();

        assert_eq!(approval.msg_type, MessageType::PlanApproved);
        assert!(approval.content.contains("Looks good"));

        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::PlanApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_reject_plan() {
        let (pm, msg_store, _) = make_plan_manager().await;

        let submission = pm
            .submit_plan("team-1", "worker-1", "leader-1", "My plan", "Details", "task-1")
            .await
            .unwrap();

        let rejection = pm
            .reject_plan(
                "team-1",
                "leader-1",
                "worker-1",
                &submission.message.id,
                "Missing error handling strategy",
            )
            .await
            .unwrap();

        assert_eq!(rejection.msg_type, MessageType::PlanRejected);
        assert!(rejection.content.contains("Missing error handling"));

        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::PlanRejected))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }
}
```

- [ ] **Step 2: Verify tests pass**

Run: `cargo test -p alephcore --lib teams::plans -- --nocapture`
Expected: All 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/teams/plans.rs
git commit -m "teams: add PlanManager — plan submission and approval over MessageRouter + ArtifactStore"
```

---

### Task 11: Build and fix all remaining compilation errors

At this point, all the core changes are in place. This task catches any remaining references.

**Files:**
- Possibly modify: any file that still references deleted types

- [ ] **Step 1: Full build check**

Run: `cargo check -p alephcore 2>&1`

- [ ] **Step 2: Fix any remaining references**

Common things to look for:
- `src/executor/builtin_registry/builder.rs` — the struct construction may reference `review_score_tool`
- `src/executor/builtin_registry/definitions.rs` — may have a tool definition entry for `review_score`
- `src/thinker/layers/agent_role.rs` — this is about agent execution modes (explore, coder), NOT team roles. Should be unaffected.
- Any file importing from `crate::teams::roles::*`

For each error, trace the reference and remove or update it.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "teams: fix remaining compilation errors from roles removal"
```

---

### Task 12: Final verification and cleanup

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests pass.

- [ ] **Step 3: Verify no orphan references remain**

```bash
grep -rn "Explorer\|Critic\|ReviewScore\|DimensionScore\|TeamRoleConfig\|review_score\|team_explorer\|team_critic" src/ --include="*.rs" --include="*.md" | grep -v target/ | grep -v "//.*Explorer\|//.*Critic"
```

Expected: No results (or only comments/docs that need updating).

- [ ] **Step 4: Verify file counts**

```bash
echo "=== Deleted files ===" && \
ls src/teams/roles/ 2>&1 && \
ls src/builtin_tools/team/review_score.rs 2>&1 && \
ls src/agents/prompts/team_explorer.md 2>&1 && \
ls src/agents/prompts/team_critic.md 2>&1 && \
echo "=== New files ===" && \
ls src/teams/lifecycle.rs src/teams/plans.rs && \
echo "=== Updated prompts ===" && \
ls src/agents/prompts/team_leader.md src/agents/prompts/team_worker.md
```

Expected: Deleted files show "No such file or directory". New files exist.

- [ ] **Step 5: Final commit (if any cleanup)**

```bash
git add -A
git commit -m "teams: final cleanup after roles removal refactor"
```
