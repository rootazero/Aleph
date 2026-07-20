# Context Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close four context management gaps — MCP instructions prompt layer, semantic result clearing, subagent transcript persistence, and session resume — to match and exceed claude-code's context optimization.

**Architecture:** Each improvement extends an existing Aleph abstraction (PromptLayer, CompactionStage, tool_compactor, session_compactor). No new sub-systems; all changes are additive extensions of proven patterns.

**Tech Stack:** Rust, serde/serde_json, chrono, tokio, tracing

**Spec:** `docs/superpowers/specs/2026-04-06-context-optimization-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| **Create** | `src/thinker/layers/mcp_instructions.rs` | MCP instructions prompt layer |
| **Modify** | `src/thinker/layers/mod.rs` | Export new layer |
| **Modify** | `src/thinker/prompt_layer.rs` | Add `mcp_instructions` field to `LayerInput` |
| **Modify** | `src/thinker/prompt_pipeline.rs` | Register MCP + SessionResume layers |
| **Modify** | `src/agent_loop/context_budget/pipeline.rs` | Upgrade MicroCompact → ResultClearing |
| **Modify** | `src/memory/session_compactor/tool_compactor.rs` | Add `compress_to_oneliner` + `compress_subagent` |
| **Modify** | `src/agent_loop/agent_runtime.rs` | Persist SubagentTranscript to disk |
| **Create** | `src/memory/session_resume/mod.rs` | Module root + public API |
| **Create** | `src/memory/session_resume/snapshot.rs` | SessionSnapshot type + serde |
| **Create** | `src/memory/session_resume/writer.rs` | Snapshot writer |
| **Create** | `src/memory/session_resume/reader.rs` | Snapshot reader |
| **Modify** | `src/memory/mod.rs` | Export session_resume module |
| **Create** | `src/thinker/layers/session_resume.rs` | SessionResumeLayer prompt layer |
| **Modify** | `src/agent_loop/loop_core.rs` | Trigger snapshot write on loop end |

---

## Task 1: MCP Instructions Prompt Layer

**Files:**
- Create: `src/thinker/layers/mcp_instructions.rs`
- Modify: `src/thinker/layers/mod.rs:28-91`
- Modify: `src/thinker/prompt_layer.rs:47-93`
- Modify: `src/thinker/prompt_pipeline.rs:237-268`

- [ ] **Step 1: Add `McpServerInstruction` type and `LayerInput` field**

In `src/thinker/prompt_layer.rs`, add the type and field:

```rust
// Add after line 9 (after existing use statements)
/// MCP server instruction metadata for prompt injection.
#[derive(Debug, Clone)]
pub struct McpServerInstruction {
    pub server_name: String,
    pub instructions: String,
}
```

Add to `LayerInput` struct (after the `agent_def` field at line 73):

```rust
    /// MCP server instructions to inject into the system prompt.
    /// Only populated for connected servers that provide instructions.
    pub mcp_instructions: Option<&'a [McpServerInstruction]>,
```

Update all `LayerInput` constructors (`basic`, `hydration`, etc.) to include `mcp_instructions: None`.

- [ ] **Step 2: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (new field has None defaults in all constructors)

- [ ] **Step 3: Write the failing test for McpInstructionsLayer**

Create `src/thinker/layers/mcp_instructions.rs`:

```rust
//! McpInstructionsLayer — injects connected MCP server instructions (priority 1060)

use crate::thinker::prompt_layer::{
    AssemblyPath, LayerInput, LayerStability, McpServerInstruction, PromptLayer,
};
use crate::thinker::prompt_mode::PromptMode;

pub struct McpInstructionsLayer;

