# Multi-Agent 2.0 Phase 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the infrastructure layer for Multi-Agent 2.0 - SubAgentRun data model, FactsDB integration, and SubAgentRegistry core.

**Architecture:** Phase 1 establishes the persistence foundation. SubAgentRun captures run lifecycle state, FactsDB stores runs as facts with TTL-based cleanup, and SubAgentRegistry provides in-memory indexing with persistence triggers on state transitions.

**Tech Stack:** Rust, rusqlite, serde, tokio, broadcast channels

---

## Task 1: Define SubAgentRun Data Model

**Files:**
- Create: `core/src/agents/sub_agents/run.rs`
- Modify: `core/src/agents/sub_agents/mod.rs`

**Step 1: Write the failing test**

Create test file with basic SubAgentRun tests:

```rust
// In core/src/agents/sub_agents/run.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_transitions() {
        assert!(RunStatus::Pending.can_transition_to(&RunStatus::Running));
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Completed));
        assert!(RunStatus::Running.can_transition_to(&RunStatus::Paused));
        assert!(!RunStatus::Completed.can_transition_to(&RunStatus::Running));
    }

    #[test]
    fn test_subagent_run_creation() {
        let run = SubAgentRun::new(
            "session-123".into(),
            "parent-456".into(),
            "Explore the codebase",
            "explore",
        );
        assert!(!run.run_id.is_empty());
        assert_eq!(run.status, RunStatus::Pending);
        assert_eq!(run.lane, Lane::Subagent);
    }

    #[test]
    fn test_lane_default_quotas() {
        assert_eq!(Lane::Main.default_max_concurrent(), 2);
        assert_eq!(Lane::Subagent.default_max_concurrent(), 8);
        assert_eq!(Lane::Cron.default_max_concurrent(), 2);
        assert_eq!(Lane::Nested.default_max_concurrent(), 4);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_run_status_transitions --no-default-features`
Expected: FAIL with "cannot find module `run`"

**Step 3: Write minimal implementation**

```rust
// core/src/agents/sub_agents/run.rs

//! SubAgentRun - Persistent run record for sub-agent lifecycle tracking

use serde::{Deserialize, Serialize};
use crate::routing::SessionKey;

/// Run status for lifecycle state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn can_transition_to(&self, target: &RunStatus) -> bool {
        matches!(
            (self, target),
            (RunStatus::Pending, RunStatus::Running)
                | (RunStatus::Pending, RunStatus::Cancelled)
                | (RunStatus::Running, RunStatus::Completed)
                | (RunStatus::Running, RunStatus::Failed)
                | (RunStatus::Running, RunStatus::Paused)
                | (RunStatus::Running, RunStatus::Cancelled)
                | (RunStatus::Paused, RunStatus::Running)
                | (RunStatus::Paused, RunStatus::Cancelled)
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled)
    }
}

/// Scheduling lane for resource isolation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lane {
    Main,
    #[default]
    Subagent,
    Cron,
    Nested,
}

impl Lane {
    pub fn default_max_concurrent(&self) -> usize {
        match self {
            Lane::Main => 2,
            Lane::Subagent => 8,
            Lane::Cron => 2,
            Lane::Nested => 4,
        }
    }

    pub fn default_priority(&self) -> i8 {
        match self {
            Lane::Main => 10,
            Lane::Nested => 8,
            Lane::Subagent => 5,
            Lane::Cron => 0,
        }
    }
}

/// Cleanup policy after run completion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CleanupPolicy {
    Delete,    // Immediate cleanup
    #[default]
    Keep,      // Keep for 1 hour
    Archive,   // Keep for 7 days
}

/// Run outcome for completed runs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub summary: String,
    pub output: Option<serde_json::Value>,
    pub artifacts_count: usize,
    pub tools_called: usize,
    pub duration_ms: u64,
}

/// SubAgentRun - Persistent record of a sub-agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRun {
    pub run_id: String,
    pub session_key: SessionKey,
    pub parent_session_key: SessionKey,
    pub task: String,
    pub agent_type: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub status: RunStatus,
    pub outcome: Option<RunOutcome>,
    pub error: Option<String>,
    pub lane: Lane,
    pub priority: u8,
    pub max_turns: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub checkpoint_id: Option<String>,
    pub retry_count: u32,
    pub cleanup_policy: CleanupPolicy,
}

impl SubAgentRun {
    pub fn new(
        session_key: SessionKey,
        parent_session_key: SessionKey,
        task: impl Into<String>,
        agent_type: impl Into<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        Self {
            run_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            parent_session_key,
            task: task.into(),
            agent_type: agent_type.into(),
            label: None,
            created_at: now,
            started_at: None,
            ended_at: None,
            archived_at: None,
            status: RunStatus::Pending,
            outcome: None,
            error: None,
            lane: Lane::default(),
            priority: 128,
            max_turns: None,
            timeout_ms: None,
            checkpoint_id: None,
            retry_count: 0,
            cleanup_policy: CleanupPolicy::default(),
        }
    }

    pub fn with_lane(mut self, lane: Lane) -> Self {
        self.lane = lane;
        self.priority = lane.default_priority() as u8 * 10 + 128;
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_run_status_transitions test_subagent_run_creation test_lane_default_quotas --no-default-features`
Expected: PASS (3 tests)

