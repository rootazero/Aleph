# Context Optimization Design Spec

> Date: 2026-04-06
> Status: Approved
> Approach: Incremental Enhancement (Option A)
> Estimated Scope: ~950 lines across 4 independent improvements

## Background

Claude Code treats context as a runtime budget with sophisticated optimization strategies:
system prompt static/dynamic separation, prompt cache boundaries, fork path cache sharing,
skill on-demand injection, MCP instructions per-connection injection, function result clearing,
tool result summarization, and compact/transcript/resume.

Aleph already has mature implementations for most of these (PromptLayer stability, SharedSnapshot,
CompactionPipeline, tool_compactor, session_compactor, skill deferred loading). This spec
addresses the four remaining gaps.

## Gap Analysis

| Claude Code Feature | Aleph Status | Gap |
|---------------------|-------------|-----|
| System prompt static/dynamic | Mature | None |
| Prompt cache boundary | Mature | None |
| Fork path shared cache | Mature | None |
| Skill on-demand injection | Mature | None |
| MCP instructions injection | Partial | No dedicated PromptLayer |
| Function result clearing | Partial | Only fixed-text clearing, no semantic preservation |
| Tool result summarization | Mature | None |
| Subagent transcript | Partial | Logged only, not persisted or compressed |
| Session resume | Missing | No cross-session context restoration |

## Design

### 1. MCP Instructions Prompt Layer

**Goal**: Inject connected MCP server instructions into system prompt, giving LLM awareness of
how to use external tools.

**New file**: `src/thinker/layers/mcp_instructions.rs`

**Implementation**:
- Struct `McpInstructionsLayer` implementing `PromptLayer`
- Priority: **1060** (after SkillInstructions at 1050)
- Stability: **Dynamic** (changes with MCP connection state)
- `inject()` logic:
  - Read connected MCP servers from `LayerInput`
  - Extract each server's `instructions` field
  - Skip servers with empty instructions
  - Format as:
    ```
    # MCP Server Instructions

    ## {server-name}
    {instructions content}
    ```

**LayerInput extension**:
- New field: `mcp_instructions: Option<Vec<McpServerInstruction>>`
- Type: `McpServerInstruction { server_name: String, instructions: String }`

**Registration**: Add to `PromptPipeline::default_layers()`

**Aleph advantage over Claude Code**: Claude Code concatenates MCP instructions at system prompt
tail. Aleph uses the PromptLayer system, gaining automatic cache separation, budget truncation,
and mode-aware filtering for free.

**Estimated scope**: ~150 lines

---

### 2. Function Result Clearing (Enhanced)

**Goal**: Replace stale tool results with semantic summaries instead of fixed-text placeholders,
preserving context information while freeing tokens.

**Current state**: `MicroCompact` stage in `CompactionPipeline` replaces old results with
`"[Old result cleared]"` — loses all semantic information.

**Implementation**:

Rename `MicroCompact` → `ResultClearing` in `src/agent_loop/context_budget/pipeline.rs`.
Upgrade clearing logic to a tiered strategy based on message age:

| Age Tier | Treatment |
|----------|-----------|
| Within fresh_tail (recent N rounds) | Keep original |
| Outside fresh_tail, within half-life | Compress via per-tool semantic compressor |
| Beyond half-life | Replace with one-liner: `[{tool_name}: {one-sentence summary}]` |

**Dynamic half-life**: Derived from `ContextPressure` — higher pressure = shorter half-life.
Formula: `half_life_rounds = max(3, (1.0 - pressure.ratio) * 20) as usize`

**Per-tool semantic compressors** (already exist in `tool_compactor.rs`):
- `compress_file_read()` → `"[Read file, N lines, LANG]"`
- `compress_search()` → `"[Search result, N matching lines]"`
- `compress_bash()` → `"[Command output, N lines]"`
- `compress_web()` → keeps first 200 chars + truncation note

**New addition to `tool_compactor.rs`**:
- `compress_to_oneliner(tool_name: &str, content: &str) -> String` — ultra-compact single-line
  summary for beyond-half-life results

**Modified files**:
- `src/agent_loop/context_budget/pipeline.rs` — `MicroCompact` → `ResultClearing`
- `src/memory/session_compactor/tool_compactor.rs` — add `compress_to_oneliner()`

**Aleph advantage over Claude Code**: Claude Code uses binary clearing (keep or clear).
Aleph's tiered strategy with per-tool semantic compressors preserves more useful information
at each degradation level.

**Estimated scope**: ~250 lines

---

### 3. Subagent Transcript Recording

**Goal**: Persist subagent conversation outcomes as structured sidechain transcripts for
future retrieval and context-aware compression.

**Current state**: `AgentRuntime` generates `SubagentTranscript` struct but only uses it for
tracing logs — not persisted or reusable.

#### 3a. Recording Side

**Modified file**: `src/agent_loop/agent_runtime.rs`

After `AgentRuntime::run()` completes:
- Serialize `SubagentTranscript` to `~/.aleph/data/transcripts/{session_id}/{agent_id}.json`
- Extended fields:
  - `key_findings: String` — first 200 characters of subagent's final assistant message
  - Existing fields: agent_id, agent_type, task_summary, outcome, iterations, tokens_used, duration_ms
- Auto-cleanup: files older than 7 days removed by existing `session_compactor` periodic sweep

#### 3b. Compression Side

**Modified file**: `src/memory/session_compactor/tool_compactor.rs`

New compressor function:
- `compress_subagent(content: &str) -> String`
- Output format: `[Subagent {agent_type}: {task_summary} -> {outcome}, {iterations} iterations]`

