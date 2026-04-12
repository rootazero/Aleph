# Context Window Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close 5 context-window management gaps (G5 → G1 → G2 → G3 → G4) identified by comparing Aleph with Claude Code's architecture.

**Architecture:** Incremental enhancement — each module plugs into existing plugin interfaces (CompactionStrategy, ConstraintSource, PostCompactCleanup). No architectural changes needed. New files are self-contained; modifications to existing files are minimal (~15 lines each).

**Tech Stack:** Rust, tokio (async for G4), tracing (logging for G3), std::fs (disk I/O for G1)

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `src/agent_loop/compaction/summary_utils.rs` | Shared `strip_analysis_block()` + `IDENTIFIER_PRESERVATION` constant |
| `src/agent_loop/tool_result_store.rs` | Disk persistence for large tool results |
| `src/agent_loop/compaction/file_content_tracker.rs` | LRU tracker for recently read files, implements `ConstraintSource` |
| `src/thinker/prompt_builder/cache_monitor.rs` | Prompt cache hit/miss monitoring |
| `src/agent_loop/compaction/session_summary_source.rs` | Zero-cost compaction via existing session summaries |

### Modified Files

| File | What Changes |
|------|-------------|
| `src/agent_loop/compaction/mod.rs` | Add `pub mod` for 3 new submodules + re-exports |
| `src/agent_loop/mod.rs` | Add `pub mod tool_result_store` |
| `src/thinker/prompt_builder/mod.rs` | Add `pub mod cache_monitor` |
| `src/memory/session_compactor/summary_engine.rs` | LEAF_PROMPT upgrade + delegate to summary_utils |
| `src/agent_loop/context_compactor.rs` | Prompt upgrade + `SessionMemoryReuse` variant + fast path |
| `src/agent_loop/tool_pipeline.rs` | `result_store` + `file_tracker` fields |
| `src/agent_loop/compaction/constraint_injector.rs` | `RecentFile` category |
| `src/agent_loop/compaction/micro_compactor.rs` | Preserve disk refs in placeholders |
| `src/agent_loop/compaction/orchestrator.rs` | Notify cache monitor after execution |
| `src/thinker/prompt_builder/cache.rs` | Update stable hash |

---

## Task 1: G5 — Extract shared summary utilities

**Files:**
- Create: `src/agent_loop/compaction/summary_utils.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Modify: `src/memory/session_compactor/summary_engine.rs`

- [ ] **Step 1: Create `summary_utils.rs` with shared constants and functions**

```rust
// src/agent_loop/compaction/summary_utils.rs

//! Shared utilities for context compression summary generation.
//!
//! Houses the `strip_analysis_block()` function and `IDENTIFIER_PRESERVATION`
//! constant used by both `context_compactor` and `summary_engine`.

/// Mandatory instruction appended to all summary prompts to preserve identifiers.
pub const IDENTIFIER_PRESERVATION: &str = "\n\n\
## Identifier Preservation (MANDATORY)\n\
When summarizing, you MUST preserve the following identifiers EXACTLY as they appear \
in the original text — do not shorten, paraphrase, or reconstruct them:\n\
- File paths (e.g., src/memory/store/lance/mod.rs)\n\
- UUIDs and hashes (e.g., a1b2c3d4-...)\n\
- URLs and endpoints (e.g., https://api.example.com/v1/...)\n\
- Commit references (e.g., 0949c9fc)\n\
- Version numbers (e.g., v2026.04.02)\n\
- Configuration keys and environment variables\n\
- Error codes and status codes\n\
\n\
If an identifier is not relevant to the summary's core meaning, omit it entirely \
rather than abbreviating it.";

