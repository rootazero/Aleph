# Session Compactor Probe Tests: Production-Grade Verification

**Date**: 2026-03-20
**Status**: Approved
**Scope**: Test infrastructure for session_compactor and memory system

## Problem

The Session Compactor was implemented with 147 unit tests covering individual components, but lacks integration-level verification that the full compression pipeline works end-to-end: tool compaction triggering in-loop, LLM summary generation, depth upgrades, history assembly, and memory search retrieval. Without this, we cannot confirm the system behaves correctly in production scenarios.

## Solution

Two-layer probe test architecture:

1. **In-process probe** (`session_compactor_probe/`): Builds a complete ExecutionEngine + SessionCompactor + MockLlm in-process. Directly asserts on internal state (MemoryFact fields, compression depth, message content). Fast, deterministic, CI-safe.

2. **End-to-end probe** (`compactor_e2e_probe/`): Spawns a real `aleph` child process, sends multi-turn conversations via WebSocket JSON-RPC using a real LLM API, queries compression state via `memory.list_facts`/`memory.search`. Marked `#[ignore]`, run manually.

3. **Observability**: Add `CompactorMetrics` counters and structured tracing to `session_compactor` for both probe assertions and production monitoring.

## Architecture

### Layer 1: In-Process Probe

```
tests/session_compactor_probe/
├── harness.rs          // CompactorProbeHarness
├── mock_llm.rs         // MockLlmProvider (reused from session_probe pattern)
├── tool_compaction.rs  // Scenario 1: ToolCompactor trigger and compression
├── summary_gen.rs      // Scenario 2: Post-turn d0 summary generation
├── depth_upgrade.rs    // Scenario 3: d0→d1 condensation upgrade
├── history_assembly.rs // Scenario 4: prepare_history assembly verification
├── session_search.rs   // Scenario 5: memory_search scope=current_session
├── fallback_chain.rs   // Scenario 6: Three-level fallback
└── prompt_layer.rs     // Scenario 7: SessionContextGuideLayer injection
```

#### CompactorProbeHarness

```rust
pub struct CompactorProbeHarness {
    pub memory_backend: MemoryBackend,
    pub session_compactor: Arc<SessionCompactor>,
    pub mock_llm: Arc<MockLlmProvider>,
    pub session_manager: Arc<SessionManager>,
    pub metrics: Arc<CompactorMetrics>,
    _temp_dir: TempDir,
}
```

Key methods:
- `new() -> Self` — Build with MockLlmProvider (default response)
- `with_failing_llm() -> Self` — Build with failing MockLlmProvider (for fallback tests)
- `send_turns(agent_id, turns: &[(&str, &str)])` — Write conversation turns to session store
- `run_post_turn_compress(session_key, agent_id) -> Vec<MemoryFact>` — Call post_turn_compress, return created facts
- `run_prepare_history(agent_id, current_input, budget) -> Vec<UnifiedMessage>` — Call prepare_history
- `query_session_facts(session_key) -> Vec<MemoryFact>` — Query LanceDB for SessionLocal facts
- `count_facts_at_depth(session_key, depth) -> usize` — Count facts at specific depth
- `get_fact_content_at_depth(session_key, depth) -> Vec<String>` — Get fact contents at depth

#### Scenario Details

**Scenario 1: ToolCompactor Trigger** (`tool_compaction.rs`)

Setup: 30 messages including large tool results (Read file 500 lines, Grep 200 matches), set token_budget low (10000).

Assertions:
- Tool results in compressible zone replaced with `[Read file, N lines, ...]` format
- Fresh tail (last 5 messages) tool results untouched
- Total estimated tokens decreased below threshold
- `metrics.tool_compactions` incremented

**Scenario 2: Post-Turn d0 Summary** (`summary_gen.rs`)

Setup: 30 conversation turns, call `post_turn_compress`.

Assertions:
- LanceDB contains new facts with `scope == SessionLocal`
- Facts have `fact_source == SessionCompressed`
- Facts have `path` matching `aleph://session/{id}/d0/{seq}`
- Fact content ends with `Expand for details: ...`
- `metrics.d0_summaries_created > 0`
- MockLlm `call_count() > 0` (LLM was used for summarization)

**Scenario 3: Depth Upgrade d0→d1** (`depth_upgrade.rs`)

Setup: Run `post_turn_compress` 5 times with 10 messages each (generating 5+ d0 summaries, exceeding `d1_min_fanout=4`).

Assertions:
- d1 fact appears in LanceDB with `path` containing `d1/`
- Source d0 facts have `is_valid == false`
- d1 fact content is more abstract than d0 content
- `metrics.d1_condensations == 1`

**Scenario 4: prepare_history Assembly** (`history_assembly.rs`)

