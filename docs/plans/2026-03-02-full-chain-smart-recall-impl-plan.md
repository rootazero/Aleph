# Full Chain + Smart Recall Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete workspace wiring chain (3 disconnections) and implement Two-Phase Smart Recall for automatic cross-workspace knowledge association.

**Architecture:** Part 1 fixes tool-level workspace propagation by adding `set_default_workspace()` to the tool registry and registering missing CRUD handlers. Part 2 adds `SmartRecallConfig` to `ProfileConfig`, extends `FactRetrieval` with two-phase retrieval, and wires it into the memory search tool.

**Tech Stack:** Rust, Tokio async, LanceDB vector search, serde/schemars for config

---

## Task 1: Workspace CRUD RPC Registration

Register the 5 existing but unregistered workspace handlers.

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/builder/handlers.rs` (lines 390-410: `register_workspace_handlers`)
- Read: `core/src/gateway/handlers/workspace.rs` (handlers at lines 44, 85, 116, 164, 237)

**Step 1: Add missing registrations**

In `register_workspace_handlers()`, add registrations for `create`, `list`, `get`, `update`, `archive`. All handlers take `Arc<WorkspaceManager>` as context, matching the existing `register_handler!` 1-arg pattern.

```rust
pub(in crate::commands::start) fn register_workspace_handlers(
    server: &mut GatewayServer,
    workspace_manager: &Arc<WorkspaceManager>,
    daemon: bool,
) {
    register_handler!(server, "workspace.create", workspace_handlers::handle_create, workspace_manager);
    register_handler!(server, "workspace.list", workspace_handlers::handle_list, workspace_manager);
    register_handler!(server, "workspace.get", workspace_handlers::handle_get, workspace_manager);
    register_handler!(server, "workspace.update", workspace_handlers::handle_update, workspace_manager);
    register_handler!(server, "workspace.archive", workspace_handlers::handle_archive, workspace_manager);
    register_handler!(server, "workspace.switch", workspace_handlers::handle_switch, workspace_manager);
    register_handler!(server, "workspace.getActive", workspace_handlers::handle_get_active, workspace_manager);

    if !daemon {
        println!("Workspace methods:");
        println!("  - workspace.create    : Create a new workspace");
        println!("  - workspace.list      : List all workspaces");
        println!("  - workspace.get       : Get workspace details");
        println!("  - workspace.update    : Update workspace");
        println!("  - workspace.archive   : Archive workspace");
        println!("  - workspace.switch    : Switch active workspace");
        println!("  - workspace.getActive : Get current active workspace");
        println!();
    }
}
```

**Step 2: Run compilation check**

Run: `cargo check --bin aleph-server 2>&1 | tail -10`
Expected: PASS (handlers already exist, just adding registrations)

**Step 3: Commit**

```bash
git add core/src/bin/aleph_server/commands/start/builder/handlers.rs
git commit -m "gateway: register workspace CRUD RPC handlers"
```

---

## Task 2: BuiltinToolRegistry Workspace Method

Add `set_default_workspace()` to update memory tools' workspace handles.

**Files:**
- Modify: `core/src/executor/builtin_registry/registry.rs` (lines 28-79: struct, lines 486-596: execute_tool)

**Step 1: Add method to BuiltinToolRegistry**

Add after the existing methods on `BuiltinToolRegistry`:

```rust
/// Update the default workspace for all workspace-aware tools.
/// Called by ExecutionEngine before each agent loop run.
pub async fn set_default_workspace(&self, workspace_id: &str) {
    *self.memory_search_tool.default_workspace_handle().write().await = workspace_id.to_string();
    *self.memory_browse_tool.default_workspace_handle().write().await = workspace_id.to_string();
}
```

**Step 2: Run compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/executor/builtin_registry/registry.rs
git commit -m "executor: add set_default_workspace to BuiltinToolRegistry"
```

---

## Task 3: Engine Workspace Injection