/// Strip the `<analysis>...</analysis>` scratchpad from LLM summary output.
///
/// The analysis block gives the LLM reasoning space but should not enter
/// the context window. If no analysis block is found, returns input unchanged.
pub fn strip_analysis_block(text: &str) -> String {
    if let Some(start) = text.find("<analysis>") {
        if let Some(end) = text.find("</analysis>") {
            let after_end = end + "</analysis>".len();
            let mut result = String::new();
            result.push_str(text[..start].trim());
            if after_end < text.len() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(text[after_end..].trim());
            }
            return result;
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_analysis_removes_block() {
        let input = "Preamble\n<analysis>\nReasoning\n</analysis>\n<summary>\nResult\n</summary>";
        let stripped = strip_analysis_block(input);
        assert!(!stripped.contains("<analysis>"));
        assert!(!stripped.contains("Reasoning"));
        assert!(stripped.contains("Result"));
    }

    #[test]
    fn strip_analysis_no_block_returns_unchanged() {
        let input = "Just a plain summary.";
        assert_eq!(strip_analysis_block(input), input);
    }

    #[test]
    fn identifier_preservation_contains_mandatory() {
        assert!(IDENTIFIER_PRESERVATION.contains("MANDATORY"));
        assert!(IDENTIFIER_PRESERVATION.contains("File paths"));
    }
}
```

- [ ] **Step 2: Export the new module from `compaction/mod.rs`**

Add after line 5 (`pub mod types;`):

```rust
pub mod summary_utils;
```

Add to the re-exports at the bottom:

```rust
pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
```

- [ ] **Step 3: Update `summary_engine.rs` to use shared utilities**

Replace the local `IDENTIFIER_PRESERVATION` constant (lines 58-71) and `strip_analysis_block` function (lines 86-102) with re-exports:

```rust
// At the top of summary_engine.rs, add:
use crate::agent_loop::compaction::summary_utils::{
    strip_analysis_block, IDENTIFIER_PRESERVATION,
};
```

Remove the local `const IDENTIFIER_PRESERVATION` (lines 58-71) and the local `pub fn strip_analysis_block` (lines 82-102). Add a re-export so downstream code using `summary_engine::strip_analysis_block` still compiles:

```rust
// Re-export for backwards compatibility
pub use crate::agent_loop::compaction::summary_utils::strip_analysis_block;
```

- [ ] **Step 4: Run tests to verify no regressions**

Run: `cargo test -p alephcore --lib -- summary_engine`

Expected: All existing tests pass (they test `strip_analysis_block` and `IDENTIFIER_PRESERVATION` via the same API).

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/summary_utils.rs src/agent_loop/compaction/mod.rs src/memory/session_compactor/summary_engine.rs
git commit -m "refactor(compaction): extract shared summary_utils for strip_analysis_block and IDENTIFIER_PRESERVATION"
```

---

## Task 2: G5 — Upgrade LEAF_PROMPT to structured sections

**Files:**
- Modify: `src/memory/session_compactor/summary_engine.rs`

- [ ] **Step 1: Replace LEAF_PROMPT with structured sections**

Replace the `LEAF_PROMPT` constant (lines 14-38) with:

```rust
const LEAF_PROMPT: &str = "\
You are a conversation compressor. Condense the following conversation into a structured summary.\n\
\n\
First, analyze the conversation in an <analysis> block (this will be stripped before the summary enters context):\n\
\n\
<analysis>\n\
1. User's primary request and current intent\n\
2. Key technical decisions made and their rationale\n\
3. Files and code sections involved (preserve exact paths)\n\
4. Errors encountered and how they were resolved\n\
5. What is still pending or unresolved\n\
</analysis>\n\
\n\
Then produce the final summary in a <summary> block with these MANDATORY sections:\n\
\n\
<summary>\n\
## Primary Request\n\
[User's core goal in 1-2 sentences]\n\
\n\
## Key Decisions\n\
[Decisions made and why, most recent first]\n\
\n\
## Files & Code\n\
[Exact file paths and what was done to each]\n\
\n\
## Current State\n\
[What was just completed, what's in progress]\n\
\n\
## Pending\n\
[Unresolved problems, next steps]\n\
</summary>\n\
\n\
Omit: greetings, filler, repeated information, verbose tool outputs already summarized.";
```

- [ ] **Step 2: Update test assertions**

The test `test_build_prompt_leaf_contains_leaf_instruction` (line 258) asserts `prompt.contains("File operations")`. Update to match the new section name:

```rust
#[test]
fn test_build_prompt_leaf_contains_leaf_instruction() {
    let messages = msgs(&[("user", "Hello"), ("assistant", "Hi there")]);
    let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
    assert!(
        prompt.contains("## Files & Code"),
        "leaf prompt should contain Files & Code section"
    );
    assert!(
        !prompt.contains("milestone summary"),
        "leaf prompt should not mention milestone summary"
    );
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- summary_engine`

Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_compactor/summary_engine.rs
git commit -m "feat(compaction): upgrade LEAF_PROMPT to 5 mandatory structured sections"
```

---

## Task 3: G5 — Upgrade context_compactor prompt

**Files:**
- Modify: `src/agent_loop/context_compactor.rs`

- [ ] **Step 1: Import shared utilities**

Add at the top of `context_compactor.rs`:

```rust
use crate::agent_loop::compaction::summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
```

- [ ] **Step 2: Replace the prompt template**

Replace lines 138-144 (the `format!` block in `compact()`) with:

```rust
        let prompt = format!(
            "You are a conversation compressor. Condense the transcript below.\n\
             \n\
             First, analyze in an <analysis> block (will be stripped):\n\
             \n\
             <analysis>\n\
             1. Primary user request and current intent\n\
             2. Key decisions made and their rationale\n\
             3. Files/paths involved (preserve exact paths)\n\
             4. Errors encountered and resolutions\n\
             5. What is still pending or unresolved\n\
             </analysis>\n\
             \n\
             Then produce the summary in a <summary> block with these MANDATORY sections:\n\
             \n\
             <summary>\n\
             ## Primary Request\n\
             [User's core goal in 1-2 sentences]\n\
             \n\
             ## Key Decisions\n\
             [Decisions made and why, most recent first]\n\
             \n\
             ## Files & Code\n\
             [Exact file paths and what was done to each]\n\
             \n\
             ## Current State\n\
             [What was just completed, what's in progress]\n\
             \n\
             ## Pending\n\
             [Unresolved problems, next steps]\n\
             </summary>\n\
             {}\n\
             \n\
             Target: ~{} tokens. Omit greetings, filler, redundant confirmations.\n\
             \n\
             ---TRANSCRIPT---\n{}\n---END---",
            IDENTIFIER_PRESERVATION, token_budget, transcript
        );
```

- [ ] **Step 3: Apply strip_analysis_block to LLM output**

In the success branch (line 150), wrap the summary:

```rust
            Ok(Ok(summary)) if !summary.trim().is_empty() => {
                let summary = strip_analysis_block(&summary);
                // ... rest unchanged
```

- [ ] **Step 4: Update the system prompt in call_llm**

Replace line 198:

```rust
        let system =
            "You are a precise conversation summarizer. Output the analysis block followed by the summary block. No other text.";
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- context_compactor`

Expected: All 4 tests pass. The mock provider returns fixed text so prompt changes don't affect test outcomes.

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/context_compactor.rs
git commit -m "feat(compaction): upgrade context_compactor prompt with structured sections and analysis scratchpad"
```

---

## Task 4: G1 — Create ToolResultStore

**Files:**
- Create: `src/agent_loop/tool_result_store.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Create `tool_result_store.rs`**

```rust
// src/agent_loop/tool_result_store.rs

//! Disk persistence for large tool results.
//!
//! When a tool result exceeds a token threshold, the full content is written
//! to disk before truncation. The context retains a reference marker so the
//! LLM knows the full output is available.

use std::path::{Path, PathBuf};

use crate::agent_loop::context_budget::pressure::estimate_tokens_smart;

/// Marker prefix used to identify persisted-output references in tool results.
const PERSISTED_REF_PREFIX: &str = "[Full output persisted: ";

/// Disk-backed store for large tool results.
///
/// Files are stored as plain text at `{base_dir}/{tool_call_id}.txt`.
pub struct ToolResultStore {
    base_dir: PathBuf,
}

impl ToolResultStore {
    /// Create a new store for the given session.
    ///
    /// Creates the directory `~/.aleph/data/tool_results/{session_id}/` if it
    /// does not exist.
    pub fn new(session_id: &str) -> std::io::Result<Self> {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("data")
            .join("tool_results")
            .join(session_id);
        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Persist the content to disk if it exceeds `threshold_tokens`.
    ///
    /// Returns a reference marker string on success, or `None` if the content
    /// is small enough to keep inline or if the write fails.
    pub fn persist_if_large(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
        threshold_tokens: usize,
    ) -> Option<String> {
        let estimated = estimate_tokens_smart(content);
        if estimated <= threshold_tokens {
            return None;
        }

        let file_path = self.base_dir.join(format!("{}.txt", tool_call_id));
        if std::fs::write(&file_path, content).is_err() {
            tracing::warn!(
                tool_call_id,
                tool_name,
                "Failed to persist large tool result to disk"
            );
            return None;
        }

        tracing::debug!(
            tool_call_id,
            tool_name,
            tokens = estimated,
            path = %file_path.display(),
            "Persisted large tool result to disk"
        );

        Some(format!(
            "{}{} ({} tokens, {})]",
            PERSISTED_REF_PREFIX,
            file_path.display(),
            estimated,
            tool_name,
        ))
    }

    /// Remove all persisted files for this session.
    pub fn cleanup(&self) {
        if self.base_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.base_dir) {
                tracing::warn!(
                    path = %self.base_dir.display(),
                    error = %e,
                    "Failed to cleanup tool result store"
                );
            }
        }
    }

    /// Return the base directory path (for testing).
    #[cfg(test)]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

impl Drop for ToolResultStore {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Extract a `[Full output persisted: ...]` reference line from text.
///
/// Used by `MicroCompactor` to preserve disk references when replacing
/// tool results with compact placeholders.
pub fn extract_persisted_ref(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with(PERSISTED_REF_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_store() -> ToolResultStore {
        let dir = std::env::temp_dir()
            .join("aleph_test_tool_results")
            .join(format!("test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        ToolResultStore { base_dir: dir }
    }

    #[test]
    fn small_result_not_persisted() {
        let store = test_store();
        let result = store.persist_if_large("call_1", "read_file", "short content", 8000);
        assert!(result.is_none());
    }

    #[test]
    fn large_result_persisted_and_recoverable() {
        let store = test_store();
        let large = "x".repeat(100_000); // well above any threshold
        let marker = store
            .persist_if_large("call_2", "bash", &large, 100)
            .expect("should persist");

        assert!(marker.starts_with(PERSISTED_REF_PREFIX));
        assert!(marker.contains("bash"));

        // File should exist and contain the original content
        let file_path = store.base_dir.join("call_2.txt");
        let on_disk = fs::read_to_string(&file_path).unwrap();
        assert_eq!(on_disk, large);
    }

    #[test]
    fn cleanup_removes_directory() {
        let store = test_store();
        let large = "y".repeat(100_000);
        store.persist_if_large("call_3", "tool", &large, 100);
        assert!(store.base_dir.exists());

        store.cleanup();
        assert!(!store.base_dir.exists());
    }

    #[test]
    fn extract_persisted_ref_finds_marker() {
        let text = "Some output\n[Full output persisted: /tmp/foo.txt (5000 tokens, bash)]\nMore text";
        let found = extract_persisted_ref(text).unwrap();
        assert!(found.starts_with(PERSISTED_REF_PREFIX));
    }

    #[test]
    fn extract_persisted_ref_returns_none_when_absent() {
        assert!(extract_persisted_ref("no marker here").is_none());
    }
}
```

- [ ] **Step 2: Export from `agent_loop/mod.rs`**

Add after line 12 (`pub mod context_compactor;`):

```rust
pub mod tool_result_store;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- tool_result_store`

Expected: All 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/tool_result_store.rs src/agent_loop/mod.rs
git commit -m "feat(compaction): add ToolResultStore for disk persistence of large tool results"
```

---

## Task 5: G1 — Integrate ToolResultStore into ToolPipeline

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Add imports and field**

Add import at the top:

```rust
use crate::agent_loop::tool_result_store::ToolResultStore;
```

Add field to `ToolPipeline` struct (after `working_dir`):

```rust
    result_store: Option<ToolResultStore>,
```

- [ ] **Step 2: Update constructor**

Add `result_store: None` to the `Self` block in `ToolPipeline::new()`.

Add a builder method:

```rust
    /// Attach a tool result store for disk persistence of large outputs.
    pub fn with_result_store(mut self, store: ToolResultStore) -> Self {
        self.result_store = Some(store);
        self
    }
```

- [ ] **Step 3: Modify `map_result` to accept a store reference**

Change the `map_result` signature from:

```rust
    fn map_result(id: &str, name: &str, result: &ToolResult) -> ToolOutcome {
```

to:

```rust
    fn map_result(id: &str, name: &str, result: &ToolResult, store: Option<&ToolResultStore>) -> ToolOutcome {
```

In the `Success` and `SuccessAndStopLoop` branches, between `compress_tool_output` and `truncate_tool_result`, insert:

```rust
                let disk_ref = store
                    .and_then(|s| s.persist_if_large(id, name, &compressed, MAX_TOOL_RESULT_TOKENS));
                let mut final_text = truncate_tool_result(&compressed);
                if let Some(ref_marker) = disk_ref {
                    final_text.push('\n');
                    final_text.push_str(&ref_marker);
                }
```

(Replace the existing `let final_text = truncate_tool_result(&compressed);` line.)

- [ ] **Step 4: Update all `map_result` call sites**

Find the call to `Self::map_result(id, name, &result)` in the `run()` method and change to:

```rust
Self::map_result(id, name, &result, self.result_store.as_ref())
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- tool_pipeline`

Expected: All existing tests pass (they don't construct a `ToolResultStore`, so `store` is `None` and behavior is unchanged).

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "feat(compaction): integrate ToolResultStore into ToolPipeline for large result persistence"
```

---

## Task 6: G1 — Preserve disk refs in MicroCompactor placeholders

**Files:**
- Modify: `src/agent_loop/compaction/micro_compactor.rs`

- [ ] **Step 1: Import extract_persisted_ref**

Add at the top:

```rust
use crate::agent_loop::tool_result_store::extract_persisted_ref;
```

- [ ] **Step 2: Modify `format_compact_placeholder` to accept original content**

Change the signature to add an `original_content` parameter:

```rust
pub fn format_compact_placeholder(
    tool_name: &str,
    original_tokens: usize,
    key_fields: Option<&[&str]>,
    success: bool,
    original_content: Option<&str>,
) -> String {
```

At the end, before the `lines.join("\n")`, add:

```rust
    // Preserve disk reference if present in original content
    if let Some(content) = original_content {
        if let Some(ref_line) = extract_persisted_ref(content) {
            lines.push(ref_line.to_string());
        }
    }
```

- [ ] **Step 3: Update the call site in `execute()`**

In the `execute` method (around line 302), change:

```rust
                let placeholder =
                    format_compact_placeholder(&entry.tool_name, original_tokens, None, true);
```

to:

```rust
                let placeholder = format_compact_placeholder(
                    &entry.tool_name,
                    original_tokens,
                    None,
                    true,
                    Some(&original_content),
                );
```

- [ ] **Step 4: Update existing tests**

The test `compact_placeholder_format` (line 403) calls `format_compact_placeholder` with 4 args. Add `None` as the 5th:

```rust
        let placeholder = format_compact_placeholder(
            "read_file",
            2500,
            Some(&["path", "content", "encoding"]),
            true,
            None,
        );
```

- [ ] **Step 5: Add a test for disk ref preservation**

```rust
    #[test]
    fn compact_placeholder_preserves_disk_ref() {
        let original = "lots of data\n[Full output persisted: /tmp/call_1.txt (5000 tokens, bash)]";
        let placeholder = format_compact_placeholder("bash", 5000, None, true, Some(original));
        assert!(
            placeholder.contains("[Full output persisted:"),
            "placeholder should preserve disk ref: {placeholder}"
        );
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib -- micro_compactor`

Expected: All tests pass including the new one.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/compaction/micro_compactor.rs
git commit -m "feat(compaction): preserve disk references in MicroCompactor placeholders"
```

---

## Task 7: G2 — Create FileContentTracker

**Files:**
- Create: `src/agent_loop/compaction/file_content_tracker.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Modify: `src/agent_loop/compaction/constraint_injector.rs`

- [ ] **Step 1: Add `RecentFile` to `ConstraintCategory`**

In `constraint_injector.rs`, add the new variant (after `UserPreference`):

```rust
    /// Recently read file content for post-compaction recovery.
    RecentFile,
```

- [ ] **Step 2: Add the `RecentFile` section to `format_injection()`**

In `format_injection()`, after the `pref_items` block (around line 113), add:

```rust
        let file_items: Vec<&str> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::RecentFile)
            .map(|c| c.content.as_str())
            .collect();
```

And after the `Key Preferences` section (around line 137), before the closing format:

```rust
        if !file_items.is_empty() {
            body.push_str("\n### Recently Read Files\n");
            for item in &file_items {
                body.push_str(item);
                body.push('\n');
            }
        }
```

- [ ] **Step 3: Create `file_content_tracker.rs`**

```rust
// src/agent_loop/compaction/file_content_tracker.rs

//! Post-compaction file content recovery via LRU tracking.
//!
//! Records the most recent file reads so their content can be restored
//! after compaction via the [`ConstraintInjector`] plugin mechanism.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::constraint_injector::{Constraint, ConstraintCategory, ConstraintSource};

/// Maximum number of recent file reads to track.
const MAX_TRACKED_FILES: usize = 5;

/// Maximum content preview per file in characters (~1.4K tokens).
const MAX_PREVIEW_CHARS: usize = 5000;

struct FileReadRecord {
    path: String,
    preview: String,
    line_count: usize,
}

/// Tracks recently read files for post-compaction context restoration.
///
/// Implements [`ConstraintSource`] — register with [`ConstraintInjector`]
/// to automatically inject file content after every compaction pass.
pub struct FileContentTracker {
    recent_reads: Mutex<VecDeque<FileReadRecord>>,
}

impl FileContentTracker {
    /// Create a new tracker with an empty history.
    pub fn new() -> Self {
        Self {
            recent_reads: Mutex::new(VecDeque::with_capacity(MAX_TRACKED_FILES)),
        }
    }

    /// Record a file read.
    ///
    /// Deduplicates by path — a newer read of the same path replaces the
    /// older entry. Evicts the oldest record when capacity is exceeded.
    pub fn record_read(&self, path: &str, content: &str) {
        let preview = truncate_preview(content, MAX_PREVIEW_CHARS);
        let line_count = content.lines().count();

        let mut reads = self.recent_reads.lock().unwrap_or_else(|e| e.into_inner());

        // Deduplicate: remove existing entry for same path
        reads.retain(|r| r.path != path);

        reads.push_back(FileReadRecord {
            path: path.to_string(),
            preview,
            line_count,
        });

        // Evict oldest if over capacity
        while reads.len() > MAX_TRACKED_FILES {
            reads.pop_front();
        }
    }

    /// Return the number of tracked files (for testing).
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.recent_reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

impl ConstraintSource for FileContentTracker {
    fn collect_constraints(&self) -> Vec<Constraint> {
        let reads = self.recent_reads.lock().unwrap_or_else(|e| e.into_inner());
        reads
            .iter()
            .map(|r| Constraint {
                category: ConstraintCategory::RecentFile,
                content: format!(
                    "**{}** ({} lines)\n```\n{}\n```",
                    r.path, r.line_count, r.preview
                ),
                priority: 60,
            })
            .collect()
    }
}

/// Truncate content to at most `max_chars`, cutting at a newline boundary.
fn truncate_preview(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    // Find the last newline before the limit for a clean cut
    let cut = content[..max_chars]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(max_chars);
    let truncated = content.get(..cut).unwrap_or(&content[..max_chars]);
    format!("{}...[truncated]", truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_collect() {
        let tracker = FileContentTracker::new();
        tracker.record_read("src/main.rs", "fn main() {}\n");
        let constraints = tracker.collect_constraints();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].category, ConstraintCategory::RecentFile);
        assert!(constraints[0].content.contains("src/main.rs"));
        assert!(constraints[0].content.contains("fn main()"));
    }

    #[test]
    fn deduplicates_by_path() {
        let tracker = FileContentTracker::new();
        tracker.record_read("src/lib.rs", "old content");
        tracker.record_read("src/lib.rs", "new content");
        assert_eq!(tracker.count(), 1);
        let constraints = tracker.collect_constraints();
        assert!(constraints[0].content.contains("new content"));
    }

    #[test]
    fn evicts_oldest_beyond_capacity() {
        let tracker = FileContentTracker::new();
        for i in 0..7 {
            tracker.record_read(&format!("file_{i}.rs"), &format!("content {i}"));
        }
        assert_eq!(tracker.count(), MAX_TRACKED_FILES);
        // Oldest files (0, 1) should be evicted
        let constraints = tracker.collect_constraints();
        assert!(!constraints.iter().any(|c| c.content.contains("file_0")));
        assert!(constraints.iter().any(|c| c.content.contains("file_6")));
    }

    #[test]
    fn truncates_large_preview() {
        let tracker = FileContentTracker::new();
        let large = "line\n".repeat(2000); // ~10K chars
        tracker.record_read("big.rs", &large);
        let constraints = tracker.collect_constraints();
        // Preview should be capped
        assert!(constraints[0].content.len() < large.len());
        assert!(constraints[0].content.contains("truncated"));
    }
}
```

- [ ] **Step 4: Export from `compaction/mod.rs`**

Add:

```rust
pub mod file_content_tracker;
```

And re-export:

```rust
pub use file_content_tracker::FileContentTracker;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- file_content_tracker constraint_injector`

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/compaction/file_content_tracker.rs src/agent_loop/compaction/constraint_injector.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add FileContentTracker for post-compaction file recovery"
```

---

## Task 8: G2 — Integrate FileContentTracker into ToolPipeline

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Add imports and field**

Add import:

```rust
use crate::agent_loop::compaction::file_content_tracker::FileContentTracker;
use std::sync::Arc;
```

Add field to `ToolPipeline` struct (after `result_store`):

```rust
    file_tracker: Option<Arc<FileContentTracker>>,
```

- [ ] **Step 2: Update constructor and add builder method**

Add `file_tracker: None` to `Self` in `new()`.

Add builder method:

```rust
    /// Attach a file content tracker for post-compaction recovery.
    pub fn with_file_tracker(mut self, tracker: Arc<FileContentTracker>) -> Self {
        self.file_tracker = Some(tracker);
        self
    }
```

- [ ] **Step 3: Record file reads after tool execution**

In the `run()` method, after `map_result` is called and before constructing the `PipelineOutcome`, add:

```rust
        // Record file reads for post-compaction recovery
        if let Some(tracker) = &self.file_tracker {
            if is_file_read_tool(name) && !outcome.is_error {
                if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                    tracker.record_read(path, &outcome.output_text);
                }
            }
        }
```

Add the helper function at the bottom of the file (in the helpers section):

```rust
/// Check if a tool name corresponds to a file read operation.
fn is_file_read_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("read_file") || lower.contains("file_read") || lower == "read"
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- tool_pipeline`

Expected: All existing tests pass (file_tracker is `None`).

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "feat(compaction): integrate FileContentTracker into ToolPipeline"
```

---

## Task 9: G3 — Create CacheMonitor

**Files:**
- Create: `src/thinker/prompt_builder/cache_monitor.rs`
- Modify: `src/thinker/prompt_builder/mod.rs`

- [ ] **Step 1: Create `cache_monitor.rs`**

```rust
// src/thinker/prompt_builder/cache_monitor.rs

//! Prompt cache hit/miss monitoring.
//!
//! Tracks the hash of the stable system prompt prefix and correlates it
//! with `cache_read_tokens` from API responses to detect cache breaks.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// Lightweight prompt cache hit/miss monitor.
pub struct CacheMonitor {
    state: Mutex<MonitorState>,
}

struct MonitorState {
    /// Hash of the last stable prompt prefix.
    stable_hash: Option<u64>,
    /// Count of consecutive cache misses.
    consecutive_misses: u32,
    /// Total API calls tracked.
    total_calls: u64,
    /// Total cache hits (cache_read_tokens > 0).
    total_hits: u64,
}

impl CacheMonitor {
    /// Create a new monitor with empty state.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MonitorState {
                stable_hash: None,
                consecutive_misses: 0,
                total_calls: 0,
                total_hits: 0,
            }),
        }
    }

    /// Update the stable prompt hash.
    ///
    /// Returns `true` if the hash changed from the previous value,
    /// indicating a potential cache break source.
    pub fn update_stable_hash(&self, stable_content: &str) -> bool {
        let new_hash = fast_hash(stable_content);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let changed = state.stable_hash.map_or(false, |h| h != new_hash);
        if changed {
            tracing::debug!(
                old_hash = state.stable_hash.unwrap_or(0),
                new_hash,
                "Stable prompt hash changed — expect cache miss"
            );
        }
        state.stable_hash = Some(new_hash);
        changed
    }

    /// Record `cache_read_tokens` from an API response.
    ///
    /// Emits a tracing warning when 3+ consecutive misses are detected.
    pub fn record_cache_usage(&self, cache_read_tokens: Option<u32>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.total_calls += 1;

        let read = cache_read_tokens.unwrap_or(0);
        if read > 0 {
            state.total_hits += 1;
            state.consecutive_misses = 0;
        } else {
            state.consecutive_misses += 1;
            if state.consecutive_misses >= 3 && state.total_calls > 3 {
                let hit_rate = state.total_hits as f64 / state.total_calls as f64 * 100.0;
                tracing::warn!(
                    consecutive_misses = state.consecutive_misses,
                    hit_rate_pct = format!("{:.0}", hit_rate),
                    "Prompt cache: {} consecutive misses (overall hit rate {:.0}%)",
                    state.consecutive_misses,
                    hit_rate,
                );
            }
        }
    }

    /// Reset the consecutive miss counter.
    ///
    /// Call after compaction — compaction legitimately changes the prompt
    /// prefix, so subsequent misses should not trigger false warnings.
    pub fn notify_compaction(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.consecutive_misses = 0;
    }

    /// Current hit rate as a percentage in `[0.0, 100.0]`.
    pub fn hit_rate(&self) -> f64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.total_calls == 0 {
            return 100.0;
        }
        state.total_hits as f64 / state.total_calls as f64 * 100.0
    }
}

/// Fast non-cryptographic hash.
fn fast_hash(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_hash_update_returns_false() {
        let monitor = CacheMonitor::new();
        assert!(!monitor.update_stable_hash("initial content"));
    }

    #[test]
    fn same_hash_returns_false() {
        let monitor = CacheMonitor::new();
        monitor.update_stable_hash("content");
        assert!(!monitor.update_stable_hash("content"));
    }

    #[test]
    fn changed_hash_returns_true() {
        let monitor = CacheMonitor::new();
        monitor.update_stable_hash("content v1");
        assert!(monitor.update_stable_hash("content v2"));
    }

    #[test]
    fn hit_rate_tracks_correctly() {
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage(Some(1000)); // hit
        monitor.record_cache_usage(Some(500));  // hit
        monitor.record_cache_usage(None);       // miss
        monitor.record_cache_usage(Some(800));  // hit
        // 3 hits out of 4 = 75%
        assert!((monitor.hit_rate() - 75.0).abs() < 0.1);
    }

    #[test]
    fn compaction_resets_consecutive_misses() {
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage(None);
        monitor.record_cache_usage(None);
        monitor.notify_compaction();
        // After reset, one more miss should NOT trigger warning (only 1 consecutive)
        monitor.record_cache_usage(None);
        // No panic or warning = pass (we can't assert on tracing output easily)
    }

    #[test]
    fn hit_rate_is_100_when_no_calls() {
        let monitor = CacheMonitor::new();
        assert!((monitor.hit_rate() - 100.0).abs() < 0.1);
    }
}
```

- [ ] **Step 2: Export from `prompt_builder/mod.rs`**

Add after line 2 (`mod sections;`):

```rust
pub mod cache_monitor;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- cache_monitor`

Expected: All 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/thinker/prompt_builder/cache_monitor.rs src/thinker/prompt_builder/mod.rs
git commit -m "feat(compaction): add CacheMonitor for prompt cache hit/miss tracking"
```

---

## Task 10: G3 — Integrate CacheMonitor into loop_core and orchestrator

**Files:**
- Modify: `src/agent_loop/compaction/orchestrator.rs`
- Modify: `src/thinker/prompt_builder/cache.rs`

Note: `loop_core.rs` integration is deferred to a final wiring task (Task 12) since it requires all components ready.

- [ ] **Step 1: Add cache monitor notification to orchestrator**

In `orchestrator.rs`, add a field to `CompactionOrchestrator`:

```rust
    cache_monitor: Option<std::sync::Arc<crate::thinker::prompt_builder::cache_monitor::CacheMonitor>>,
```

Add field to `OrchestratorBuilder`:

```rust
    cache_monitor: Option<std::sync::Arc<crate::thinker::prompt_builder::cache_monitor::CacheMonitor>>,
```

Initialize it as `None` in `CompactionOrchestrator::builder()`.

Add builder method:

```rust
    pub fn cache_monitor(mut self, monitor: std::sync::Arc<crate::thinker::prompt_builder::cache_monitor::CacheMonitor>) -> Self {
        self.cache_monitor = Some(monitor);
        self
    }
```

Pass it through in `build()`.

At the end of `execute()`, after `self.run_cleanups(&aggregate)`:

```rust
        if let Some(monitor) = &self.cache_monitor {
            monitor.notify_compaction();
        }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- orchestrator`

Expected: All tests pass (cache_monitor is `None` in tests).

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/compaction/orchestrator.rs
git commit -m "feat(compaction): notify CacheMonitor after compaction execution"
```

---

## Task 11: G4 — Create SessionSummarySource

**Files:**
- Create: `src/agent_loop/compaction/session_summary_source.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Modify: `src/agent_loop/context_compactor.rs`

- [ ] **Step 1: Add `SessionMemoryReuse` to `CompactStrategy`**

In `context_compactor.rs`, add the new variant:

```rust
pub enum CompactStrategy {
    LlmSummary,
    DeterministicTruncation,
    /// Reused existing session summaries — zero API cost.
    SessionMemoryReuse,
    Skipped { reason: String },
}
```

Update the `PartialEq` derive — this variant has no fields so it works automatically.

- [ ] **Step 2: Create `session_summary_source.rs`**

```rust
// src/agent_loop/compaction/session_summary_source.rs

//! Zero-cost compaction via existing session summaries.
//!
//! When the [`SessionCompactor`] has already generated summaries covering
//! the compression window, this source allows [`ContextCompactor`] to skip
//! the LLM API call and reuse the existing summaries directly.

use crate::memory::context::MemoryScope;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::MemoryBackend;
use crate::providers::message::UnifiedMessage;

use super::super::context_compactor::{CompactResult, CompactStrategy};

/// Provides existing session summaries as a zero-cost compaction alternative.
pub struct SessionSummarySource {
    database: MemoryBackend,
    session_id: String,
}

impl SessionSummarySource {
    /// Create a new source backed by the given memory store and session.
    pub fn new(database: MemoryBackend, session_id: String) -> Self {
        Self {
            database,
            session_id,
        }
    }

    /// Try to replace the compression window with existing summaries.
    ///
    /// Returns `None` if there are no summaries or insufficient coverage,
    /// causing the caller to fall through to the LLM compaction path.
    pub async fn try_reuse(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        window_start: usize,
        cut_end: usize,
    ) -> Option<CompactResult> {
        let path_prefix = format!("aleph://session/{}/", self.session_id);
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);

        let summaries = self
            .database
            .get_facts_by_path_prefix(&path_prefix, &filter, 50)
            .await
            .ok()?;

        if summaries.is_empty() {
            return None;
        }

        // Sort highest-depth-first (d2 > d1 > d0) to prefer more condensed summaries
        let mut sorted = summaries;
        sorted.sort_by(|a, b| {
            let da = extract_depth(&a.path);
            let db = extract_depth(&b.path);
            db.cmp(&da).then_with(|| a.path.cmp(&b.path))
        });

        // Estimate token size of the compression window
        let window_text: String = messages[window_start..cut_end]
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");
        let tokens_before = estimate_tokens(&window_text);

        // Budget: at most 50% of original window tokens
        let budget = tokens_before / 2;
        let mut assembled = String::new();
        let mut used_tokens = 0usize;

        for fact in &sorted {
            let fact_tokens = estimate_tokens(&fact.content);
            if used_tokens + fact_tokens > budget {
                break;
            }
            if !assembled.is_empty() {
                assembled.push_str("\n\n");
            }
            assembled.push_str(&fact.content);
            used_tokens += fact_tokens;
        }

        if assembled.is_empty() {
            return None;
        }

        // Replace window with assembled summaries
        let summary_msg = UnifiedMessage::user(format!(
            "[Context Summary (from session memory)]\n{}",
            assembled
        ));
        let tokens_after = used_tokens;

        messages.drain(window_start..cut_end);
        messages.insert(window_start, summary_msg);

        Some(CompactResult {
            tokens_before,
            tokens_after,
            strategy_used: CompactStrategy::SessionMemoryReuse,
        })
    }
}

/// Extract the depth number from a session summary path.
///
/// Path format: `aleph://session/{id}/d{depth}/{seq}`
fn extract_depth(path: &str) -> u32 {
    path.split('/')
        .find_map(|segment| {
            segment
                .strip_prefix('d')
                .and_then(|rest| rest.parse::<u32>().ok())
        })
        .unwrap_or(0)
}

/// Estimate token count using the 3.5 chars/token heuristic.
fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    (char_count as f64 / 3.5).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_depth_parses_d0() {
        assert_eq!(extract_depth("aleph://session/abc/d0/3"), 0);
    }

    #[test]
    fn extract_depth_parses_d2() {
        assert_eq!(extract_depth("aleph://session/abc/d2/1"), 2);
    }

    #[test]
    fn extract_depth_returns_0_for_invalid() {
        assert_eq!(extract_depth("invalid/path"), 0);
    }
}
```

- [ ] **Step 3: Modify `compact()` to accept a summary source**

In `context_compactor.rs`, change the `compact` method signature:

```rust
    pub async fn compact(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        fresh_tail: usize,
        summary_source: Option<&SessionSummarySource>,
    ) -> anyhow::Result<CompactResult> {
```

Add the import at the top:

```rust
use crate::agent_loop::compaction::session_summary_source::SessionSummarySource;
```

After the idempotency check (line 128) and before the window serialization (line 131), insert:

```rust
        // Fast path: try to reuse existing session summaries (zero API cost)
        if let Some(source) = summary_source {
            if let Some(reuse_result) = source.try_reuse(messages, window_start, cut_end).await {
                tracing::info!(
                    tokens_before = reuse_result.tokens_before,
                    tokens_after = reuse_result.tokens_after,
                    "Compaction via session memory reuse (zero API cost)"
                );
                return Ok(reuse_result);
            }
        }
```

- [ ] **Step 4: Update the `CompactionStrategy::execute` impl**

In the `execute` impl for `ContextCompactor` (around line 293), change:

```rust
            let result = self
                .compact(&mut ctx.messages, ctx.fresh_tail_count)
                .await?;
```

to:

```rust
            let result = self
                .compact(&mut ctx.messages, ctx.fresh_tail_count, None)
                .await?;
```

- [ ] **Step 5: Update all test calls**

In the `#[cfg(test)]` module, update all `compact` calls to pass `None` as the third arg:

```rust
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
```

(4 test functions: `compacts_when_window_available`, `skips_when_window_too_small`, `falls_back_to_truncation_on_provider_failure`, `idempotent_on_already_compacted`)

- [ ] **Step 6: Export from `compaction/mod.rs`**

Add:

```rust
pub mod session_summary_source;
```

And re-export:

```rust
pub use session_summary_source::SessionSummarySource;
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib -- context_compactor session_summary_source`

Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/compaction/session_summary_source.rs src/agent_loop/context_compactor.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add SessionSummarySource for zero-cost compaction via existing summaries"
```

---

## Task 12: Final wiring — compile check and integration

**Files:**
- Modify: `src/agent_loop/mod.rs` (verify exports)
- No new code — this task verifies everything compiles together

- [ ] **Step 1: Verify all module exports are correct**

Check `src/agent_loop/mod.rs` has `pub mod tool_result_store;`.

Check `src/agent_loop/compaction/mod.rs` has:
```rust
pub mod file_content_tracker;
pub mod session_summary_source;
pub mod summary_utils;
```

Check `src/thinker/prompt_builder/mod.rs` has:
```rust
pub mod cache_monitor;
```

- [ ] **Step 2: Run full compile check**

Run: `cargo check -p alephcore`

Expected: No errors. Fix any remaining import issues.

- [ ] **Step 3: Run all tests**

Run: `cargo test -p alephcore --lib`

Expected: All tests pass.

- [ ] **Step 4: Commit if any fixes were needed**

```bash
git add -A
git commit -m "chore: fix compilation issues from context window optimization"
```

---

## Task 13: Cleanup — remove dead code

**Files:**
- Verify: `src/memory/session_compactor/summary_engine.rs` — no local duplicates remain

- [ ] **Step 1: Verify no duplicate `strip_analysis_block` or `IDENTIFIER_PRESERVATION`**

Run: `cargo clippy -p alephcore -- -D warnings`

Ensure no warnings about dead code or unused imports from the refactoring.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p alephcore`

Expected: All tests pass, no warnings.

- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "chore: cleanup dead code after context window optimization"
```