Setup: Generate d0 and d1 summaries first, then call `prepare_history`.

Assertions:
- Returned messages start with `<session_context depth="1">` (highest depth first)
- Then `<session_context depth="0">` for remaining valid d0s
- Then raw messages (fresh tail)
- Invalidated d0 summaries NOT in returned messages
- `metrics.prepare_history_calls` incremented
- Total message count < original 50 (compression effective)

**Scenario 5: Session Memory Search** (`session_search.rs`)

Setup: Generate summaries, then query LanceDB with SessionLocal filter.

Assertions:
- Search with `scope=SessionLocal, path_prefix=aleph://session/{id}/` returns facts
- Search with `is_valid=None` includes invalidated d0s (for detail retrieval)
- Search with `is_valid=true` excludes invalidated d0s
- Fact content matches compressed conversation content

**Scenario 6: Three-Level Fallback** (`fallback_chain.rs`)

Setup: MockLlm set to failing mode, call `post_turn_compress`.

Assertions:
- Facts still created (deterministic fallback succeeded)
- Fact content contains `[Truncated]` marker
- `metrics.fallback_count > 0`
- No panic, no error propagation

**Scenario 7: Prompt Layer Injection** (`prompt_layer.rs`)

Setup: Build `LayerInput` with `has_session_summaries = true`, invoke layer.

Assertions:
- Output contains "Session Context Notes"
- Output contains `memory_search` and `scope="current_session"`
- With `has_session_summaries = false`: output is empty (zero overhead)

### Layer 2: End-to-End Probe

```
tests/compactor_e2e_probe/
├── harness.rs           // CompactorE2eHarness: spawn aleph + WebSocket
├── multi_turn.rs        // Scenario A: multi-turn → summary generation
├── compression_depth.rs // Scenario B: long conversation → depth upgrade
├── session_recall.rs    // Scenario C: memory recall after compression
└── mod.rs               // Module entry + #[ignore] markers
```

#### CompactorE2eHarness

Based on `provider_rpc_probe/harness.rs` pattern:

```rust
pub struct CompactorE2eHarness {
    server: AlephTestServer,
    session_key: String,
    agent_id: String,
    log_buffer: Arc<Mutex<String>>,  // Captured stderr from child process
}
```

Key methods:
- `start() -> Self` — Spawn aleph with compression-friendly config
- `send_message(text) -> String` — `chat.send` via WebSocket, wait for `stream.run_complete`, return AI response
- `query_memory_stats() -> MemoryStats` — Call `memory.stats`
- `query_session_facts() -> Vec<FactInfo>` — Call `memory.list_facts`, filter SessionCompressed
- `search_session_memory(query) -> Vec<SearchResult>` — Call `memory.search`
- `wait_for_compression(timeout) -> bool` — Poll `memory.list_facts` until SessionCompressed facts appear
- `assert_log_contains(pattern)` — Check captured stderr for tracing output

#### Test Configuration

Spawned aleph uses aggressive thresholds for faster compression:

```toml
[memory.session_compactor]
enabled = true
fresh_tail_count = 5
context_threshold = 0.5
leaf_chunk_tokens = 300
d1_min_fanout = 3
d2_min_fanout = 3
max_summary_depth = 2
```

#### Scenario Details

**Scenario A: Multi-Turn Conversation** (`multi_turn.rs`) `#[ignore]`

```
1. send_message("你好，我是测试用户")
2. send_message("请帮我解释 Rust 的所有权机制")        → Long AI response
3. send_message("那借用检查器呢？")
4. send_message("生命周期标注的规则是什么？")
5. send_message("总结一下刚才讨论的内容")
   -- wait_for_compression(30s) --
6. Assert: query_session_facts() contains fact_source=SessionCompressed
7. Assert: fact.path contains "aleph://session/"
8. send_message("我们刚才讨论了什么？") → AI references prior discussion
9. Assert: AI response mentions ownership/borrowing (compressed info retrieved)
```

**Scenario B: Compression Depth Upgrade** (`compression_depth.rs`) `#[ignore]`

```
1. Send 15-20 messages (mix of Q&A and task requests)
   -- wait_for_compression after every 5 messages --
2. Assert: d0 facts appear progressively
3. Continue until d0 count >= d1_min_fanout(3)
   -- wait_for_compression --
4. Assert: d1 fact appears (path contains "d1/")
5. Assert: some d0 facts have is_valid=false
6. send_message("回顾整个对话") → AI summarizes based on d1 summary
```

**Scenario C: Memory Recall** (`session_recall.rs`) `#[ignore]`

