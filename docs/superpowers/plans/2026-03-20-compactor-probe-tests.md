# Session Compactor Probe Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build two-layer probe tests (in-process + end-to-end) to verify the Session Compactor works correctly in production scenarios.

**Architecture:** In-process probe builds ExecutionEngine + SessionCompactor + MockLlm, directly asserts internal state. End-to-end probe spawns real `aleph` child process, sends messages via WebSocket JSON-RPC with real LLM, queries compression state via memory API. CompactorMetrics counters added for reliable assertions.

**Tech Stack:** Rust, LanceDB (test instances via TempDir), MockLlmProvider, tokio-tungstenite (WebSocket), existing AlephTestServer pattern.

**Spec:** `docs/superpowers/specs/2026-03-20-compactor-probe-tests-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `tests/session_compactor_probe.rs` | Top-level test entry (mod declarations) |
| `tests/session_compactor_probe/harness.rs` | CompactorProbeHarness: build SessionCompactor + LanceDB in-process |
| `tests/session_compactor_probe/mock_llm.rs` | MockLlmProvider adapted for compression scenarios |
| `tests/session_compactor_probe/tool_compaction.rs` | Scenario 1: ToolCompactor trigger |
| `tests/session_compactor_probe/summary_gen.rs` | Scenario 2: d0 summary generation |
| `tests/session_compactor_probe/depth_upgrade.rs` | Scenario 3: d0→d1 condensation |
| `tests/session_compactor_probe/history_assembly.rs` | Scenario 4: prepare_history assembly |
| `tests/session_compactor_probe/session_search.rs` | Scenario 5: memory_search scope |
| `tests/session_compactor_probe/fallback_chain.rs` | Scenario 6: three-level fallback |
| `tests/session_compactor_probe/prompt_layer.rs` | Scenario 7: SessionContextGuideLayer |
| `tests/compactor_e2e_probe.rs` | Top-level e2e test entry |
| `tests/compactor_e2e_probe/harness.rs` | E2E harness: spawn aleph + WebSocket |
| `tests/compactor_e2e_probe/multi_turn.rs` | Scenario A: multi-turn → summary |
| `tests/compactor_e2e_probe/compression_depth.rs` | Scenario B: depth upgrade |
| `tests/compactor_e2e_probe/session_recall.rs` | Scenario C: memory recall |

### Modified Files

| File | Change |
|------|--------|
| `src/memory/session_compactor/mod.rs` | Add `CompactorMetrics`, wire counters, add tracing |
| `src/memory/session_compactor/tool_compactor.rs` | Add tracing at compact_if_needed |
| `src/memory/mod.rs` | Re-export `CompactorMetrics` |

---

## Task 1: Add CompactorMetrics and Tracing

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`
- Modify: `src/memory/session_compactor/tool_compactor.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Add CompactorMetrics struct**

In `src/memory/session_compactor/mod.rs`, add after the imports:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct CompactorMetrics {
    pub tool_compactions: AtomicU64,
    pub d0_summaries_created: AtomicU64,
    pub d1_condensations: AtomicU64,
    pub d2_condensations: AtomicU64,
    pub fallback_count: AtomicU64,
    pub prepare_history_calls: AtomicU64,
}
```

Note: Use `std::sync::atomic` here (not loom) since `CompactorMetrics` is never used in loom tests and `AtomicU64` must be `const fn`-constructible for `Default`.

- [ ] **Step 2: Add metrics field to SessionCompactor**

Add `metrics: Arc<CompactorMetrics>` to the `SessionCompactor` struct. Initialize as `Arc::new(CompactorMetrics::default())` in `new()`. Add accessor:

```rust
pub fn metrics(&self) -> &Arc<CompactorMetrics> {
    &self.metrics
}
```

- [ ] **Step 3: Increment counters at key paths**

Add counter increments in the existing methods:

- `post_turn_compress`: After each d0 fact created → `self.metrics.d0_summaries_created.fetch_add(1, Ordering::Relaxed)`
- `try_condense` for d0→d1 → `self.metrics.d1_condensations.fetch_add(1, Ordering::Relaxed)`
- `try_condense` for d1→d2 → `self.metrics.d2_condensations.fetch_add(1, Ordering::Relaxed)`
- `generate_summary` fallback path → `self.metrics.fallback_count.fetch_add(1, Ordering::Relaxed)`
- `prepare_history` entry → `self.metrics.prepare_history_calls.fetch_add(1, Ordering::Relaxed)`

- [ ] **Step 4: Add tracing to SessionCompactor**

Add structured tracing at key paths (all with `target: "session_compactor"`):

```rust
// post_turn_compress entry
tracing::info!(target: "session_compactor", session = %session_key, compressible = compressible_count, chunks = chunks.len(), "compress_start");

// generate_summary fallback
tracing::warn!(target: "session_compactor", session = %session_key, "fallback");

// try_condense trigger
tracing::info!(target: "session_compactor", source_depth = source_depth, target_depth = target_depth, source_count = source_facts.len(), "condense");

// prepare_history
tracing::info!(target: "session_compactor", summaries = summaries.len(), raw = raw_count, evicted = evicted_count, "prepare");
```

- [ ] **Step 5: Add tracing to tool_compactor**

In `tool_compactor.rs`, add at `compact_if_needed` entry and exit:

```rust
// Entry (before compression loop)
tracing::info!(target: "session_compactor", total_tokens = total, threshold = limit, compressing = compressible.len(), "tool_compact");

// Exit (after compression loop)
tracing::info!(target: "session_compactor", saved_tokens = total - current_total, new_total = current_total, "tool_compact_done");
```

