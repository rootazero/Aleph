# Logic Review System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a three-layer logic bug detection system — L1 proptest (property testing), L2 loom (concurrency verification), L3 AI semantic audit skill.

**Architecture:** Three independent defense layers. L1/L2 run automatically in CI via `cargo test`. L3 is a Claude Code skill triggered manually via `/review-logic`. Each layer targets a different class of logic bugs and can be developed independently.

**Tech Stack:** proptest 1.4, loom 0.7, Claude Code skill (.md), justfile, GitHub Actions

**Design Doc:** `docs/plans/2026-02-28-logic-review-system-design.md`

---

## Phase 0: Infrastructure

### Task 1: Add proptest and loom dependencies

**Files:**
- Modify: `core/Cargo.toml:11` (features section) and `:232` (dev-dependencies section)

**Step 1: Add loom feature flag and both dev-dependencies**

In `core/Cargo.toml`, add `loom` to the `[features]` section (after `test-helpers = []`):

```toml
# Loom concurrency testing (replaces std sync primitives)
loom = ["dep:loom"]
```

Add to `[dev-dependencies]` (after `aleph-protocol`):

```toml
proptest = "1.4"
loom = { version = "0.7", optional = true }
```

Note: `loom` is `optional = true` because it's activated via feature flag, not always included.

**Step 2: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles without errors. proptest and loom are downloaded but not yet used.

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add proptest and loom for logic review system"
```

---

### Task 2: Create sync_primitives module

**Files:**
- Create: `core/src/sync_primitives.rs`
- Modify: `core/src/lib.rs:94` (add module declaration)

**Step 1: Write the sync_primitives module**

Create `core/src/sync_primitives.rs`:

```rust
//! Conditional sync primitives for loom compatibility.
//!
//! Under normal compilation, these re-export `std::sync` types at zero cost.
//! Under `--features loom` (with `RUSTFLAGS="--cfg loom"`), these switch to
//! loom's instrumented versions that enable exhaustive concurrency testing.

#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
```

**Step 2: Register the module in lib.rs**

In `core/src/lib.rs`, add after line 98 (`pub mod secrets;`):

```rust
pub(crate) mod sync_primitives;
```

**Step 3: Verify compilation in both modes**

Run: `cargo check --workspace`
Expected: PASS

Run: `RUSTFLAGS="--cfg loom" cargo check --workspace --features loom`
Expected: PASS (loom primitives resolve correctly)

**Step 4: Commit**

```bash
git add core/src/sync_primitives.rs core/src/lib.rs
git commit -m "infra: add sync_primitives module for loom conditional compilation"
```

---

### Task 3: Add justfile commands

**Files:**
- Modify: `justfile:121` (after existing `test` recipe)

**Step 1: Add three new recipes**

After the `test` recipe (line 122), add:

```makefile

# Run proptest with high coverage (1024 cases per test)
test-proptest:
    PROPTEST_CASES=1024 cargo test --workspace --lib

# Run loom concurrency tests
test-loom:
    RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib

# Run full logic review suite (proptest + loom)
test-logic: test-proptest test-loom
```

**Step 2: Verify recipes are listed**

Run: `just --list`
Expected: Shows `test-proptest`, `test-loom`, `test-logic` in the list.

**Step 3: Commit**

```bash
git add justfile
git commit -m "build: add test-proptest, test-loom, test-logic just recipes"
```

---

### Task 4: Add loom CI job

**Files:**
- Modify: `.github/workflows/rust-core.yml:28` (after test job)

**Step 1: Add environment variable to existing test job**

In the `test` job, add `PROPTEST_CASES` environment variable to the "Run tests" step:

```yaml
      - name: Run tests
        run: cargo test --workspace
        env:
          PROPTEST_CASES: 1024
```

**Step 2: Add loom-tests job**

After the `lint` job (after line 44), add:

```yaml
  loom-tests:
    name: Loom Concurrency
    runs-on: ubuntu-latest
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: core
      - name: Run loom tests
        run: cargo test --features loom --lib
        env:
          RUSTFLAGS: "--cfg loom"
          LOOM_MAX_PREEMPTIONS: 3
        timeout-minutes: 30
```

**Step 3: Commit**

```bash
git add .github/workflows/rust-core.yml
git commit -m "ci: add loom concurrency test job and increase proptest coverage"
```

---

## Phase 1: Core Module Property Tests

### Task 5: dispatcher — TaskGraph DAG invariants (proptest)

**Files:**
- Create: `core/src/dispatcher/agent_types/proptest_graph.rs`
- Modify: `core/src/dispatcher/agent_types/mod.rs` (add `#[cfg(test)] mod proptest_graph;`)

**Step 1: Read existing code to understand test patterns**

Read these files to understand the types and existing tests:
- `core/src/dispatcher/agent_types/graph.rs` — TaskGraph struct, validate(), topological_order()
- `core/src/dispatcher/agent_types/task.rs` — Task, TaskStatus enum
- `core/src/dispatcher/agent_types/mod.rs` — module declarations

**Step 2: Write the proptest file**

Create `core/src/dispatcher/agent_types/proptest_graph.rs`:

