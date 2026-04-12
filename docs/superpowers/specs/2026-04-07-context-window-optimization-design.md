# Context Window Optimization — Learning from Claude Code

**Date:** 2026-04-07
**Status:** Approved
**Scope:** 5 modules (G1-G5), ~480 new lines + ~150 modified lines

## Background

Analysis of Claude Code's context window management reveals a mature "context economics" system with multi-layered compression, cache optimization, and post-compaction recovery. Aleph already has strong foundations (5-layer defense, 3D compressibility scoring, hierarchical summary tree, truncation recovery state machine), but gaps remain in 5 areas.

G6 (Cache-safe content editing via `cache_edits` API) was evaluated and **rejected** — Anthropic's prompt caching is prefix-match only, no partial edit API exists.

## Implementation Order

```
G5 (structured summary template) → G1 (disk persistence) → G2 (file recovery) → G3 (cache monitor) → G4 (zero-cost compact)
```

Rationale: G5 improves all subsequent compression quality (pure prompt change). G1 builds disk I/O infrastructure reused by G2. G3 adds observability once the compression system is complete. G4 is most complex (async MemoryStore queries) and goes last.

---

## G5: Structured Compression Summary Templates

**Problem:** `context_compactor.rs` uses a 3-line prompt. `summary_engine.rs` LEAF_PROMPT uses free-form bullet lists. LLM may omit critical dimensions.

### Changes

#### 1. Upgrade `context_compactor.rs` prompt (lines 138-143)

Replace the simple prompt with `<analysis>` + `<summary>` structure matching `summary_engine.rs`, using 5 mandatory sections:

```
## Primary Request
## Key Decisions
## Files & Code
## Current State
## Pending
```

Also: attach `IDENTIFIER_PRESERVATION` directive, use `strip_analysis_block()` on output.

#### 2. Upgrade `summary_engine.rs` LEAF_PROMPT (lines 27-36)

Change `<summary>` from free-form bullets to the same 5 mandatory sections. D1/D2 templates unchanged (already well-structured).

#### 3. Extract shared utilities

New file: `src/agent_loop/compaction/summary_utils.rs`
- `strip_analysis_block()` (moved from `summary_engine.rs`, re-exported)
- `IDENTIFIER_PRESERVATION` constant (moved from `summary_engine.rs`, re-exported)

### Files

| File | Action |
|------|--------|
| `src/agent_loop/compaction/summary_utils.rs` | **New** ~40 lines |
| `src/agent_loop/context_compactor.rs` | Modify prompt + import strip_analysis_block |
| `src/memory/session_compactor/summary_engine.rs` | LEAF_PROMPT sections + re-export shared code |
| `src/agent_loop/compaction/mod.rs` | Export new module |

---

## G1: Large Tool Result Disk Persistence

**Problem:** Tool results exceeding 8K tokens are destructively truncated. The original content is permanently lost.

### Design

Insert a disk persistence step between `compress_tool_output()` and `truncate_tool_result()` in `tool_pipeline.rs::map_result()`.

#### New: `src/agent_loop/tool_result_store.rs` (~100 lines)

```rust
pub struct ToolResultStore {
    base_dir: PathBuf,  // ~/.aleph/data/tool_results/{session_id}/
}

impl ToolResultStore {
    pub fn new(session_id: &str) -> Self;
    
    /// Write to disk if content exceeds threshold. Returns reference marker.
    pub fn persist_if_large(
        &self, tool_call_id: &str, tool_name: &str,
        content: &str, threshold_tokens: usize,
    ) -> Option<String>;
    
    /// Clean up session files (called on session end).
    pub fn cleanup(&self);
}
```

Storage: plain text at `~/.aleph/data/tool_results/{session_id}/{tool_call_id}.txt`.

#### Modified: `tool_pipeline.rs::map_result()`

```
raw → compress_tool_output() → persist_if_large() → truncate_tool_result()
```

When persisted, append `[Full output persisted: {path} ({tokens} tokens)]` to the truncated text.

#### Modified: `micro_compactor.rs::format_compact_placeholder()`

When replacing old tool results with placeholders, detect and preserve `[Full output persisted: ...]` lines via `extract_persisted_ref()`.

### Files

