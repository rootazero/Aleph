# SharedArena Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement SharedArena — a multi-agent workspace collaboration layer that enables peer-level agents to share artifacts, progress, and memory while preserving the existing 1:1 Agent↔Workspace binding.

**Architecture:** SharedArena is a DDD aggregate root layered on top of existing infrastructure (EventBus for communication, LanceDB for memory settling, SQLite for persistence). Each agent retains its own workspace; the Arena provides a shared coordination protocol with partition-isolated slots per agent. SubAgents are excluded — they live entirely within the parent agent's workspace.

**Tech Stack:** Rust, Tokio, rusqlite (SQLite), LanceDB, serde, chrono, uuid

---

### Task 1: Core Domain Types

**Files:**
- Create: `core/src/arena/mod.rs`
- Create: `core/src/arena/types.rs`
- Modify: `core/src/lib.rs` (add `pub mod arena;`)
- Test: `core/src/arena/types.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Entity, AggregateRoot, ValueObject};

    #[test]
    fn arena_id_display_and_eq() {
        let id1 = ArenaId::new();
        let id2 = ArenaId::new();
        assert_ne!(id1, id2);
        assert_eq!(id1, id1.clone());
        assert!(!id1.to_string().is_empty());
    }

    #[test]
    fn arena_status_is_value_object() {
        let s1 = ArenaStatus::Created;
        let s2 = ArenaStatus::Created;
        assert_eq!(s1, s2);
        let s3 = s1.clone();
        assert_eq!(s1, s3);
    }

    #[test]
    fn participant_role_default_permissions() {
        let coord = ParticipantRole::Coordinator;
        let perms = ArenaPermissions::from_role(&coord);
        assert!(perms.can_merge);
        assert!(perms.can_write_own_slot);

        let worker = ParticipantRole::Worker;
        let perms = ArenaPermissions::from_role(&worker);
        assert!(!perms.can_merge);
        assert!(perms.can_write_own_slot);
        assert!(perms.can_read_other_slots);

        let observer = ParticipantRole::Observer;
        let perms = ArenaPermissions::from_role(&observer);
        assert!(!perms.can_merge);
        assert!(!perms.can_write_own_slot);
        assert!(perms.can_read_other_slots);
    }

    #[test]
    fn coordination_strategy_clone_eq() {
        let s1 = CoordinationStrategy::Peer {
            coordinator: "main".to_string(),
        };
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }

    #[test]
    fn artifact_content_variants() {
        let inline = ArtifactContent::Inline("hello".to_string());
        assert!(matches!(inline, ArtifactContent::Inline(_)));

        let reference = ArtifactContent::Reference(std::path::PathBuf::from("/tmp/file.txt"));
        assert!(matches!(reference, ArtifactContent::Reference(_)));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::types::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `arena` not found

**Step 3: Write minimal implementation**

In `core/src/lib.rs`, add after the `agents` module declaration:

```rust
pub mod arena;
```

In `core/src/arena/mod.rs`:

```rust
mod types;

pub use types::*;
```

In `core/src/arena/types.rs`:

```rust
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::{AggregateRoot, Entity, ValueObject};

// ===== Identity Types =====

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArenaId(String);

impl ArenaId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArenaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// AgentId is String in the codebase
pub type AgentId = String;

// ===== Arena Status (ValueObject) =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArenaStatus {
    Created,
    Active,
    Settling,
    Archived,
}

impl ValueObject for ArenaStatus {}

// ===== Coordination Strategy (ValueObject) =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordinationStrategy {
    Peer { coordinator: AgentId },
    Pipeline { stages: Vec<StageSpec> },
}