**Modified file**: `src/agent_loop/context_budget/pipeline.rs`

The `ResultClearing` stage (from §2) recognizes subagent tool results by matching
`tool_name == "subagent"` (the SubagentTool's registered name in tool registry) and
dispatches to `compress_subagent()` instead of generic compressors.

#### 3c. Retrieval Side (Phase 2, not in scope)

Future: embed `key_findings` via `transcript_indexer` for semantic retrieval.
Data format already includes all fields needed for this extension.

**Aleph advantage over Claude Code**: Claude Code's `recordSidechainTranscript()` targets UI
display. Aleph's transcripts are JSON-structured for retrieval — naturally extensible to
semantic search via existing embedding infrastructure.

**Estimated scope**: ~150 lines

---

### 4. Session Resume

**Goal**: Save compressed conversation snapshot at session end; restore context in next session.

**New module**: `src/memory/session_resume/`
- `mod.rs` — public API
- `snapshot.rs` — type definitions and serialization
- `writer.rs` — snapshot persistence
- `reader.rs` — snapshot loading

#### 4a. Snapshot Type

```rust
pub struct SessionSnapshot {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub summary: String,            // From session_compactor d1 layer
    pub key_decisions: Vec<String>,  // Extracted from d1 summary by splitting on decision markers
                                     // (sentences containing "decided", "chose", "will use", "switched to")
    pub active_files: Vec<String>,   // Recently operated file paths
    pub tool_state: Option<String>,  // Tool state summary (cwd, open connections)
    pub pending_tasks: Vec<String>,  // Incomplete tasks
}
```

#### 4b. Write Triggers

Two trigger points:
1. **Session end** — `AgentLoop` stop triggers snapshot write
2. **High pressure insurance** — `ContextPressure.ratio > 0.8` and no snapshot yet saved

#### 4c. Storage

- Path: `~/.aleph/data/sessions/{session_id}/resume.json`
- Retention: keep last 10 session snapshots, older ones auto-cleaned

#### 4d. Summary Generation

**No additional LLM calls**. Reuse `session_compactor`'s existing d1-level summary —
it already produces high-quality conversation compression as part of normal operation.
Simply extract the latest d1 summary as the `summary` field.

#### 4e. Restore Injection

**New file**: `src/thinker/layers/session_resume.rs`

`SessionResumeLayer` implementing `PromptLayer`:
- Priority: **1760** (after SessionContextGuide at 1750, last dynamic layer)
- Stability: **Dynamic**
- Activation: only on new session start when previous session's resume.json exists
- Injection format:
  ```
  # Previous Session Context
  Summary: {summary}
  Key decisions: {decisions}
  Active files: {files}
  Pending tasks: {tasks}
  ```

**Modified files**:
- New: `src/memory/session_resume/` (4 files)
- `src/agent_loop/loop_core.rs` — call snapshot writer on loop end
- New: `src/thinker/layers/session_resume.rs`
- `src/thinker/prompt_pipeline.rs` — register `SessionResumeLayer`

**Aleph advantage over Claude Code**: Claude Code resumes by replaying full transcript files.
Aleph uses compressed snapshots (typically < 2KB) generated by the existing hierarchical
compactor — fast to load, small footprint, and summary quality guaranteed by the proven
compression pipeline.

**Estimated scope**: ~400 lines

---

## File Change Summary

| Action | File | Section |
|--------|------|---------|
| **New** | `src/thinker/layers/mcp_instructions.rs` | §1 |
| **Modify** | `src/thinker/prompt_layer.rs` (LayerInput field) | §1 |
| **Modify** | `src/thinker/prompt_pipeline.rs` (register layers) | §1, §4 |
| **Modify** | `src/agent_loop/context_budget/pipeline.rs` (MicroCompact → ResultClearing) | §2, §3 |
| **Modify** | `src/memory/session_compactor/tool_compactor.rs` (new compressors) | §2, §3 |
| **Modify** | `src/agent_loop/agent_runtime.rs` (transcript persistence) | §3 |
| **New** | `src/memory/session_resume/mod.rs` | §4 |
| **New** | `src/memory/session_resume/snapshot.rs` | §4 |
| **New** | `src/memory/session_resume/writer.rs` | §4 |
| **New** | `src/memory/session_resume/reader.rs` | §4 |
| **New** | `src/thinker/layers/session_resume.rs` | §4 |
| **Modify** | `src/agent_loop/loop_core.rs` (snapshot trigger) | §4 |

## Dependencies Between Sections

None — all four sections are independent and can be implemented in parallel.

## Testing Strategy

Each section requires:
- Unit tests for new types and functions
- Integration test verifying the optimization's effect on context pressure

Specific tests:
- §1: Layer outputs correct format; empty instructions skipped; budget truncation works
- §2: Tiered clearing produces correct output per age tier; pressure-adaptive half-life
- §3: Transcript file written and readable; compress_subagent format correct
- §4: Snapshot serialization round-trip; SessionResumeLayer only activates with existing snapshot

## Design Principles Alignment

- **P1 Low Coupling**: Each section extends existing abstractions without cross-dependencies
- **P2 High Cohesion**: MCP layer in thinker/layers, compression in session_compactor, resume in memory
- **P3 Extensibility**: All new components follow existing trait patterns (PromptLayer, CompactionStage)
- **P6 Simplicity**: No new abstractions introduced — reuses PromptLayer, CompactionStage, tool_compactor
- **R8 LLM Sovereignty**: Session resume summary reuses compactor output, no extra LLM calls