**Step 5: Update mod.rs exports**

Add to `core/src/agents/sub_agents/mod.rs`:
```rust
mod run;
pub use run::{SubAgentRun, RunStatus, RunOutcome, Lane, CleanupPolicy};
```

**Step 6: Commit**

```bash
git add core/src/agents/sub_agents/run.rs core/src/agents/sub_agents/mod.rs
git commit -m "feat(sub_agents): add SubAgentRun data model for Multi-Agent 2.0"
```

---

## Task 2: Add SubAgent Fact Types to FactType Enum

**Files:**
- Modify: `core/src/memory/context.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_subagent_fact_types() {
    assert_eq!(FactType::from_str("subagent_run"), FactType::SubagentRun);
    assert_eq!(FactType::from_str("subagent_session"), FactType::SubagentSession);
    assert_eq!(FactType::from_str("subagent_checkpoint"), FactType::SubagentCheckpoint);
    assert_eq!(FactType::SubagentRun.as_str(), "subagent_run");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_subagent_fact_types --no-default-features`
Expected: FAIL with "no variant named `SubagentRun`"

**Step 3: Write minimal implementation**

Add to `FactType` enum in `core/src/memory/context.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FactType {
    Preference,
    Plan,
    Learning,
    Project,
    Personal,
    #[default]
    Other,
    // Multi-Agent 2.0 fact types
    SubagentRun,
    SubagentSession,
    SubagentCheckpoint,
    SubagentTranscript,
}

impl FactType {
    pub fn as_str(&self) -> &str {
        match self {
            FactType::Preference => "preference",
            FactType::Plan => "plan",
            FactType::Learning => "learning",
            FactType::Project => "project",
            FactType::Personal => "personal",
            FactType::Other => "other",
            FactType::SubagentRun => "subagent_run",
            FactType::SubagentSession => "subagent_session",
            FactType::SubagentCheckpoint => "subagent_checkpoint",
            FactType::SubagentTranscript => "subagent_transcript",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "preference" => FactType::Preference,
            "plan" => FactType::Plan,
            "learning" => FactType::Learning,
            "project" => FactType::Project,
            "personal" => FactType::Personal,
            "subagent_run" => FactType::SubagentRun,
            "subagent_session" => FactType::SubagentSession,
            "subagent_checkpoint" => FactType::SubagentCheckpoint,
            "subagent_transcript" => FactType::SubagentTranscript,
            _ => FactType::Other,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_subagent_fact_types --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/context.rs
git commit -m "feat(memory): add SubAgent fact types for Multi-Agent 2.0 persistence"
```

---

## Task 3: Create SubAgentRegistry Core Structure