Wire ExecutionEngine to call `set_default_workspace()` before each agent loop run.

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs` (lines 391-520: `run_agent_loop`)

**Step 1: Investigate engine → registry access path**

Read the engine code to find how the tool registry is accessed. The engine creates a `RunContext` and dispatches to `AgentLoop::run()` which uses an executor with the registry. Find the point where the registry reference is available, between workspace resolution (line ~412) and loop start (line ~550).

The engine likely holds a reference to the executor or registry. Check what fields `ExecutionEngine` has and how to access the `BuiltinToolRegistry`.

**Step 2: Add workspace injection call**

After resolving `active_workspace` (around line 412), and before creating the RunContext, inject the workspace_id into the tool registry:

```rust
// After active_workspace resolution, before agent loop start:
// Access the tool registry and update workspace context
if let Some(ref registry) = self.tool_registry {
    registry.set_default_workspace(&active_workspace.workspace_id).await;
}
```

The exact access path depends on how the registry is stored. If it's behind an Arc, clone and call. If the engine doesn't hold a direct reference, you may need to:
- Store `Arc<BuiltinToolRegistry>` in the engine during construction
- OR extract workspace handles at construction time into `Vec<Arc<RwLock<String>>>`

Choose the approach that requires minimal architectural changes.

**Step 3: Run compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: PASS

**Step 4: Run existing tests**

Run: `cargo test -p alephcore --lib gateway::execution_engine 2>&1 | tail -20`
Expected: All existing tests pass

**Step 5: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: inject workspace_id into tool registry before agent loop"
```

---

## Task 4: SmartRecallConfig

Add `SmartRecallConfig` struct and field to `ProfileConfig`.

**Files:**
- Modify: `core/src/config/types/profile.rs` (line ~79: ProfileConfig struct)

**Step 1: Write test**

Add to existing test module in `profile.rs`:

```rust
#[test]
fn test_smart_recall_config_defaults() {
    let config = SmartRecallConfig::default();
    assert!(config.enabled);
    assert!((config.score_threshold - 0.60).abs() < 0.01);
    assert_eq!(config.min_primary_results, 2);
    assert_eq!(config.max_cross_results, 3);
}

#[test]
fn test_profile_with_smart_recall_deserialize() {
    let toml_str = r#"
        model = "claude-sonnet"
        [smart_recall]
        enabled = true
        score_threshold = 0.5
        max_cross_results = 5
    "#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    let recall = profile.smart_recall.unwrap();
    assert!(recall.enabled);
    assert!((recall.score_threshold - 0.5).abs() < 0.01);
    assert_eq!(recall.max_cross_results, 5);
    assert_eq!(recall.min_primary_results, 2); // default
}

#[test]
fn test_profile_without_smart_recall() {
    let toml_str = r#"model = "claude-sonnet""#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    assert!(profile.smart_recall.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::types::profile::tests::test_smart_recall 2>&1`
Expected: FAIL — `SmartRecallConfig` not defined

**Step 3: Implement SmartRecallConfig**

Add above `ProfileConfig`:

```rust
/// Smart recall configuration for automatic cross-workspace knowledge association.
///
/// When enabled, memory retrieval uses a two-phase approach:
/// Phase 1 searches the current workspace. If results are sparse or low-relevance,
/// Phase 2 automatically expands to all workspaces.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SmartRecallConfig {
    /// Enable automatic cross-workspace recall
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Phase 2 triggers when top result score is below this threshold
    #[serde(default = "default_score_threshold")]
    pub score_threshold: f32,

    /// Phase 2 triggers when primary result count is below this
    #[serde(default = "default_min_primary_results")]
    pub min_primary_results: usize,

    /// Max cross-workspace results to include
    #[serde(default = "default_max_cross_results")]
    pub max_cross_results: usize,
}

fn default_true() -> bool { true }
fn default_score_threshold() -> f32 { 0.60 }
fn default_min_primary_results() -> usize { 2 }
fn default_max_cross_results() -> usize { 3 }

impl Default for SmartRecallConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            score_threshold: default_score_threshold(),
            min_primary_results: default_min_primary_results(),
            max_cross_results: default_max_cross_results(),
        }
    }
}
```