```rust
use super::graph::{TaskDependency, TaskGraph, TaskGraphMeta};
use super::task::{Task, TaskStatus};
use proptest::prelude::*;
use std::collections::HashSet;

/// Strategy: generate a TaskGraph with N tasks and random edges
fn arb_task_graph(max_tasks: usize, max_edges: usize) -> impl Strategy<Value = TaskGraph> {
    (1..=max_tasks)
        .prop_flat_map(move |n| {
            let edges = prop::collection::vec((0..n, 0..n), 0..max_edges);
            (Just(n), edges)
        })
        .prop_map(|(n, raw_edges)| {
            let mut graph = TaskGraph {
                id: "test-graph".to_string(),
                tasks: (0..n)
                    .map(|i| Task::new(&format!("task-{i}"), &format!("Task {i}")))
                    .collect(),
                edges: Vec::new(),
                metadata: TaskGraphMeta {
                    title: "Test".to_string(),
                    created_at: 0,
                    estimated_duration: None,
                    original_request: None,
                },
            };
            for (from_idx, to_idx) in raw_edges {
                if from_idx != to_idx {
                    graph.edges.push(TaskDependency {
                        from: format!("task-{from_idx}"),
                        to: format!("task-{to_idx}"),
                    });
                }
            }
            graph
        })
}

proptest! {
    /// validate() should never panic, regardless of input graph structure
    #[test]
    fn validate_never_panics(graph in arb_task_graph(20, 30)) {
        let _ = graph.validate();
    }

    /// A graph with no edges always validates successfully
    #[test]
    fn no_edges_always_valid(n in 1..20usize) {
        let graph = TaskGraph {
            id: "test".to_string(),
            tasks: (0..n)
                .map(|i| Task::new(&format!("task-{i}"), &format!("Task {i}")))
                .collect(),
            edges: Vec::new(),
            metadata: TaskGraphMeta {
                title: "Test".to_string(),
                created_at: 0,
                estimated_duration: None,
                original_request: None,
            },
        };
        prop_assert!(graph.validate().is_ok());
    }

    /// Self-loops are always detected by validate()
    #[test]
    fn self_loop_always_detected(n in 1..10usize, loop_idx in 0..10usize) {
        let loop_idx = loop_idx % n;
        let mut graph = TaskGraph {
            id: "test".to_string(),
            tasks: (0..n)
                .map(|i| Task::new(&format!("task-{i}"), &format!("Task {i}")))
                .collect(),
            edges: vec![TaskDependency {
                from: format!("task-{loop_idx}"),
                to: format!("task-{loop_idx}"),
            }],
            metadata: TaskGraphMeta {
                title: "Test".to_string(),
                created_at: 0,
                estimated_duration: None,
                original_request: None,
            },
        };
        prop_assert!(graph.validate().is_err());
    }

    /// If validate() succeeds, topological_order() returns all tasks
    #[test]
    fn valid_graph_topo_order_complete(graph in arb_task_graph(15, 20)) {
        if graph.validate().is_ok() {
            let order = graph.topological_order();
            prop_assert_eq!(order.len(), graph.tasks.len());
            // All task IDs are present
            let order_ids: HashSet<_> = order.iter().map(|t| t.id.as_str()).collect();
            let task_ids: HashSet<_> = graph.tasks.iter().map(|t| t.id.as_str()).collect();
            prop_assert_eq!(order_ids, task_ids);
        }
    }

    /// Root tasks have no predecessors
    #[test]
    fn root_tasks_have_no_predecessors(graph in arb_task_graph(10, 15)) {
        if graph.validate().is_ok() {
            let roots = graph.get_root_tasks();
            for root in &roots {
                let preds = graph.get_predecessors(&root.id);
                prop_assert!(preds.is_empty(), "Root task {} has predecessors", root.id);
            }
        }
    }

    /// Leaf tasks have no successors
    #[test]
    fn leaf_tasks_have_no_successors(graph in arb_task_graph(10, 15)) {
        if graph.validate().is_ok() {
            let leaves = graph.get_leaf_tasks();
            for leaf in &leaves {
                let succs = graph.get_successors(&leaf.id);
                prop_assert!(succs.is_empty(), "Leaf task {} has successors", leaf.id);
            }
        }
    }

    /// overall_progress is always in [0.0, 1.0]
    #[test]
    fn progress_always_bounded(graph in arb_task_graph(10, 10)) {
        let progress = graph.overall_progress();
        prop_assert!(progress >= 0.0 && progress <= 1.0,
            "Progress {} out of bounds", progress);
    }
}
```

**Step 3: Register the test module**

In `core/src/dispatcher/agent_types/mod.rs`, add at the end of the file:

```rust
#[cfg(test)]
mod proptest_graph;
```

**Step 4: Run to verify all pass**

Run: `cargo test --lib dispatcher::agent_types::proptest_graph -- --nocapture`
Expected: All 7 proptest tests pass (256 cases each).

**Step 5: Commit**

```bash
git add core/src/dispatcher/agent_types/proptest_graph.rs core/src/dispatcher/agent_types/mod.rs
git commit -m "test(dispatcher): add proptest for TaskGraph DAG invariants"
```

---

### Task 6: dispatcher — TaskStatus state transition properties (proptest)

**Files:**
- Create: `core/src/dispatcher/agent_types/proptest_task.rs`
- Modify: `core/src/dispatcher/agent_types/mod.rs` (add `#[cfg(test)] mod proptest_task;`)

**Step 1: Read TaskStatus and Task types**

Read `core/src/dispatcher/agent_types/task.rs` — particularly TaskStatus enum (line 333), Task struct, and any transition methods.

**Step 2: Write the proptest file**

Create `core/src/dispatcher/agent_types/proptest_task.rs`:

```rust
use super::task::{Task, TaskStatus, TaskType, TaskResult};
use proptest::prelude::*;

fn arb_task_status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
        Just(TaskStatus::Pending),
        (0.0..1.0f32, any::<Option<String>>())
            .prop_map(|(p, m)| TaskStatus::Running { progress: p, message: m }),
        any::<String>()
            .prop_map(|s| TaskStatus::Completed {
                result: TaskResult::text(s),
            }),
        (any::<String>(), any::<bool>())
            .prop_map(|(e, r)| TaskStatus::Failed { error: e, recoverable: r }),
        Just(TaskStatus::Cancelled),
    ]
}

proptest! {
    /// TaskStatus serde roundtrip: serialize → deserialize = original
    #[test]
    fn task_status_serde_roundtrip(status in arb_task_status()) {
        let json = serde_json::to_string(&status).unwrap();
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        // Compare via debug representation since TaskStatus may not impl PartialEq
        prop_assert_eq!(format!("{:?}", status), format!("{:?}", back));
    }

    /// Task with any status always has a non-empty ID
    #[test]
    fn task_always_has_id(
        name in "[a-z]{1,20}",
        desc in ".*",
    ) {
        let task = Task::new(&name, &desc);
        prop_assert!(!task.id.is_empty());
    }

    /// Default TaskStatus is Pending
    #[test]
    fn default_status_is_pending(_dummy in 0..1u8) {
        let status = TaskStatus::default();
        prop_assert!(matches!(status, TaskStatus::Pending));
    }

    /// Running progress is always in [0.0, 1.0] when constructed via builder
    #[test]
    fn running_progress_bounded(progress in any::<f32>()) {
        let status = TaskStatus::Running {
            progress: progress.clamp(0.0, 1.0),
            message: None,
        };
        if let TaskStatus::Running { progress: p, .. } = status {
            prop_assert!(p >= 0.0 && p <= 1.0);
        }
    }
}
```