**Files:**
- Create: `core/src/agents/sub_agents/registry.rs`
- Modify: `core/src/agents/sub_agents/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = SubAgentRegistry::new_in_memory();
        let run = SubAgentRun::new(
            "session-1".into(),
            "parent-1".into(),
            "Test task",
            "explore",
        );
        let run_id = run.run_id.clone();

        registry.register(run).await.unwrap();

        let retrieved = registry.get(&run_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().task, "Test task");
    }

    #[tokio::test]
    async fn test_registry_get_by_session() {
        let registry = SubAgentRegistry::new_in_memory();
        let run = SubAgentRun::new(
            "session-abc".into(),
            "parent-1".into(),
            "Task",
            "plan",
        );
        let run_id = run.run_id.clone();

        registry.register(run).await.unwrap();

        let found = registry.get_by_session(&"session-abc".into()).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap(), run_id);
    }

    #[tokio::test]
    async fn test_registry_get_children() {
        let registry = SubAgentRegistry::new_in_memory();

        let run1 = SubAgentRun::new("s1".into(), "parent-x".into(), "Task 1", "explore");
        let run2 = SubAgentRun::new("s2".into(), "parent-x".into(), "Task 2", "plan");
        let run3 = SubAgentRun::new("s3".into(), "parent-y".into(), "Task 3", "execute");

        registry.register(run1).await.unwrap();
        registry.register(run2).await.unwrap();
        registry.register(run3).await.unwrap();

        let children = registry.get_children(&"parent-x".into()).await;
        assert_eq!(children.len(), 2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_registry_register_and_get --no-default-features`
Expected: FAIL with "cannot find struct `SubAgentRegistry`"

**Step 3: Write minimal implementation**

```rust
// core/src/agents/sub_agents/registry.rs

use std::collections::HashMap;
use tokio::sync::{RwLock, broadcast};
use crate::error::Result;
use crate::routing::SessionKey;
use super::run::{SubAgentRun, RunStatus};

#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    Registered { run_id: String },
    StatusChanged { run_id: String, old: RunStatus, new: RunStatus },
}

pub struct SubAgentRegistry {
    runs: RwLock<HashMap<String, SubAgentRun>>,
    by_session: RwLock<HashMap<SessionKey, String>>,
    by_parent: RwLock<HashMap<SessionKey, Vec<String>>>,
    event_tx: broadcast::Sender<LifecycleEvent>,
}

impl SubAgentRegistry {
    pub fn new_in_memory() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            runs: RwLock::new(HashMap::new()),
            by_session: RwLock::new(HashMap::new()),
            by_parent: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    pub async fn register(&self, run: SubAgentRun) -> Result<String> {
        let run_id = run.run_id.clone();
        self.runs.write().await.insert(run_id.clone(), run.clone());
        self.by_session.write().await.insert(run.session_key.clone(), run_id.clone());
        self.by_parent.write().await.entry(run.parent_session_key.clone())
            .or_default().push(run_id.clone());
        let _ = self.event_tx.send(LifecycleEvent::Registered { run_id: run_id.clone() });
        Ok(run_id)
    }

    pub async fn get(&self, run_id: &str) -> Result<Option<SubAgentRun>> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    pub async fn get_by_session(&self, key: &SessionKey) -> Option<String> {
        self.by_session.read().await.get(key).cloned()
    }

    pub async fn get_children(&self, parent: &SessionKey) -> Vec<String> {
        self.by_parent.read().await.get(parent).cloned().unwrap_or_default()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_registry_ --no-default-features`
Expected: PASS (3 tests)

**Step 5: Update mod.rs**

```rust
mod registry;
pub use registry::{SubAgentRegistry, LifecycleEvent};
```

**Step 6: Commit**

```bash
git add core/src/agents/sub_agents/registry.rs core/src/agents/sub_agents/mod.rs
git commit -m "feat(sub_agents): add SubAgentRegistry with in-memory indexing"
```

---

## Task 4: Implement Registry State Transitions

**Files:**
- Modify: `core/src/agents/sub_agents/registry.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_registry_transition() {
    let registry = SubAgentRegistry::new_in_memory();
    let run = SubAgentRun::new("s1".into(), "p1".into(), "Task", "explore");
    let run_id = run.run_id.clone();
    registry.register(run).await.unwrap();

    registry.transition(&run_id, RunStatus::Running).await.unwrap();
    let run = registry.get(&run_id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running);
    assert!(run.started_at.is_some());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_registry_transition --no-default-features`