Add field to `ProfileConfig`:

```rust
pub struct ProfileConfig {
    // ... existing fields ...

    /// Smart recall: automatic cross-workspace knowledge association
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart_recall: Option<SmartRecallConfig>,
}
```

Update `ProfileConfig::default()` to include `smart_recall: None`.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib config::types::profile 2>&1 | tail -20`
Expected: All tests pass including the 3 new ones

**Step 5: Commit**

```bash
git add core/src/config/types/profile.rs
git commit -m "config: add SmartRecallConfig to ProfileConfig"
```

---

## Task 5: FactRetrieval Smart Recall

Implement Two-Phase retrieval in FactRetrieval.

**Files:**
- Modify: `core/src/memory/fact_retrieval.rs` (after `retrieve_with_filter` at line ~205)

**Step 1: Write test**

Add test in `fact_retrieval.rs` test module:

```rust
#[test]
fn test_smart_retrieval_result_structure() {
    let primary = RetrievalResult {
        facts: vec![],
        raw_memories: vec![],
    };
    let result = SmartRetrievalResult {
        primary,
        cross_workspace: vec![],
        recall_triggered: false,
        trigger_reason: None,
    };
    assert!(!result.recall_triggered);
    assert!(result.cross_workspace.is_empty());
}

#[test]
fn test_cross_workspace_fact_format() {
    let fact = CrossWorkspaceFact {
        content: "test content".to_string(),
        source_workspace: "health".to_string(),
        relevance_score: 0.75,
    };
    assert_eq!(fact.tagged_content(), "[from: health] test content");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::fact_retrieval::tests::test_smart 2>&1`
Expected: FAIL — types not defined

**Step 3: Implement types and method**

Add types:

```rust
use crate::config::types::profile::SmartRecallConfig;

/// Result from smart recall retrieval (Two-Phase)
#[derive(Debug)]
pub struct SmartRetrievalResult {
    /// Primary workspace results (Phase 1)
    pub primary: RetrievalResult,
    /// Cross-workspace results (Phase 2, may be empty)
    pub cross_workspace: Vec<CrossWorkspaceFact>,
    /// Whether Phase 2 was triggered
    pub recall_triggered: bool,
    /// Reason for trigger (for logging)
    pub trigger_reason: Option<String>,
}

/// A fact retrieved from another workspace
#[derive(Debug, Clone)]
pub struct CrossWorkspaceFact {
    pub content: String,
    pub source_workspace: String,
    pub relevance_score: f32,
}

impl CrossWorkspaceFact {
    /// Format with workspace source tag for LLM context
    pub fn tagged_content(&self) -> String {
        format!("[from: {}] {}", self.source_workspace, self.content)
    }
}
```

Add method to `FactRetrieval`:

```rust
/// Two-Phase Smart Recall retrieval.
///
/// Phase 1: Search primary workspace.
/// Phase 2: If results are sparse or low-relevance, expand to all workspaces.
pub async fn retrieve_with_smart_recall(
    &self,
    query: &str,
    primary_workspace: &str,
    config: &SmartRecallConfig,
) -> Result<SmartRetrievalResult, AlephError> {
    // Phase 1: Search primary workspace
    let primary = self
        .retrieve_with_filter(query, WorkspaceFilter::Single(primary_workspace.to_string()))
        .await?;

    // Check if Phase 2 should trigger
    let top_score = primary
        .facts
        .first()
        .and_then(|f| f.similarity_score)
        .unwrap_or(0.0);
    let result_count = primary.facts.len();

    let trigger_reason = if result_count < config.min_primary_results {
        Some(format!(
            "sparse results: {} < min {}",
            result_count, config.min_primary_results
        ))
    } else if top_score < config.score_threshold {
        Some(format!(
            "low relevance: top score {:.2} < threshold {:.2}",
            top_score, config.score_threshold
        ))
    } else {
        None
    };

    if !config.enabled || trigger_reason.is_none() {
        return Ok(SmartRetrievalResult {
            primary,
            cross_workspace: vec![],
            recall_triggered: false,
            trigger_reason: None,
        });
    }

    // Phase 2: Expand to all workspaces
    let all_results = self
        .retrieve_with_filter(query, WorkspaceFilter::All)
        .await?;

    // Filter: exclude primary workspace, take top N by score
    let mut cross_facts: Vec<CrossWorkspaceFact> = all_results
        .facts
        .into_iter()
        .filter(|f| {
            f.workspace
                .as_deref()
                .map(|ws| ws != primary_workspace)
                .unwrap_or(false)
        })
        .filter_map(|f| {
            let score = f.similarity_score.unwrap_or(0.0);
            if score >= self.config.similarity_threshold {
                Some(CrossWorkspaceFact {
                    content: f.compressed_content.unwrap_or_else(|| {
                        f.user_input.clone().unwrap_or_default()
                    }),
                    source_workspace: f.workspace.unwrap_or_else(|| "unknown".to_string()),
                    relevance_score: score,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort by score descending, take top N
    cross_facts.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));
    cross_facts.truncate(config.max_cross_results);

    tracing::debug!(
        trigger = ?trigger_reason,
        cross_count = cross_facts.len(),
        "Smart recall Phase 2 triggered"
    );

    Ok(SmartRetrievalResult {
        primary,
        cross_workspace: cross_facts,
        recall_triggered: true,
        trigger_reason,
    })
}
```

**NOTE:** The `MemoryFact` struct must have `workspace`, `compressed_content`, `user_input`, and `similarity_score` fields. Check the actual struct definition and adjust field names if needed. Key fields to verify:
- `core/src/memory/store/types.rs` — `MemoryFact` struct definition
- Check field for workspace: might be `workspace` or `workspace_id`
- Check field for content: might be `compressed_content`, `content`, `ai_output`, etc.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib memory::fact_retrieval 2>&1 | tail -20`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/memory/fact_retrieval.rs
git commit -m "memory: implement Two-Phase Smart Recall in FactRetrieval"
```

---

## Task 6: Memory Search Tool Smart Recall Integration

Wire the memory search tool to use smart recall when profile config enables it.

**Files:**
- Modify: `core/src/builtin_tools/memory_search.rs` (lines 170-210: `call_impl`)

**Step 1: Add SmartRecallConfig to MemorySearchTool**

The tool needs access to `SmartRecallConfig`. Add a shared handle (same pattern as `default_workspace`):

```rust
pub struct MemorySearchTool {
    // ... existing fields ...
    /// Smart recall config (updated per-request from active profile)
    smart_recall_config: Arc<RwLock<Option<SmartRecallConfig>>>,
}
```

Add handle method:

```rust
pub fn smart_recall_handle(&self) -> Arc<RwLock<Option<SmartRecallConfig>>> {
    Arc::clone(&self.smart_recall_config)
}
```

Initialize in constructor with `Arc::new(RwLock::new(None))`.

**Step 2: Update BuiltinToolRegistry to propagate smart recall config**

In `BuiltinToolRegistry::set_default_workspace()` (from Task 2), also accept and propagate the smart recall config:

Rename to `set_workspace_context()`:

```rust
pub async fn set_workspace_context(
    &self,
    workspace_id: &str,
    smart_recall: Option<SmartRecallConfig>,
) {
    *self.memory_search_tool.default_workspace_handle().write().await = workspace_id.to_string();
    *self.memory_browse_tool.default_workspace_handle().write().await = workspace_id.to_string();
    *self.memory_search_tool.smart_recall_handle().write().await = smart_recall;
}
```

Update the engine call from Task 3 to pass the profile's smart_recall config.

**Step 3: Update call_impl to use smart recall**

In `memory_search.rs` `call_impl()`, after resolving `workspace_filter`:

```rust
// If smart recall is enabled AND this is a single-workspace query (not explicit cross-workspace)
let smart_recall_cfg = self.smart_recall_config.read().await;
let use_smart_recall = smart_recall_cfg
    .as_ref()
    .map(|c| c.enabled)
    .unwrap_or(false)
    && !args.cross_workspace.unwrap_or(false)
    && args.workspaces.is_none();

if use_smart_recall {
    let config = smart_recall_cfg.as_ref().unwrap();
    let primary_ws = args
        .workspace
        .as_deref()
        .unwrap_or(&default_ws);

    let smart_result = self
        .fact_retrieval
        .retrieve_with_smart_recall(query, primary_ws, config)
        .await?;

    // Format results
    let mut output = format_retrieval_result(&smart_result.primary);

    if smart_result.recall_triggered && !smart_result.cross_workspace.is_empty() {
        output.push_str("\n\n--- Cross-workspace knowledge ---\n");
        for fact in &smart_result.cross_workspace {
            output.push_str(&fact.tagged_content());
            output.push('\n');
        }
    }

    return Ok(output);
}

// Existing non-smart-recall path continues here...
```

Adjust the above to match the actual output format of the existing `call_impl()`.

**Step 4: Write test**

```rust
#[test]
fn test_smart_recall_tool_args_no_conflict() {
    // cross_workspace=true should NOT trigger smart recall (explicit > auto)
    let args: MemorySearchArgs = serde_json::from_value(serde_json::json!({
        "query": "test",
        "cross_workspace": true
    })).unwrap();
    assert!(args.cross_workspace.unwrap_or(false));
    // Smart recall should be skipped when cross_workspace is explicit
}
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::memory_search 2>&1 | tail -20`
Expected: All tests pass

**Step 6: Commit**

```bash
git add core/src/builtin_tools/memory_search.rs core/src/executor/builtin_registry/registry.rs core/src/gateway/execution_engine/engine.rs
git commit -m "tools: integrate Smart Recall into memory_search tool"
```

---

## Task 7: End-to-End Verification

**Step 1: Full compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Run: `cargo check --bin aleph-server 2>&1 | tail -10`
Expected: Both pass

**Step 2: Run all affected tests**

```bash
cargo test -p alephcore --lib -- \
    config::types::profile \
    memory::fact_retrieval \
    builtin_tools::memory_search \
    executor::builtin_registry \
    gateway::handlers::workspace \
    gateway::execution_engine \
    routing::resolve \
    2>&1 | tail -30
```

Expected: All tests pass

**Step 3: Commit verification**

```bash
git log --oneline -10
```

Verify commits:
1. `gateway: register workspace CRUD RPC handlers`
2. `executor: add set_default_workspace to BuiltinToolRegistry`
3. `engine: inject workspace_id into tool registry before agent loop`
4. `config: add SmartRecallConfig to ProfileConfig`
5. `memory: implement Two-Phase Smart Recall in FactRetrieval`
6. `tools: integrate Smart Recall into memory_search tool`

---

## Execution Order

```
Task 1 (Workspace CRUD registration) ← standalone
    ↓
Task 2 (Registry workspace method) ← standalone
    ↓
Task 3 (Engine workspace injection) ← depends on Task 2
    ↓
Task 4 (SmartRecallConfig) ← standalone
    ↓
Task 5 (FactRetrieval smart recall) ← depends on Task 4
    ↓
Task 6 (Tool integration) ← depends on Tasks 2, 3, 4, 5
    ↓
Task 7 (End-to-end verification) ← depends on all
```

**Recommended execution order:** 1 → 2 → 3 → 4 → 5 → 6 → 7

Tasks 1 and 4 are fully independent and can be parallelized.
Tasks 2-3 are a pair (foundation → wiring).
Tasks 4-5-6 form the smart recall chain.
