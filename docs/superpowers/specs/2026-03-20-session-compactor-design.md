# Session Compactor: Intra-Session Context Management

**Date**: 2026-03-20
**Status**: Approved
**Scope**: Core memory system enhancement

## Problem

Aleph currently sends ALL conversation messages to the LLM provider without truncation or compression. The only limits are:
- Session history fetch: last 50 messages at loop start
- Per-request token budget: reactive (stops execution after budget exceeded, doesn't prevent overflow)
- System prompt: 80K character budget with prioritized layer removal

This means long conversations (especially with tool-heavy agent loops doing 25+ iterations) can exceed the provider's context window, causing failures or degraded reasoning quality.

## Solution

Add intra-session context management inspired by [lossless-claw](https://github.com/Martian-Engineering/lossless-claw)'s LCM (Lossless Context Management), adapted to Aleph's existing architecture:

- **DAG-less design**: No SQLite DAG. Summaries stored as `MemoryFact` in LanceDB with `scope=SessionLocal`, leveraging existing storage and retrieval infrastructure.
- **Dual-layer compression**: Loop-internal deterministic tool result compaction (zero LLM cost) + post-turn async LLM summarization (depth-aware prompts).
- **Three-level fallback**: Normal LLM → Aggressive LLM → Deterministic truncation. Compression always makes progress.
- **"Expand for details" annotations**: Each summary lists what was compressed away, enabling the agent to search for details via `memory_search`.

### Key Design Principles

- **R3 Core Minimalism**: No new storage backends or external dependencies
- **R8 LLM Sovereignty**: LLM generates summaries; system only orchestrates compression timing
- **R9 Everything is a Tool**: Retrieval of compressed history via existing `memory_search` tool
- **P6 Simplicity**: No DAG data structure; flat MemoryFact with depth metadata
- **P7 Defensive Design**: Compression failure never blocks user interaction

## Architecture

### New Module: `src/memory/session_compactor/`

```
src/memory/session_compactor/
├── mod.rs              // SessionCompactor struct + orchestration
├── tool_compactor.rs   // Deterministic tool result compression
├── summary_engine.rs   // LLM summary generation (depth-aware prompts)
├── context_window.rs   // Context window management (threshold detection, message partitioning)
└── fallback.rs         // Deterministic fallback strategy
```

### Data Flow

```
User Input
  ↓
ExecutionEngine::execute()
  ↓
SessionCompactor::prepare_history()
  ├─ Query LanceDB for session summaries (scope=SessionLocal, fact_source=SessionCompressed)
  ├─ Fetch recent N raw messages from session store (fresh tail)
  ├─ Assemble: [summary_d2, summary_d1, ..., summary_d0_1, ..., raw_msg_31, ..., raw_msg_50]
  │             ├── summary zone (older = higher depth = more abstract) ──┤  ├── fresh tail ──┤
  └─ Evict oldest low-depth summaries if still over budget
  ↓
AgentLoop::run_with_history(compressed_history)
  ↓
  ┌─── Loop Iteration ───┐
  │                       │
  │  [Threshold Check]    │  ← Before each provider.call()
  │  Estimate token count │    If > 75% of token budget:
  │  of current messages  │    → ToolCompactor deterministically compresses processed tool results
  │                       │
  │  provider.call()      │  ← Think
  │  messages.push()      │
  │  tool execution       │  ← Act
  │  messages.push()      │
  │                       │
  └───────────────────────┘
  ↓
Loop Complete
  ↓
SessionCompactor::post_turn_compress()  ← async, non-blocking
  ├─ Select old message chunks outside fresh tail
  ├─ LLM summary → MemoryFact (scope=SessionLocal, source=SessionCompressed)
  ├─ Store in LanceDB
  └─ Deterministic fallback on LLM failure
```

## Component Details

### 1. ToolCompactor (Loop-Internal, Synchronous)

**Purpose**: Deterministically compress already-processed tool results to free token space within the loop. Zero LLM calls.

**Trigger**: Before each `provider.call()`, estimate `messages` token count. If > `context_threshold` (default 75% of token budget), compress.

**Token Estimation**: `content.len() / 3.5` (heuristic for mixed CJK/English content). No tokenizer needed.

**Strategy by tool type**:

| Tool Type | Compression | Example |
|-----------|------------|---------|
| File Read (Read/Glob) | `"[Read {path}, {lines} lines, {lang}]"` | `"[Read src/main.rs, 312 lines, Rust]"` |
| Search (Grep/Search) | `"[Search '{pattern}', {n} matches in {m} files]"` | `"[Search 'fn execute', 5 matches in 3 files]"` |
| Bash | `"[Executed {cmd_summary}, exit {code}, {n} lines output]"` | `"[Executed cargo test, exit 0, 47 lines]"` |
| Web Fetch | Keep first 200 chars + `"[Truncated, original {n} tokens]"` | Preserve semantics for web content |
| Other | If > 500 tokens, truncate to first 200 chars + tail note | Generic fallback |

**Constraints**:
- Only compress tool results that have been "consumed" (assistant message follows the tool_result)
- Never compress fresh tail tool results
- Preserve `tool_use_id` to maintain tool_use/result pairing
- Compress oldest first, stop when under threshold

**Integration**: Inject `Option<Arc<ToolCompactor>>` into `AgentLoop` via constructor. Single call before `provider.call()`:

```rust
// loop_core.rs, before provider.call()
if let Some(ref compactor) = self.tool_compactor {
    compactor.compact_if_needed(&mut messages, self.config.token_budget);
}
```

### 2. SummaryEngine (Post-Turn, Asynchronous)

**Purpose**: Generate high-quality LLM summaries of old message chunks, stored as session-scoped MemoryFacts.

**Trigger**: `ExecutionEngine::execute()` calls `SessionCompactor::post_turn_compress()` via `tokio::spawn` after agent loop completes, parallel with `write_conversation_memory()`.

**Message Partitioning**:

```
[Message List]
├── Already compressed: existing session summaries from prior rounds
├── Compressible zone: raw messages outside fresh tail (compression candidates)
└── Fresh tail: last 20 raw messages (protected, never compressed)
```

**Chunking**: Compressible messages grouped into chunks of ~800-1200 tokens each.

**Depth-Aware Prompts** (borrowed from lossless-claw):

| Depth | Trigger | Prompt Strategy | Retains |
|-------|---------|----------------|---------|
| d0 (leaf) | Raw message chunk ≥ 800 tokens | Preserve details | Key decisions, file operations, errors, TODOs |
| d1 (session) | 4+ consecutive d0 summaries | Distill patterns | Decisions with rationale, task status, blockers |
| d2 (milestone) | 3+ consecutive d1 summaries | Abstractify | Completed work, active constraints, evolution |

No d3+ — single sessions rarely need a fourth abstraction layer. YAGNI.

**"Expand for details" Annotation**: Each summary ends with:

```
Expand for details: specific code diffs, test output details, intermediate compilation errors
```

This tells the LLM what was compressed away and that it can use `memory_search` to retrieve details.

**Previous Context Continuity**: When generating a leaf summary, pass the most recent prior summary as `previous_context` to the LLM prompt, avoiding information duplication.

**Three-Level Fallback** (borrowed from lossless-claw):

1. **Normal**: Standard LLM summary, target = `max(128, min(800, input_tokens * 0.35))` tokens
2. **Aggressive**: If normal output ≥ input tokens, switch to compact prompt, target = `max(64, min(400, input_tokens * 0.2))` tokens
3. **Fallback**: If aggressive still fails or LLM call errors, deterministic truncation — extract first sentence of each message, concatenate, limit to 512 chars + `[Truncated]`

**Empty Output Protection**: If LLM returns empty content → retry once at temperature 0.05 → deterministic truncation fallback.

### 3. ContextWindow (History Assembly)

**Purpose**: Assemble compressed history for `AgentLoop` at session start.

**SessionCompactor Construction & Dependencies**:

`SessionCompactor` is constructed in `ExecutionEngine` and holds:

```rust
pub struct SessionCompactor {
    database: MemoryBackend,           // For reading/writing SessionLocal facts (LanceDB)
    agent_provider: Arc<dyn AiProvider>, // For LLM summary calls (reuses agent's provider)
    config: SessionCompactorConfig,    // Thresholds, fresh_tail_count, etc.
}
```

It is injected into `ExecutionEngine` via builder pattern (like `compression_service`), stored as `Option<Arc<SessionCompactor>>`. In `run_agent_loop()`, it is passed the `AgentInstance` and `SessionKey` to access session store and agent config.

**`SessionCompactor::prepare_history()` signature**:

```rust
pub async fn prepare_history(
    &self,
    agent: &AgentInstance,       // Access to session store via agent.get_history()
    session_key: &SessionKey,   // Identifies current session for LanceDB queries
    current_input: &str,        // Excluded from history (same as build_loop_history)
    token_budget: u64,          // From agent config or default
) -> Vec<UnifiedMessage>
```

This replaces `build_loop_history()` in `run_loop.rs`. When `SessionCompactor` is `None`, the existing `build_loop_history()` logic is used as fallback (backward compatible).

**Assembly steps**:

1. Query LanceDB for session summary facts: `scope=SessionLocal, fact_source=SessionCompressed, is_valid=true`
2. Fetch last `fresh_tail_count` (default 20) raw messages from session store
3. Assemble: summaries (highest depth first, then by seq) + fresh tail raw messages
4. Summary injection format (user role message):

```xml
<session_context depth="0" time_range="14:30-14:45" source_messages="8">
Refactored Gateway module routing logic...
Expand for details: specific code diffs, test output details
</session_context>
```

5. Eviction: If summaries + fresh tail still exceed budget, evict oldest low-depth summaries first (higher depth = more condensed = higher retention value)

**Summary Deduplication**: When d0 summaries are condensed into d1, source d0 facts are marked `is_valid = false`. `prepare_history()` only injects `is_valid = true` summaries. Invalidated d0s remain in LanceDB for `memory_search` retrieval.

### 4. System Prompt Injection

When session summaries exist in the message list, inject a `SessionContextGuideLayer` (new PromptLayer, priority near MemoryAugmentationLayer):

```
## Session Context Notes
Messages tagged with <session_context> are compressed summaries of earlier conversation.
- Summaries preserve key decisions and results but omit details
- If you need specific details (code, error messages, configs), use memory_search with scope="current_session"
- Do not guess specific details from summaries — search first when uncertain
```

Only injected when summaries are present. Zero overhead for short sessions.

### 5. MemoryFact Storage Schema

Summaries stored as MemoryFact with these field mappings:

```rust
MemoryFact {
    id: format!("sess_{session_id}_{depth}_{seq}"),
    content: summary_text,                          // Summary body + "Expand for details: ..."
    fact_type: FactType::Event,                     // Session event
    fact_source: FactSource::SessionCompressed,     // NEW enum value
    tier: MemoryTier::ShortTerm,                    // Session summaries are short-term
    scope: MemoryScope::SessionLocal,               // NEW enum value
    layer: match depth {
        0 => MemoryLayer::L2Detail,
        1 => MemoryLayer::L1Overview,
        _ => MemoryLayer::L0Abstract,
    },
    path: format!("aleph://session/{session_id}/d{depth}/{seq}"),
    confidence: 0.9,
    // metadata fields:
    //   depth: u32
    //   source_message_count: u32
    //   source_token_count: u32
    //   earliest_at: timestamp
    //   latest_at: timestamp
}
```

### 6. memory_search Extension

Add optional `scope` parameter to `MemorySearchTool`:

```json
{
  "query": "Gateway routing refactor details",
  "scope": "current_session"
}
```

Scope values:
- `"all"` (default): Existing behavior, cross-session memory search
- `"current_session"`: Only search current session's compressed summaries (including invalidated d0s for detail retrieval)
- `"both"`: Merge results, session-internal results ranked first

`current_session` filter:

```rust
SearchFilter {
    scope: Some(MemoryScope::SessionLocal),
    path_prefix: Some(format!("aleph://session/{}/", current_session_id)),
    is_valid: None,  // Include invalidated d0s (they hold details)
    ..Default::default()
}
```

Results annotated with depth and status:

```
[d0 | 14:30-14:45 | condensed into d1] Refactored Gateway module...
[d1 | 14:30-15:20 | active] Completed Gateway refactor and testing...
```

### 7. Session Lifecycle

**Retention**: SessionLocal facts retained for 24 hours after session ends (configurable).

**Promotion**: During periodic compression, `CompressionService` promotes high-value SessionLocal facts (confidence ≥ 0.8) to `scope = Agent` (the existing `MemoryScope::Agent` variant, scoped to the current workspace) for long-term memory.

**Cleanup**: `DreamDaemon` deletes expired SessionLocal facts during idle windows.

## Files to Modify

| File | Change |
|------|--------|
| `memory/store/types.rs` | Add `MemoryScope::SessionLocal`, `FactSource::SessionCompressed` enum values. Note: `SessionCompressed` is distinct from existing `FactSource::Summary` — `Summary` is used by L1 VFS overviews (cross-session), while `SessionCompressed` marks intra-session DAG-style summaries with depth metadata. |
| `memory/context/fact.rs` | Metadata support for depth/source_message_count fields |
| `gateway/execution_engine/engine.rs` | Inject SessionCompactor, trigger post_turn_compress after loop |
| `gateway/execution_engine/run_loop.rs` | Replace `build_loop_history()` with `SessionCompactor::prepare_history()` |
| `agent_loop/loop_core.rs` | Inject optional ToolCompactor, add threshold check before provider.call() |
| `builtin_tools/memory_search.rs` | Add scope parameter, construct SessionLocal filter |
| `memory/compression/service.rs` | Handle SessionLocal fact promotion/expiry in cleanup logic |

## New Files

| File | Purpose |
|------|---------|
| `memory/session_compactor/mod.rs` | SessionCompactor struct + orchestration |
| `memory/session_compactor/tool_compactor.rs` | Deterministic tool result compression |
| `memory/session_compactor/summary_engine.rs` | LLM summary generation with depth-aware prompts |
| `memory/session_compactor/context_window.rs` | Context window management, message partitioning, history assembly |
| `memory/session_compactor/fallback.rs` | Deterministic fallback strategy (three-level) |

## Error Handling

| Failure | Response |
|---------|----------|
| LLM summary call fails (network/timeout) | Three-level fallback: Normal → Aggressive → Deterministic truncation |
| LLM returns empty content | Retry once at temperature 0.05, then deterministic truncation |
| LLM summary longer than input | Auto-switch to Aggressive prompt, then deterministic truncation |
| LanceDB write fails | Log, skip this round, retry next turn (raw messages remain in session store) |
| Token estimation inaccurate causing overflow | Provider-level truncation/error; next loop iteration triggers more aggressive ToolCompactor |
| SessionCompactor initialization fails | Fall back to no-compression mode (existing behavior), log warning |

**Invariants**:
- Raw messages always retained in session store; compression never deletes source data
- ToolCompactor only modifies the in-memory `messages` vector, never persistent data
- post_turn_compress runs via `tokio::spawn`; panic does not affect main thread

## Performance

| Component | Timing | Impact |
|-----------|--------|--------|
| ToolCompactor (in-loop, sync) | < 1ms | Token estimation O(n) over messages, string replacement |
| SummaryEngine (post-loop, async) | 1-3s per LLM call | Non-blocking, user's next input sees results |
| LanceDB query (prepare_history) | < 10ms | Small result set (< 20 summaries per session) |
| LanceDB insert (post-compress) | < 5ms | Single fact insert |

## Configuration

```toml
[memory.session_compactor]
enabled = true
fresh_tail_count = 20              # Protected recent message count
context_threshold = 0.75           # Token ratio triggering compression
leaf_chunk_tokens = 1000           # Max source tokens per d0 summary
d1_min_fanout = 4                  # Min d0 summaries to trigger d1 condensation
d2_min_fanout = 3                  # Min d1 summaries to trigger d2 condensation
max_summary_depth = 2              # Max summary depth (no d3+)
token_estimate_ratio = 3.5         # chars / ratio = estimated tokens
session_fact_retention_hours = 24  # SessionLocal fact retention after session ends
promote_confidence_threshold = 0.8 # Confidence threshold for promotion to long-term memory
```

## Testing Strategy

**Unit Tests** (`cargo test -p alephcore --lib`):

| Module | Test Focus |
|--------|-----------|
| `tool_compactor` | Per-tool-type deterministic compression formats, threshold trigger logic, oldest-first compression order |
| `context_window` | Message partitioning (summary/compressible/fresh tail), eviction strategy, token estimation |
| `summary_engine` | Prompt construction, depth increment logic, previous_context passing |
| `fallback` | Three-level fallback chain, deterministic truncation output format |
| `prepare_history` | Summary + raw message assembly, deduplication (invalidated d0 not injected), eviction order |

**Integration Tests** (mock LLM provider):

- Full loop: 50 messages → ToolCompactor → provider call → post_turn_compress → next prepare_history retrieves summaries
- Multi-round compression: d0 accumulation → d1 condensation triggered → d0 invalidated → prepare_history injects only d1
- Fallback path: Mock LLM returns empty/oversized → verify fallback engages

## What This Design Does NOT Do

- **No DAG data structure**: Summaries are flat MemoryFacts with depth metadata, not nodes in a graph
- **No SQLite**: All storage via existing LanceDB backend
- **No cross-session compression changes**: Existing CompressionService pipeline unchanged
- **No new tools**: Extended existing memory_search rather than adding new retrieval tools
- **No d3+ depth**: Single sessions don't need four abstraction layers
- **No precise tokenizer**: Heuristic estimation sufficient for threshold checks