Expected: FAIL with "no method named `transition`"

**Step 3: Add transition method**

```rust
impl SubAgentRegistry {
    pub async fn transition(&self, run_id: &str, new_status: RunStatus) -> Result<()> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)
            .ok_or_else(|| crate::error::AlephError::config("Run not found"))?;

        let old_status = run.status;
        if !old_status.can_transition_to(&new_status) {
            return Err(crate::error::AlephError::config("Invalid transition"));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;

        match new_status {
            RunStatus::Running => run.started_at = Some(now),
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled => {
                run.ended_at = Some(now);
            }
            _ => {}
        }
        run.status = new_status;

        let _ = self.event_tx.send(LifecycleEvent::StatusChanged {
            run_id: run_id.to_string(), old: old_status, new: new_status,
        });
        Ok(())
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_registry_transition --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agents/sub_agents/registry.rs
git commit -m "feat(sub_agents): add state transition to SubAgentRegistry"
```

---

## Task 5: Implement FactsDB Persistence Helpers

**Files:**
- Create: `core/src/agents/sub_agents/persistence.rs`
- Modify: `core/src/agents/sub_agents/mod.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_run_to_fact_conversion() {
    let run = SubAgentRun::new("s1".into(), "p1".into(), "Test task", "explore");
    let fact = SubAgentRunFact::from_run(&run);
    assert!(fact.id.starts_with("subagent:run:"));
    assert_eq!(fact.fact_type, FactType::SubagentRun);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_run_to_fact --no-default-features`
Expected: FAIL

**Step 3: Write implementation**

```rust
// core/src/agents/sub_agents/persistence.rs
use crate::memory::context::{FactType, MemoryFact, FactSpecificity, TemporalScope};
use crate::error::{AlephError, Result};
use super::run::SubAgentRun;

pub struct SubAgentRunFact;

impl SubAgentRunFact {
    pub fn from_run(run: &SubAgentRun) -> MemoryFact {
        MemoryFact {
            id: format!("subagent:run:{}", run.run_id),
            content: serde_json::to_string(run).unwrap_or_default(),
            fact_type: FactType::SubagentRun,
            embedding: None,
            source_memory_ids: vec![],
            created_at: run.created_at / 1000,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::Instance,
            temporal_scope: TemporalScope::Ephemeral,
            similarity_score: None,
        }
    }

    pub fn to_run(fact: &MemoryFact) -> Result<SubAgentRun> {
        serde_json::from_str(&fact.content)
            .map_err(|e| AlephError::config(format!("Deserialize failed: {}", e)))
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_run_to_fact --no-default-features`
Expected: PASS

**Step 5: Update mod.rs**

```rust
mod persistence;
pub use persistence::SubAgentRunFact;
```

**Step 6: Commit**

```bash
git add core/src/agents/sub_agents/persistence.rs core/src/agents/sub_agents/mod.rs
git commit -m "feat(sub_agents): add FactsDB persistence helpers"
```

---

## Task 6: Add Active Runs Query and Stats

**Files:**
- Modify: `core/src/agents/sub_agents/registry.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_get_active_runs() {
    let registry = SubAgentRegistry::new_in_memory();
    let run1 = SubAgentRun::new("s1".into(), "p1".into(), "Task 1", "explore");
    let run2 = SubAgentRun::new("s2".into(), "p1".into(), "Task 2", "plan");

    registry.register(run1.clone()).await.unwrap();
    registry.register(run2.clone()).await.unwrap();
    registry.transition(&run1.run_id, RunStatus::Running).await.unwrap();
    registry.transition(&run2.run_id, RunStatus::Running).await.unwrap();
    registry.transition(&run2.run_id, RunStatus::Completed).await.unwrap();

    let active = registry.get_active_runs().await;
    assert_eq!(active.len(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_get_active_runs --no-default-features`
Expected: FAIL

**Step 3: Add implementation**