**Step 3: Register the test module**

In `core/src/dispatcher/agent_types/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_task;
```

**Step 4: Run tests**

Run: `cargo test --lib dispatcher::agent_types::proptest_task -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/dispatcher/agent_types/proptest_task.rs core/src/dispatcher/agent_types/mod.rs
git commit -m "test(dispatcher): add proptest for TaskStatus serde and invariants"
```

---

### Task 7: gateway — Protocol message serde roundtrip (proptest)

**Files:**
- Create: `core/src/gateway/proptest_protocol.rs`
- Modify: `core/src/gateway/mod.rs` (add `#[cfg(test)] mod proptest_protocol;`)

**Step 1: Read protocol types**

Read `core/src/gateway/protocol.rs` — JsonRpcRequest (line 36), JsonRpcResponse (line 91), JsonRpcError, ToolCallParams (line 205), ToolCallResult (line 232).

**Step 2: Write the proptest file**

Create `core/src/gateway/proptest_protocol.rs`:

```rust
use super::protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError};
use proptest::prelude::*;
use serde_json::Value;

fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,50}".prop_map(|s| Value::String(s)),
    ]
}

fn arb_json_rpc_request() -> impl Strategy<Value = JsonRpcRequest> {
    (
        "[a-z.]{1,30}",              // method
        prop::option::of(arb_json_value()), // params
        prop::option::of(arb_json_value()), // id
    )
        .prop_map(|(method, params, id)| JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method,
            params,
            id,
        })
}

fn arb_json_rpc_error() -> impl Strategy<Value = JsonRpcError> {
    (
        prop::num::i32::ANY,
        "[a-zA-Z ]{1,50}",
        prop::option::of(arb_json_value()),
    )
        .prop_map(|(code, message, data)| JsonRpcError {
            code,
            message,
            data,
        })
}

proptest! {
    /// JsonRpcRequest serde roundtrip
    #[test]
    fn request_serde_roundtrip(req in arb_json_rpc_request()) {
        let json = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(req.method, back.method);
        prop_assert_eq!(req.jsonrpc, back.jsonrpc);
        prop_assert_eq!(req.params, back.params);
        prop_assert_eq!(req.id, back.id);
    }

    /// JsonRpcError serde roundtrip
    #[test]
    fn error_serde_roundtrip(err in arb_json_rpc_error()) {
        let json = serde_json::to_string(&err).unwrap();
        let back: JsonRpcError = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(err.code, back.code);
        prop_assert_eq!(err.message, back.message);
    }

    /// jsonrpc field is always "2.0" after roundtrip
    #[test]
    fn jsonrpc_version_preserved(req in arb_json_rpc_request()) {
        let json = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back.jsonrpc, "2.0");
    }

    /// Request with no params serializes to JSON without "params" key (or with null)
    #[test]
    fn no_params_serialization(method in "[a-z.]{1,20}") {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.clone(),
            params: None,
            id: Some(Value::Number(1.into())),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        // Roundtrip preserves None params
        prop_assert!(back.params.is_none() || back.params == Some(Value::Null));
    }
}
```

**Step 3: Register the test module**

In `core/src/gateway/mod.rs`, find the `#[cfg(test)]` section or add at the end:

```rust
#[cfg(test)]
mod proptest_protocol;
```

**Step 4: Run tests**

Run: `cargo test --lib gateway::proptest_protocol --features gateway -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/gateway/proptest_protocol.rs core/src/gateway/mod.rs
git commit -m "test(gateway): add proptest for JSON-RPC protocol serde roundtrip"
```

---

### Task 8: gateway — Channel types serde roundtrip (proptest)

**Files:**
- Create: `core/src/gateway/proptest_channel.rs`
- Modify: `core/src/gateway/mod.rs` (add `#[cfg(test)] mod proptest_channel;`)

**Step 1: Read channel types**

Read `core/src/gateway/channel.rs` — ChannelId, ConversationId, InboundMessage, OutboundMessage, ChannelStatus, PairingData, Attachment, ChannelCapabilities.

**Step 2: Write the proptest file**

Create `core/src/gateway/proptest_channel.rs`:

```rust
use super::channel::*;
use proptest::prelude::*;

fn arb_channel_status() -> impl Strategy<Value = ChannelStatus> {
    prop_oneof![
        Just(ChannelStatus::Disconnected),
        Just(ChannelStatus::Connecting),
        Just(ChannelStatus::Connected),
        "[a-zA-Z ]{1,30}".prop_map(|e| ChannelStatus::Error(e)),
        Just(ChannelStatus::Disabled),
    ]
}

fn arb_pairing_data() -> impl Strategy<Value = PairingData> {
    prop_oneof![
        Just(PairingData::None),
        "[a-zA-Z0-9]{4,8}".prop_map(PairingData::Code),
        "[a-zA-Z0-9+/=]{10,50}".prop_map(PairingData::QrCode),
    ]
}

proptest! {
    /// ChannelId display matches inner string
    #[test]
    fn channel_id_display(s in "[a-z0-9-]{1,30}") {
        let id = ChannelId::new(&s);
        prop_assert_eq!(id.to_string(), s);
    }

    /// ChannelStatus serde roundtrip
    #[test]
    fn channel_status_roundtrip(status in arb_channel_status()) {
        let json = serde_json::to_string(&status).unwrap();
        let back: ChannelStatus = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(format!("{:?}", status), format!("{:?}", back));
    }

    /// PairingData serde roundtrip
    #[test]
    fn pairing_data_roundtrip(data in arb_pairing_data()) {
        let json = serde_json::to_string(&data).unwrap();
        let back: PairingData = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(format!("{:?}", data), format!("{:?}", back));
    }

    /// ChannelCapabilities: all defaults are false/zero
    #[test]
    fn capabilities_default_is_minimal(_dummy in 0..1u8) {
        let caps = ChannelCapabilities::default();
        prop_assert!(!caps.supports_attachments);
        prop_assert!(!caps.supports_reactions);
        prop_assert!(!caps.supports_threads);
    }
}
```