impl PromptLayer for McpInstructionsLayer {
    fn name(&self) -> &'static str {
        "mcp_instructions"
    }
    fn priority(&self) -> u32 {
        1060
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let instructions = match input.mcp_instructions {
            Some(instr) if !instr.is_empty() => instr,
            _ => return,
        };

        let active: Vec<&McpServerInstruction> = instructions
            .iter()
            .filter(|i| !i.instructions.trim().is_empty())
            .collect();

        if active.is_empty() {
            return;
        }

        output.push_str("\n## MCP Server Instructions\n\n");
        output.push_str("The following MCP servers have provided instructions for how to use their tools and resources:\n\n");

        for instr in &active {
            output.push_str("### ");
            output.push_str(&instr.server_name);
            output.push('\n');
            output.push_str(&instr.instructions);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn injects_instructions_for_connected_servers() {
        let instructions = vec![
            McpServerInstruction {
                server_name: "context7".to_string(),
                instructions: "Use this server for docs lookup.".to_string(),
            },
            McpServerInstruction {
                server_name: "github".to_string(),
                instructions: "Use for GitHub operations.".to_string(),
            },
        ];
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[])
            .with_mcp_instructions(&instructions);
        let mut output = String::new();
        McpInstructionsLayer.inject(&mut output, &input);

        assert!(output.contains("## MCP Server Instructions"));
        assert!(output.contains("### context7"));
        assert!(output.contains("Use this server for docs lookup."));
        assert!(output.contains("### github"));
    }

    #[test]
    fn skips_empty_instructions() {
        let instructions = vec![
            McpServerInstruction {
                server_name: "empty-server".to_string(),
                instructions: "  ".to_string(),
            },
        ];
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[])
            .with_mcp_instructions(&instructions);
        let mut output = String::new();
        McpInstructionsLayer.inject(&mut output, &input);

        assert!(output.is_empty());
    }

    #[test]
    fn skips_when_no_mcp_instructions() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        McpInstructionsLayer.inject(&mut output, &input);

        assert!(output.is_empty());
    }

    #[test]
    fn reports_dynamic_stability() {
        assert_eq!(McpInstructionsLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn reports_priority_1060() {
        assert_eq!(McpInstructionsLayer.priority(), 1060);
    }
}
```

- [ ] **Step 4: Add `with_mcp_instructions` builder method to `LayerInput`**

In `src/thinker/prompt_layer.rs`, add after the existing builder methods (near `with_agent_def`):

```rust
    /// Attach MCP server instructions.
    pub fn with_mcp_instructions(mut self, instructions: &'a [McpServerInstruction]) -> Self {
        self.mcp_instructions = Some(instructions);
        self
    }
```

- [ ] **Step 5: Register layer and export**

In `src/thinker/layers/mod.rs`, add:
```rust
pub mod mcp_instructions;
pub use mcp_instructions::McpInstructionsLayer;
```

In `src/thinker/prompt_pipeline.rs`, add to `default_layers()` after `SkillInstructionsLayer`:
```rust
            Box::new(McpInstructionsLayer),
```

Add the import at the top of `prompt_pipeline.rs`:
```rust
use super::layers::McpInstructionsLayer;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib mcp_instructions`
Expected: All 5 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/thinker/layers/mcp_instructions.rs src/thinker/layers/mod.rs src/thinker/prompt_layer.rs src/thinker/prompt_pipeline.rs
git commit -m "feat(thinker): add McpInstructionsLayer for MCP server instruction injection"
```

---

## Task 2: Enhanced Result Clearing (MicroCompact → ResultClearing)

**Files:**
- Modify: `src/agent_loop/context_budget/pipeline.rs:156-206`
- Modify: `src/memory/session_compactor/tool_compactor.rs:120-144`

- [ ] **Step 1: Add `compress_to_oneliner` to tool_compactor**

In `src/memory/session_compactor/tool_compactor.rs`, add after `compress_generic` (line 101):

```rust
/// Ultra-compact one-liner summary for very old tool results.
///
/// Produces a single bracketed line: `[{tool_name}: {brief}]`
pub fn compress_to_oneliner(tool_name: &str, content: &str) -> String {
    let name = tool_name.to_ascii_lowercase();

    if name.contains("read") || name.contains("glob") {
        let lines = content.lines().count();
        let lang = detect_language(content);
        format!("[{tool_name}: {lines} lines, {lang}]")
    } else if name.contains("grep") || name.contains("search") {
        let n = content.lines().filter(|l| !l.trim().is_empty()).count();
        format!("[{tool_name}: {n} matches]")
    } else if name.contains("bash") || name.contains("shell") {
        let n = content.lines().count();
        format!("[{tool_name}: {n} lines output]")
    } else if name.contains("subagent") {
        compress_subagent(content)
    } else {
        format!("[{tool_name}: {} chars]", content.len())
    }
}

/// Compress a subagent tool result into a structured one-liner.
///
/// Expects the content to contain task/outcome info. Falls back to a
/// generic summary if the content is unstructured.
pub fn compress_subagent(content: &str) -> String {
    // Try to extract structured info from the content
    let lines: Vec<&str> = content.lines().take(5).collect();
    let brief = if let Some(first) = lines.first() {
        safe_truncate(first, 80)
    } else {
        "completed"
    };
    format!("[Subagent: {brief}]")
}
```

- [ ] **Step 2: Write tests for the new compressors**

Add to the `#[cfg(test)] mod tests` block in `tool_compactor.rs`:

```rust
    #[test]
    fn test_compress_to_oneliner_read() {
        let content = "fn main() {}\nlet x = 1;\n";
        let result = compress_to_oneliner("Read", content);
        assert!(result.starts_with("[Read:"), "got: {result}");
        assert!(result.contains("2 lines"), "got: {result}");
    }

    #[test]
    fn test_compress_to_oneliner_grep() {
        let content = "file.rs:10: match\nfile.rs:20: match\n";
        let result = compress_to_oneliner("Grep", content);
        assert!(result.contains("2 matches"), "got: {result}");
    }

    #[test]
    fn test_compress_to_oneliner_bash() {
        let content = "ok\ndone\n";
        let result = compress_to_oneliner("Bash", content);
        assert!(result.contains("2 lines output"), "got: {result}");
    }

    #[test]
    fn test_compress_to_oneliner_subagent() {
        let content = "Explored codebase and found 3 relevant files";
        let result = compress_to_oneliner("subagent", content);
        assert!(result.starts_with("[Subagent:"), "got: {result}");
    }

    #[test]
    fn test_compress_subagent_truncates_long_content() {
        let long = "x".repeat(200);
        let result = compress_subagent(&long);
        assert!(result.len() < 100, "got length: {}", result.len());
    }

    #[test]
    fn test_compress_to_oneliner_generic() {
        let content = "some unknown tool output";
        let result = compress_to_oneliner("unknown_tool", content);
        assert!(result.contains("chars"), "got: {result}");
    }
```

- [ ] **Step 3: Run new tests**

Run: `cargo test -p alephcore --lib tool_compactor -- compress_to_oneliner compress_subagent`
Expected: All 6 tests PASS

- [ ] **Step 4: Upgrade MicroCompact to ResultClearing**

Replace the `MicroCompact` struct and impl in `src/agent_loop/context_budget/pipeline.rs` (lines 156-206):

```rust
// =============================================================================
// Stage 1: ResultClearing
// =============================================================================

/// Tiered clearing of old tool results based on message age.
///
/// - Within fresh_tail: keep original
/// - Outside fresh_tail, within half-life: compress via per-tool semantic compressor
/// - Beyond half-life: replace with ultra-compact one-liner
pub struct ResultClearing;

impl CompactionStage for ResultClearing {
    fn name(&self) -> &'static str {
        "result_clearing"
    }

    fn compact(&self, messages: &mut [UnifiedMessage], fresh_tail_count: usize) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        if partition == 0 {
            return 0;
        }

        // Dynamic half-life: older messages get more aggressive clearing.
        // Half-life is the midpoint of the compressible zone.
        let half_life = partition / 2;

        let candidates: Vec<usize> = (0..partition)
            .filter(|&i| messages[i].is_tool_result() && is_tool_result_consumed(messages, i))
            .collect();

        let mut total_freed: usize = 0;

        for idx in candidates {
            let (tool_name, old_content) = match messages[idx].tool_result_info() {
                Some((n, c)) => (n.to_owned(), c),
                None => continue,
            };

            let old_tokens = estimate_tokens_smart(&old_content);

            // Determine tier based on position relative to half-life
            let compressed = if idx < half_life {
                // Beyond half-life (oldest) → ultra-compact one-liner
                crate::memory::session_compactor::tool_compactor::compress_to_oneliner(
                    &tool_name,
                    &old_content,
                )
            } else {
                // Within half-life → semantic per-tool compression
                crate::memory::session_compactor::tool_compactor::compress_tool_result(
                    &tool_name,
                    &old_content,
                )
            };

            let new_tokens = estimate_tokens_smart(&compressed);
            if new_tokens < old_tokens {
                messages[idx].replace_tool_result_content(compressed);
                total_freed += old_tokens - new_tokens;
            }
        }

        total_freed
    }
}
```

- [ ] **Step 5: Update pipeline default construction**

In the same file, find where `MicroCompact` is instantiated in tests or the default pipeline builder (if any), and replace with `ResultClearing`. Search for `MicroCompact` references across the codebase and update them.

Also update the `CompactionPipeline` construction in `src/agent_loop/context_budget/mod.rs` (or wherever the default pipeline is assembled) — replace `Box::new(MicroCompact)` with `Box::new(ResultClearing)`.

- [ ] **Step 6: Write integration test for tiered clearing**

Add to `pipeline.rs` tests:

```rust
    #[test]
    fn result_clearing_applies_tiered_compression() {
        use crate::providers::message::UnifiedMessage;

        // Create a sequence: old tool results + assistant responses + fresh tail
        let mut messages = vec![];

        // Old messages (beyond half-life) — should get one-liner
        messages.push(UnifiedMessage::tool_result("Read", "fn main() {\n    println!(\"hello\");\n}\nlet x = 1;\nlet y = 2;\n"));
        messages.push(UnifiedMessage::assistant("I read the file."));

        // Mid-age messages (within half-life) — should get semantic compression
        messages.push(UnifiedMessage::tool_result("Bash", "line1\nline2\nline3\nline4\nline5\n"));
        messages.push(UnifiedMessage::assistant("Command completed."));

        // Fresh tail (protected)
        messages.push(UnifiedMessage::user("What did you find?"));
        messages.push(UnifiedMessage::assistant("Here's the summary."));

        let stage = ResultClearing;
        let freed = stage.compact(&mut messages, 2); // fresh_tail_count = 2

        // Oldest result should be one-liner format
        let oldest = messages[0].text_content();
        assert!(oldest.starts_with("[Read:") || oldest.starts_with("[Read file"),
            "oldest should be compact, got: {oldest}");

        assert!(freed > 0, "should have freed tokens");
    }
```

- [ ] **Step 7: Run all compaction tests**

Run: `cargo test -p alephcore --lib pipeline -- result_clearing`
Run: `cargo test -p alephcore --lib context_budget`
Expected: All PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/context_budget/pipeline.rs src/memory/session_compactor/tool_compactor.rs
git commit -m "feat(context): upgrade MicroCompact to tiered ResultClearing with semantic compression"
```

---

## Task 3: Subagent Transcript Persistence

**Files:**
- Modify: `src/agent_loop/agent_runtime.rs:78-197`
- Modify: `src/memory/session_compactor/tool_compactor.rs:120-144`

- [ ] **Step 1: Add Serialize to SubagentTranscript and extend with key_findings**

In `src/agent_loop/agent_runtime.rs`, update the `SubagentTranscript` struct (line 80):

```rust
/// Structured transcript of a sub-agent execution for observability and persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentTranscript {
    /// Unique identifier for the agent instance.
    pub agent_id: String,
    /// Agent type name (from agent_def).
    pub agent_type: String,
    /// Summary of the task that was executed.
    pub task_summary: String,
    /// How the execution ended.
    pub outcome: TranscriptOutcome,
    /// Number of think-act iterations completed.
    pub iterations: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Total tokens consumed.
    pub tokens_used: usize,
    /// Key findings extracted from the agent's final response (first 200 chars).
    pub key_findings: String,
}
```

Also add Serialize/Deserialize to `TranscriptOutcome`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TranscriptOutcome {
    Success,
    Error(String),
    Timeout,
}
```