```rust
impl SubAgentRegistry {
    pub async fn get_active_runs(&self) -> Vec<SubAgentRun> {
        self.runs.read().await.values()
            .filter(|r| !r.status.is_terminal())
            .cloned().collect()
    }

    pub async fn stats(&self) -> RegistryStats {
        let runs = self.runs.read().await;
        let mut stats = RegistryStats::default();
        for run in runs.values() {
            stats.total += 1;
            match run.status {
                RunStatus::Pending => stats.pending += 1,
                RunStatus::Running => stats.running += 1,
                RunStatus::Paused => stats.paused += 1,
                RunStatus::Completed => stats.completed += 1,
                RunStatus::Failed => stats.failed += 1,
                RunStatus::Cancelled => stats.cancelled += 1,
            }
        }
        stats
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub paused: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_get_active_runs --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agents/sub_agents/registry.rs
git commit -m "feat(sub_agents): add active runs query and stats"
```

---

## Task 7: Add SubAgent Fact Types to FactType Enum

**Files:**
- Modify: `core/src/memory/context.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_subagent_fact_types() {
    assert_eq!(FactType::from_str("subagent_run"), FactType::SubagentRun);
    assert_eq!(FactType::SubagentRun.as_str(), "subagent_run");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_subagent_fact_types --no-default-features`
Expected: FAIL

**Step 3: Add variants to FactType**

Add to `FactType` enum:
```rust
SubagentRun,
SubagentSession,
SubagentCheckpoint,
SubagentTranscript,
```

Update `as_str()`:
```rust
FactType::SubagentRun => "subagent_run",
FactType::SubagentSession => "subagent_session",
FactType::SubagentCheckpoint => "subagent_checkpoint",
FactType::SubagentTranscript => "subagent_transcript",
```

Update `from_str()`:
```rust
"subagent_run" => FactType::SubagentRun,
"subagent_session" => FactType::SubagentSession,
"subagent_checkpoint" => FactType::SubagentCheckpoint,
"subagent_transcript" => FactType::SubagentTranscript,
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_subagent_fact_types --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/context.rs
git commit -m "feat(memory): add SubAgent fact types"
```

---

## Task 8: BDD Integration Tests

**Files:**
- Create: `core/tests/features/sub_agents/registry.feature`
- Create: `core/tests/steps/sub_agent_registry_steps.rs`
- Modify: `core/tests/steps/mod.rs`

**Step 1: Write the BDD feature**

```gherkin
# core/tests/features/sub_agents/registry.feature

Feature: SubAgent Registry Lifecycle

  Scenario: Register and track a sub-agent run
    Given a fresh SubAgentRegistry
    When I register a sub-agent run with task "Explore codebase"
    Then the run should have status "Pending"
    And the run should be retrievable by its ID

  Scenario: State transitions follow lifecycle rules
    Given a fresh SubAgentRegistry
    And a registered sub-agent run
    When I transition the run to "Running"
    Then the run should have status "Running"
    And the run should have a started_at timestamp
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --test bdd registry`
Expected: FAIL

**Step 3: Write step definitions**

Create `core/tests/steps/sub_agent_registry_steps.rs` with step implementations.

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --test bdd registry`
Expected: PASS

**Step 5: Commit**

```bash
git add core/tests/features/sub_agents/ core/tests/steps/
git commit -m "test(sub_agents): add BDD tests for SubAgentRegistry"
```

---

## Summary

Phase 1 delivers the persistence foundation for Multi-Agent 2.0:

| Task | Component | Purpose |
|------|-----------|---------|
| 1 | `SubAgentRun` | Data model for run lifecycle |
| 2 | `FactType` variants | FactsDB integration |
| 3 | `SubAgentRegistry` | In-memory indexing |
| 4 | State transitions | Lifecycle management |
| 5 | `SubAgentRunFact` | Persistence helpers |
| 6 | Active runs query | Recovery support |
| 7 | Fact types | Memory integration |
| 8 | BDD tests | Integration validation |

**Next Phase:** Phase 2 implements `LaneScheduler` for resource isolation and anti-starvation.
