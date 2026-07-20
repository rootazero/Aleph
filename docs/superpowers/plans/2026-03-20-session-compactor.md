# Session Compactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add intra-session context management to prevent token overflow in long conversations, using dual-layer compression (deterministic tool compaction + async LLM summarization).

**Architecture:** New `session_compactor` module under `src/memory/` with four components: ToolCompactor (sync, in-loop), SummaryEngine (async, post-loop), ContextWindow (history assembly), and Fallback (deterministic degradation). Integrates via existing ExecutionEngine builder pattern and optional injection into AgentLoop.

**Tech Stack:** Rust, LanceDB (existing MemoryBackend), existing AiProvider trait for LLM calls.

**Spec:** `docs/superpowers/specs/2026-03-20-session-compactor-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/memory/session_compactor/mod.rs` | SessionCompactor struct, config, orchestration (prepare_history + post_turn_compress) |
| `src/memory/session_compactor/tool_compactor.rs` | Deterministic tool result compression by tool type |
| `src/memory/session_compactor/summary_engine.rs` | LLM summary generation with depth-aware prompts |
| `src/memory/session_compactor/context_window.rs` | Token estimation, message partitioning, eviction logic |
| `src/memory/session_compactor/fallback.rs` | Three-level fallback chain (Normal → Aggressive → Deterministic) |

### Modified Files

| File | Change |
|------|--------|
| `src/memory/context/enums.rs` | Add `MemoryScope::SessionLocal`, `FactSource::SessionCompressed` |
| `src/memory/mod.rs` | Add `pub mod session_compactor` declaration + re-exports |
| `src/memory/store/types.rs` | Handle `SessionLocal` in `SearchFilter::to_lance_filter()` |
| `src/agent_loop/loop_core.rs` | Add optional `tool_compactor` field, call before `provider.call()` |
| `src/gateway/execution_engine/engine.rs` | Add `session_compactor` field, builder method, wire in execute() |
| `src/gateway/execution_engine/run_loop.rs` | Use `SessionCompactor::prepare_history()` when available |
| `src/builtin_tools/memory_search.rs` | Add `scope` parameter to args, construct SessionLocal filter |
| `src/thinker/layers/` | New `session_context_guide.rs` PromptLayer |

---

## Task 1: Add Enum Values and Config Types

**Files:**
- Modify: `src/memory/context/enums.rs:136-146` (FactSource), `src/memory/context/enums.rs:360-370` (MemoryScope)
- Modify: `src/memory/store/types.rs:197-267` (to_lance_filter)
- Modify: `src/memory/mod.rs:1-66` (module declaration)
- Create: `src/memory/session_compactor/mod.rs`

- [ ] **Step 1: Add `SessionLocal` to `MemoryScope` enum**

In `src/memory/context/enums.rs`, add `SessionLocal` variant to `MemoryScope` (after line 370):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MemoryScope {
    #[default]
    Global,
    Agent,
    Persona,
    SessionLocal,  // NEW: scoped to a single conversation session
}
```

- [ ] **Step 2: Add `SessionCompressed` to `FactSource` enum**

In `src/memory/context/enums.rs`, add `SessionCompressed` variant to `FactSource` (after line 146):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FactSource {
    #[default]
    Extracted,
    Summary,
    Document,
    Manual,
    SessionCompressed,  // NEW: intra-session DAG-style summaries with depth metadata
}
```

- [ ] **Step 3: Update `SearchFilter::to_lance_filter()` for SessionLocal**

In `src/memory/store/types.rs`, find the scope handling in `to_lance_filter()` (around line 246) and ensure `SessionLocal` serializes correctly in the DataFusion SQL filter. The existing scope match should already handle it via serde serialization, but verify the string representation matches LanceDB storage.

- [ ] **Step 4: Create `SessionCompactorConfig` and module skeleton**

Create `src/memory/session_compactor/mod.rs`:

```rust
use serde::{Deserialize, Serialize};

pub mod tool_compactor;
pub mod summary_engine;
pub mod context_window;
pub mod fallback;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCompactorConfig {
    pub enabled: bool,
    pub fresh_tail_count: usize,
    pub context_threshold: f64,
    pub leaf_chunk_tokens: usize,
    pub d1_min_fanout: usize,
    pub d2_min_fanout: usize,
    pub max_summary_depth: u32,
    pub token_estimate_ratio: f64,
    pub session_fact_retention_hours: u64,
    pub promote_confidence_threshold: f32,
}

impl Default for SessionCompactorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fresh_tail_count: 20,
            context_threshold: 0.75,
            leaf_chunk_tokens: 1000,
            d1_min_fanout: 4,
            d2_min_fanout: 3,
            max_summary_depth: 2,
            token_estimate_ratio: 3.5,
            session_fact_retention_hours: 24,
            promote_confidence_threshold: 0.8,
        }
    }
}
```

- [ ] **Step 5: Add module declaration to `memory/mod.rs`**

In `src/memory/mod.rs`, add `pub mod session_compactor;` with the other module declarations. Add re-exports for `SessionCompactorConfig`.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors. New enum values may produce "unused" warnings — that's fine.

- [ ] **Step 7: Commit**

```bash
git add src/memory/context/enums.rs src/memory/store/types.rs src/memory/mod.rs src/memory/session_compactor/
git commit -m "session_compactor: add SessionLocal scope, SessionCompressed source, and config types"
```

---

## Task 2: Token Estimation and Context Window

**Files:**
- Create: `src/memory/session_compactor/context_window.rs`

- [ ] **Step 1: Write tests for token estimation and message partitioning**

Create `src/memory/session_compactor/context_window.rs` with tests:

```rust
use crate::providers::message::UnifiedMessage;

/// Estimate token count for a message using char-length heuristic.
pub fn estimate_tokens(content: &str, ratio: f64) -> usize {
    (content.len() as f64 / ratio) as usize
}

/// Estimate total tokens across all messages.
pub fn estimate_total_tokens(messages: &[UnifiedMessage], ratio: f64) -> usize {
    messages.iter().map(|m| estimate_tokens(&m.text_content(), ratio)).sum()
}

/// Partition messages into (compressible, fresh_tail).
/// Returns the index where fresh_tail begins.
pub fn partition_fresh_tail(messages: &[UnifiedMessage], fresh_tail_count: usize) -> usize {
    if messages.len() <= fresh_tail_count {
        0
    } else {
        messages.len() - fresh_tail_count
    }
}

/// Check if a tool result at position `idx` has been "consumed"
/// (an assistant message follows it in the message list).
pub fn is_tool_result_consumed(messages: &[UnifiedMessage], idx: usize) -> bool {
    // A tool result is consumed if any subsequent message is an assistant message
    for i in (idx + 1)..messages.len() {
        if messages[i].is_assistant() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_english() {
        // ~4 chars per token for English
        let content = "Hello world, this is a test message for token estimation.";
        let tokens = estimate_tokens(content, 3.5);
        assert!(tokens > 10 && tokens < 25);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens("", 3.5), 0);
    }

    #[test]
    fn test_partition_fresh_tail_normal() {
        let messages: Vec<UnifiedMessage> = (0..50)
            .map(|i| UnifiedMessage::user(format!("msg {}", i)))
            .collect();
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 30);
    }

    #[test]
    fn test_partition_fresh_tail_short_history() {
        let messages: Vec<UnifiedMessage> = (0..5)
            .map(|i| UnifiedMessage::user(format!("msg {}", i)))
            .collect();
        let split = partition_fresh_tail(&messages, 20);
        assert_eq!(split, 0); // All messages are in fresh tail
    }

    #[test]
    fn test_is_tool_result_consumed() {
        let messages = vec![
            UnifiedMessage::user("query".to_string()),
            UnifiedMessage::tool_result("id1", "tool", "result data", false),
            UnifiedMessage::assistant("I found...".to_string()),
        ];
        assert!(is_tool_result_consumed(&messages, 1));
        assert!(!is_tool_result_consumed(&messages, 2));
    }
}
```

Note: `UnifiedMessage::tool_result()` constructor — check exact signature in `src/providers/message.rs:61-75`. Adapt constructor args to match. `UnifiedMessage::is_assistant()` may need to be added or use pattern matching.

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib session_compactor::context_window`
Expected: All tests pass. If `UnifiedMessage` API doesn't match, adapt the test helpers.

- [ ] **Step 3: Commit**

```bash
git add src/memory/session_compactor/context_window.rs
git commit -m "session_compactor: add token estimation and message partitioning"
```

---

## Task 3: Deterministic Fallback

**Files:**
- Create: `src/memory/session_compactor/fallback.rs`

- [ ] **Step 1: Write tests for deterministic truncation**

Create `src/memory/session_compactor/fallback.rs`:

```rust
/// Extract the first sentence from text content.
fn first_sentence(text: &str) -> &str {
    // Find first sentence-ending punctuation
    for (i, c) in text.char_indices() {
        if (c == '.' || c == '!' || c == '?' || c == '\n') && i > 0 {
            return &text[..=i];
        }
    }
    text
}

/// Deterministic fallback: extract first sentence from each message,
/// concatenate, limit to max_chars.
pub fn deterministic_truncate(messages: &[(String, String)], max_chars: usize) -> String {
    // messages is Vec<(role, content)>
    let mut result = String::new();
    for (role, content) in messages {
        let sentence = first_sentence(content);
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("[{}] {}", role, sentence));
    }
    if result.len() > max_chars {
        // Truncate at char boundary
        let truncated = &result[..result.floor_char_boundary(max_chars)];
        format!("{}\n[Truncated]", truncated)
    } else {
        result
    }
}