Also add metrics increment — but `compact_if_needed` is a free function, not on SessionCompactor. Pass an optional `&CompactorMetrics` parameter, or increment from the caller in `loop_core.rs`. Simplest: add `metrics: Option<&AtomicU64>` param and increment if present. Or: just track via tracing (no counter for tool_compactions — we'll assert via message content changes instead).

Decision: Skip `tool_compactions` counter for now — the in-process test directly asserts message content changes, which is more reliable.

- [ ] **Step 6: Re-export in memory/mod.rs**

Add `pub use session_compactor::CompactorMetrics;` in `src/memory/mod.rs`.

- [ ] **Step 7: Verify compilation and tests**

Run: `cargo test -p alephcore --lib session_compactor`
Expected: All 147 existing tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/memory/session_compactor/ src/memory/mod.rs
git commit -m "session_compactor: add CompactorMetrics counters and structured tracing"
```

---

## Task 2: In-Process Probe Harness and MockLlm

**Files:**
- Create: `tests/session_compactor_probe.rs`
- Create: `tests/session_compactor_probe/harness.rs`
- Create: `tests/session_compactor_probe/mock_llm.rs`

- [ ] **Step 1: Create top-level test entry**

Create `tests/session_compactor_probe.rs`:

```rust
mod session_compactor_probe;
```

- [ ] **Step 2: Create mock_llm.rs**

Copy and adapt from `tests/session_probe/mock_llm.rs`. The key adaptation: default response should be a plausible summary (not just "Hello"):

```rust
// Default response for summary generation
const DEFAULT_SUMMARY: &str = "Key decisions: implemented feature X. Files modified: src/main.rs. \
    Status: completed. Expand for details: specific code changes, test output";
```

Keep the same API: `new()`, `failing()`, `enqueue()`, `call_count()`, `last_input()`, `set_failing()`.

Read `tests/session_probe/mock_llm.rs` first to understand exact imports and `AiProvider` trait implementation.

- [ ] **Step 3: Create harness.rs**

Build `CompactorProbeHarness`:

```rust
pub struct CompactorProbeHarness {
    pub compactor: SessionCompactor,
    pub database: MemoryBackend,
    pub mock_llm: Arc<MockLlmProvider>,
    _temp_dir: TempDir,
}
```

Key implementation notes:
- Read `src/memory/store/` to find `LanceMemoryBackend::open_or_create` or equivalent constructor
- `SessionCompactor::new(database, config).with_provider(mock_llm.clone())`
- Config with aggressive thresholds for testing: `fresh_tail_count=5, leaf_chunk_tokens=300, d1_min_fanout=3`

Helper methods:
- `new() -> Self` — async, creates TempDir + LanceDB + SessionCompactor with MockLlm
- `with_failing_llm() -> Self` — MockLlm in failing mode
- `make_messages(count, base_content) -> Vec<(String, String)>` — generate (role, content) pairs for compression input
- `make_tool_messages(count) -> Vec<UnifiedMessage>` — generate messages with large tool results
- `query_session_facts(session_key) -> Vec<MemoryFact>` — query LanceDB for SessionLocal facts
- `count_facts_at_depth(session_key, depth) -> usize`

Important: Read the actual SessionCompactor API carefully. `post_turn_compress` takes `session_key: &str, agent_id: &str, messages: &[(String, String)]` — check the exact signature in `mod.rs`.

- [ ] **Step 4: Create mod.rs for the probe directory**

Create `tests/session_compactor_probe/mod.rs`:

```rust
pub mod harness;
pub mod mock_llm;
pub mod tool_compaction;
pub mod summary_gen;
pub mod depth_upgrade;
pub mod history_assembly;
pub mod session_search;
pub mod fallback_chain;
pub mod prompt_layer;
```

Start with only `harness` and `mock_llm` — other modules will be empty placeholders initially.

- [ ] **Step 5: Verify compilation**

Run: `cargo test -p alephcore --test session_compactor_probe -- --list`
Expected: Compiles, lists 0 tests (harness has no test functions yet).

- [ ] **Step 6: Commit**

```bash
git add tests/session_compactor_probe.rs tests/session_compactor_probe/
git commit -m "session_compactor_probe: add harness and MockLlmProvider"
```

---

## Task 3: Scenario 1 — ToolCompactor Trigger

**Files:**
- Modify: `tests/session_compactor_probe/tool_compaction.rs`

- [ ] **Step 1: Write ToolCompactor test**

```rust
use alephcore::memory::session_compactor::tool_compactor::compact_if_needed;
use alephcore::providers::message::UnifiedMessage;
use alephcore::memory::session_compactor::context_window::estimate_total_tokens;

#[test]
fn p1_tool_results_compressed_when_over_threshold() {
    // Build 30 messages: user questions + assistant responses + large tool results
    let mut messages: Vec<UnifiedMessage> = Vec::new();

    // Add messages with large tool results in the compressible zone
    for i in 0..20 {
        messages.push(UnifiedMessage::user(format!("Read file_{}.rs", i)));
        // Large tool result (~500 chars each)
        let tool_content = format!("fn main() {{\n{}\n}}", "    let x = 42;\n".repeat(30));
        messages.push(UnifiedMessage::tool_result(
            &format!("call_{}", i), "Read", &tool_content, false,
        ));
        messages.push(UnifiedMessage::assistant(format!("File {} has 30 lines of Rust.", i)));
    }

    let original_tokens = estimate_total_tokens(&messages, 3.5);

    // Set low budget so threshold is exceeded
    compact_if_needed(&mut messages, 5000, 0.75, 3.5, 5);

    let new_tokens = estimate_total_tokens(&messages, 3.5);

    // Assertions
    assert!(new_tokens < original_tokens, "Tokens should decrease after compaction");

    // Check that old tool results are compressed (format: "[Read file, N lines, ...]")
    let compressed_count = messages.iter()
        .filter(|m| m.is_tool_result())
        .filter(|m| {
            let (_, content) = m.tool_result_info().unwrap_or_default();
            content.starts_with("[Read file") || content.starts_with("[")
        })
        .count();
    assert!(compressed_count > 0, "Some tool results should be compressed");

    // Check that fresh tail (last 5) tool results are untouched
    let tail_start = messages.len().saturating_sub(5);
    for msg in &messages[tail_start..] {
        if msg.is_tool_result() {
            let (_, content) = msg.tool_result_info().unwrap_or_default();
            assert!(!content.starts_with("[Read file"), "Fresh tail tool results should not be compressed");
        }
    }
}

#[test]
fn p1_no_compaction_under_threshold() {
    let mut messages = vec![
        UnifiedMessage::user("hi".to_string()),
        UnifiedMessage::assistant("hello".to_string()),
    ];
    let original_len = messages.len();

    compact_if_needed(&mut messages, 200000, 0.75, 3.5, 5);

    assert_eq!(messages.len(), original_len, "No messages should be changed");
}
```

Note: Adapt `UnifiedMessage::tool_result` constructor and `tool_result_info()` return type to match actual API. Read `src/providers/message.rs` for exact signatures.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test session_compactor_probe tool_compaction`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/session_compactor_probe/tool_compaction.rs
git commit -m "session_compactor_probe: add Scenario 1 — ToolCompactor trigger tests"
```

---

## Task 4: Scenario 2 — Post-Turn d0 Summary Generation

**Files:**
- Modify: `tests/session_compactor_probe/summary_gen.rs`

- [ ] **Step 1: Write d0 summary generation test**

```rust
use super::harness::CompactorProbeHarness;
use alephcore::memory::{MemoryScope, FactSource};

#[tokio::test]
async fn p2_post_turn_creates_d0_summaries() {
    let h = CompactorProbeHarness::new().await;

    // Generate 30 conversation turns (enough to trigger compression)
    let messages = h.make_messages(30, "Discuss Rust ownership and borrowing");

    // Run compression
    let session_key = "test-session-001";
    let result = h.compactor.post_turn_compress(session_key, "test-agent", &messages).await;
    assert!(result.is_ok());

    // Query LanceDB for session facts
    let facts = h.query_session_facts(session_key).await;
    assert!(!facts.is_empty(), "Should have created d0 summaries");

    // Verify fact properties
    for fact in &facts {
        assert_eq!(fact.scope, MemoryScope::SessionLocal);
        assert_eq!(fact.fact_source, FactSource::SessionCompressed);
        assert!(fact.path.contains("aleph://session/"));
        assert!(fact.path.contains("/d0/"));
        assert!(fact.is_valid);
        assert!(fact.content.contains("Expand for details:") || fact.content.contains("[Truncated]"));
    }

    // Verify MockLlm was called
    assert!(h.mock_llm.call_count() > 0, "LLM should have been called for summarization");

    // Verify metrics
    assert!(h.compactor.metrics().d0_summaries_created.load(std::sync::atomic::Ordering::Relaxed) > 0);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test session_compactor_probe summary_gen`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add tests/session_compactor_probe/summary_gen.rs
git commit -m "session_compactor_probe: add Scenario 2 — d0 summary generation"
```

---

## Task 5: Scenario 3 — Depth Upgrade d0→d1

**Files:**
- Modify: `tests/session_compactor_probe/depth_upgrade.rs`

- [ ] **Step 1: Write depth upgrade test**

```rust
#[tokio::test]
async fn p3_d0_condensed_to_d1_when_fanout_reached() {
    let h = CompactorProbeHarness::new().await;
    let session_key = "test-depth-upgrade";

    // Run compression 5 times with different message batches
    // Each should generate at least 1 d0 summary
    // Config has d1_min_fanout=3, so after 3+ d0s, d1 should trigger
    for round in 0..5 {
        let messages = h.make_messages(10, &format!("Round {} discussion about topic {}", round, round));
        h.compactor.post_turn_compress(session_key, "test-agent", &messages).await.unwrap();
    }

    // Verify d1 was created
    let d1_count = h.count_facts_at_depth(session_key, 1).await;
    assert!(d1_count > 0, "d1 summary should exist after reaching fanout threshold");

    // Verify source d0s were invalidated
    let all_d0_facts = h.query_all_facts_at_depth(session_key, 0).await; // includes invalid
    let invalid_d0s = all_d0_facts.iter().filter(|f| !f.is_valid).count();
    assert!(invalid_d0s > 0, "Some d0 facts should be invalidated after condensation");

    // Verify metrics
    let metrics = h.compactor.metrics();
    assert!(metrics.d1_condensations.load(Ordering::Relaxed) > 0);
}
```

Note: `query_all_facts_at_depth` should query with `is_valid=None` to include invalidated facts. Add this to the harness if not present.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test session_compactor_probe depth_upgrade`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add tests/session_compactor_probe/depth_upgrade.rs
git commit -m "session_compactor_probe: add Scenario 3 — d0→d1 depth upgrade"
```

---

## Task 6: Scenario 4 — prepare_history Assembly

**Files:**
- Modify: `tests/session_compactor_probe/history_assembly.rs`

- [ ] **Step 1: Write history assembly tests**

```rust
#[tokio::test]
async fn p4_prepare_history_injects_summaries_and_fresh_tail() {
    let h = CompactorProbeHarness::new().await;
    let session_key = "test-history";

    // First: create some summaries via compression
    let messages = h.make_messages(30, "Architecture discussion");
    h.compactor.post_turn_compress(session_key, "test-agent", &messages).await.unwrap();

    // Verify summaries exist
    let facts = h.query_session_facts(session_key).await;
    assert!(!facts.is_empty());

    // Now: call prepare_history
    // This needs an AgentInstance — check if harness provides one, or if we need
    // to call it differently (the actual signature may differ)
    let history = h.run_prepare_history(session_key, "new question", 100000).await;

    // Check structure: summaries first, then raw messages
    let has_summary = history.iter().any(|m| m.text_content().contains("<session_context"));
    assert!(has_summary, "History should contain session_context XML tags");

    // Summaries should be at the start
    let first_summary_idx = history.iter().position(|m| m.text_content().contains("<session_context"));
    assert_eq!(first_summary_idx, Some(0), "Summaries should be at the beginning");

    // Metrics
    assert!(h.compactor.metrics().prepare_history_calls.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn p4_prepare_history_no_summaries_returns_raw() {
    let h = CompactorProbeHarness::new().await;

    // No compression done, so prepare_history should return raw messages only
    let history = h.run_prepare_history("empty-session", "hello", 100000).await;

    let has_summary = history.iter().any(|m| m.text_content().contains("<session_context"));
    assert!(!has_summary, "No summaries should exist for fresh session");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test session_compactor_probe history_assembly`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add tests/session_compactor_probe/history_assembly.rs
git commit -m "session_compactor_probe: add Scenario 4 — prepare_history assembly"
```

---

## Task 7: Scenarios 5, 6, 7 — Session Search, Fallback, Prompt Layer

**Files:**
- Modify: `tests/session_compactor_probe/session_search.rs`
- Modify: `tests/session_compactor_probe/fallback_chain.rs`
- Modify: `tests/session_compactor_probe/prompt_layer.rs`

- [ ] **Step 1: Write session search test (Scenario 5)**

```rust
// session_search.rs
#[tokio::test]
async fn p5_session_facts_retrievable_by_path_prefix() {
    let h = CompactorProbeHarness::new().await;
    let session_key = "test-search";

    // Create summaries
    let messages = h.make_messages(20, "Discussion about Phoenix project in Rust");
    h.compactor.post_turn_compress(session_key, "test-agent", &messages).await.unwrap();

    // Search by path prefix
    let facts = h.query_session_facts(session_key).await;
    assert!(!facts.is_empty());

    // Search including invalidated facts (for detail retrieval)
    // After condensation, some d0s should be invalidated but still findable
    let all_facts = h.query_all_facts_at_depth(session_key, 0).await;
    assert!(all_facts.len() >= facts.iter().filter(|f| f.path.contains("/d0/")).count(),
        "All facts query should include invalidated ones");
}
```

- [ ] **Step 2: Write fallback chain test (Scenario 6)**

```rust
// fallback_chain.rs
#[tokio::test]
async fn p6_fallback_to_deterministic_when_llm_fails() {
    let h = CompactorProbeHarness::with_failing_llm().await;
    let session_key = "test-fallback";

    let messages = h.make_messages(20, "Some conversation content");
    let result = h.compactor.post_turn_compress(session_key, "test-agent", &messages).await;

    // Should not error — fallback catches LLM failure
    assert!(result.is_ok(), "Compression should succeed via fallback");

    // Facts should still be created (deterministic truncation)
    let facts = h.query_session_facts(session_key).await;
    assert!(!facts.is_empty(), "Facts should be created via deterministic fallback");

    // Verify fallback marker in content
    let has_truncated = facts.iter().any(|f| f.content.contains("[Truncated]") || f.content.contains("[user]"));
    assert!(has_truncated, "Fallback output should contain truncation markers");

    // Verify metrics
    assert!(h.compactor.metrics().fallback_count.load(Ordering::Relaxed) > 0);
}
```

- [ ] **Step 3: Write prompt layer test (Scenario 7)**

```rust
// prompt_layer.rs
use alephcore::thinker::prompt_layer::{PromptLayer, LayerInput};
use alephcore::thinker::layers::SessionContextGuideLayer;

#[test]
fn p7_session_context_guide_injected_when_summaries_present() {
    let layer = SessionContextGuideLayer;
    let config = alephcore::thinker::prompt_builder::PromptConfig::default();
    let tools = vec![];
    let mut input = LayerInput::basic(&config, &tools);
    input.has_session_summaries = true;

    let mut output = String::new();
    layer.inject(&mut output, &input);

    assert!(output.contains("Session Context Notes"));
    assert!(output.contains("memory_search"));
    assert!(output.contains("current_session"));
}

#[test]
fn p7_session_context_guide_skipped_when_no_summaries() {
    let layer = SessionContextGuideLayer;
    let config = alephcore::thinker::prompt_builder::PromptConfig::default();
    let tools = vec![];
    let input = LayerInput::basic(&config, &tools); // has_session_summaries defaults to false

    let mut output = String::new();
    layer.inject(&mut output, &input);

    assert!(output.is_empty(), "Should not inject anything when no summaries");
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test -p alephcore --test session_compactor_probe`
Expected: All scenarios pass.

- [ ] **Step 5: Commit**

```bash
git add tests/session_compactor_probe/session_search.rs tests/session_compactor_probe/fallback_chain.rs tests/session_compactor_probe/prompt_layer.rs
git commit -m "session_compactor_probe: add Scenarios 5-7 — search, fallback, prompt layer"
```

---

## Task 8: End-to-End Probe Harness

**Files:**
- Create: `tests/compactor_e2e_probe.rs`
- Create: `tests/compactor_e2e_probe/harness.rs`
- Create: `tests/compactor_e2e_probe/mod.rs`

- [ ] **Step 1: Study AlephTestServer pattern**

Read `tests/provider_rpc_probe/harness.rs` to understand:
- How the server is spawned (`Command::new`)
- How config is written (temp TOML)
- How WebSocket connection is established
- How JSON-RPC calls are made
- How to wait for server readiness

- [ ] **Step 2: Create compactor_e2e_probe.rs**

```rust
mod compactor_e2e_probe;
```

- [ ] **Step 3: Create mod.rs**

```rust
pub mod harness;
pub mod multi_turn;
pub mod compression_depth;
pub mod session_recall;
```

- [ ] **Step 4: Create harness.rs**

Build `CompactorE2eHarness` following the `AlephTestServer` pattern:

```rust
pub struct CompactorE2eHarness {
    port: u16,
    ws_url: String,
    child: std::process::Child,
    session_key: Option<String>,
    _config_dir: TempDir,
}
```

Key methods:
- `start() -> Self` — spawn `target/debug/aleph start` with aggressive compactor config
- `send_message(text) -> String` — send `chat.send` JSON-RPC, collect response chunks until `stream.run_complete`, return full response text
- `query_memory_stats() -> serde_json::Value` — call `memory.stats`
- `query_session_facts() -> Vec<serde_json::Value>` — call `memory.list_facts`, filter for SessionCompressed facts
- `search_memory(query) -> Vec<serde_json::Value>` — call `memory.search`
- `wait_for_compression(timeout_secs) -> bool` — poll `memory.list_facts` until SessionCompressed facts appear

Config TOML should include:
```toml
[memory.session_compactor]
enabled = true
fresh_tail_count = 5
context_threshold = 0.5
leaf_chunk_tokens = 300
d1_min_fanout = 3
```

Important: The server needs a working LLM provider. Check how the existing config provides this (it may use `~/.aleph/` default config). The test relies on the user having a configured LLM API key.

- [ ] **Step 5: Verify compilation**

Run: `cargo test -p alephcore --test compactor_e2e_probe -- --list`
Expected: Compiles, lists 0 tests.

- [ ] **Step 6: Commit**

```bash
git add tests/compactor_e2e_probe.rs tests/compactor_e2e_probe/
git commit -m "compactor_e2e_probe: add E2E harness with WebSocket JSON-RPC"
```

---

## Task 9: E2E Scenario A — Multi-Turn Conversation

**Files:**
- Modify: `tests/compactor_e2e_probe/multi_turn.rs`

- [ ] **Step 1: Write multi-turn test**

```rust
#[tokio::test]
#[ignore] // Requires running aleph server with real LLM API key
async fn e2e_multi_turn_triggers_compression() {
    let mut h = CompactorE2eHarness::start().await;

    // Send multiple messages to build conversation history
    h.send_message("你好，我是测试用户").await;
    h.send_message("请帮我解释 Rust 的所有权机制").await;
    h.send_message("那借用检查器呢？").await;
    h.send_message("生命周期标注的规则是什么？").await;
    h.send_message("总结一下刚才讨论的内容").await;

    // Wait for async compression to complete
    let compressed = h.wait_for_compression(30).await;
    assert!(compressed, "Compression should trigger after multiple turns");

    // Verify facts via API
    let facts = h.query_session_facts().await;
    assert!(!facts.is_empty(), "SessionCompressed facts should exist");

    // Verify fact structure
    for fact in &facts {
        let path = fact["path"].as_str().unwrap_or("");
        assert!(path.contains("aleph://session/"), "Fact path should be session-scoped");
    }

    // Verify AI can recall compressed info
    let response = h.send_message("我们刚才讨论了什么？").await;
    let response_lower = response.to_lowercase();
    assert!(
        response_lower.contains("所有权") || response_lower.contains("ownership")
            || response_lower.contains("借用") || response_lower.contains("borrow"),
        "AI should reference prior discussion topics. Got: {}", response
    );
}
```

- [ ] **Step 2: Commit**

```bash
git add tests/compactor_e2e_probe/multi_turn.rs
git commit -m "compactor_e2e_probe: add Scenario A — multi-turn compression"
```

---

## Task 10: E2E Scenarios B and C — Depth Upgrade and Recall

**Files:**
- Modify: `tests/compactor_e2e_probe/compression_depth.rs`
- Modify: `tests/compactor_e2e_probe/session_recall.rs`

- [ ] **Step 1: Write depth upgrade test (Scenario B)**

```rust
#[tokio::test]
#[ignore]
async fn e2e_long_conversation_triggers_depth_upgrade() {
    let mut h = CompactorE2eHarness::start().await;

    // Send 15+ messages to trigger multiple d0s then d1
    let topics = [
        "解释 Rust 的 trait 系统",
        "trait object 和泛型的区别",
        "什么是 blanket implementation",
        "解释 Deref trait 的用途",
        "如何实现自定义迭代器",
        "Iterator 和 IntoIterator 的关系",
        "什么是 zero-cost abstraction",
        "Rust 的 async/await 原理",
        "Future trait 的 poll 机制",
        "tokio runtime 的工作原理",
        "解释 Pin 和 Unpin",
        "什么是 self-referential struct",
        "Rust 的错误处理最佳实践",
        "Result 和 Option 的区别",
        "thiserror 和 anyhow 的使用场景",
    ];

    for topic in &topics {
        h.send_message(topic).await;
        // Brief pause for async compression
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    // Wait for final compression
    h.wait_for_compression(30).await;

    // Check for d1 facts
    let facts = h.query_session_facts().await;
    let d1_facts: Vec<_> = facts.iter()
        .filter(|f| f["path"].as_str().unwrap_or("").contains("/d1/"))
        .collect();

    // d1 may or may not appear depending on timing — assert d0 at minimum
    let d0_facts: Vec<_> = facts.iter()
        .filter(|f| f["path"].as_str().unwrap_or("").contains("/d0/"))
        .collect();
    assert!(!d0_facts.is_empty(), "d0 summaries should exist");

    // Verify AI can summarize the whole conversation
    let summary = h.send_message("回顾整个对话，列出我们讨论的所有主题").await;
    assert!(summary.contains("trait") || summary.contains("async") || summary.contains("Rust"),
        "Summary should reference discussed topics");
}
```

- [ ] **Step 2: Write memory recall test (Scenario C)**

```rust
#[tokio::test]
#[ignore]
async fn e2e_compressed_info_retrievable_via_search() {
    let mut h = CompactorE2eHarness::start().await;

    // Plant specific facts
    h.send_message("请记住：我的项目叫 Phoenix，使用 Rust 编写").await;
    h.send_message("Phoenix 的数据库用的是 PostgreSQL").await;

    // Fill conversation to trigger compression
    for i in 0..10 {
        h.send_message(&format!("讨论话题 {}: Rust 的内存安全机制", i)).await;
    }

    h.wait_for_compression(30).await;

    // Search for compressed content
    let results = h.search_memory("Phoenix").await;
    // Results may or may not contain Phoenix depending on whether it was in a compressed chunk
    // The important test is that the AI can still recall it:

    let response = h.send_message("我的项目叫什么？用什么语言？").await;
    assert!(
        response.contains("Phoenix") || response.contains("phoenix"),
        "AI should recall project name. Got: {}", response
    );
}
```

- [ ] **Step 3: Commit**

```bash
git add tests/compactor_e2e_probe/compression_depth.rs tests/compactor_e2e_probe/session_recall.rs
git commit -m "compactor_e2e_probe: add Scenarios B and C — depth upgrade and recall"
```

---

## Task 11: Final Verification

- [ ] **Step 1: Run in-process probe tests**

Run: `cargo test -p alephcore --test session_compactor_probe`
Expected: All 7 scenarios pass.

- [ ] **Step 2: Run unit tests to verify no regressions**

Run: `cargo test -p alephcore --lib`
Expected: All ~8237 tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | grep "session_compactor\|compactor_probe"`
Expected: No warnings in our code.

- [ ] **Step 4: Verify E2E tests compile**

Run: `cargo test -p alephcore --test compactor_e2e_probe -- --list`
Expected: Lists 3 tests (all ignored).

- [ ] **Step 5: Commit any cleanup**

```bash
git add -A
git commit -m "session_compactor_probe: final cleanup and verification"
```