```
1. send_message("请记住：我的项目叫 Phoenix，使用 Rust 编写")
2. send_message("Phoenix 的数据库用的是 PostgreSQL")
3. Send 10+ filler messages to trigger compression
   -- wait_for_compression --
4. search_session_memory("Phoenix")
   → Assert: results contain Phoenix-related compressed summary
5. send_message("我的项目叫什么？用什么语言？")
   → Assert: AI correctly answers Phoenix + Rust (from compressed memory)
```

#### Log Monitoring

End-to-end tests capture `aleph` child process stderr and verify:
- `assert_log_contains("session_compactor")` — compactor was active
- `assert_log_contains("compress_start")` — compression triggered
- `assert_log_contains("condense")` — condensation triggered (scenario B)

## Observability: CompactorMetrics

Added to `src/memory/session_compactor/mod.rs`:

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

impl CompactorMetrics {
    pub fn reset(&self) {
        self.tool_compactions.store(0, Ordering::Relaxed);
        self.d0_summaries_created.store(0, Ordering::Relaxed);
        self.d1_condensations.store(0, Ordering::Relaxed);
        self.d2_condensations.store(0, Ordering::Relaxed);
        self.fallback_count.store(0, Ordering::Relaxed);
        self.prepare_history_calls.store(0, Ordering::Relaxed);
    }
}
```

`SessionCompactor` holds `metrics: Arc<CompactorMetrics>` (always present, not optional). Counters incremented at each key path. Both probes and production can read them.

## Tracing Enhancement

All tracing uses `target: "session_compactor"` for precise filtering.

| Location | Level | Message | Structured Fields |
|----------|-------|---------|------------------|
| `tool_compactor::compact_if_needed` start | `info` | `tool_compact` | `total_tokens`, `threshold`, `compressing_count` |
| `tool_compactor::compact_if_needed` done | `info` | `tool_compact_done` | `saved_tokens`, `new_total` |
| `SessionCompactor::post_turn_compress` start | `info` | `compress_start` | `session`, `compressible_messages`, `chunks` |
| `SessionCompactor::generate_summary` per level | `debug` | `summary` | `level`, `input_tokens`, `output_tokens` |
| `SessionCompactor::generate_summary` fallback | `warn` | `fallback` | `session`, `from_level`, `reason` |
| `SessionCompactor::try_condense` trigger | `info` | `condense` | `source_depth`, `target_depth`, `source_count` |
| `SessionCompactor::try_condense` invalidate | `debug` | `invalidate` | `fact_id`, `reason` |
| `SessionCompactor::prepare_history` | `info` | `prepare` | `summaries`, `raw_messages`, `evicted` |

## Files to Modify

| File | Change |
|------|--------|
| `src/memory/session_compactor/mod.rs` | Add `CompactorMetrics`, wire into SessionCompactor, increment at key paths |
| `src/memory/session_compactor/tool_compactor.rs` | Add tracing at compact_if_needed entry/exit |
| `src/memory/mod.rs` | Re-export `CompactorMetrics` |

## New Files

| File | Purpose |
|------|---------|
| `tests/session_compactor_probe/harness.rs` | In-process probe harness |
| `tests/session_compactor_probe/mock_llm.rs` | MockLlmProvider |
| `tests/session_compactor_probe/tool_compaction.rs` | Scenario 1 |
| `tests/session_compactor_probe/summary_gen.rs` | Scenario 2 |
| `tests/session_compactor_probe/depth_upgrade.rs` | Scenario 3 |
| `tests/session_compactor_probe/history_assembly.rs` | Scenario 4 |
| `tests/session_compactor_probe/session_search.rs` | Scenario 5 |
| `tests/session_compactor_probe/fallback_chain.rs` | Scenario 6 |
| `tests/session_compactor_probe/prompt_layer.rs` | Scenario 7 |
| `tests/compactor_e2e_probe/harness.rs` | E2E probe harness |
| `tests/compactor_e2e_probe/multi_turn.rs` | Scenario A |
| `tests/compactor_e2e_probe/compression_depth.rs` | Scenario B |
| `tests/compactor_e2e_probe/session_recall.rs` | Scenario C |
| `tests/compactor_e2e_probe/mod.rs` | Module entry |

## Running

```bash
# In-process probe (CI-safe, fast, ~5s)
cargo test -p alephcore --test session_compactor_probe

# End-to-end probe (manual, needs LLM API key, ~2-5min)
cargo test -p alephcore --test compactor_e2e_probe -- --ignored

# With detailed tracing
RUST_LOG=session_compactor=debug cargo test -p alephcore --test session_compactor_probe

# Specific scenario
cargo test -p alephcore --test session_compactor_probe tool_compaction
cargo test -p alephcore --test compactor_e2e_probe multi_turn -- --ignored
```

## Test Data Isolation

- Each test creates its own `TempDir` for LanceDB and SQLite
- End-to-end tests use isolated `--config-dir` temp directory
- No state sharing between tests — fully parallelizable