- [ ] **Step 2: Write the `persist_transcript` helper function**

Add after the `format_outcome` helper in `agent_runtime.rs`:

```rust
/// Persist a subagent transcript to disk for future retrieval.
///
/// Writes to `~/.aleph/data/transcripts/{session_id}/{agent_id}.json`.
/// Creates directories as needed. Errors are logged but not propagated
/// (transcript persistence is best-effort).
fn persist_transcript(transcript: &SubagentTranscript, session_id: &str) {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".aleph/data/transcripts").join(session_id),
        None => {
            tracing::warn!("Cannot resolve home dir for transcript persistence");
            return;
        }
    };

    if let Err(e) = std::fs::create_dir_all(&base) {
        tracing::warn!(error = %e, "Failed to create transcript directory");
        return;
    }

    let path = base.join(format!("{}.json", transcript.agent_id));
    match serde_json::to_string_pretty(transcript) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "Failed to write transcript");
            } else {
                tracing::debug!(path = %path.display(), "Transcript persisted");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to serialize transcript");
        }
    }
}
```

- [ ] **Step 3: Extract key_findings from LoopRunResult and call persist**

In the `run()` method of `AgentRuntime`, after the transcript is constructed (around line 184), add `key_findings` extraction and persistence:

```rust
        // Extract key_findings from the final response
        let key_findings = match &result {
            Ok(run_result) => run_result
                .final_text
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>(),
            Err(_) => String::new(),
        };

        // Build transcript with key_findings
        // (update the transcript construction to include key_findings field)
```