/// Compute target token count for a summary at a given level.
pub fn target_tokens(input_tokens: usize, level: FallbackLevel) -> usize {
    match level {
        FallbackLevel::Normal => input_tokens.mul_f64(0.35).clamp(128, 800),
        FallbackLevel::Aggressive => input_tokens.mul_f64(0.2).clamp(64, 400),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FallbackLevel {
    Normal,
    Aggressive,
}

trait ClampExt {
    fn clamp(self, min: usize, max: usize) -> usize;
    fn mul_f64(self, factor: f64) -> usize;
}

impl ClampExt for usize {
    fn clamp(self, min: usize, max: usize) -> usize {
        self.max(min).min(max)
    }
    fn mul_f64(self, factor: f64) -> usize {
        (self as f64 * factor) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_sentence() {
        assert_eq!(first_sentence("Hello world. More text."), "Hello world.");
        assert_eq!(first_sentence("No period"), "No period");
        assert_eq!(first_sentence("Line one\nLine two"), "Line one\n");
    }

    #[test]
    fn test_deterministic_truncate_short() {
        let messages = vec![
            ("user".to_string(), "What is X?".to_string()),
            ("assistant".to_string(), "X is a thing. More details here.".to_string()),
        ];
        let result = deterministic_truncate(&messages, 512);
        assert!(result.contains("[user] What is X?"));
        assert!(result.contains("[assistant] X is a thing."));
        assert!(!result.contains("[Truncated]"));
    }

    #[test]
    fn test_deterministic_truncate_long() {
        let messages: Vec<(String, String)> = (0..100)
            .map(|i| ("user".to_string(), format!("Message number {} with content.", i)))
            .collect();
        let result = deterministic_truncate(&messages, 512);
        assert!(result.ends_with("[Truncated]"));
        assert!(result.len() <= 512 + 20); // Allow for [Truncated] suffix
    }

    #[test]
    fn test_target_tokens_normal() {
        assert_eq!(target_tokens(1000, FallbackLevel::Normal), 350);
        assert_eq!(target_tokens(100, FallbackLevel::Normal), 128); // min clamp
        assert_eq!(target_tokens(5000, FallbackLevel::Normal), 800); // max clamp
    }

    #[test]
    fn test_target_tokens_aggressive() {
        assert_eq!(target_tokens(1000, FallbackLevel::Aggressive), 200);
        assert_eq!(target_tokens(100, FallbackLevel::Aggressive), 64); // min clamp
        assert_eq!(target_tokens(5000, FallbackLevel::Aggressive), 400); // max clamp
    }
}
```

Note: `str::floor_char_boundary` is nightly-only. Use a helper: `text.char_indices().take_while(|(i, _)| *i <= max_chars).last().map(|(i, _)| i).unwrap_or(0)` instead.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib session_compactor::fallback`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/memory/session_compactor/fallback.rs
git commit -m "session_compactor: add deterministic fallback with three-level target tokens"
```

---

## Task 4: ToolCompactor

**Files:**
- Create: `src/memory/session_compactor/tool_compactor.rs`
- Reference: `src/providers/message.rs:15-27` (UnifiedMessage enum)

- [ ] **Step 1: Write tests for tool result compression**

Create `src/memory/session_compactor/tool_compactor.rs`:

```rust
use crate::providers::message::UnifiedMessage;
use super::context_window::{estimate_tokens, estimate_total_tokens, partition_fresh_tail, is_tool_result_consumed};

/// Compress a tool result based on tool name and content.
pub fn compress_tool_result(tool_name: &str, content: &str) -> String {
    let token_estimate = estimate_tokens(content, 3.5);
    match tool_name {
        name if is_read_tool(name) => compress_read_result(content),
        name if is_search_tool(name) => compress_search_result(content),
        name if is_bash_tool(name) => compress_bash_result(content),
        name if is_web_tool(name) => compress_web_result(content, token_estimate),
        _ => compress_generic(content, token_estimate),
    }
}

fn is_read_tool(name: &str) -> bool {
    matches!(name, "Read" | "Glob" | "read_file" | "glob")
}

fn is_search_tool(name: &str) -> bool {
    matches!(name, "Grep" | "Search" | "grep" | "search" | "ripgrep")
}

fn is_bash_tool(name: &str) -> bool {
    matches!(name, "Bash" | "bash" | "shell" | "execute_command")
}

fn is_web_tool(name: &str) -> bool {
    matches!(name, "WebFetch" | "web_fetch" | "fetch_url")
}

fn compress_read_result(content: &str) -> String {
    let line_count = content.lines().count();
    // Try to detect language from content
    let lang = detect_language_hint(content);
    format!("[Read file, {} lines, {}]", line_count, lang)
}

fn compress_search_result(content: &str) -> String {
    let match_count = content.lines().count();
    format!("[Search result, {} matching lines]", match_count)
}

fn compress_bash_result(content: &str) -> String {
    let line_count = content.lines().count();
    // Try to extract exit code if present
    format!("[Command output, {} lines]", line_count)
}

fn compress_web_result(content: &str, tokens: usize) -> String {
    let preview_chars = 200;
    let preview = if content.len() > preview_chars {
        let boundary = content.char_indices()
            .take_while(|(i, _)| *i <= preview_chars)
            .last()
            .map(|(i, _)| i + 1)
            .unwrap_or(preview_chars.min(content.len()));
        format!("{}... [Truncated, original ~{} tokens]", &content[..boundary], tokens)
    } else {
        content.to_string()
    };
    preview
}

fn compress_generic(content: &str, tokens: usize) -> String {
    if tokens > 500 {
        let preview_chars = 200;
        let boundary = content.char_indices()
            .take_while(|(i, _)| *i <= preview_chars)
            .last()
            .map(|(i, _)| i + 1)
            .unwrap_or(preview_chars.min(content.len()));
        format!("{}... [Truncated, original ~{} tokens]", &content[..boundary], tokens)
    } else {
        content.to_string()
    }
}

fn detect_language_hint(content: &str) -> &'static str {
    if content.contains("fn ") && content.contains("let ") { "Rust" }
    else if content.contains("def ") && content.contains("import ") { "Python" }
    else if content.contains("function ") || content.contains("const ") { "JavaScript" }
    else { "text" }
}

/// Compact tool results in-place if total tokens exceed threshold.
/// Only compresses consumed tool results (assistant replied after them).
/// Compresses oldest first, stops when under threshold.
pub fn compact_if_needed(messages: &mut Vec<UnifiedMessage>, token_budget: u64, threshold: f64, ratio: f64, fresh_tail_count: usize) {
    let total = estimate_total_tokens(messages, ratio);
    let limit = (token_budget as f64 * threshold) as usize;

    if total <= limit {
        return;
    }

    let fresh_tail_start = partition_fresh_tail(messages, fresh_tail_count);

    // Collect indices of compressible tool results (oldest first)
    let compressible: Vec<usize> = (0..fresh_tail_start)
        .filter(|&i| messages[i].is_tool_result() && is_tool_result_consumed(messages, i))
        .collect();

    let mut current_total = total;
    for idx in compressible {
        if current_total <= limit {
            break;
        }
        let (tool_name, content) = messages[idx].tool_result_info();
        let old_tokens = estimate_tokens(&content, ratio);
        let compressed = compress_tool_result(&tool_name, &content);
        let new_tokens = estimate_tokens(&compressed, ratio);

        messages[idx].replace_tool_result_content(compressed);
        current_total = current_total.saturating_sub(old_tokens - new_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_read_result() {
        let content = "fn main() {\n    let x = 42;\n    println!(\"{}\", x);\n}\n";
        let result = compress_read_result(content);
        assert!(result.contains("4 lines"));
        assert!(result.contains("Rust"));
    }

    #[test]
    fn test_compress_search_result() {
        let content = "src/main.rs:10: fn execute()\nsrc/lib.rs:20: fn execute_impl()";
        let result = compress_search_result(content);
        assert!(result.contains("2 matching lines"));
    }

    #[test]
    fn test_compress_generic_short() {
        let short = "OK";
        assert_eq!(compress_generic(short, 1), "OK"); // Not truncated
    }

    #[test]
    fn test_compress_generic_long() {
        let long = "x".repeat(3000);
        let result = compress_generic(&long, 800);
        assert!(result.contains("[Truncated"));
        assert!(result.len() < 300); // Much shorter than original
    }

    #[test]
    fn test_compact_if_needed_under_threshold() {
        let mut messages = vec![
            UnifiedMessage::user("hi".to_string()),
            UnifiedMessage::assistant("hello".to_string()),
        ];
        let original_len = messages.len();
        compact_if_needed(&mut messages, 200000, 0.75, 3.5, 20);
        assert_eq!(messages.len(), original_len); // No change
    }
}
```

Note: `UnifiedMessage::is_tool_result()`, `tool_result_info()`, and `replace_tool_result_content()` may not exist yet. These helper methods need to be added to `UnifiedMessage` in `src/providers/message.rs`. Add them as part of this task:

```rust
impl UnifiedMessage {
    pub fn is_tool_result(&self) -> bool {
        matches!(self, UnifiedMessage::ToolResult { .. })
    }

    pub fn is_assistant(&self) -> bool {
        matches!(self, UnifiedMessage::Assistant { .. })
    }

    /// Returns (tool_name, content) for ToolResult messages.
    pub fn tool_result_info(&self) -> (String, String) {
        match self {
            UnifiedMessage::ToolResult { tool_name, content, .. } => {
                (tool_name.clone(), content.clone())
            }
            _ => (String::new(), String::new()),
        }
    }

    /// Replace the content of a ToolResult message.
    pub fn replace_tool_result_content(&mut self, new_content: String) {
        if let UnifiedMessage::ToolResult { content, .. } = self {
            *content = new_content;
        }
    }

    /// Get text content from any message variant.
    pub fn text_content(&self) -> String {
        match self {
            UnifiedMessage::User { content } | UnifiedMessage::Assistant { content } => {
                content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("")
            }
            UnifiedMessage::ToolResult { content, .. } => content.clone(),
        }
    }
}
```

- [ ] **Step 2: Add helper methods to UnifiedMessage**

In `src/providers/message.rs`, add the helper methods listed above (`is_tool_result`, `is_assistant`, `tool_result_info`, `replace_tool_result_content`, `text_content`). Check if any of these already exist — only add what's missing.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib session_compactor::tool_compactor`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_compactor/tool_compactor.rs src/providers/message.rs
git commit -m "session_compactor: add ToolCompactor with per-type deterministic compression"
```

---

## Task 5: Summary Engine (Depth-Aware Prompts)

**Files:**
- Create: `src/memory/session_compactor/summary_engine.rs`
- Reference: `src/memory/context/enums.rs` (FactSource, MemoryScope)
- Reference: `src/memory/context/fact.rs:35-104` (MemoryFact)

- [ ] **Step 1: Write prompt construction and chunking logic with tests**

Create `src/memory/session_compactor/summary_engine.rs`:

```rust
use crate::memory::{MemoryFact, FactType, FactSource, MemoryScope, MemoryTier, MemoryLayer};
use super::fallback::{FallbackLevel, target_tokens, deterministic_truncate};
use super::context_window::estimate_tokens;

/// Build the LLM prompt for summarizing a chunk of messages.
pub fn build_summary_prompt(
    messages: &[(String, String)],  // (role, content)
    depth: u32,
    previous_context: Option<&str>,
    level: FallbackLevel,
) -> String {
    let target = target_tokens(
        messages.iter().map(|(_, c)| estimate_tokens(c, 3.5)).sum(),
        level,
    );

    let depth_instruction = match depth {
        0 => LEAF_PROMPT,
        1 => D1_PROMPT,
        _ => D2_PROMPT,
    };

    let previous = previous_context
        .map(|ctx| format!("\n\n<previous_context>\n{}\n</previous_context>\nDo not repeat information already in the previous context.", ctx))
        .unwrap_or_default();

    let conversation = messages.iter()
        .map(|(role, content)| format!("[{}]: {}", role, content))
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "{depth_instruction}\n\nTarget length: approximately {target} tokens.\n\
         {previous}\n\n\
         <conversation>\n{conversation}\n</conversation>\n\n\
         End your summary with a line: \"Expand for details: <comma-separated list of compressed details>\"",
    )
}

const LEAF_PROMPT: &str = "\
Summarize this conversation segment. Preserve:\n\
- Key decisions made and their rationale\n\
- File operations (reads, writes, modifications) with paths\n\
- Error messages and how they were resolved\n\
- TODOs and pending items\n\
- Important technical details (function names, config values)\n\
\n\
Omit: greetings, filler, repeated information, verbose tool outputs already summarized.";

const D1_PROMPT: &str = "\
Condense these summaries into a higher-level session summary. Preserve:\n\
- Decisions and their rationale (especially decisions that supersede earlier ones)\n\
- Current task status (completed, in-progress, blocked)\n\
- Unresolved problems and blockers\n\
- Key files and components affected\n\
\n\
Omit: operational details of individual steps, file contents, error messages (unless unresolved).";

const D2_PROMPT: &str = "\
Create a milestone summary from these session summaries. Preserve:\n\
- Completed work and outcomes\n\
- Active constraints and decisions still in effect\n\
- Evolution of approach (what changed and why)\n\
- Remaining work items\n\
\n\
Omit: individual operation details, file-level changes, resolved errors.";

/// Group messages into chunks of approximately `chunk_tokens` tokens each.
pub fn chunk_messages(
    messages: &[(String, String)],
    chunk_tokens: usize,
    ratio: f64,
) -> Vec<Vec<(String, String)>> {
    let mut chunks = Vec::new();
    let mut current_chunk = Vec::new();
    let mut current_tokens = 0;

    for msg in messages {
        let msg_tokens = estimate_tokens(&msg.1, ratio);
        if current_tokens + msg_tokens > chunk_tokens && !current_chunk.is_empty() {
            chunks.push(std::mem::take(&mut current_chunk));
            current_tokens = 0;
        }
        current_tokens += msg_tokens;
        current_chunk.push(msg.clone());
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

/// Create a MemoryFact from a summary.
pub fn summary_to_fact(
    session_id: &str,
    depth: u32,
    seq: u32,
    summary_text: String,
    source_message_count: usize,
    source_token_count: usize,
    agent_id: &str,
) -> MemoryFact {
    let layer = match depth {
        0 => MemoryLayer::L2Detail,
        1 => MemoryLayer::L1Overview,
        _ => MemoryLayer::L0Abstract,
    };

    MemoryFact::new(summary_text, FactType::Other, vec![])
        .with_id(format!("sess_{}_{depth}_{seq}", &session_id[..8.min(session_id.len())]))
        .with_fact_source(FactSource::SessionCompressed)
        .with_tier(MemoryTier::ShortTerm)
        .with_scope(MemoryScope::SessionLocal)
        .with_layer(layer)
        .with_path(format!("aleph://session/{session_id}/d{depth}/{seq}"))
        .with_confidence(0.9)
        .with_agent(agent_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_leaf_prompt() {
        let messages = vec![
            ("user".to_string(), "Read the file".to_string()),
            ("assistant".to_string(), "Here is the content...".to_string()),
        ];
        let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
        assert!(prompt.contains("Key decisions"));
        assert!(prompt.contains("Expand for details"));
        assert!(prompt.contains("[user]: Read the file"));
    }

    #[test]
    fn test_build_d1_prompt() {
        let messages = vec![
            ("summary".to_string(), "Did thing A...".to_string()),
            ("summary".to_string(), "Did thing B...".to_string()),
        ];
        let prompt = build_summary_prompt(&messages, 1, Some("Previous context here"), FallbackLevel::Normal);
        assert!(prompt.contains("Condense"));
        assert!(prompt.contains("previous_context"));
        assert!(prompt.contains("Do not repeat"));
    }

    #[test]
    fn test_chunk_messages() {
        // Each message ~10 tokens at ratio 3.5 (~35 chars)
        let messages: Vec<(String, String)> = (0..10)
            .map(|i| ("user".to_string(), format!("Message number {} with some content.", i)))
            .collect();
        let chunks = chunk_messages(&messages, 30, 3.5); // ~3 messages per chunk
        assert!(chunks.len() >= 3);
        for chunk in &chunks {
            assert!(!chunk.is_empty());
        }
    }

    #[test]
    fn test_summary_to_fact_d0() {
        let fact = summary_to_fact(
            "test-session-123", 0, 1,
            "Summary text".to_string(), 5, 500, "agent-1",
        );
        assert_eq!(fact.scope, MemoryScope::SessionLocal);
        assert_eq!(fact.fact_source, FactSource::SessionCompressed);
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
        assert_eq!(fact.layer, MemoryLayer::L2Detail);
        assert!(fact.path.contains("d0/1"));
    }

    #[test]
    fn test_summary_to_fact_d1() {
        let fact = summary_to_fact(
            "test-session-123", 1, 0,
            "Condensed".to_string(), 4, 2000, "agent-1",
        );
        assert_eq!(fact.layer, MemoryLayer::L1Overview);
        assert!(fact.path.contains("d1/0"));
    }
}
```

Note: `MemoryFact::with_id()`, `with_fact_source()`, `with_layer()`, `with_path()`, `with_confidence()`, `with_agent()` — check which builder methods exist on `MemoryFact` (fact.rs:118-300). Add any missing ones. The existing builders include `with_tier()` (line 261) and `with_scope()` (line 267). Missing builders should follow the same pattern.

- [ ] **Step 2: Add any missing builder methods to MemoryFact**

In `src/memory/context/fact.rs`, add missing builder methods following the existing pattern (e.g., `with_tier` at line 261):

```rust
pub fn with_id(mut self, id: String) -> Self { self.id = id; self }
pub fn with_fact_source(mut self, source: FactSource) -> Self { self.fact_source = source; self }
pub fn with_layer(mut self, layer: MemoryLayer) -> Self { self.layer = layer; self }
pub fn with_path(mut self, path: String) -> Self { self.path = path; self }
pub fn with_confidence(mut self, confidence: f32) -> Self { self.confidence = confidence; self }
pub fn with_agent(mut self, agent: String) -> Self { self.agent = agent; self }
```

Only add methods that don't already exist.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib session_compactor::summary_engine`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_compactor/summary_engine.rs src/memory/context/fact.rs
git commit -m "session_compactor: add SummaryEngine with depth-aware prompts and fact construction"
```

---

## Task 6: SessionCompactor Orchestrator

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`
- Reference: `src/memory/store/` (MemoryBackend)
- Reference: `src/providers/message.rs` (UnifiedMessage)

- [ ] **Step 1: Implement SessionCompactor with prepare_history and post_turn_compress**

Expand `src/memory/session_compactor/mod.rs` with the main orchestrator:

```rust
use std::sync::Arc;
use crate::memory::store::MemoryBackend;
use crate::memory::{MemoryFact, MemoryScope, FactSource, SearchFilter};
use crate::providers::message::UnifiedMessage;

pub struct SessionCompactor {
    database: MemoryBackend,
    config: SessionCompactorConfig,
}

impl SessionCompactor {
    pub fn new(database: MemoryBackend, config: SessionCompactorConfig) -> Self {
        Self { database, config }
    }

    /// Assemble compressed history for the agent loop.
    /// Replaces build_loop_history() when session compactor is available.
    pub async fn prepare_history(
        &self,
        agent: &crate::gateway::agent_instance::AgentInstance,
        session_key: &crate::gateway::router::SessionKey,
        current_input: &str,
        token_budget: u64,
    ) -> Vec<UnifiedMessage> {
        // 1. Fetch session summaries from LanceDB
        let summaries = self.fetch_session_summaries(session_key).await;

        // 2. Fetch raw messages (fresh tail)
        let raw_history = agent.get_history(session_key, Some(self.config.fresh_tail_count as u32)).await;

        // 3. Build message list: summaries first, then raw messages
        let mut messages = Vec::new();

        // Inject summaries as user-role messages with XML tags
        for fact in &summaries {
            let depth = self.extract_depth(&fact.path);
            let msg = UnifiedMessage::user(format!(
                "<session_context depth=\"{}\" source_messages=\"{}\">\n{}\n</session_context>",
                depth,
                fact.source_memory_ids.len(),
                fact.content,
            ));
            messages.push(msg);
        }

        // Add raw messages (skip current input to avoid duplication)
        for msg in &raw_history {
            if msg.role == crate::providers::message::MessageRole::User && msg.content == current_input {
                continue;
            }
            messages.push(msg.to_unified());
        }

        // 4. Evict oldest low-depth summaries if over budget
        self.evict_if_over_budget(&mut messages, token_budget);

        messages
    }

    /// Async post-turn compression. Called via tokio::spawn after agent loop.
    pub async fn post_turn_compress(
        &self,
        session_key: &str,
        agent_id: &str,
        messages: &[(String, String)],  // (role, content) of the full conversation
    ) -> Result<Vec<MemoryFact>, crate::error::AlephError> {
        let mut new_facts = Vec::new();

        // 1. Partition: skip fresh tail
        let fresh_start = context_window::partition_fresh_tail_pairs(messages, self.config.fresh_tail_count);
        let compressible = &messages[..fresh_start];

        if compressible.is_empty() {
            return Ok(new_facts);
        }

        // 2. Chunk compressible messages
        let chunks = summary_engine::chunk_messages(compressible, self.config.leaf_chunk_tokens, self.config.token_estimate_ratio);

        // 3. Get existing summaries to determine seq and previous_context
        let existing = self.fetch_session_summaries_by_key(session_key).await;
        let mut seq = existing.iter()
            .filter(|f| self.extract_depth(&f.path) == 0)
            .count() as u32;
        let previous_context = existing.last().map(|f| f.content.as_str());

        // 4. Generate d0 summaries for each chunk
        for chunk in &chunks {
            let summary = self.generate_summary(chunk, 0, previous_context, session_key, agent_id, seq).await?;
            new_facts.push(summary);
            seq += 1;
        }

        // 5. Check for condensation (d0 → d1, d1 → d2)
        let d0_count = existing.iter().filter(|f| self.extract_depth(&f.path) == 0).count() + new_facts.len();
        if d0_count >= self.config.d1_min_fanout {
            // Trigger d1 condensation
            if let Some(d1_fact) = self.condense(session_key, agent_id, 0, 1).await? {
                new_facts.push(d1_fact);
            }
        }

        // 6. Store all new facts
        for fact in &new_facts {
            self.database.insert_fact(fact.clone()).await?;
        }

        Ok(new_facts)
    }

    async fn fetch_session_summaries(&self, session_key: &crate::gateway::router::SessionKey) -> Vec<MemoryFact> {
        let filter = SearchFilter::new()
            .with_scope(MemoryScope::SessionLocal)
            .with_fact_source(FactSource::SessionCompressed)
            .with_valid_only(true)
            .with_path_prefix(format!("aleph://session/{}/", session_key.to_key_string()));

        self.database.search_facts_by_filter(filter).await.unwrap_or_default()
    }

    async fn fetch_session_summaries_by_key(&self, session_key: &str) -> Vec<MemoryFact> {
        let filter = SearchFilter::new()
            .with_scope(MemoryScope::SessionLocal)
            .with_fact_source(FactSource::SessionCompressed)
            .with_path_prefix(format!("aleph://session/{}/", session_key));

        self.database.search_facts_by_filter(filter).await.unwrap_or_default()
    }

    fn extract_depth(&self, path: &str) -> u32 {
        // Path format: aleph://session/{id}/d{depth}/{seq}
        path.split("/d").nth(1)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    fn evict_if_over_budget(&self, messages: &mut Vec<UnifiedMessage>, token_budget: u64) {
        let limit = (token_budget as f64 * self.config.context_threshold) as usize;
        let total = context_window::estimate_total_tokens(messages, self.config.token_estimate_ratio);

        if total <= limit {
            return;
        }

        // Evict from the front (oldest summaries, lowest depth first)
        while context_window::estimate_total_tokens(messages, self.config.token_estimate_ratio) > limit && !messages.is_empty() {
            // Check if first message is a session_context summary
            if messages[0].text_content().contains("<session_context") {
                messages.remove(0);
            } else {
                break; // Don't evict non-summary messages
            }
        }
    }

    async fn generate_summary(
        &self,
        chunk: &[(String, String)],
        depth: u32,
        previous_context: Option<&str>,
        session_key: &str,
        agent_id: &str,
        seq: u32,
    ) -> Result<MemoryFact, crate::error::AlephError> {
        // Try normal level first, then aggressive, then fallback
        // Note: LLM call integration will be wired in Task 11 (Wire LLM Provider into SummaryEngine)
        // For now, use deterministic fallback
        let summary_text = fallback::deterministic_truncate(chunk, 512);

        let source_token_count: usize = chunk.iter()
            .map(|(_, c)| context_window::estimate_tokens(c, self.config.token_estimate_ratio))
            .sum();

        Ok(summary_engine::summary_to_fact(
            session_key, depth, seq, summary_text,
            chunk.len(), source_token_count, agent_id,
        ))
    }

    async fn condense(
        &self,
        session_key: &str,
        agent_id: &str,
        source_depth: u32,
        target_depth: u32,
    ) -> Result<Option<MemoryFact>, crate::error::AlephError> {
        // Fetch all valid facts at source_depth
        let filter = SearchFilter::new()
            .with_scope(MemoryScope::SessionLocal)
            .with_fact_source(FactSource::SessionCompressed)
            .with_valid_only(true)
            .with_path_prefix(format!("aleph://session/{}/d{}/", session_key, source_depth));

        let source_facts = self.database.search_facts_by_filter(filter).await.unwrap_or_default();

        let min_fanout = if target_depth == 1 { self.config.d1_min_fanout } else { self.config.d2_min_fanout };

        if source_facts.len() < min_fanout {
            return Ok(None);
        }

        // Build messages from source facts for condensation
        let messages: Vec<(String, String)> = source_facts.iter()
            .map(|f| ("summary".to_string(), f.content.clone()))
            .collect();

        let source_token_count: usize = source_facts.iter()
            .map(|f| context_window::estimate_tokens(&f.content, self.config.token_estimate_ratio))
            .sum();

        // Generate condensed summary (fallback for now, LLM wired in Task 8)
        let summary_text = fallback::deterministic_truncate(&messages, 512);

        let seq = 0u32; // First condensed summary at this depth
        let fact = summary_engine::summary_to_fact(
            session_key, target_depth, seq, summary_text,
            source_facts.len(), source_token_count, agent_id,
        );

        // Invalidate source facts
        for source in &source_facts {
            self.database.invalidate_fact(&source.id, "Condensed into higher depth").await?;
        }

        Ok(Some(fact))
    }
}
```

Note: Some MemoryBackend methods (`search_facts_by_filter`, `insert_fact`, `invalidate_fact`) need to be verified against the actual API. Check `src/memory/store/` for exact method signatures and adapt. Also `SearchFilter::new()`, `with_fact_source()`, `with_valid_only()`, `with_path_prefix()` — verify against `types.rs` builders and add missing ones.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles. Some methods may need signature adjustments based on actual MemoryBackend API.

- [ ] **Step 3: Commit**

```bash
git add src/memory/session_compactor/mod.rs
git commit -m "session_compactor: add SessionCompactor orchestrator with prepare_history and post_turn_compress"
```

---

## Task 7: AgentLoop Integration (ToolCompactor Injection)

**Files:**
- Modify: `src/agent_loop/loop_core.rs:97-103` (AgentLoop struct), `src/agent_loop/loop_core.rs:176-190` (main loop)

- [ ] **Step 1: Add optional tool_compactor to AgentLoop**

In `src/agent_loop/loop_core.rs`, add a field to `AgentLoop`:

```rust
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    tool_compactor_config: Option<ToolCompactorConfig>,  // NEW
}
```

Where `ToolCompactorConfig` is:

```rust
pub struct ToolCompactorConfig {
    pub token_budget: u64,
    pub context_threshold: f64,
    pub token_estimate_ratio: f64,
    pub fresh_tail_count: usize,
}
```

- [ ] **Step 2: Add threshold check before provider.call()**

In the main loop (around line 187, before `self.provider.call()`), add:

```rust
// Compact tool results if context is too large
if let Some(ref tc_config) = self.tool_compactor_config {
    crate::memory::session_compactor::tool_compactor::compact_if_needed(
        &mut messages,
        tc_config.token_budget,
        tc_config.context_threshold,
        tc_config.token_estimate_ratio,
        tc_config.fresh_tail_count,
    );
}

let response = self.provider.call(&messages, &system_prompt, &tool_defs).await?;
```

- [ ] **Step 3: Update AgentLoop constructor**

Add a builder method or constructor parameter for `tool_compactor_config`. Follow the existing pattern for how `config` and `safety_guard` are passed in.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles. The new field is `Option`, so existing call sites pass `None` by default.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "agent_loop: inject optional ToolCompactor config for in-loop context compression"
```

---

## Task 8: ExecutionEngine Integration

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs:29-50` (struct), `src/gateway/execution_engine/engine.rs:83-89` (builders), `src/gateway/execution_engine/engine.rs:128-424` (execute)
- Modify: `src/gateway/execution_engine/run_loop.rs:30-36` (run_agent_loop), `src/gateway/execution_engine/run_loop.rs:188-215` (build_loop_history)

- [ ] **Step 1: Add session_compactor field to ExecutionEngine**

In `engine.rs`, add to the struct (after `compression_service`):

```rust
session_compactor: Option<Arc<crate::memory::session_compactor::SessionCompactor>>,
```

Add builder method:

```rust
pub fn with_session_compactor(mut self, compactor: Arc<crate::memory::session_compactor::SessionCompactor>) -> Self {
    self.session_compactor = Some(compactor);
    self
}
```

- [ ] **Step 2: Wire prepare_history into run_agent_loop**

In `run_loop.rs`, modify `run_agent_loop()` to use SessionCompactor when available:

```rust
// Replace:
let history = build_loop_history(&agent, &request.session_key, &request.input).await;

// With:
let history = if let Some(ref sc) = self.session_compactor {
    sc.prepare_history(&agent, &request.session_key, &request.input, self.config.token_budget).await
} else {
    build_loop_history(&agent, &request.session_key, &request.input).await
};
```

- [ ] **Step 3: Wire post_turn_compress after agent loop**

In `engine.rs`, in `execute()`, after the existing `write_conversation_memory` spawn (around line 370-384), add:

```rust
// Session compaction (async, non-blocking)
// Must pass full session history, not just current turn — the compressor
// needs the complete conversation to identify the compressible zone vs fresh tail.
if let Some(ref sc) = self.session_compactor {
    let sc = sc.clone();
    let sk = request.session_key.to_key_string();
    let agent_id = request.session_key.agent_id().to_string();
    let agent_clone = agent.clone();
    let session_key_clone = request.session_key.clone();

    tokio::spawn(async move {
        // Fetch full session history for compression
        let history = agent_clone.get_history(&session_key_clone, Some(50)).await;
        let messages: Vec<(String, String)> = history.iter()
            .map(|m| (m.role.to_string(), m.content.clone()))
            .collect();
        if let Err(e) = sc.post_turn_compress(&sk, &agent_id, &messages).await {
            tracing::warn!(error = %e, "Session compaction failed");
        }
    });
}
```

- [ ] **Step 4: Pass ToolCompactorConfig to AgentLoop**

In `run_agent_loop()`, when constructing `AgentLoop`, pass the config:

```rust
let tool_compactor_config = self.session_compactor.as_ref().map(|sc| {
    crate::agent_loop::loop_core::ToolCompactorConfig {
        token_budget: self.config.token_budget,
        context_threshold: sc.config.context_threshold,
        token_estimate_ratio: sc.config.token_estimate_ratio,
        fresh_tail_count: sc.config.fresh_tail_count,
    }
});
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles. SessionCompactor is optional — existing code paths unchanged.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/engine.rs src/gateway/execution_engine/run_loop.rs
git commit -m "execution_engine: wire SessionCompactor into prepare_history and post_turn_compress"
```

---

## Task 9: System Prompt Layer

**Files:**
- Create: `src/thinker/layers/session_context_guide.rs`
- Modify: `src/thinker/layers/mod.rs` (add module declaration)
- Reference: `src/thinker/prompt_layer.rs:144-174` (PromptLayer trait)

- [ ] **Step 1: Create SessionContextGuideLayer**

Create `src/thinker/layers/session_context_guide.rs`:

```rust
use super::super::prompt_layer::{AssemblyPath, LayerStability, PromptLayer, LayerInput};

/// Injects session context usage guide when compressed summaries are present.
pub struct SessionContextGuideLayer;

impl PromptLayer for SessionContextGuideLayer {
    fn name() -> &'static str { "session_context_guide" }

    fn priority(&self) -> u32 { 1750 }  // Just after MemoryAugmentationLayer (1740)

    fn paths() -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }

    fn supports_mode() -> bool { true }

    fn stability() -> LayerStability { LayerStability::Dynamic }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        // Only inject when session summaries are present
        if !input.has_session_summaries {
            return;
        }

        output.push_str("\n\n## Session Context Notes\n\
            Messages tagged with <session_context> are compressed summaries of earlier conversation.\n\
            - Summaries preserve key decisions and results but omit details\n\
            - If you need specific details (code, error messages, configs), use memory_search with scope=\"current_session\"\n\
            - Do not guess specific details from summaries — search first when uncertain\n");
    }
}
```

Note: `LayerInput` struct may need a new `has_session_summaries: bool` field. Check `src/thinker/prompt_layer.rs` for `LayerInput` definition and add the field. Set it to `true` in the prompt building code when `prepare_history` returned summaries.

- [ ] **Step 2: Add `has_session_summaries` to LayerInput**

In `src/thinker/prompt_layer.rs`, add to `LayerInput`:

```rust
pub has_session_summaries: bool,  // Set by ExecutionEngine when session summaries exist
```

Default to `false` in the LayerInput construction.

- [ ] **Step 3: Register layer in mod.rs**

In `src/thinker/layers/mod.rs`, add `pub mod session_context_guide;` and register it in the layer stack where other layers are collected.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/session_context_guide.rs src/thinker/layers/mod.rs src/thinker/prompt_layer.rs
git commit -m "thinker: add SessionContextGuideLayer for compressed summary usage guidance"
```

---

## Task 10: Extend memory_search with Scope Parameter

**Files:**
- Modify: `src/builtin_tools/memory_search.rs:23-40` (MemorySearchArgs), `src/builtin_tools/memory_search.rs:189-192` (call_impl)

- [ ] **Step 1: Add `scope` field to MemorySearchArgs**

In `src/builtin_tools/memory_search.rs`, add to `MemorySearchArgs`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchArgs {
    /// Search query
    pub query: String,
    /// Maximum results to return
    pub max_results: Option<usize>,
    /// Workspace filter
    pub workspace: Option<String>,
    /// Multi-workspace filter
    pub workspaces: Option<Vec<String>>,
    /// Search across all workspaces
    pub cross_workspace: Option<bool>,
    /// Search scope: "all" (default), "current_session", or "both"
    #[serde(default = "default_scope")]
    pub scope: String,  // NEW
}

fn default_scope() -> String { "all".to_string() }
```

- [ ] **Step 2: Handle `current_session` scope in call_impl**

In `call_impl()`, before the existing search logic, add session-local search path:

```rust
if args.scope == "current_session" || args.scope == "both" {
    let session_filter = SearchFilter::new()
        .with_scope(MemoryScope::SessionLocal)
        .with_path_prefix(format!("aleph://session/{}/", current_session_id));
    // is_valid = None to include condensed d0s for detail retrieval

    let session_results = self.database.search_facts_by_filter(session_filter).await?;

    // Format results with depth and status annotations
    for fact in &session_results {
        let depth = extract_depth_from_path(&fact.path);
        let status = if fact.is_valid { "active" } else { "condensed" };
        // Add to output with annotation: [d0 | condensed] content...
    }
}

if args.scope == "all" || args.scope == "both" {
    // Existing search logic unchanged
}
```

Note: `current_session_id` — the tool needs access to the current session. This is available via the shared workspace handle pattern already used for `default_workspace`. Add a similar `default_session_key: Arc<RwLock<String>>` to `MemorySearchTool`.

- [ ] **Step 3: Add session_key handle to MemorySearchTool**

Add `default_session_key: Arc<RwLock<String>>` field, with a `set_session_key_handle()` method matching the existing `set_workspace_handle()` pattern. Wire it from `ExecutionEngine` the same way `default_workspace` is wired.

- [ ] **Step 4: Update tool description/schema**

Update the tool's JSON Schema description to mention the new `scope` parameter, so the LLM knows it can use `"current_session"` to search compressed history.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles.

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/memory_search.rs
git commit -m "memory_search: add scope parameter for current_session search"
```

---

## Task 11: Wire LLM Provider into SummaryEngine

**Files:**
- Modify: `src/memory/session_compactor/mod.rs` (SessionCompactor struct)
- Reference: `src/providers/adapter.rs` or similar (AiProvider trait)

- [ ] **Step 1: Add AiProvider to SessionCompactor**

Add `provider` field to `SessionCompactor`:

```rust
pub struct SessionCompactor {
    database: MemoryBackend,
    provider: Option<Arc<dyn AiProvider>>,  // For LLM summary calls
    config: SessionCompactorConfig,
}
```

- [ ] **Step 2: Implement LLM-based generate_summary with three-level fallback**

Replace the `generate_summary` method with actual LLM calls:

```rust
async fn generate_summary(
    &self,
    chunk: &[(String, String)],
    depth: u32,
    previous_context: Option<&str>,
    session_key: &str,
    agent_id: &str,
    seq: u32,
) -> Result<MemoryFact, AlephError> {
    let source_token_count: usize = chunk.iter()
        .map(|(_, c)| context_window::estimate_tokens(c, self.config.token_estimate_ratio))
        .sum();

    let summary_text = if let Some(ref provider) = self.provider {
        // Level 1: Normal
        let prompt = summary_engine::build_summary_prompt(chunk, depth, previous_context, FallbackLevel::Normal);
        match self.call_llm(provider, &prompt).await {
            Ok(text) if !text.is_empty() && estimate_tokens(&text, self.config.token_estimate_ratio) < source_token_count => text,
            _ => {
                // Level 2: Aggressive
                let prompt = summary_engine::build_summary_prompt(chunk, depth, previous_context, FallbackLevel::Aggressive);
                match self.call_llm(provider, &prompt).await {
                    Ok(text) if !text.is_empty() && estimate_tokens(&text, self.config.token_estimate_ratio) < source_token_count => text,
                    _ => {
                        // Level 3: Deterministic fallback
                        tracing::warn!("LLM summary fallback for session {}", session_key);
                        fallback::deterministic_truncate(chunk, 512)
                    }
                }
            }
        }
    } else {
        fallback::deterministic_truncate(chunk, 512)
    };

    Ok(summary_engine::summary_to_fact(
        session_key, depth, seq, summary_text,
        chunk.len(), source_token_count, agent_id,
    ))
}

async fn call_llm(&self, provider: &Arc<dyn AiProvider>, prompt: &str) -> Result<String, AlephError> {
    let messages = vec![UnifiedMessage::user(prompt.to_string())];
    let response = provider.process(&messages, "You are a precise summarizer. Output only the summary.", &[]).await?;
    Ok(response.text)
}
```

Note: Check the actual `AiProvider` trait signature. It may be `process()` or `call()` or `complete()`. Adapt to match. The key is using the same provider interface that `AgentLoop` uses.

- [ ] **Step 3: Wire provider in ExecutionEngine builder**

When constructing `SessionCompactor` in the server startup code, pass the AI provider:

```rust
let session_compactor = SessionCompactor::new(
    memory_backend.clone(),
    session_compactor_config,
).with_provider(provider.clone());
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_compactor/mod.rs src/gateway/execution_engine/engine.rs
git commit -m "session_compactor: wire LLM provider with three-level fallback for summary generation"
```

---

## Task 12: Server Startup Wiring

**Files:**
- Modify: `src/bin/aleph/server_init.rs` or wherever ExecutionEngine is constructed
- Reference: `src/bin/aleph/commands/start/` (server startup)

- [ ] **Step 1: Find server startup code**

Check `src/bin/aleph/server_init.rs` and `src/bin/aleph/commands/start/builder/subsystems.rs` for where `ExecutionEngine` is constructed and `with_compression_service()` is called. Add `with_session_compactor()` in the same location:

```rust
// After existing compression_service wiring:
let session_compactor = if config.memory.session_compactor.enabled {
    Some(Arc::new(SessionCompactor::new(
        memory_backend.clone(),
        config.memory.session_compactor.clone(),
    ).with_provider(ai_provider.clone())))
} else {
    None
};

engine = engine.with_session_compactor(session_compactor);
```

- [ ] **Step 2: Add SessionCompactorConfig to main config**

Add `session_compactor` field to the memory config struct (wherever `CompressionConfig` is defined), defaulting to `SessionCompactorConfig::default()`.

- [ ] **Step 3: Verify compilation and startup**

Run: `cargo check -p alephcore`
Then: `cargo build --bin aleph` (if different from core)
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph/ src/config/
git commit -m "server: wire SessionCompactor into server startup with config"
```

---

## Task 13: Integration Test

**Files:**
- Create: `tests/session_compactor_integration.rs` or add to existing test module

- [ ] **Step 1: Write integration test for full compression cycle**

```rust
// Test: messages → ToolCompactor → post_turn_compress → prepare_history retrieves summaries
#[tokio::test]
async fn test_session_compaction_full_cycle() {
    // 1. Create MemoryBackend (test instance)
    // 2. Create SessionCompactor with no LLM provider (deterministic fallback)
    // 3. Simulate 50 messages with tool results
    // 4. Call post_turn_compress
    // 5. Verify MemoryFacts created with scope=SessionLocal
    // 6. Call prepare_history
    // 7. Verify: summaries appear in returned messages, raw messages are fresh tail only
}
```

- [ ] **Step 2: Write integration test for condensation**

```rust
#[tokio::test]
async fn test_session_compaction_condensation() {
    // 1. Create 5+ d0 summaries (above d1_min_fanout threshold)
    // 2. Call post_turn_compress
    // 3. Verify: d1 summary created, d0 summaries invalidated
    // 4. Call prepare_history
    // 5. Verify: only d1 summary in messages, d0 not injected
}
```

- [ ] **Step 3: Write test for ToolCompactor in-loop behavior**

```rust
#[test]
fn test_tool_compactor_reduces_context() {
    // 1. Build messages vec with large tool results (total > threshold)
    // 2. Call compact_if_needed
    // 3. Verify: old tool results compressed, fresh tail untouched
    // 4. Verify: total tokens now under threshold
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test -p alephcore --lib session_compactor`
Run: `cargo test -p alephcore session_compactor_integration` (if separate test file)
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/
git commit -m "session_compactor: add integration tests for full compression cycle"
```

---

## Task 14: Cleanup and Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests still pass + new session_compactor tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings. Fix any clippy issues.

- [ ] **Step 3: Verify no unused imports or dead code**

Run: `cargo check -p alephcore 2>&1 | grep "warning"`
Expected: Minimal warnings (existing pre-known ones only).

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "session_compactor: cleanup and final verification"
```