impl ValueObject for CoordinationStrategy {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageSpec {
    pub agent_id: AgentId,
    pub description: String,
    pub depends_on: Vec<AgentId>,
}

// ===== Participant =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticipantRole {
    Coordinator,
    Worker,
    Observer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArenaPermissions {
    pub can_write_own_slot: bool,
    pub can_read_other_slots: bool,
    pub can_write_shared_memory: bool,
    pub can_merge: bool,
}

impl ArenaPermissions {
    pub fn from_role(role: &ParticipantRole) -> Self {
        match role {
            ParticipantRole::Coordinator => Self {
                can_write_own_slot: true,
                can_read_other_slots: true,
                can_write_shared_memory: true,
                can_merge: true,
            },
            ParticipantRole::Worker => Self {
                can_write_own_slot: true,
                can_read_other_slots: true,
                can_write_shared_memory: true,
                can_merge: false,
            },
            ParticipantRole::Observer => Self {
                can_write_own_slot: false,
                can_read_other_slots: true,
                can_write_shared_memory: false,
                can_merge: false,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub agent_id: AgentId,
    pub role: ParticipantRole,
    pub permissions: ArenaPermissions,
}

// ===== Arena Manifest =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArenaManifest {
    pub goal: String,
    pub strategy: CoordinationStrategy,
    pub participants: Vec<Participant>,
    pub created_by: AgentId,
    pub created_at: DateTime<Utc>,
}

// ===== Artifact =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    Text,
    Code,
    File,
    StructuredData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactContent {
    Inline(String),
    Reference(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub content: ArtifactContent,
    pub metadata: HashMap<String, Value>,
    pub created_at: DateTime<Utc>,
}

// ===== Slot =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotStatus {
    Idle,
    Working,
    Done,
    Failed,
}

impl ValueObject for SlotStatus {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArenaSlot {
    pub agent_id: AgentId,
    pub artifacts: Vec<Artifact>,
    pub status: SlotStatus,
    pub updated_at: DateTime<Utc>,
}

impl ArenaSlot {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            artifacts: Vec::new(),
            status: SlotStatus::Idle,
            updated_at: Utc::now(),
        }
    }
}

// ===== Progress =====

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentProgress {
    pub assigned: Vec<String>,
    pub completed: Vec<String>,
    pub current: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArenaProgress {
    pub total_steps: usize,
    pub completed_steps: usize,
    pub agent_progress: HashMap<AgentId, AgentProgress>,
}

// ===== Shared Fact =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedFact {
    pub content: String,
    pub source_agent: AgentId,
    pub confidence: f32,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

// ===== Settle Report =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleReport {
    pub arena_id: ArenaId,
    pub facts_persisted: usize,
    pub artifacts_archived: usize,
    pub events_cleared: usize,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::types::tests -- --nocapture`
Expected: All 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/ core/src/lib.rs
git commit -m "arena: add core domain types for SharedArena"
```

---

### Task 2: SharedArena Aggregate Root

**Files:**
- Create: `core/src/arena/arena.rs`
- Modify: `core/src/arena/mod.rs` (add submodule)
- Test: `core/src/arena/arena.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/arena.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Entity, AggregateRoot};

    fn test_manifest() -> ArenaManifest {
        ArenaManifest {
            goal: "Test collaboration".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "agent-a".to_string(),
            },
            participants: vec![
                Participant {
                    agent_id: "agent-a".to_string(),
                    role: ParticipantRole::Coordinator,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
                },
                Participant {
                    agent_id: "agent-b".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
            ],
            created_by: "agent-a".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn create_arena_initializes_slots_per_participant() {
        let manifest = test_manifest();
        let arena = SharedArena::new(manifest);

        assert_eq!(arena.slots.len(), 2);
        assert!(arena.slots.contains_key("agent-a"));
        assert!(arena.slots.contains_key("agent-b"));
        assert_eq!(arena.status, ArenaStatus::Created);
    }

    #[test]
    fn arena_implements_entity_and_aggregate_root() {
        let arena = SharedArena::new(test_manifest());
        let _id: &ArenaId = arena.id(); // Entity trait
        fn assert_aggregate_root<T: AggregateRoot>() {}
        assert_aggregate_root::<SharedArena>();
    }

    #[test]
    fn activate_transitions_from_created() {
        let mut arena = SharedArena::new(test_manifest());
        assert!(arena.activate().is_ok());
        assert_eq!(arena.status, ArenaStatus::Active);
    }

    #[test]
    fn activate_fails_if_not_created() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();
        assert!(arena.activate().is_err());
    }

    #[test]
    fn put_artifact_to_own_slot() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("result".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };

        let result = arena.put_artifact("agent-b", artifact);
        assert!(result.is_ok());
        assert_eq!(arena.slots["agent-b"].artifacts.len(), 1);
    }

    #[test]
    fn put_artifact_fails_for_unknown_agent() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("result".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };

        let result = arena.put_artifact("unknown-agent", artifact);
        assert!(result.is_err());
    }

    #[test]
    fn begin_settling_transitions_from_active() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();
        assert!(arena.begin_settling().is_ok());
        assert_eq!(arena.status, ArenaStatus::Settling);
    }

    #[test]
    fn archive_transitions_from_settling() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();
        arena.begin_settling().unwrap();
        assert!(arena.archive().is_ok());
        assert_eq!(arena.status, ArenaStatus::Archived);
    }

    #[test]
    fn shared_facts_accumulate() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();

        arena.add_shared_fact(SharedFact {
            content: "Key insight".to_string(),
            source_agent: "agent-b".to_string(),
            confidence: 0.9,
            tags: vec!["insight".to_string()],
            created_at: Utc::now(),
        });

        assert_eq!(arena.shared_facts().len(), 1);
    }

    #[test]
    fn drain_shared_facts_empties_list() {
        let mut arena = SharedArena::new(test_manifest());
        arena.activate().unwrap();

        arena.add_shared_fact(SharedFact {
            content: "fact".to_string(),
            source_agent: "agent-a".to_string(),
            confidence: 0.8,
            tags: vec![],
            created_at: Utc::now(),
        });

        let facts = arena.drain_shared_facts();
        assert_eq!(facts.len(), 1);
        assert_eq!(arena.shared_facts().len(), 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::arena::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `arena` not found in `arena/mod.rs`

**Step 3: Write minimal implementation**

Update `core/src/arena/mod.rs`:

```rust
mod arena;
mod types;

pub use arena::*;
pub use types::*;
```

In `core/src/arena/arena.rs`:

```rust
use std::collections::HashMap;

use chrono::Utc;

use crate::domain::{AggregateRoot, Entity};

use super::types::*;

pub struct SharedArena {
    id: ArenaId,
    pub(crate) manifest: ArenaManifest,
    pub(crate) slots: HashMap<AgentId, ArenaSlot>,
    pub(crate) progress: ArenaProgress,
    pub(crate) status: ArenaStatus,
    shared_facts: Vec<SharedFact>,
}

impl Entity for SharedArena {
    type Id = ArenaId;
    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for SharedArena {}

impl SharedArena {
    pub fn new(manifest: ArenaManifest) -> Self {
        let slots: HashMap<AgentId, ArenaSlot> = manifest
            .participants
            .iter()
            .map(|p| (p.agent_id.clone(), ArenaSlot::new(p.agent_id.clone())))
            .collect();

        let mut agent_progress = HashMap::new();
        for p in &manifest.participants {
            agent_progress.insert(p.agent_id.clone(), AgentProgress::default());
        }

        Self {
            id: ArenaId::new(),
            manifest,
            slots,
            progress: ArenaProgress {
                total_steps: 0,
                completed_steps: 0,
                agent_progress,
            },
            status: ArenaStatus::Created,
            shared_facts: Vec::new(),
        }
    }

    pub fn manifest(&self) -> &ArenaManifest {
        &self.manifest
    }

    pub fn status(&self) -> &ArenaStatus {
        &self.status
    }

    pub fn progress(&self) -> &ArenaProgress {
        &self.progress
    }

    pub fn slots(&self) -> &HashMap<AgentId, ArenaSlot> {
        &self.slots
    }

    // ===== State transitions =====

    pub fn activate(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Created {
            return Err(format!("Cannot activate arena in {:?} state", self.status));
        }
        self.status = ArenaStatus::Active;
        Ok(())
    }

    pub fn begin_settling(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Active {
            return Err(format!("Cannot settle arena in {:?} state", self.status));
        }
        self.status = ArenaStatus::Settling;
        Ok(())
    }

    pub fn archive(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Settling {
            return Err(format!("Cannot archive arena in {:?} state", self.status));
        }
        self.status = ArenaStatus::Archived;
        Ok(())
    }

    // ===== Artifact operations =====

    pub fn put_artifact(&mut self, agent_id: &str, artifact: Artifact) -> Result<ArtifactId, String> {
        let slot = self.slots.get_mut(agent_id).ok_or_else(|| {
            format!("Agent '{}' is not a participant in this arena", agent_id)
        })?;
        let id = artifact.id.clone();
        slot.artifacts.push(artifact);
        slot.status = SlotStatus::Working;
        slot.updated_at = Utc::now();
        Ok(id)
    }

    pub fn get_artifacts(&self, agent_id: &str) -> Result<&[Artifact], String> {
        let slot = self.slots.get(agent_id).ok_or_else(|| {
            format!("Agent '{}' is not a participant in this arena", agent_id)
        })?;
        Ok(&slot.artifacts)
    }

    // ===== Progress =====

    pub fn report_progress(
        &mut self,
        agent_id: &str,
        current: Option<String>,
        completed: Option<String>,
    ) -> Result<(), String> {
        let progress = self.progress.agent_progress.get_mut(agent_id).ok_or_else(|| {
            format!("Agent '{}' is not a participant in this arena", agent_id)
        })?;
        if let Some(done) = completed {
            progress.completed.push(done);
            self.progress.completed_steps += 1;
        }
        progress.current = current;
        Ok(())
    }

    // ===== Shared facts =====

    pub fn add_shared_fact(&mut self, fact: SharedFact) {
        self.shared_facts.push(fact);
    }

    pub fn shared_facts(&self) -> &[SharedFact] {
        &self.shared_facts
    }

    pub fn drain_shared_facts(&mut self) -> Vec<SharedFact> {
        std::mem::take(&mut self.shared_facts)
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::arena::tests -- --nocapture`
Expected: All 9 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/
git commit -m "arena: add SharedArena aggregate root with state machine"
```

---

### Task 3: ArenaEvent Integration with EventBus

**Files:**
- Create: `core/src/arena/events.rs`
- Modify: `core/src/arena/mod.rs` (add submodule)
- Modify: `core/src/agents/swarm/events.rs` (add arena event variants)
- Test: `core/src/arena/events.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/events.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_event_serialization_roundtrip() {
        let event = ArenaEvent::ArtifactPublished {
            arena_id: ArenaId::from_string("test-arena"),
            agent_id: "agent-a".to_string(),
            artifact_id: ArtifactId::new(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ArenaEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ArenaEvent::ArtifactPublished { .. }));
    }

    #[test]
    fn arena_event_tier_classification() {
        let critical = ArenaEvent::ArtifactPublished {
            arena_id: ArenaId::from_string("a"),
            agent_id: "x".to_string(),
            artifact_id: ArtifactId::new(),
        };
        assert_eq!(critical.tier(), ArenaEventTier::Critical);

        let important = ArenaEvent::ProgressUpdated {
            arena_id: ArenaId::from_string("a"),
            agent_id: "x".to_string(),
            current: "working".to_string(),
        };
        assert_eq!(important.tier(), ArenaEventTier::Important);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::events::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `events` not found

**Step 3: Write minimal implementation**

In `core/src/arena/events.rs`:

```rust
use serde::{Deserialize, Serialize};

use super::types::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaEventTier {
    Critical,
    Important,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArenaEvent {
    ArtifactPublished {
        arena_id: ArenaId,
        agent_id: AgentId,
        artifact_id: ArtifactId,
    },
    StageCompleted {
        arena_id: ArenaId,
        agent_id: AgentId,
    },
    ProgressUpdated {
        arena_id: ArenaId,
        agent_id: AgentId,
        current: String,
    },
    MergeRequested {
        arena_id: ArenaId,
        coordinator: AgentId,
    },
    ConflictDetected {
        arena_id: ArenaId,
        description: String,
    },
    SettlingStarted {
        arena_id: ArenaId,
    },
}

impl ArenaEvent {
    pub fn tier(&self) -> ArenaEventTier {
        match self {
            ArenaEvent::ArtifactPublished { .. } | ArenaEvent::StageCompleted { .. } => {
                ArenaEventTier::Critical
            }
            _ => ArenaEventTier::Important,
        }
    }

    pub fn arena_id(&self) -> &ArenaId {
        match self {
            ArenaEvent::ArtifactPublished { arena_id, .. }
            | ArenaEvent::StageCompleted { arena_id, .. }
            | ArenaEvent::ProgressUpdated { arena_id, .. }
            | ArenaEvent::MergeRequested { arena_id, .. }
            | ArenaEvent::ConflictDetected { arena_id, .. }
            | ArenaEvent::SettlingStarted { arena_id } => arena_id,
        }
    }
}
```

Update `core/src/arena/mod.rs`:

```rust
mod arena;
mod events;
mod types;

pub use arena::*;
pub use events::*;
pub use types::*;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::events::tests -- --nocapture`
Expected: All 2 tests PASS

**Step 5: Integrate with swarm events**

Read `core/src/agents/swarm/events.rs` to understand the exact enum structure. Add an `Arena(ArenaEvent)` variant to `ImportantEvent` and `CriticalEvent` enums so ArenaEvents flow through the existing bus.

> **Note to implementer:** The exact modification depends on the current enum shapes in `events.rs`. Add the variant and ensure serde tagging is consistent. If the swarm event enums are not directly extensible, wrap ArenaEvent in AgentLoopEvent instead (see `core/src/agent_loop/events.rs`).

**Step 6: Commit**

```bash
git add core/src/arena/events.rs core/src/arena/mod.rs core/src/agents/swarm/events.rs
git commit -m "arena: add ArenaEvent types with tier classification"
```

---

### Task 4: ArenaHandle — Permission-Guarded Access

**Files:**
- Create: `core/src/arena/handle.rs`
- Modify: `core/src/arena/mod.rs` (add submodule)
- Test: `core/src/arena/handle.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/handle.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_arena() -> SharedArena {
        let manifest = ArenaManifest {
            goal: "Test".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "coord".to_string(),
            },
            participants: vec![
                Participant {
                    agent_id: "coord".to_string(),
                    role: ParticipantRole::Coordinator,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
                },
                Participant {
                    agent_id: "worker".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
                Participant {
                    agent_id: "observer".to_string(),
                    role: ParticipantRole::Observer,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Observer),
                },
            ],
            created_by: "coord".to_string(),
            created_at: Utc::now(),
        };
        let mut arena = SharedArena::new(manifest);
        arena.activate().unwrap();
        arena
    }

    #[test]
    fn worker_can_put_artifact_to_own_slot() {
        let arena = Arc::new(RwLock::new(test_arena()));
        let handle = ArenaHandle::new(
            arena.clone(),
            "worker".to_string(),
            ParticipantRole::Worker,
            ArenaPermissions::from_role(&ParticipantRole::Worker),
        );

        let artifact = Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("result".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };

        assert!(handle.put_artifact(artifact).is_ok());
    }

    #[test]
    fn observer_cannot_put_artifact() {
        let arena = Arc::new(RwLock::new(test_arena()));
        let handle = ArenaHandle::new(
            arena.clone(),
            "observer".to_string(),
            ParticipantRole::Observer,
            ArenaPermissions::from_role(&ParticipantRole::Observer),
        );

        let artifact = Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("result".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };

        assert!(handle.put_artifact(artifact).is_err());
    }

    #[test]
    fn worker_can_read_other_slots() {
        let arena = Arc::new(RwLock::new(test_arena()));

        // Coordinator puts an artifact
        let coord_handle = ArenaHandle::new(
            arena.clone(),
            "coord".to_string(),
            ParticipantRole::Coordinator,
            ArenaPermissions::from_role(&ParticipantRole::Coordinator),
        );
        let artifact = Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("coord result".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };
        coord_handle.put_artifact(artifact).unwrap();

        // Worker reads coordinator's slot
        let worker_handle = ArenaHandle::new(
            arena.clone(),
            "worker".to_string(),
            ParticipantRole::Worker,
            ArenaPermissions::from_role(&ParticipantRole::Worker),
        );
        let artifacts = worker_handle.list_artifacts("coord").unwrap();
        assert_eq!(artifacts.len(), 1);
    }

    #[test]
    fn only_coordinator_can_begin_settling() {
        let arena = Arc::new(RwLock::new(test_arena()));

        let worker_handle = ArenaHandle::new(
            arena.clone(),
            "worker".to_string(),
            ParticipantRole::Worker,
            ArenaPermissions::from_role(&ParticipantRole::Worker),
        );
        assert!(worker_handle.begin_settling().is_err());

        let coord_handle = ArenaHandle::new(
            arena.clone(),
            "coord".to_string(),
            ParticipantRole::Coordinator,
            ArenaPermissions::from_role(&ParticipantRole::Coordinator),
        );
        assert!(coord_handle.begin_settling().is_ok());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::handle::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `handle` not found

**Step 3: Write minimal implementation**

In `core/src/arena/handle.rs`:

```rust
use std::collections::HashMap;

use crate::sync_primitives::{Arc, RwLock};

use super::arena::SharedArena;
use super::types::*;

pub struct ArenaHandle {
    arena: Arc<RwLock<SharedArena>>,
    agent_id: AgentId,
    role: ParticipantRole,
    permissions: ArenaPermissions,
}

impl ArenaHandle {
    pub fn new(
        arena: Arc<RwLock<SharedArena>>,
        agent_id: AgentId,
        role: ParticipantRole,
        permissions: ArenaPermissions,
    ) -> Self {
        Self {
            arena,
            agent_id,
            role,
            permissions,
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn role(&self) -> &ParticipantRole {
        &self.role
    }

    // ===== Artifact operations =====

    pub fn put_artifact(&self, artifact: Artifact) -> Result<ArtifactId, String> {
        if !self.permissions.can_write_own_slot {
            return Err(format!(
                "Agent '{}' ({:?}) does not have write permission",
                self.agent_id, self.role
            ));
        }
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.put_artifact(&self.agent_id, artifact)
    }

    pub fn list_artifacts(&self, target_agent_id: &str) -> Result<Vec<Artifact>, String> {
        if target_agent_id != self.agent_id && !self.permissions.can_read_other_slots {
            return Err(format!(
                "Agent '{}' ({:?}) cannot read other agents' slots",
                self.agent_id, self.role
            ));
        }
        let arena = self.arena.read().unwrap_or_else(|e| e.into_inner());
        let artifacts = arena.get_artifacts(target_agent_id)?;
        Ok(artifacts.to_vec())
    }

    // ===== Progress =====

    pub fn report_progress(
        &self,
        current: Option<String>,
        completed: Option<String>,
    ) -> Result<(), String> {
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.report_progress(&self.agent_id, current, completed)
    }

    pub fn get_progress(&self) -> ArenaProgress {
        let arena = self.arena.read().unwrap_or_else(|e| e.into_inner());
        arena.progress().clone()
    }

    // ===== Shared facts =====

    pub fn add_shared_fact(&self, fact: SharedFact) -> Result<(), String> {
        if !self.permissions.can_write_shared_memory {
            return Err(format!(
                "Agent '{}' ({:?}) cannot write shared memory",
                self.agent_id, self.role
            ));
        }
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.add_shared_fact(fact);
        Ok(())
    }

    // ===== Coordinator-only =====

    pub fn begin_settling(&self) -> Result<(), String> {
        if !self.permissions.can_merge {
            return Err(format!(
                "Agent '{}' ({:?}) is not a coordinator — cannot settle",
                self.agent_id, self.role
            ));
        }
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.begin_settling()
    }
}
```

Update `core/src/arena/mod.rs`:

```rust
mod arena;
mod events;
mod handle;
mod types;

pub use arena::*;
pub use events::*;
pub use handle::*;
pub use types::*;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::handle::tests -- --nocapture`
Expected: All 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/
git commit -m "arena: add ArenaHandle with permission-guarded access"
```

---

### Task 5: ArenaManager — Lifecycle Management

**Files:**
- Create: `core/src/arena/manager.rs`
- Modify: `core/src/arena/mod.rs` (add submodule)
- Test: `core/src/arena/manager.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/manager.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_manifest() -> ArenaManifest {
        ArenaManifest {
            goal: "Analyze report".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "main".to_string(),
            },
            participants: vec![
                Participant {
                    agent_id: "main".to_string(),
                    role: ParticipantRole::Coordinator,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
                },
                Participant {
                    agent_id: "researcher".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
            ],
            created_by: "main".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn create_arena_returns_handles_for_all_participants() {
        let mut manager = ArenaManager::new();
        let (arena_id, handles) = manager.create_arena(test_manifest()).unwrap();

        assert_eq!(handles.len(), 2);
        assert!(handles.contains_key("main"));
        assert!(handles.contains_key("researcher"));
        assert!(!arena_id.as_str().is_empty());
    }

    #[test]
    fn get_handle_for_existing_participant() {
        let mut manager = ArenaManager::new();
        let (arena_id, _) = manager.create_arena(test_manifest()).unwrap();

        let handle = manager.get_handle(&arena_id, "main");
        assert!(handle.is_ok());
        assert_eq!(handle.unwrap().agent_id(), "main");
    }

    #[test]
    fn get_handle_fails_for_nonexistent_arena() {
        let manager = ArenaManager::new();
        let fake_id = ArenaId::from_string("nonexistent");
        assert!(manager.get_handle(&fake_id, "main").is_err());
    }

    #[test]
    fn active_arenas_for_agent() {
        let mut manager = ArenaManager::new();
        let _ = manager.create_arena(test_manifest()).unwrap();

        let active = manager.active_arenas_for("researcher");
        assert_eq!(active.len(), 1);

        let active = manager.active_arenas_for("unknown");
        assert_eq!(active.len(), 0);
    }

    #[test]
    fn settle_drains_facts_and_archives() {
        let mut manager = ArenaManager::new();
        let (arena_id, handles) = manager.create_arena(test_manifest()).unwrap();

        // Add a shared fact via handle
        let researcher_handle = &handles["researcher"];
        researcher_handle
            .add_shared_fact(SharedFact {
                content: "Key finding".to_string(),
                source_agent: "researcher".to_string(),
                confidence: 0.95,
                tags: vec!["analysis".to_string()],
                created_at: Utc::now(),
            })
            .unwrap();

        // Settle
        let coordinator_handle = &handles["main"];
        coordinator_handle.begin_settling().unwrap();

        let report = manager.settle(&arena_id).unwrap();
        assert_eq!(report.facts_persisted, 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::manager::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `manager` not found

**Step 3: Write minimal implementation**

In `core/src/arena/manager.rs`:

```rust
use std::collections::HashMap;

use crate::sync_primitives::{Arc, RwLock};

use super::arena::SharedArena;
use super::handle::ArenaHandle;
use super::types::*;

pub struct ArenaManager {
    arenas: HashMap<ArenaId, Arc<RwLock<SharedArena>>>,
}

impl ArenaManager {
    pub fn new() -> Self {
        Self {
            arenas: HashMap::new(),
        }
    }

    pub fn create_arena(
        &mut self,
        manifest: ArenaManifest,
    ) -> Result<(ArenaId, HashMap<AgentId, ArenaHandle>), String> {
        let mut arena = SharedArena::new(manifest.clone());
        arena.activate()?;

        let arena_id = arena.id().clone();
        let arena = Arc::new(RwLock::new(arena));

        let mut handles = HashMap::new();
        for participant in &manifest.participants {
            let handle = ArenaHandle::new(
                arena.clone(),
                participant.agent_id.clone(),
                participant.role.clone(),
                participant.permissions.clone(),
            );
            handles.insert(participant.agent_id.clone(), handle);
        }

        self.arenas.insert(arena_id.clone(), arena);
        Ok((arena_id, handles))
    }

    pub fn get_handle(
        &self,
        arena_id: &ArenaId,
        agent_id: &str,
    ) -> Result<ArenaHandle, String> {
        let arena = self
            .arenas
            .get(arena_id)
            .ok_or_else(|| format!("Arena '{}' not found", arena_id))?;

        let arena_read = arena.read().unwrap_or_else(|e| e.into_inner());
        let participant = arena_read
            .manifest()
            .participants
            .iter()
            .find(|p| p.agent_id == agent_id)
            .ok_or_else(|| {
                format!("Agent '{}' is not a participant in arena '{}'", agent_id, arena_id)
            })?;

        Ok(ArenaHandle::new(
            arena.clone(),
            participant.agent_id.clone(),
            participant.role.clone(),
            participant.permissions.clone(),
        ))
    }

    pub fn active_arenas_for(&self, agent_id: &str) -> Vec<ArenaId> {
        self.arenas
            .iter()
            .filter(|(_, arena)| {
                let arena = arena.read().unwrap_or_else(|e| e.into_inner());
                arena.status() != &ArenaStatus::Archived
                    && arena.slots().contains_key(agent_id)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Settle an arena: drain shared facts (to be persisted by caller),
    /// archive the arena, and return a report.
    pub fn settle(&mut self, arena_id: &ArenaId) -> Result<SettleReport, String> {
        let arena_lock = self
            .arenas
            .get(arena_id)
            .ok_or_else(|| format!("Arena '{}' not found", arena_id))?;

        let mut arena = arena_lock.write().unwrap_or_else(|e| e.into_inner());

        let facts = arena.drain_shared_facts();
        let facts_count = facts.len();

        // Count artifacts across all slots
        let artifacts_count: usize = arena.slots().values().map(|s| s.artifacts.len()).sum();

        arena.archive()?;

        Ok(SettleReport {
            arena_id: arena_id.clone(),
            facts_persisted: facts_count,
            artifacts_archived: artifacts_count,
            events_cleared: 0,
        })
    }
}
```

Update `core/src/arena/mod.rs`:

```rust
mod arena;
mod events;
mod handle;
mod manager;
mod types;

pub use arena::*;
pub use events::*;
pub use handle::*;
pub use manager::*;
pub use types::*;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::manager::tests -- --nocapture`
Expected: All 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/
git commit -m "arena: add ArenaManager with lifecycle management"
```

---

### Task 6: SQLite Persistence

**Files:**
- Create: `core/src/arena/storage.rs`
- Modify: `core/src/arena/mod.rs` (add submodule)
- Modify: `core/src/resilience/database/state_database.rs` (add arena schema in `create_schema`)
- Test: `core/src/arena/storage.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test**

In `core/src/arena/storage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::database::StateDatabase;
    use chrono::Utc;

    fn test_db() -> StateDatabase {
        StateDatabase::open_in_memory().unwrap()
    }

    fn test_manifest() -> ArenaManifest {
        ArenaManifest {
            goal: "Test task".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "agent-a".to_string(),
            },
            participants: vec![Participant {
                agent_id: "agent-a".to_string(),
                role: ParticipantRole::Coordinator,
                permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
            }],
            created_by: "agent-a".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn save_and_load_arena() {
        let db = test_db();
        let arena_id = ArenaId::from_string("test-1");
        let manifest = test_manifest();

        save_arena(&db, &arena_id, &manifest, &ArenaStatus::Active).unwrap();
        let loaded = load_arena(&db, &arena_id).unwrap();

        assert!(loaded.is_some());
        let (goal, status) = loaded.unwrap();
        assert_eq!(goal, "Test task");
        assert_eq!(status, "active");
    }

    #[test]
    fn update_arena_status() {
        let db = test_db();
        let arena_id = ArenaId::from_string("test-2");
        save_arena(&db, &arena_id, &test_manifest(), &ArenaStatus::Active).unwrap();

        update_arena_status(&db, &arena_id, &ArenaStatus::Archived).unwrap();
        let (_, status) = load_arena(&db, &arena_id).unwrap().unwrap();
        assert_eq!(status, "archived");
    }

    #[test]
    fn save_and_load_artifact() {
        let db = test_db();
        let arena_id = ArenaId::from_string("test-3");
        save_arena(&db, &arena_id, &test_manifest(), &ArenaStatus::Active).unwrap();

        let artifact_id = ArtifactId::new();
        save_artifact(
            &db,
            &artifact_id,
            &arena_id,
            "agent-a",
            &ArtifactKind::Text,
            Some("hello world"),
            None,
        )
        .unwrap();

        let artifacts = load_artifacts(&db, &arena_id, "agent-a").unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].1, "hello world");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::storage::tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — module not found

**Step 3: Add schema to StateDatabase**

Read `core/src/resilience/database/state_database.rs` to find `create_schema()` or `schema_sql()`. Add these tables:

```sql
CREATE TABLE IF NOT EXISTS arenas (
    id            TEXT PRIMARY KEY,
    goal          TEXT NOT NULL,
    strategy      TEXT NOT NULL,
    participants  TEXT NOT NULL,
    created_by    TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'created',
    created_at    TEXT NOT NULL,
    settled_at    TEXT,
    settle_report TEXT
);

CREATE TABLE IF NOT EXISTS arena_slots (
    arena_id      TEXT NOT NULL,
    agent_id      TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'idle',
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (arena_id, agent_id)
);

CREATE TABLE IF NOT EXISTS arena_artifacts (
    id            TEXT PRIMARY KEY,
    arena_id      TEXT NOT NULL,
    agent_id      TEXT NOT NULL,
    kind          TEXT NOT NULL,
    content       TEXT,
    reference     TEXT,
    metadata      TEXT,
    created_at    TEXT NOT NULL
);
```

> **Note to implementer:** Check whether StateDatabase uses a migration system or inline `CREATE TABLE IF NOT EXISTS`. Follow the existing pattern.

**Step 4: Implement storage functions**

In `core/src/arena/storage.rs`:

```rust
use crate::resilience::database::StateDatabase;
use super::types::*;

pub fn save_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
    manifest: &ArenaManifest,
    status: &ArenaStatus,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
    let status_str = status_to_str(status);
    let strategy_json = serde_json::to_string(&manifest.strategy).map_err(|e| e.to_string())?;
    let participants_json =
        serde_json::to_string(&manifest.participants).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO arenas (id, goal, strategy, participants, created_by, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            arena_id.as_str(),
            &manifest.goal,
            &strategy_json,
            &participants_json,
            &manifest.created_by,
            status_str,
            manifest.created_at.to_rfc3339(),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn update_arena_status(
    db: &StateDatabase,
    arena_id: &ArenaId,
    status: &ArenaStatus,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute(
        "UPDATE arenas SET status = ?1 WHERE id = ?2",
        rusqlite::params![status_to_str(status), arena_id.as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_arena(
    db: &StateDatabase,
    arena_id: &ArenaId,
) -> Result<Option<(String, String)>, String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = conn
        .prepare("SELECT goal, status FROM arenas WHERE id = ?1")
        .map_err(|e| e.to_string())?;

    let result = stmt
        .query_row(rusqlite::params![arena_id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result)
}

pub fn save_artifact(
    db: &StateDatabase,
    artifact_id: &ArtifactId,
    arena_id: &ArenaId,
    agent_id: &str,
    kind: &ArtifactKind,
    content: Option<&str>,
    reference: Option<&str>,
) -> Result<(), String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
    let kind_str = serde_json::to_string(kind).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT INTO arena_artifacts (id, arena_id, agent_id, kind, content, reference, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
        rusqlite::params![
            artifact_id.as_str(),
            arena_id.as_str(),
            agent_id,
            kind_str,
            content,
            reference,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_artifacts(
    db: &StateDatabase,
    arena_id: &ArenaId,
    agent_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let conn = db.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = conn
        .prepare(
            "SELECT id, content FROM arena_artifacts WHERE arena_id = ?1 AND agent_id = ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(rusqlite::params![arena_id.as_str(), agent_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

fn status_to_str(status: &ArenaStatus) -> &'static str {
    match status {
        ArenaStatus::Created => "created",
        ArenaStatus::Active => "active",
        ArenaStatus::Settling => "settling",
        ArenaStatus::Archived => "archived",
    }
}
```

> **Note to implementer:** Check if `StateDatabase.conn` is `pub(crate)` or needs a method accessor. Also check if `rusqlite::OptionalExtension` is imported for `.optional()`. Adjust access patterns to match the existing codebase style.

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::storage::tests -- --nocapture`
Expected: All 3 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/storage.rs core/src/arena/mod.rs core/src/resilience/database/
git commit -m "arena: add SQLite persistence for arenas and artifacts"
```

---

### Task 7: Memory Settling Integration

**Files:**
- Modify: `core/src/arena/manager.rs` (add async `settle_with_memory`)
- Test: `core/src/arena/manager.rs` (add integration test)

**Step 1: Write the failing test**

Add to `core/src/arena/manager.rs` tests:

```rust
#[tokio::test]
async fn settle_with_memory_persists_facts() {
    // This test requires a mock MemoryStore
    // Use the existing test-helpers pattern from the codebase

    let mut manager = ArenaManager::new();
    let (arena_id, handles) = manager.create_arena(test_manifest()).unwrap();

    handles["researcher"]
        .add_shared_fact(SharedFact {
            content: "Important discovery".to_string(),
            source_agent: "researcher".to_string(),
            confidence: 0.9,
            tags: vec!["finding".to_string()],
            created_at: Utc::now(),
        })
        .unwrap();

    handles["main"].begin_settling().unwrap();

    // settle() returns facts for the caller to persist to MemoryStore
    let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();
    assert_eq!(report.facts_persisted, 1);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "Important discovery");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib arena::manager::tests::settle_with_memory -- --nocapture`
Expected: FAIL — `settle_with_facts` method not found

**Step 3: Implement `settle_with_facts`**

Add to `ArenaManager`:

```rust
/// Settle and return drained facts for caller to persist to MemoryStore.
/// The caller is responsible for writing facts to MemoryStore with:
///   namespace: "shared", metadata: { arena_id, source_agent }
pub fn settle_with_facts(
    &mut self,
    arena_id: &ArenaId,
) -> Result<(SettleReport, Vec<SharedFact>), String> {
    let arena_lock = self
        .arenas
        .get(arena_id)
        .ok_or_else(|| format!("Arena '{}' not found", arena_id))?;

    let mut arena = arena_lock.write().unwrap_or_else(|e| e.into_inner());
    let facts = arena.drain_shared_facts();
    let facts_count = facts.len();
    let artifacts_count: usize = arena.slots().values().map(|s| s.artifacts.len()).sum();

    arena.archive()?;

    let report = SettleReport {
        arena_id: arena_id.clone(),
        facts_persisted: facts_count,
        artifacts_archived: artifacts_count,
        events_cleared: 0,
    };

    Ok((report, facts))
}
```

> **Note to implementer:** The actual MemoryStore write happens at the call site (Dispatcher or Agent Loop), not inside ArenaManager. This keeps ArenaManager free from async and MemoryStore dependency. Example call site code:
>
> ```rust
> let (report, facts) = arena_manager.settle_with_facts(&arena_id)?;
> for fact in facts {
>     memory_store.add(MemoryEntry {
>         content: fact.content,
>         workspace: arena_manifest.created_by.clone(),
>         namespace: "shared".to_string(),
>         metadata: json!({
>             "arena_id": arena_id.as_str(),
>             "source_agent": fact.source_agent,
>             "confidence": fact.confidence,
>             "tags": fact.tags,
>         }),
>         ..Default::default()
>     }).await?;
> }
> ```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib arena::manager::tests -- --nocapture`
Expected: All 6 tests PASS

**Step 5: Commit**

```bash
git add core/src/arena/manager.rs
git commit -m "arena: add settle_with_facts for memory settling integration"
```

---

### Task 8: Agent Loop Integration — RunContext

**Files:**
- Modify: `core/src/agent_loop/state.rs` or wherever RunContext is defined (add `arena_handles` field)
- Test: Verify compilation

> **Note to implementer:** This is an integration task. Read the exact RunContext struct location first. The change is adding one field:
>
> ```rust
> pub arena_handles: Vec<crate::arena::ArenaHandle>,
> ```
>
> And initializing it as `Vec::new()` in the constructor. ArenaHandle is not Clone (it holds `Arc<RwLock<SharedArena>>`), so consider using `Arc<ArenaHandle>` if RunContext needs to be cloned.

**Step 1: Find RunContext definition**

Run: `grep -rn "pub struct RunContext" core/src/`

**Step 2: Add arena_handles field**

Add the field and update all construction sites to include `arena_handles: Vec::new()`.

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors

**Step 4: Commit**

```bash
git add core/src/agent_loop/
git commit -m "arena: add arena_handles field to RunContext"
```

---

### Task 9: Dispatcher Integration — Collaborative Route

**Files:**
- Modify: `core/src/dispatcher/agent_types.rs` or equivalent (add `Collaborative` variant)
- Test: Verify compilation

> **Note to implementer:** Read the exact TaskRoute/TaskType enum. Add a variant like:
>
> ```rust
> Collaborative {
>     manifest: crate::arena::ArenaManifest,
> }
> ```
>
> The actual dispatching logic (creating Arena, distributing handles) will be implemented when the Dispatcher's execution engine is extended — that's a follow-up task outside this plan's scope.

**Step 1: Find TaskRoute/TaskType definition**

Run: `grep -rn "enum TaskType\|enum TaskRoute" core/src/dispatcher/`

**Step 2: Add Collaborative variant**

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors

**Step 4: Commit**

```bash
git add core/src/dispatcher/
git commit -m "arena: add Collaborative variant to task routing"
```

---

### Task 10: Full Integration Test

**Files:**
- Create: `core/src/arena/integration_test.rs` (or inline in `mod.rs`)
- Test: End-to-end scenario

**Step 1: Write integration test**

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    /// Simulates the Peer collaboration scenario from the design doc:
    /// "Analyze a technical report" with coordinator + researcher + coder
    #[test]
    fn peer_collaboration_full_lifecycle() {
        // 1. Create Arena
        let mut manager = ArenaManager::new();
        let manifest = ArenaManifest {
            goal: "Analyze technical report".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: "main".to_string(),
            },
            participants: vec![
                Participant {
                    agent_id: "main".to_string(),
                    role: ParticipantRole::Coordinator,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
                },
                Participant {
                    agent_id: "researcher".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
                Participant {
                    agent_id: "coder".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
            ],
            created_by: "main".to_string(),
            created_at: Utc::now(),
        };

        let (arena_id, handles) = manager.create_arena(manifest).unwrap();

        // 2. Workers produce artifacts (parallel in production, sequential here)
        let researcher = &handles["researcher"];
        researcher
            .put_artifact(Artifact {
                id: ArtifactId::new(),
                kind: ArtifactKind::Text,
                content: ArtifactContent::Inline("Key findings: ...".to_string()),
                metadata: HashMap::new(),
                created_at: Utc::now(),
            })
            .unwrap();
        researcher
            .report_progress(None, Some("Reading complete".to_string()))
            .unwrap();

        let coder = &handles["coder"];
        coder
            .put_artifact(Artifact {
                id: ArtifactId::new(),
                kind: ArtifactKind::Code,
                content: ArtifactContent::Inline("fn verify() { ... }".to_string()),
                metadata: HashMap::new(),
                created_at: Utc::now(),
            })
            .unwrap();
        coder
            .report_progress(None, Some("Code verification done".to_string()))
            .unwrap();

        // 3. Workers add shared facts
        researcher
            .add_shared_fact(SharedFact {
                content: "Report identifies 3 critical risks".to_string(),
                source_agent: "researcher".to_string(),
                confidence: 0.9,
                tags: vec!["risk".to_string()],
                created_at: Utc::now(),
            })
            .unwrap();

        // 4. Coordinator reads all slots
        let coordinator = &handles["main"];
        let researcher_artifacts = coordinator.list_artifacts("researcher").unwrap();
        assert_eq!(researcher_artifacts.len(), 1);
        let coder_artifacts = coordinator.list_artifacts("coder").unwrap();
        assert_eq!(coder_artifacts.len(), 1);

        // 5. Check progress
        let progress = coordinator.get_progress();
        assert_eq!(progress.completed_steps, 2);

        // 6. Settle
        coordinator.begin_settling().unwrap();
        let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();

        assert_eq!(report.facts_persisted, 1);
        assert_eq!(report.artifacts_archived, 2);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Report identifies 3 critical risks");
    }

    /// Simulates Pipeline scenario: translator → polisher
    #[test]
    fn pipeline_collaboration_full_lifecycle() {
        let mut manager = ArenaManager::new();
        let manifest = ArenaManifest {
            goal: "Translate and polish article".to_string(),
            strategy: CoordinationStrategy::Pipeline {
                stages: vec![
                    StageSpec {
                        agent_id: "translator".to_string(),
                        description: "Translate to Chinese".to_string(),
                        depends_on: vec![],
                    },
                    StageSpec {
                        agent_id: "polisher".to_string(),
                        description: "Polish translation".to_string(),
                        depends_on: vec!["translator".to_string()],
                    },
                ],
            },
            participants: vec![
                Participant {
                    agent_id: "translator".to_string(),
                    role: ParticipantRole::Coordinator, // First stage acts as coordinator
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Coordinator),
                },
                Participant {
                    agent_id: "polisher".to_string(),
                    role: ParticipantRole::Worker,
                    permissions: ArenaPermissions::from_role(&ParticipantRole::Worker),
                },
            ],
            created_by: "translator".to_string(),
            created_at: Utc::now(),
        };

        let (arena_id, handles) = manager.create_arena(manifest).unwrap();

        // Stage 1: Translator produces draft
        let translator = &handles["translator"];
        translator
            .put_artifact(Artifact {
                id: ArtifactId::new(),
                kind: ArtifactKind::Text,
                content: ArtifactContent::Inline("中文翻译初稿...".to_string()),
                metadata: HashMap::new(),
                created_at: Utc::now(),
            })
            .unwrap();
        translator
            .add_shared_fact(SharedFact {
                content: "Term mapping: quantum → 量子".to_string(),
                source_agent: "translator".to_string(),
                confidence: 1.0,
                tags: vec!["terminology".to_string()],
                created_at: Utc::now(),
            })
            .unwrap();

        // Stage 2: Polisher reads translator's output and refines
        let polisher = &handles["polisher"];
        let draft = polisher.list_artifacts("translator").unwrap();
        assert_eq!(draft.len(), 1);

        polisher
            .put_artifact(Artifact {
                id: ArtifactId::new(),
                kind: ArtifactKind::Text,
                content: ArtifactContent::Inline("中文润色终稿...".to_string()),
                metadata: HashMap::new(),
                created_at: Utc::now(),
            })
            .unwrap();

        // Settle
        translator.begin_settling().unwrap();
        let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();

        assert_eq!(report.facts_persisted, 1);
        assert_eq!(report.artifacts_archived, 2);
        assert_eq!(facts[0].content, "Term mapping: quantum → 量子");
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib arena::integration_tests -- --nocapture`
Expected: All 2 tests PASS

**Step 3: Run full arena test suite**

Run: `cargo test -p alephcore --lib arena -- --nocapture`
Expected: All tests PASS (types + arena + events + handle + manager + integration)

**Step 4: Commit**

```bash
git add core/src/arena/
git commit -m "arena: add full integration tests for peer and pipeline scenarios"
```

---

## Summary

| Task | Component | Files | Tests |
|------|-----------|-------|-------|
| 1 | Core domain types | `arena/types.rs`, `arena/mod.rs`, `lib.rs` | 5 |
| 2 | SharedArena aggregate root | `arena/arena.rs` | 9 |
| 3 | ArenaEvent + tier classification | `arena/events.rs`, swarm events | 2 |
| 4 | ArenaHandle (permission guard) | `arena/handle.rs` | 4 |
| 5 | ArenaManager (lifecycle) | `arena/manager.rs` | 5 |
| 6 | SQLite persistence | `arena/storage.rs`, StateDatabase schema | 3 |
| 7 | Memory settling | `arena/manager.rs` (extend) | 1 |
| 8 | Agent Loop integration | `agent_loop/state.rs` | compile check |
| 9 | Dispatcher integration | `dispatcher/agent_types.rs` | compile check |
| 10 | Full integration test | `arena/` (inline) | 2 |
| **Total** | | | **~31 tests** |

### Out of Scope (follow-up work)

- Dispatcher execution engine for `Collaborative` route (creating Arena, distributing handles, managing agent execution)
- ContextInjector extension to inject Arena state into team_awareness XML
- RPC handlers for Arena operations (create, query, settle via Gateway)
- Arena tools (AlephTool implementations for agents to interact with Arena)