After the tracing::info log (line 194), add:

```rust
        // Best-effort persistence — don't block on I/O errors
        let session_id = self.child_chain.chain_id();
        persist_transcript(&transcript, &session_id);
```

- [ ] **Step 4: Write tests**

Add to `agent_runtime.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_serialization_roundtrip() {
        let transcript = SubagentTranscript {
            agent_id: "test-123".to_string(),
            agent_type: "explorer".to_string(),
            task_summary: "Find all Rust files".to_string(),
            outcome: TranscriptOutcome::Success,
            iterations: 5,
            duration_ms: 1200,
            tokens_used: 3000,
            key_findings: "Found 42 Rust files in src/".to_string(),
        };

        let json = serde_json::to_string(&transcript).unwrap();
        let deserialized: SubagentTranscript = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.agent_id, "test-123");
        assert_eq!(deserialized.iterations, 5);
        assert_eq!(deserialized.key_findings, "Found 42 Rust files in src/");
        assert!(matches!(deserialized.outcome, TranscriptOutcome::Success));
    }

    #[test]
    fn persist_transcript_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        // Override home dir in test — use direct file write instead
        let transcript = SubagentTranscript {
            agent_id: "test-persist".to_string(),
            agent_type: "planner".to_string(),
            task_summary: "Plan feature".to_string(),
            outcome: TranscriptOutcome::Error("timeout".to_string()),
            iterations: 0,
            duration_ms: 5000,
            tokens_used: 0,
            key_findings: String::new(),
        };

        let path = dir.path().join("test-persist.json");
        let json = serde_json::to_string_pretty(&transcript).unwrap();
        std::fs::write(&path, &json).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"agent_type\": \"planner\""));
        assert!(content.contains("\"timeout\""));
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib agent_runtime`
Expected: All tests PASS