| File | Action |
|------|--------|
| `src/agent_loop/tool_result_store.rs` | **New** ~100 lines |
| `src/agent_loop/tool_pipeline.rs` | Add `result_store` field + persist_if_large call ~30 lines |
| `src/agent_loop/compaction/micro_compactor.rs` | Preserve disk refs in placeholders ~15 lines |
| `src/agent_loop/mod.rs` | Export new module |

### Constraints

- Plain text storage, no serialization dependencies
- `ToolResultStore` is `Option` in `ToolPipeline` — existing tests pass `None`
- Session cleanup via explicit `cleanup()` or `Drop`

---

## G2: Post-Compaction File Content Recovery

**Problem:** After compaction, the LLM loses the content of recently read files. It must re-read them, wasting a tool call.

### Design

New `FileContentTracker` implements the existing `ConstraintSource` trait (zero-invasive plugin).

#### New: `src/agent_loop/compaction/file_content_tracker.rs` (~80 lines)

```rust
pub struct FileContentTracker {
    recent_reads: Mutex<VecDeque<FileReadRecord>>,  // LRU, max 5
}

struct FileReadRecord {
    path: String,
    preview: String,     // first 5000 chars (~1.4K tokens)
    line_count: usize,
}

impl FileContentTracker {
    pub fn new() -> Self;
    pub fn record_read(&self, path: &str, content: &str);
}

impl ConstraintSource for FileContentTracker {
    fn collect_constraints(&self) -> Vec<Constraint>;
    // Emits ConstraintCategory::RecentFile with priority 60
}
```

#### Modified: `constraint_injector.rs`

- Add `RecentFile` variant to `ConstraintCategory`
- Add `### Recently Read Files` section in `format_injection()`

#### Modified: `tool_pipeline.rs`

- Add `file_tracker: Option<Arc<FileContentTracker>>` field
- After successful `read_file` tool execution, call `tracker.record_read(path, content)`

#### Modified: `loop_core.rs`

Register `FileContentTracker` as a `ConstraintSource` in `ConstraintInjector::new()`.

### Files

| File | Action |
|------|--------|
| `src/agent_loop/compaction/file_content_tracker.rs` | **New** ~80 lines |
| `src/agent_loop/compaction/constraint_injector.rs` | New category + format section ~15 lines |
| `src/agent_loop/tool_pipeline.rs` | New field + record_read ~15 lines |
| `src/agent_loop/loop_core.rs` | Register tracker ~5 lines |
| `src/agent_loop/compaction/mod.rs` | Export new module |

### Constraints

- Max 5 files, 5000 chars preview each → ~7K tokens total worst case
- Same-path deduplication (newer replaces older)
- `Send + Sync` via `Mutex` — safe for pipeline/injector sharing

---

## G3: Prompt Cache Break Detection

**Problem:** `TokenUsage.cache_read_tokens` exists but is never consumed. No visibility into cache hit rates or break causes.

### Design

Lightweight monitor that correlates stable prompt hash with API cache_read_tokens.

#### New: `src/thinker/prompt_builder/cache_monitor.rs` (~90 lines)

```rust
pub struct CacheMonitor {
    state: Mutex<MonitorState>,
}

struct MonitorState {
    stable_hash: Option<u64>,
    consecutive_misses: u32,
    total_calls: u64,
    total_hits: u64,
}

impl CacheMonitor {
    pub fn new() -> Self;
    
    /// Update stable prompt hash. Returns true if hash changed.
    pub fn update_stable_hash(&self, stable_content: &str) -> bool;
    
    /// Record cache usage from API response. Warns after 3 consecutive misses.
    pub fn record_cache_usage(&self, cache_read_tokens: Option<u32>);
    
    /// Reset consecutive miss counter after compaction (legitimate cache break).
    pub fn notify_compaction(&self);
    
    /// Current hit rate percentage.
    pub fn hit_rate(&self) -> f64;
}
```

#### Integration points

| Location | Change |
|----------|--------|
| `loop_core.rs::record_response_usage()` | Call `monitor.record_cache_usage(usage.cache_read_tokens)` |
| `cache.rs::build_system_prompt_cached()` | Call `monitor.update_stable_hash(&stable)` |
| `compaction/orchestrator.rs` | Call `monitor.notify_compaction()` after execution |

### Files