**Step 3: Register the test module**

In `core/src/gateway/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_channel;
```

**Step 4: Run tests**

Run: `cargo test --lib gateway::proptest_channel --features gateway -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/gateway/proptest_channel.rs core/src/gateway/mod.rs
git commit -m "test(gateway): add proptest for channel types serde roundtrip"
```

---

### Task 9: memory — FactType and enum roundtrip (proptest)

**Files:**
- Create: `core/src/memory/proptest_enums.rs`
- Modify: `core/src/memory/mod.rs` (add `#[cfg(test)] mod proptest_enums;`)

**Step 1: Read memory enum types**

Read `core/src/memory/context/enums.rs` — FactType (line 14), MemoryLayer (line 188), MemoryCategory (line 240), MemoryTier (line 301), MemoryScope (line 356). Pay attention to `as_str()` and `from_str()` methods.

**Step 2: Write the proptest file**

Create `core/src/memory/proptest_enums.rs`:

```rust
use crate::memory::context::enums::*;
use proptest::prelude::*;
use std::collections::HashSet;

fn arb_fact_type() -> impl Strategy<Value = FactType> {
    prop_oneof![
        Just(FactType::Preference),
        Just(FactType::Plan),
        Just(FactType::Learning),
        Just(FactType::Project),
        Just(FactType::Personal),
        Just(FactType::Tool),
        Just(FactType::Other),
        Just(FactType::SubagentRun),
        Just(FactType::SubagentSession),
        Just(FactType::SubagentCheckpoint),
        Just(FactType::SubagentTranscript),
    ]
}

fn arb_memory_layer() -> impl Strategy<Value = MemoryLayer> {
    prop_oneof![
        Just(MemoryLayer::L0Abstract),
        Just(MemoryLayer::L1Overview),
        Just(MemoryLayer::L2Detail),
    ]
}

fn arb_memory_tier() -> impl Strategy<Value = MemoryTier> {
    prop_oneof![
        Just(MemoryTier::Core),
        Just(MemoryTier::ShortTerm),
        Just(MemoryTier::LongTerm),
    ]
}

fn arb_memory_scope() -> impl Strategy<Value = MemoryScope> {
    prop_oneof![
        Just(MemoryScope::Global),
        Just(MemoryScope::Workspace),
        Just(MemoryScope::Persona),
    ]
}

proptest! {
    /// FactType serde roundtrip
    #[test]
    fn fact_type_serde_roundtrip(ft in arb_fact_type()) {
        let json = serde_json::to_string(&ft).unwrap();
        let back: FactType = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(ft, back);
    }

    /// FactType as_str → from_str roundtrip
    #[test]
    fn fact_type_str_roundtrip(ft in arb_fact_type()) {
        let s = ft.as_str();
        let back = FactType::from_str(s);
        prop_assert_eq!(ft, back);
    }

    /// FactType default_path always starts with "aleph://"
    #[test]
    fn fact_type_path_prefix(ft in arb_fact_type()) {
        let path = ft.default_path();
        prop_assert!(path.starts_with("aleph://"),
            "Path '{}' doesn't start with aleph://", path);
    }

    /// MemoryLayer serde roundtrip
    #[test]
    fn memory_layer_roundtrip(layer in arb_memory_layer()) {
        let json = serde_json::to_string(&layer).unwrap();
        let back: MemoryLayer = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(layer, back);
    }

    /// MemoryTier serde roundtrip
    #[test]
    fn memory_tier_roundtrip(tier in arb_memory_tier()) {
        let json = serde_json::to_string(&tier).unwrap();
        let back: MemoryTier = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(tier, back);
    }

    /// MemoryScope serde roundtrip
    #[test]
    fn memory_scope_roundtrip(scope in arb_memory_scope()) {
        let json = serde_json::to_string(&scope).unwrap();
        let back: MemoryScope = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(scope, back);
    }

    /// All FactType as_str values are unique
    #[test]
    fn fact_type_as_str_unique(_dummy in 0..1u8) {
        let all = vec![
            FactType::Preference, FactType::Plan, FactType::Learning,
            FactType::Project, FactType::Personal, FactType::Tool,
            FactType::Other, FactType::SubagentRun, FactType::SubagentSession,
            FactType::SubagentCheckpoint, FactType::SubagentTranscript,
        ];
        let strs: HashSet<_> = all.iter().map(|ft| ft.as_str()).collect();
        prop_assert_eq!(strs.len(), all.len(),
            "Duplicate as_str values found");
    }
}
```

**Step 3: Register the test module**

In `core/src/memory/mod.rs`, add at the end:

```rust
#[cfg(test)]
mod proptest_enums;
```

**Step 4: Run tests**

Run: `cargo test --lib memory::proptest_enums -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/memory/proptest_enums.rs core/src/memory/mod.rs
git commit -m "test(memory): add proptest for FactType and memory enum roundtrips"
```

---

### Task 10: poe — Budget invariants (proptest)

**Files:**
- Create: `core/src/poe/proptest_budget.rs`
- Modify: `core/src/poe/mod.rs` (add `#[cfg(test)] mod proptest_budget;`)

**Step 1: Read budget types**

Read `core/src/poe/budget.rs` — PoeBudget (line 90), BudgetStatus (line 41), methods like `record_attempt()`, `is_exhausted()`, `status()`, `should_continue()`, `entropy_trend()`.

**Step 2: Write the proptest file**

Create `core/src/poe/proptest_budget.rs`:

```rust
use super::budget::{PoeBudget, BudgetStatus};
use proptest::prelude::*;

proptest! {
    /// PoeBudget serde roundtrip
    #[test]
    fn budget_serde_roundtrip(
        max_attempts in 1..20u8,
        max_tokens in 100..100000u32,
    ) {
        let budget = PoeBudget::new(max_attempts, max_tokens);
        let json = serde_json::to_string(&budget).unwrap();
        let back: PoeBudget = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(budget.max_attempts, back.max_attempts);
        prop_assert_eq!(budget.max_tokens, back.max_tokens);
        prop_assert_eq!(budget.current_attempt, back.current_attempt);
        prop_assert_eq!(budget.tokens_used, back.tokens_used);
    }

    /// Fresh budget is never exhausted
    #[test]
    fn fresh_budget_not_exhausted(
        max_attempts in 1..20u8,
        max_tokens in 1..100000u32,
    ) {
        let budget = PoeBudget::new(max_attempts, max_tokens);
        prop_assert!(!budget.is_exhausted());
    }

    /// After max_attempts record_attempt calls, budget is exhausted
    #[test]
    fn exhausted_after_max_attempts(max_attempts in 1..10u8) {
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..max_attempts {
            budget.record_attempt(100, 0.5);
        }
        prop_assert!(budget.is_exhausted());
    }

    /// should_continue is false when exhausted
    #[test]
    fn exhausted_means_no_continue(max_attempts in 1..10u8) {
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..max_attempts {
            budget.record_attempt(100, 0.5);
        }
        prop_assert!(!budget.should_continue());
    }

    /// tokens_used never overflows (saturating arithmetic)
    #[test]
    fn tokens_saturating(
        max_attempts in 2..5u8,
        tokens_per_attempt in (u32::MAX / 2)..u32::MAX,
    ) {
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..max_attempts {
            budget.record_attempt(tokens_per_attempt, 0.5);
        }
        // Should not panic, tokens_used should saturate at u32::MAX
        prop_assert!(budget.tokens_used <= u32::MAX);
    }

    /// entropy_history scores are clamped to [0.0, 1.0]
    #[test]
    fn entropy_clamped(score in -10.0..10.0f32) {
        let mut budget = PoeBudget::new(5, 100000);
        budget.record_attempt(100, score);
        let last = budget.entropy_history.last().unwrap();
        prop_assert!(*last >= 0.0 && *last <= 1.0,
            "Entropy {} not in [0.0, 1.0]", last);
    }

    /// Strictly decreasing entropy → negative trend (improving)
    #[test]
    fn decreasing_entropy_negative_trend(n in 3..8usize) {
        let mut budget = PoeBudget::new(20, u32::MAX);
        for i in 0..n {
            let score = 1.0 - (i as f32 / n as f32);
            budget.record_attempt(100, score);
        }
        let trend = budget.entropy_trend();
        prop_assert!(trend < 0.0,
            "Expected negative trend for decreasing entropy, got {}", trend);
    }

    /// BudgetStatus::Exhausted → should_continue is false
    #[test]
    fn exhausted_status_no_continue(
        max_attempts in 1..5u8,
    ) {
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..max_attempts {
            budget.record_attempt(100, 0.5);
        }
        prop_assert_eq!(budget.status(), BudgetStatus::Exhausted);
        prop_assert!(!budget.should_continue());
    }
}
```

**Step 3: Register the test module**

In `core/src/poe/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_budget;
```

**Step 4: Run tests**

Run: `cargo test --lib poe::proptest_budget -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/poe/proptest_budget.rs core/src/poe/mod.rs
git commit -m "test(poe): add proptest for PoeBudget invariants and arithmetic safety"
```

---

### Task 11: poe — ValidationRule and Verdict serde (proptest)

**Files:**
- Create: `core/src/poe/proptest_types.rs`
- Modify: `core/src/poe/mod.rs` (add `#[cfg(test)] mod proptest_types;`)

**Step 1: Read POE types**

Read `core/src/poe/types.rs` — ValidationRule (line 165), Verdict (line 292), PoeOutcome (line 611), WorkerState (line 487), SuccessManifest (line 47), SoftMetric (line 117).

**Step 2: Write the proptest file**

Create `core/src/poe/proptest_types.rs`:

```rust
use super::types::*;
use proptest::prelude::*;
use std::path::PathBuf;

fn arb_validation_rule() -> impl Strategy<Value = ValidationRule> {
    prop_oneof![
        "[a-z/]{1,30}".prop_map(|p| ValidationRule::FileExists { path: PathBuf::from(p) }),
        "[a-z/]{1,30}".prop_map(|p| ValidationRule::FileNotExists { path: PathBuf::from(p) }),
        ("[a-z/]{1,30}", "[a-z]{1,20}")
            .prop_map(|(p, pat)| ValidationRule::FileContains {
                path: PathBuf::from(p),
                pattern: pat,
            }),
        ("[a-z]{1,10}", prop::collection::vec("[a-z]{1,5}", 0..3), 100..10000u64)
            .prop_map(|(cmd, args, t)| ValidationRule::CommandPasses {
                cmd, args, timeout_ms: t,
            }),
    ]
}

fn arb_verdict() -> impl Strategy<Value = Verdict> {
    (
        any::<bool>(),
        0.0..1.0f32,
        "[a-zA-Z ]{1,50}",
        prop::option::of("[a-zA-Z ]{1,50}"),
    )
        .prop_map(|(passed, score, reason, suggestion)| Verdict {
            passed,
            distance_score: score,
            reason,
            suggestion,
            hard_results: Vec::new(),
            soft_results: Vec::new(),
        })
}

fn arb_worker_state() -> impl Strategy<Value = WorkerState> {
    prop_oneof![
        "[a-zA-Z ]{1,50}".prop_map(|s| WorkerState::Completed { summary: s }),
        "[a-zA-Z ]{1,50}".prop_map(|r| WorkerState::Failed { reason: r }),
        "[a-zA-Z? ]{1,50}".prop_map(|q| WorkerState::NeedsInput { question: q }),
    ]
}

proptest! {
    /// ValidationRule serde roundtrip
    #[test]
    fn validation_rule_roundtrip(rule in arb_validation_rule()) {
        let json = serde_json::to_string(&rule).unwrap();
        let back: ValidationRule = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(format!("{:?}", rule), format!("{:?}", back));
    }

    /// Verdict serde roundtrip
    #[test]
    fn verdict_roundtrip(verdict in arb_verdict()) {
        let json = serde_json::to_string(&verdict).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(verdict.passed, back.passed);
        prop_assert_eq!(verdict.reason, back.reason);
    }

    /// Verdict distance_score always in [0.0, 1.0]
    #[test]
    fn verdict_distance_score_bounded(verdict in arb_verdict()) {
        prop_assert!(verdict.distance_score >= 0.0 && verdict.distance_score <= 1.0);
    }

    /// WorkerState serde roundtrip
    #[test]
    fn worker_state_roundtrip(state in arb_worker_state()) {
        let json = serde_json::to_string(&state).unwrap();
        let back: WorkerState = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(format!("{:?}", state), format!("{:?}", back));
    }

    /// SoftMetric weight and threshold are clamped to [0.0, 1.0]
    #[test]
    fn soft_metric_clamped(weight in -5.0..5.0f32, threshold in -5.0..5.0f32) {
        let metric = SoftMetric {
            rule: ValidationRule::FileExists { path: PathBuf::from("test") },
            weight: weight.clamp(0.0, 1.0),
            threshold: threshold.clamp(0.0, 1.0),
        };
        prop_assert!(metric.weight >= 0.0 && metric.weight <= 1.0);
        prop_assert!(metric.threshold >= 0.0 && metric.threshold <= 1.0);
    }

    /// PoeOutcome::Success with passed=true → is_success() is true
    #[test]
    fn success_outcome_is_success(verdict in arb_verdict()) {
        let outcome = PoeOutcome::Success(Verdict { passed: true, ..verdict });
        prop_assert!(outcome.is_success());
    }

    /// PoeOutcome::BudgetExhausted → is_success() is false
    #[test]
    fn budget_exhausted_not_success(
        attempts in 1..20u8,
        error in "[a-zA-Z ]{1,30}",
    ) {
        let outcome = PoeOutcome::BudgetExhausted {
            attempts,
            last_error: error,
        };
        prop_assert!(!outcome.is_success());
    }
}
```