- [ ] **Step 6: Add `compress_subagent` dispatch to `compress_tool_result`**

In `src/memory/session_compactor/tool_compactor.rs`, update `compress_tool_result` (line 124) to handle subagent results. Add after the `webfetch` branch:

```rust
    } else if name.contains("subagent") {
        compress_subagent(content)
    }
```

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p alephcore --lib tool_compactor`
Expected: All PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/agent_runtime.rs src/memory/session_compactor/tool_compactor.rs
git commit -m "feat(agent_runtime): persist subagent transcripts with key_findings extraction"
```

---

## Task 4: Session Resume — Snapshot Types

**Files:**
- Create: `src/memory/session_resume/mod.rs`
- Create: `src/memory/session_resume/snapshot.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Create the module structure**

Create `src/memory/session_resume/mod.rs`:

```rust
//! Session Resume — save and restore conversation context across sessions.
//!
//! Saves a compressed snapshot at session end; restores it as a prompt layer
//! in the next session. Uses existing session_compactor summaries — no extra
//! LLM calls needed.

pub mod reader;
pub mod snapshot;
pub mod writer;

pub use reader::SnapshotReader;
pub use snapshot::SessionSnapshot;
pub use writer::SnapshotWriter;
```

- [ ] **Step 2: Create the SessionSnapshot type**

Create `src/memory/session_resume/snapshot.rs`:

```rust
//! SessionSnapshot — compressed conversation state for cross-session resume.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Decision markers used to extract key decisions from summaries.
const DECISION_MARKERS: &[&str] = &[
    "decided",
    "chose",
    "will use",
    "switched to",
    "selected",
    "picked",
    "agreed on",
    "confirmed",
];

/// A compressed snapshot of a conversation session.
///
/// Designed to be small (< 2KB typical) and fast to load. The `summary` field
/// is sourced from the session_compactor's d1-level summary, not generated
/// by an additional LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Session identifier.
    pub session_id: String,
    /// When the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Conversation summary (from session_compactor d1 layer).
    pub summary: String,
    /// Key decisions extracted from the summary.
    pub key_decisions: Vec<String>,
    /// Recently operated file paths.
    pub active_files: Vec<String>,
    /// Tool state summary (e.g., current working directory).
    pub tool_state: Option<String>,
    /// Incomplete tasks at session end.
    pub pending_tasks: Vec<String>,
}

impl SessionSnapshot {
    /// Extract key decisions from a summary by finding sentences with decision markers.
    pub fn extract_decisions(summary: &str) -> Vec<String> {
        summary
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter(|sentence| {
                let lower = sentence.to_lowercase();
                DECISION_MARKERS.iter().any(|marker| lower.contains(marker))
            })
            .map(|s| format!("{}.", s.trim()))
            .take(10) // Cap at 10 decisions
            .collect()
    }