| File | Action |
|------|--------|
| `src/thinker/prompt_builder/cache_monitor.rs` | **New** ~90 lines |
| `src/agent_loop/loop_core.rs` | New field + record call ~10 lines |
| `src/thinker/prompt_builder/cache.rs` | Hash update call ~5 lines |
| `src/agent_loop/compaction/orchestrator.rs` | Notify compaction ~3 lines |
| `src/thinker/prompt_builder/mod.rs` | Export new module |

### Constraints

- Pure observation, zero side effects
- Uses `std::collections::hash_map::DefaultHasher` (no new dependencies)
- 3 consecutive misses threshold to avoid noise
- Compaction resets counter to prevent false positives

---

## G4: Zero-Cost Session Memory Compaction

**Problem:** `ContextCompactor::compact()` always makes an LLM API call, even when `SessionCompactor` has already generated summaries covering the same message window.

### Design

New "summary reuse" fast path in `compact()` that queries LanceDB for existing summaries before falling back to LLM.

#### New: `src/agent_loop/compaction/session_summary_source.rs` (~90 lines)

```rust
pub struct SessionSummarySource {
    database: MemoryBackend,
    session_id: String,
}

impl SessionSummarySource {
    pub fn new(database: MemoryBackend, session_id: String) -> Self;
    
    /// Try to replace compression window with existing summaries.
    /// Returns None if insufficient coverage → caller falls through to LLM.
    pub async fn try_reuse(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        window_start: usize,
        cut_end: usize,
    ) -> Option<CompactResult>;
}
```

Reuse strategy:
1. Query LanceDB for `aleph://session/{id}/d*` facts (valid, SessionLocal scope)
2. Sort highest-depth-first (d2 > d1 > d0)
3. Assemble within 50% of original window token budget
4. Replace window with `[Context Summary (from session memory)]` message

#### Modified: `context_compactor.rs`

- Add `SessionMemoryReuse` variant to `CompactStrategy`
- `compact()` accepts optional `&SessionSummarySource`, tries reuse before LLM path
- Signature change: `compact(&self, messages, fresh_tail, summary_source: Option<&SessionSummarySource>)`

#### Modified: `loop_core.rs`

Construct `SessionSummarySource` from existing `MemoryBackend` and `session_id`, pass to `compact()`.

### Files

| File | Action |
|------|--------|
| `src/agent_loop/compaction/session_summary_source.rs` | **New** ~90 lines |
| `src/agent_loop/context_compactor.rs` | New strategy variant + fast path ~15 lines |
| `src/agent_loop/loop_core.rs` | Construct source + pass to compact ~5 lines |
| `src/agent_loop/compaction/mod.rs` | Export new module |

### Constraints

- Read-only access to LanceDB — does not modify SessionCompactor's write path
- `try_reuse` returns `None` on any error — transparent fallthrough
- Token budget conservative (50% of original window)

---

## Summary of All New Files

| # | File | Module | Lines |
|---|------|--------|-------|
| 1 | `src/agent_loop/compaction/summary_utils.rs` | G5 | ~40 |
| 2 | `src/agent_loop/tool_result_store.rs` | G1 | ~100 |
| 3 | `src/agent_loop/compaction/file_content_tracker.rs` | G2 | ~80 |
| 4 | `src/thinker/prompt_builder/cache_monitor.rs` | G3 | ~90 |
| 5 | `src/agent_loop/compaction/session_summary_source.rs` | G4 | ~90 |

**Total new code:** ~400 lines across 5 files
**Total modifications:** ~150 lines across 10 existing files

## Testing Strategy

Each module includes `#[cfg(test)] mod tests`:
- **G5:** Verify prompt contains mandatory sections, strip_analysis_block works from shared location
- **G1:** Persist/retrieve roundtrip, cleanup removes files, small results not persisted
- **G2:** LRU eviction, deduplication, format_injection includes file section
- **G3:** Consecutive miss detection, compaction reset, hit rate calculation
- **G4:** Reuse when summaries exist, fallthrough when empty, token budget enforcement

## Non-Goals

- No changes to D1/D2 summary templates (already well-structured)
- No changes to `SessionCompactor::post_turn_compress()` write path
- No new external dependencies
- No `cache_edits` API support (G6 — blocked on Anthropic API)