**Step 3: Register the test module**

In `core/src/poe/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_types;
```

**Step 4: Run tests**

Run: `cargo test --lib poe::proptest_types -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/poe/proptest_types.rs core/src/poe/mod.rs
git commit -m "test(poe): add proptest for ValidationRule, Verdict, and PoeOutcome serde"
```

---

### Task 12: agent_loop — Decision and Action serde (proptest)

**Files:**
- Create: `core/src/agent_loop/proptest_decision.rs`
- Modify: `core/src/agent_loop/mod.rs` (add `#[cfg(test)] mod proptest_decision;`)

**Step 1: Read decision types**

Read `core/src/agent_loop/decision.rs` — Decision enum (line 28), Action enum (line 102), ActionResult enum (line 229). Check which methods exist for `is_terminal()`, `decision_type()`, etc.

**Step 2: Write the proptest file**

Create `core/src/agent_loop/proptest_decision.rs`:

```rust
use super::decision::*;
use proptest::prelude::*;
use serde_json::Value;

fn arb_decision() -> impl Strategy<Value = Decision> {
    prop_oneof![
        ("[a-z_]{1,20}", arb_json_value())
            .prop_map(|(name, args)| Decision::UseTool {
                tool_name: name,
                arguments: args,
            }),
        "[a-zA-Z? ]{1,50}"
            .prop_map(|q| Decision::AskUser { question: q, options: None }),
        "[a-zA-Z ]{1,50}"
            .prop_map(|s| Decision::Complete { summary: s }),
        "[a-zA-Z ]{1,50}"
            .prop_map(|r| Decision::Fail { reason: r }),
        Just(Decision::Silent),
        Just(Decision::HeartbeatOk),
    ]
}

fn arb_action_result() -> impl Strategy<Value = ActionResult> {
    prop_oneof![
        (arb_json_value(), 0..10000u64)
            .prop_map(|(out, dur)| ActionResult::ToolSuccess { output: out, duration_ms: dur }),
        ("[a-zA-Z ]{1,30}", any::<bool>())
            .prop_map(|(err, retry)| ActionResult::ToolError { error: err, retryable: retry }),
        "[a-zA-Z ]{1,30}"
            .prop_map(|r| ActionResult::UserResponse { response: r }),
        Just(ActionResult::Completed),
        Just(ActionResult::Failed),
    ]
}

fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,30}".prop_map(|s| Value::String(s)),
    ]
}

proptest! {
    /// Decision serde roundtrip
    #[test]
    fn decision_serde_roundtrip(d in arb_decision()) {
        let json = serde_json::to_string(&d).unwrap();
        let back: Decision = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(d, back);
    }

    /// ActionResult serde roundtrip
    #[test]
    fn action_result_serde_roundtrip(ar in arb_action_result()) {
        let json = serde_json::to_string(&ar).unwrap();
        let back: ActionResult = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(ar, back);
    }

    /// Terminal decisions: Complete and Fail are terminal, others are not
    #[test]
    fn terminal_decision_classification(d in arb_decision()) {
        let is_terminal = d.is_terminal();
        match &d {
            Decision::Complete { .. } | Decision::Fail { .. } => {
                prop_assert!(is_terminal, "{:?} should be terminal", d);
            }
            _ => {
                prop_assert!(!is_terminal, "{:?} should not be terminal", d);
            }
        }
    }

    /// ActionResult success classification
    #[test]
    fn action_result_success_classification(ar in arb_action_result()) {
        let is_success = ar.is_success();
        match &ar {
            ActionResult::ToolSuccess { .. } | ActionResult::Completed |
            ActionResult::UserResponse { .. } => {
                prop_assert!(is_success, "{:?} should be success", ar);
            }
            ActionResult::ToolError { .. } | ActionResult::Failed => {
                prop_assert!(!is_success, "{:?} should not be success", ar);
            }
            _ => {} // other variants — just ensure no panic
        }
    }
}
```

**Step 3: Register the test module**

In `core/src/agent_loop/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_decision;
```

**Step 4: Run tests**

Run: `cargo test --lib agent_loop::proptest_decision -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/agent_loop/proptest_decision.rs core/src/agent_loop/mod.rs
git commit -m "test(agent_loop): add proptest for Decision and ActionResult serde and classification"
```

---

### Task 13: agent_loop — LoopState invariants (proptest)

**Files:**
- Create: `core/src/agent_loop/proptest_state.rs`
- Modify: `core/src/agent_loop/mod.rs` (add `#[cfg(test)] mod proptest_state;`)