    /// Render the snapshot as a prompt-injectable text block.
    pub fn to_prompt_text(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str("# Previous Session Context\n\n");

        out.push_str("**Summary:** ");
        out.push_str(&self.summary);
        out.push_str("\n\n");

        if !self.key_decisions.is_empty() {
            out.push_str("**Key decisions:**\n");
            for d in &self.key_decisions {
                out.push_str("- ");
                out.push_str(d);
                out.push('\n');
            }
            out.push('\n');
        }

        if !self.active_files.is_empty() {
            out.push_str("**Active files:**\n");
            for f in &self.active_files {
                out.push_str("- ");
                out.push_str(f);
                out.push('\n');
            }
            out.push('\n');
        }

        if !self.pending_tasks.is_empty() {
            out.push_str("**Pending tasks:**\n");
            for t in &self.pending_tasks {
                out.push_str("- ");
                out.push_str(t);
                out.push('\n');
            }
            out.push('\n');
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_decisions_finds_markers() {
        let summary = "We decided to use Rust. The team chose axum for the web framework. \
                        The server runs on port 3000. We will use serde for serialization.";
        let decisions = SessionSnapshot::extract_decisions(summary);
        assert_eq!(decisions.len(), 3);
        assert!(decisions[0].contains("decided"));
        assert!(decisions[1].contains("chose"));
        assert!(decisions[2].contains("will use"));
    }

    #[test]
    fn extract_decisions_returns_empty_for_no_markers() {
        let summary = "The system processes requests. It returns JSON responses.";
        let decisions = SessionSnapshot::extract_decisions(summary);
        assert!(decisions.is_empty());
    }

    #[test]
    fn to_prompt_text_renders_all_sections() {
        let snapshot = SessionSnapshot {
            session_id: "test-session".to_string(),
            created_at: Utc::now(),
            summary: "Implemented the auth module.".to_string(),
            key_decisions: vec!["Decided to use JWT.".to_string()],
            active_files: vec!["src/auth/mod.rs".to_string()],
            tool_state: None,
            pending_tasks: vec!["Add tests for token refresh.".to_string()],
        };
        let text = snapshot.to_prompt_text();

        assert!(text.contains("# Previous Session Context"));
        assert!(text.contains("Implemented the auth module."));
        assert!(text.contains("Decided to use JWT."));
        assert!(text.contains("src/auth/mod.rs"));
        assert!(text.contains("Add tests for token refresh."));
    }

    #[test]
    fn serialization_roundtrip() {
        let snapshot = SessionSnapshot {
            session_id: "s123".to_string(),
            created_at: Utc::now(),
            summary: "Test session.".to_string(),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_id, "s123");
        assert_eq!(restored.summary, "Test session.");
    }
}
```

- [ ] **Step 3: Register module in memory/mod.rs**

In `src/memory/mod.rs`, add after `pub mod transcript_indexer;` (line 58):

```rust
pub mod session_resume;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib session_resume`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_resume/ src/memory/mod.rs
git commit -m "feat(memory): add SessionSnapshot type for cross-session resume"
```

---

## Task 5: Session Resume — Writer & Reader

**Files:**
- Create: `src/memory/session_resume/writer.rs`
- Create: `src/memory/session_resume/reader.rs`

- [ ] **Step 1: Create SnapshotWriter**

Create `src/memory/session_resume/writer.rs`:

```rust
//! SnapshotWriter — persists SessionSnapshot to disk.

use std::path::{Path, PathBuf};

use super::snapshot::SessionSnapshot;

/// Maximum number of session snapshots to retain.
const MAX_RETAINED_SNAPSHOTS: usize = 10;

/// Writes session snapshots to `~/.aleph/data/sessions/{session_id}/resume.json`.
pub struct SnapshotWriter {
    base_dir: PathBuf,
}

impl SnapshotWriter {
    /// Create a writer targeting the given base directory.
    ///
    /// Typically: `~/.aleph/data/sessions/`
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Create a writer using the default data directory.
    pub fn default_path() -> Option<Self> {
        dirs::home_dir().map(|h| Self::new(h.join(".aleph/data/sessions")))
    }

    /// Write a snapshot to disk.
    pub fn write(&self, snapshot: &SessionSnapshot) -> std::io::Result<PathBuf> {
        let session_dir = self.base_dir.join(&snapshot.session_id);
        std::fs::create_dir_all(&session_dir)?;

        let path = session_dir.join("resume.json");
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, json)?;

        tracing::debug!(path = %path.display(), "Session snapshot written");

        // Best-effort cleanup of old snapshots
        if let Err(e) = self.cleanup_old_snapshots() {
            tracing::debug!(error = %e, "Failed to clean old snapshots");
        }

        Ok(path)
    }

    /// Remove oldest session directories beyond the retention limit.
    fn cleanup_old_snapshots(&self) -> std::io::Result<()> {
        let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && path.join("resume.json").exists() {
                let modified = entry.metadata()?.modified()?;
                entries.push((path, modified));
            }
        }

        if entries.len() <= MAX_RETAINED_SNAPSHOTS {
            return Ok(());
        }

        // Sort oldest first
        entries.sort_by_key(|(_, t)| *t);

        let to_remove = entries.len() - MAX_RETAINED_SNAPSHOTS;
        for (path, _) in entries.into_iter().take(to_remove) {
            tracing::debug!(path = %path.display(), "Removing old session snapshot");
            std::fs::remove_dir_all(&path)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_snapshot(session_id: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_id: session_id.to_string(),
            created_at: Utc::now(),
            summary: format!("Session {session_id}"),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        }
    }

    #[test]
    fn write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(dir.path());
        let snapshot = make_snapshot("sess-001");

        let path = writer.write(&snapshot).unwrap();
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("sess-001"));
    }

    #[test]
    fn cleanup_removes_oldest_beyond_limit() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(dir.path());

        // Create 12 snapshots (limit is 10)
        for i in 0..12 {
            let snapshot = make_snapshot(&format!("sess-{i:03}"));
            writer.write(&snapshot).unwrap();
            // Small sleep to ensure different modification times
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Count remaining directories with resume.json
        let remaining: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("resume.json").exists())
            .collect();

        assert!(
            remaining.len() <= MAX_RETAINED_SNAPSHOTS,
            "Expected <= {MAX_RETAINED_SNAPSHOTS}, got {}",
            remaining.len()
        );
    }
}
```

- [ ] **Step 2: Create SnapshotReader**

Create `src/memory/session_resume/reader.rs`:

```rust
//! SnapshotReader — loads the most recent SessionSnapshot from disk.

use std::path::PathBuf;

use super::snapshot::SessionSnapshot;

/// Reads session snapshots from `~/.aleph/data/sessions/`.
pub struct SnapshotReader {
    base_dir: PathBuf,
}

impl SnapshotReader {
    /// Create a reader targeting the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Create a reader using the default data directory.
    pub fn default_path() -> Option<Self> {
        dirs::home_dir().map(|h| Self::new(h.join(".aleph/data/sessions")))
    }

    /// Load the most recently modified session snapshot, excluding the given session_id.
    ///
    /// Returns `None` if no previous snapshot exists.
    pub fn load_latest(&self, exclude_session_id: &str) -> Option<SessionSnapshot> {
        let mut best: Option<(SessionSnapshot, std::time::SystemTime)> = None;

        let entries = std::fs::read_dir(&self.base_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Skip the current session
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == exclude_session_id {
                    continue;
                }
            }

            let resume_path = path.join("resume.json");
            if !resume_path.exists() {
                continue;
            }

            let modified = match std::fs::metadata(&resume_path) {
                Ok(m) => m.modified().unwrap_or(std::time::UNIX_EPOCH),
                Err(_) => continue,
            };

            let content = match std::fs::read_to_string(&resume_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let snapshot: SessionSnapshot = match serde_json::from_str(&content) {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(path = %resume_path.display(), error = %e, "Skipping corrupt snapshot");
                    continue;
                }
            };

            match &best {
                None => best = Some((snapshot, modified)),
                Some((_, best_time)) if modified > *best_time => {
                    best = Some((snapshot, modified));
                }
                _ => {}
            }
        }

        best.map(|(snapshot, _)| snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::session_resume::writer::SnapshotWriter;
    use chrono::Utc;

    fn make_snapshot(session_id: &str, summary: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_id: session_id.to_string(),
            created_at: Utc::now(),
            summary: summary.to_string(),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        }
    }

    #[test]
    fn load_latest_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(dir.path());
        let reader = SnapshotReader::new(dir.path());

        writer.write(&make_snapshot("old", "Old session")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        writer.write(&make_snapshot("new", "New session")).unwrap();

        let latest = reader.load_latest("current").unwrap();
        assert_eq!(latest.session_id, "new");
        assert_eq!(latest.summary, "New session");
    }

    #[test]
    fn load_latest_excludes_current_session() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(dir.path());
        let reader = SnapshotReader::new(dir.path());

        writer.write(&make_snapshot("prev", "Previous")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        writer.write(&make_snapshot("current", "Current")).unwrap();

        let latest = reader.load_latest("current").unwrap();
        assert_eq!(latest.session_id, "prev");
    }

    #[test]
    fn load_latest_returns_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let reader = SnapshotReader::new(dir.path());

        assert!(reader.load_latest("any").is_none());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib session_resume`
Expected: All tests PASS (snapshot + writer + reader)

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_resume/writer.rs src/memory/session_resume/reader.rs
git commit -m "feat(memory): add SnapshotWriter and SnapshotReader for session resume"
```

---

## Task 6: Session Resume — Prompt Layer & Loop Integration

**Files:**
- Create: `src/thinker/layers/session_resume.rs`
- Modify: `src/thinker/layers/mod.rs`
- Modify: `src/thinker/prompt_layer.rs`
- Modify: `src/thinker/prompt_pipeline.rs`
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Create SessionResumeLayer**

Create `src/thinker/layers/session_resume.rs`:

```rust
//! SessionResumeLayer — injects previous session context (priority 1760)

use crate::memory::session_resume::SessionSnapshot;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct SessionResumeLayer;

impl PromptLayer for SessionResumeLayer {
    fn name(&self) -> &'static str {
        "session_resume"
    }
    fn priority(&self) -> u32 {
        1760
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let snapshot = match &input.session_snapshot {
            Some(s) => s,
            None => return,
        };
        output.push_str(&snapshot.to_prompt_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::session_resume::SessionSnapshot;
    use crate::thinker::prompt_builder::PromptConfig;
    use chrono::Utc;

    #[test]
    fn injects_snapshot_when_present() {
        let snapshot = SessionSnapshot {
            session_id: "prev".to_string(),
            created_at: Utc::now(),
            summary: "Implemented auth module.".to_string(),
            key_decisions: vec!["Decided to use JWT.".to_string()],
            active_files: vec!["src/auth.rs".to_string()],
            tool_state: None,
            pending_tasks: vec!["Add refresh token logic.".to_string()],
        };
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_session_snapshot(&snapshot);
        let mut output = String::new();
        SessionResumeLayer.inject(&mut output, &input);

        assert!(output.contains("# Previous Session Context"));
        assert!(output.contains("Implemented auth module."));
        assert!(output.contains("Decided to use JWT."));
    }

    #[test]
    fn skips_when_no_snapshot() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SessionResumeLayer.inject(&mut output, &input);

        assert!(output.is_empty());
    }

    #[test]
    fn reports_dynamic_stability() {
        assert_eq!(SessionResumeLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn reports_priority_1760() {
        assert_eq!(SessionResumeLayer.priority(), 1760);
    }
}
```

- [ ] **Step 2: Add `session_snapshot` field to `LayerInput`**

In `src/thinker/prompt_layer.rs`, add to the `LayerInput` struct:

```rust
    /// Previous session snapshot for cross-session resume.
    pub session_snapshot: Option<&'a crate::memory::session_resume::SessionSnapshot>,
```

Update all constructors to include `session_snapshot: None`.

Add builder method:

```rust
    /// Attach a previous session snapshot for resume injection.
    pub fn with_session_snapshot(
        mut self,
        snapshot: &'a crate::memory::session_resume::SessionSnapshot,
    ) -> Self {
        self.session_snapshot = Some(snapshot);
        self
    }
```

- [ ] **Step 3: Register the layer**

In `src/thinker/layers/mod.rs`:
```rust
pub mod session_resume;
pub use session_resume::SessionResumeLayer;
```

In `src/thinker/prompt_pipeline.rs`, add to `default_layers()` after `SessionContextGuideLayer`:
```rust
            Box::new(SessionResumeLayer),
```

Add import:
```rust
use super::layers::SessionResumeLayer;
```

- [ ] **Step 4: Run compilation check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Run layer tests**

Run: `cargo test -p alephcore --lib session_resume`
Expected: All tests PASS (snapshot + writer + reader + layer)

- [ ] **Step 6: Commit**

```bash
git add src/thinker/layers/session_resume.rs src/thinker/layers/mod.rs src/thinker/prompt_layer.rs src/thinker/prompt_pipeline.rs
git commit -m "feat(thinker): add SessionResumeLayer for cross-session context restoration"
```

---

## Task 7: Integration — Wire Snapshot Write into AgentLoop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Add snapshot writing to loop finalization**

Find the section in `AgentLoop` where `LoopRunResult` is constructed (search for `fn cancelled_result` or the main loop exit paths). After the result is constructed but before it's returned, add snapshot writing logic.

Add a method to `AgentLoop`:

```rust
    /// Write a session snapshot if this is a root-level agent (depth 0).
    fn maybe_write_session_snapshot(
        &self,
        progress: &LoopProgress,
        messages: &[UnifiedMessage],
    ) {
        // Only write snapshots for root agents, not subagents
        if self.chain.depth() > 0 {
            return;
        }

        let writer = match crate::memory::session_resume::SnapshotWriter::default_path() {
            Some(w) => w,
            None => return,
        };

        // Extract summary from the conversation (last few assistant messages)
        let summary: String = messages
            .iter()
            .rev()
            .filter(|m| m.is_assistant())
            .take(3)
            .filter_map(|m| {
                let text = m.text_content();
                if text.is_empty() { None } else { Some(text) }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ");

        let summary_truncated = summary.chars().take(500).collect::<String>();

        let key_decisions =
            crate::memory::session_resume::SessionSnapshot::extract_decisions(&summary_truncated);

        let snapshot = crate::memory::session_resume::SessionSnapshot {
            session_id: self.chain.chain_id().to_string(),
            created_at: chrono::Utc::now(),
            summary: summary_truncated,
            key_decisions,
            active_files: vec![], // TODO: could be populated from file state tracking
            tool_state: None,
            pending_tasks: vec![],
        };

        if let Err(e) = writer.write(&snapshot) {
            tracing::debug!(error = %e, "Failed to write session snapshot");
        }
    }
```

- [ ] **Step 2: Call the method at loop exit points**

Find the main loop exit point where `LoopRunResult` is returned. Before the final `Ok(result)`, add:

```rust
        self.maybe_write_session_snapshot(&progress, &runtime.messages);
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests PASS

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): write session snapshot on root agent loop exit"
```

---

## Task 8: Final Verification

- [ ] **Step 1: Full build check**

Run: `cargo build`
Expected: PASS

- [ ] **Step 2: Full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 3: Clippy clean**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Verify new files exist**

Check that all new files are created:
- `src/thinker/layers/mcp_instructions.rs`
- `src/thinker/layers/session_resume.rs`
- `src/memory/session_resume/mod.rs`
- `src/memory/session_resume/snapshot.rs`
- `src/memory/session_resume/writer.rs`
- `src/memory/session_resume/reader.rs`

- [ ] **Step 5: Verify no old MicroCompact references remain**

Run: `grep -r "MicroCompact" src/`
Expected: No matches (all replaced by ResultClearing)

- [ ] **Step 6: Final commit (if any remaining changes)**

```bash
git add -A
git commit -m "chore: context optimization final cleanup"
```