**Step 1: Read LoopState**

Read `core/src/agent_loop/state.rs` — LoopState (line 55), LoopStep (line 107), methods like `record_step()`, `needs_compression()`, `recent_steps()`.

Also read `core/src/agent_loop/guards.rs` — GuardViolation, guard check functions.

**Step 2: Write the proptest file**

Create `core/src/agent_loop/proptest_state.rs`:

```rust
use super::state::*;
use super::decision::*;
use proptest::prelude::*;
use serde_json::Value;

fn arb_loop_step() -> impl Strategy<Value = LoopStep> {
    (
        0..1000usize,                        // step_id
        0..10000usize,                       // tokens_used
        0..5000u64,                          // duration_ms
    )
        .prop_map(|(step_id, tokens, dur)| LoopStep {
            step_id,
            action: Action::Completion { summary: "test".to_string() },
            result: ActionResult::Completed,
            tokens_used: tokens,
            duration_ms: dur,
            thinking: Thinking::None,
        })
}

proptest! {
    /// step_count always equals steps.len() after construction
    #[test]
    fn step_count_equals_len(n_steps in 0..20usize) {
        let mut state = LoopState::new("test-session");
        for i in 0..n_steps {
            state.steps.push(LoopStep {
                step_id: i,
                action: Action::Completion { summary: "test".to_string() },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 50,
                thinking: Thinking::None,
            });
            state.step_count = state.steps.len();
            state.total_tokens += 100;
        }
        prop_assert_eq!(state.step_count, state.steps.len());
    }

    /// total_tokens is sum of all step tokens
    #[test]
    fn total_tokens_is_sum(steps in prop::collection::vec(arb_loop_step(), 0..15)) {
        let expected_total: usize = steps.iter().map(|s| s.tokens_used).sum();
        let mut state = LoopState::new("test-session");
        for step in steps {
            state.total_tokens += step.tokens_used;
            state.steps.push(step);
        }
        prop_assert_eq!(state.total_tokens, expected_total);
    }

    /// compressed_until_step never exceeds steps.len()
    #[test]
    fn compressed_never_exceeds_len(
        n_steps in 1..20usize,
        compress_at in 0..20usize,
    ) {
        let mut state = LoopState::new("test-session");
        for i in 0..n_steps {
            state.steps.push(LoopStep {
                step_id: i,
                action: Action::Completion { summary: "test".to_string() },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 50,
                thinking: Thinking::None,
            });
        }
        state.compressed_until_step = compress_at.min(state.steps.len());
        prop_assert!(state.compressed_until_step <= state.steps.len());
    }

    /// recent_steps returns at most N steps
    #[test]
    fn recent_steps_bounded(
        n_steps in 0..30usize,
        n_recent in 1..10usize,
    ) {
        let mut state = LoopState::new("test-session");
        for i in 0..n_steps {
            state.steps.push(LoopStep {
                step_id: i,
                action: Action::Completion { summary: "test".to_string() },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 50,
                thinking: Thinking::None,
            });
        }
        let recent = state.recent_steps(n_recent);
        prop_assert!(recent.len() <= n_recent);
        prop_assert!(recent.len() <= n_steps);
    }
}
```

**Step 3: Register the test module**

In `core/src/agent_loop/mod.rs`, add:

```rust
#[cfg(test)]
mod proptest_state;
```

**Step 4: Run tests**

Run: `cargo test --lib agent_loop::proptest_state -- --nocapture`
Expected: All proptest tests pass.

**Step 5: Commit**

```bash
git add core/src/agent_loop/proptest_state.rs core/src/agent_loop/mod.rs
git commit -m "test(agent_loop): add proptest for LoopState invariants"
```

---

### Task 14: Create /review-logic AI skill

**Files:**
- Create: `.claude/skills/review-logic.md`

**Step 1: Verify the skills directory exists**

Run: `ls -la .claude/skills/` (or `mkdir -p .claude/skills/` if needed)

**Step 2: Write the skill file**

Create `.claude/skills/review-logic.md`:

````markdown
---
name: review-logic
description: >
  Deep logic bug review for Aleph codebase. Performs four-phase semantic analysis:
  context alignment, invariant checking, control flow simulation, and red-teaming.
  Use when reviewing code for logic bugs, state machine errors, error propagation
  issues, or concurrency defects. Trigger: /review-logic [module|commit] [--strict]
---

# Logic Review — AI Semantic Audit (L3)

You are performing a deep logic review of Aleph code. Follow the four phases below strictly and in order. Output a structured report at the end.

## Arguments

Parse the user's input:
- No args → review uncommitted changes (`git diff` + `git diff --cached`)
- Module name (e.g., `dispatcher`, `gateway`) → review that module's recent changes
- Commit hash → review that specific commit (`git show <hash>`)
- `--strict` flag → lower the Warning threshold (report more potential issues)

## Phase 1: Context Alignment

Before reading any code, establish what "correct" means:

1. **Identify the target module** from the diff/commit
2. **Read the reference doc** using this mapping:

| Module | Reference Doc |
|--------|--------------|
| agent_loop | docs/reference/AGENT_SYSTEM.md |
| dispatcher | docs/reference/AGENT_SYSTEM.md |
| gateway | docs/reference/GATEWAY.md |
| memory | docs/reference/MEMORY_SYSTEM.md |
| poe | docs/plans/2026-02-01-poe-architecture-design.md |
| tools | docs/reference/TOOL_SYSTEM.md |
| exec | docs/reference/SECURITY.md |
| extension | docs/reference/EXTENSION_SYSTEM.md |
| domain | docs/reference/DOMAIN_MODELING.md |

3. **Extract business invariants** from the doc (state machine rules, data constraints, ordering guarantees)
4. **Read the change intent** from commit message or PR description

## Phase 2: Semantic Invariant Checking

For each changed function, check:

### State Machine Legality
- Find all `enum` types used in the changed code
- Enumerate all variant transitions in `match` arms
- Flag any transition that skips intermediate states
- Flag any `_ => {}` that silently swallows new variants

### Error Propagation
- Trace every `?` operator — does the caller handle the error type correctly?
- Check `map_err` chains — is context preserved or silently lost?
- On error paths: is cleanup/rollback performed? Are resources released?

### unwrap/panic Audit
- Flag every `.unwrap()`, `.expect()`, `panic!()`, `unreachable!()`
- For each: is it in a test-only path, or could it trigger in production?
- Rate: SAFE (proven by type system), RISKY (depends on runtime state), CRITICAL (user-facing path)

### Lock Scope Analysis
- Find all `Mutex::lock()`, `RwLock::read()`, `RwLock::write()`
- Check: is the guard held across an `.await` point? (deadlock risk with std::sync::Mutex)
- Check: are multiple locks acquired? If so, is the order consistent across all call sites?

### Type Coercion
- Find all `as` casts — can they truncate or overflow?
- Prefer `try_into()` or `From` implementations

## Phase 3: Control Flow Simulation

For each changed function, mentally execute it:

### Branch Coverage
- List every `if/else`, `match`, and `?` branch
- For each: what happens on the "other" path?
- Flag: `if condition { do_something() }` with no `else` — is the implicit else correct?

### Loop Boundaries
- What happens when the collection is empty?
- What happens when the collection has millions of items?
- Is there a guaranteed termination condition?

### Option/Result Chains
- Trace `.map().and_then().unwrap_or()` chains
- Where does `None` propagate to? Is the final default value correct?

### Async Ordering
- For `tokio::spawn` or `join_all`: does the order of completion matter?
- For channels: can messages arrive out of order? Is that handled?

## Phase 4: Red-teaming

Switch to attacker mindset:

1. **Malicious inputs**: What if the input is:
   - Empty string / zero-length slice
   - Extremely long string (> 1MB)
   - Invalid UTF-8 (if accepting bytes)
   - NaN or Infinity (for floats)
   - Negative numbers where positive expected
   - Null/None where Some expected

2. **Abnormal timing**:
   - Network timeout mid-operation
   - Two identical requests arrive simultaneously
   - Component crashes between step 1 and step 2 of a multi-step operation

3. **Generate test suggestions**: Write 3-5 concrete test cases (as Rust code snippets) that target the most likely failure modes discovered above.

## Report Format

Output the report in this exact format:

```markdown
# Logic Review Report
**Module**: <module name>
**Scope**: <what was reviewed>
**Date**: <today>
**Mode**: <normal|strict>

## Findings

### [Critical] <title>
- **Location**: `file:line`
- **Trigger condition**: <how to trigger>
- **Expected behavior**: <what should happen>
- **Actual behavior**: <what actually happens>
- **Suggested fix**: <how to fix>

### [Warning] <title>
- **Location**: `file:line`
- **Risk**: <what could go wrong>
- **Current impact**: <low/medium/high>
- **Suggestion**: <what to do>

### [Suggested Test] <title>
```rust
#[test]
fn test_name() {
    // test code
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | N |
| Warning | N |
| Suggested Test | N |
```

## Quality Bar

- **Critical**: Must be filed. The code WILL produce wrong results under described conditions.
- **Warning**: Should be addressed. The code MIGHT produce wrong results under edge conditions, or is fragile to future changes.
- **Suggested Test**: Nice to have. A test that would increase confidence in the logic.

In `--strict` mode: lower the bar for Warning (include more "might be a problem" items).

## Aleph-Specific Rules

These are project-specific invariants to always check:

1. **R1 Brain-Limb Separation**: No platform-specific APIs (AppKit, Vision, windows-rs) in `core/src/`
2. **DAG Acyclicity**: Any code touching TaskGraph must preserve the no-cycle invariant
3. **POE Budget Monotonicity**: `current_attempt` and `tokens_used` must only increase
4. **Session Key Determinism**: Same input must always route to the same session
5. **Memory Namespace Isolation**: Facts in namespace A must never leak to namespace B queries
6. **Approval Flow Integrity**: exec approval cannot be bypassed by crafted tool names
7. **Gateway Auth State Machine**: Connection must authenticate before any RPC call succeeds
````

**Step 3: Verify skill loads**

The skill will be available as `/review-logic` in Claude Code sessions.

**Step 4: Commit**

```bash
git add .claude/skills/review-logic.md
git commit -m "skill: add /review-logic AI semantic audit skill for logic bug detection"
```

---

### Task 15: Validate full pipeline

**Step 1: Run proptest suite**

Run: `just test-proptest`
Expected: All property tests pass with 1024 cases each.

**Step 2: Run loom suite**

Run: `just test-loom`
Expected: Pass (no loom tests written yet in this phase, so it should be a no-op or pass trivially).

**Step 3: Run full logic suite**

Run: `just test-logic`
Expected: Both steps pass.

**Step 4: Run full test suite to check for regressions**

Run: `cargo test --workspace`
Expected: All existing 6300+ tests still pass, plus the new proptest tests.

**Step 5: Commit validation result (no code change needed)**

No commit needed — this task is a verification checkpoint.

---

## Summary

| Task | Module | Type | What it does |
|------|--------|------|-------------|
| 1 | infra | deps | Add proptest + loom to Cargo.toml |
| 2 | infra | module | Create sync_primitives.rs for loom |
| 3 | infra | build | Add justfile recipes |
| 4 | infra | CI | Add loom job + proptest env var |
| 5 | dispatcher | proptest | TaskGraph DAG invariants (7 properties) |
| 6 | dispatcher | proptest | TaskStatus serde + invariants (4 properties) |
| 7 | gateway | proptest | JSON-RPC protocol serde roundtrip (4 properties) |
| 8 | gateway | proptest | Channel types serde roundtrip (4 properties) |
| 9 | memory | proptest | FactType + memory enum roundtrips (7 properties) |
| 10 | poe | proptest | PoeBudget invariants + arithmetic (8 properties) |
| 11 | poe | proptest | ValidationRule, Verdict, PoeOutcome serde (7 properties) |
| 12 | agent_loop | proptest | Decision + ActionResult serde + classification (4 properties) |
| 13 | agent_loop | proptest | LoopState invariants (4 properties) |
| 14 | skill | AI | /review-logic semantic audit skill |
| 15 | validation | verify | Run full pipeline, check regressions |

Total: **49 property tests** across 5 modules + 1 AI skill + infrastructure.
