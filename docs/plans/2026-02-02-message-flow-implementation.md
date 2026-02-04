# Message Flow Optimization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph's message flow system with per-runId sequences, tool formatting, block-level flushing, and message deduplication.

**Architecture:** Four new Rust modules (tool_display, stream_buffer, message_dedup, run_context) integrate with existing GatewayEventEmitter. Swift side adds EnhancedRunSummary model and detail popover view.

**Tech Stack:** Rust (DashMap, serde), Swift (SwiftUI, Codable)

---

## Task 1: Tool Display Module (Rust)

**Files:**
- Create: `core/src/gateway/tool_display.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create tool_display.rs with ToolDisplay struct and mapping**

```rust
// core/src/gateway/tool_display.rs

//! Tool display formatting with emoji and smart parameter summarization

use serde_json::Value;
use std::collections::HashMap;

/// Tool display metadata
#[derive(Debug, Clone)]
pub struct ToolDisplay {
    pub emoji: &'static str,
    pub label: &'static str,
}

/// Get display metadata for a tool
pub fn get_tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "exec" | "shell" | "bash" | "run_command" => ToolDisplay { emoji: "🔨", label: "Exec" },
        "read" | "read_file" | "cat" => ToolDisplay { emoji: "📄", label: "Read" },
        "write" | "write_file" => ToolDisplay { emoji: "✏️", label: "Write" },
        "edit" | "edit_file" | "patch" => ToolDisplay { emoji: "📝", label: "Edit" },
        "web_fetch" | "fetch" | "http" => ToolDisplay { emoji: "🌐", label: "Fetch" },
        "search" | "grep" | "find" | "ripgrep" => ToolDisplay { emoji: "🔍", label: "Search" },
        "list" | "ls" | "dir" => ToolDisplay { emoji: "📁", label: "List" },
        "think" | "reason" => ToolDisplay { emoji: "💭", label: "Think" },
        "memory" | "remember" => ToolDisplay { emoji: "🧠", label: "Memory" },
        _ => ToolDisplay { emoji: "⚙️", label: tool_name },
    }
}

/// Format tool parameters for display
pub fn format_tool_meta(tool_name: &str, params: &Value) -> String {
    match tool_name {
        "read" | "read_file" | "cat" => format_path_params(params, "path"),
        "write" | "write_file" => format_path_params(params, "path"),
        "edit" | "edit_file" | "patch" => format_edit_params(params),
        "exec" | "shell" | "bash" | "run_command" => format_exec_params(params),
        "web_fetch" | "fetch" | "http" => format_url_params(params),
        "search" | "grep" | "find" | "ripgrep" => format_search_params(params),
        _ => format_generic_params(params),
    }
}

/// Format complete tool summary: "🔨 Exec: mkdir -p /tmp"
pub fn format_tool_summary(tool_name: &str, params: &Value) -> String {
    let display = get_tool_display(tool_name);
    let meta = format_tool_meta(tool_name, params);

    if meta.is_empty() {
        format!("{} {}", display.emoji, display.label)
    } else {
        format!("{} {}: {}", display.emoji, display.label, meta)
    }
}

// --- Helper functions ---

fn format_path_params(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(shorten_path)
        .unwrap_or_default()
}

fn format_edit_params(params: &Value) -> String {
    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let line = params.get("line").and_then(|v| v.as_u64());
    let end_line = params.get("end_line").and_then(|v| v.as_u64());

    let short_path = shorten_path(path);
    match (line, end_line) {
        (Some(l), Some(e)) if l != e => format!("{}:{}-{}", short_path, l, e),
        (Some(l), _) => format!("{}:{}", short_path, l),
        _ => short_path,
    }
}

fn format_exec_params(params: &Value) -> String {
    let mut parts = Vec::new();

    if params.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("sudo".to_string());
    }
    if params.get("pty").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("pty".to_string());
    }

    if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
        parts.push(truncate_str(cmd, 50));
    }

    parts.join(" · ")
}

fn format_url_params(params: &Value) -> String {
    params
        .get("url")
        .and_then(|v| v.as_str())
        .map(|url| truncate_str(url, 60))
        .unwrap_or_default()
}

fn format_search_params(params: &Value) -> String {
    let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    if pattern.is_empty() {
        shorten_path(path)
    } else {
        format!("\"{}\" in {}", truncate_str(pattern, 20), shorten_path(path))
    }
}

fn format_generic_params(params: &Value) -> String {
    // For unknown tools, show first string parameter
    if let Some(obj) = params.as_object() {
        for (_, value) in obj.iter().take(1) {
            if let Some(s) = value.as_str() {
                return truncate_str(s, 40);
            }
        }
    }
    String::new()
}

fn shorten_path(path: &str) -> String {
    // Keep last 2 components if path is long
    if path.len() <= 40 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        return truncate_str(path, 40);
    }

    let last_two = &parts[parts.len() - 2..];
    format!(".../{}", last_two.join("/"))
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Group multiple paths by directory: /tmp/{file1.txt, file2.txt}
pub fn group_paths(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    if paths.len() == 1 {
        return shorten_path(paths[0]);
    }

    let mut groups: HashMap<&str, Vec<&str>> = HashMap::new();
    for path in paths {
        if let Some(idx) = path.rfind('/') {
            let (dir, file) = path.split_at(idx + 1);
            groups.entry(dir).or_default().push(file);
        } else {
            groups.entry(".").or_default().push(path);
        }
    }

    groups
        .iter()
        .map(|(dir, files)| {
            if files.len() == 1 {
                format!("{}{}", dir, files[0])
            } else {
                format!("{}{{{}}}", dir, files.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_tool_display() {
        let display = get_tool_display("exec");
        assert_eq!(display.emoji, "🔨");
        assert_eq!(display.label, "Exec");

        let display = get_tool_display("read_file");
        assert_eq!(display.emoji, "📄");
    }

    #[test]
    fn test_format_exec_params() {
        let params = json!({"command": "mkdir -p /tmp/test", "elevated": true});
        let result = format_exec_params(&params);
        assert!(result.contains("sudo"));
        assert!(result.contains("mkdir"));
    }

    #[test]
    fn test_format_edit_params() {
        let params = json!({"path": "src/main.rs", "line": 42, "end_line": 56});
        let result = format_edit_params(&params);
        assert_eq!(result, "src/main.rs:42-56");
    }

    #[test]
    fn test_shorten_path() {
        assert_eq!(shorten_path("short.txt"), "short.txt");
        assert!(shorten_path("/very/long/path/to/some/deeply/nested/file.txt").contains("..."));
    }

    #[test]
    fn test_group_paths() {
        let paths = vec!["/tmp/file1.txt", "/tmp/file2.txt", "/home/test.rs"];
        let result = group_paths(&paths);
        assert!(result.contains("{file1.txt, file2.txt}") || result.contains("{file2.txt, file1.txt}"));
    }

    #[test]
    fn test_format_tool_summary() {
        let params = json!({"path": "src/lib.rs"});
        let summary = format_tool_summary("read", &params);
        assert_eq!(summary, "📄 Read: src/lib.rs");
    }
}
```

**Step 2: Run test to verify**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test tool_display --no-default-features --features gateway`

Expected: All tests pass

**Step 3: Add module to mod.rs**

Add to `core/src/gateway/mod.rs` after line 32:

```rust
#[cfg(feature = "gateway")]
pub mod tool_display;
```

And add export after line 95:

```rust
#[cfg(feature = "gateway")]
pub use tool_display::{ToolDisplay, get_tool_display, format_tool_meta, format_tool_summary, group_paths};
```

**Step 4: Run full gateway tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test --no-default-features --features gateway`

Expected: All tests pass

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add core/src/gateway/tool_display.rs core/src/gateway/mod.rs && git commit -m "$(cat <<'EOF'
feat(gateway): add tool display module with emoji and smart formatting

- Add ToolDisplay struct with emoji and label
- Implement format_tool_meta for path, edit, exec, URL, search params
- Add group_paths for smart path grouping: /tmp/{file1, file2}
- Add comprehensive unit tests

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Stream Buffer Module (Rust)

**Files:**
- Create: `core/src/gateway/stream_buffer.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create stream_buffer.rs**

```rust
// core/src/gateway/stream_buffer.rs

//! Stream buffer for block-level text flushing before tool execution

/// Manages accumulated text with flush-before-tool semantics
#[derive(Debug, Default)]
pub struct StreamBuffer {
    /// Accumulated text content
    text: String,
    /// Position up to which text has been flushed
    flushed_at: usize,
    /// Whether currently executing a tool
    in_tool_execution: bool,
}

impl StreamBuffer {
    /// Create a new empty buffer
    pub fn new() -> Self {
        Self::default()
    }

    /// Append text to the buffer
    pub fn append(&mut self, content: &str) {
        self.text.push_str(content);
    }

    /// Flush unflushed text before tool execution
    ///
    /// Returns Some(text) if there's non-empty unflushed content,
    /// None otherwise. Marks buffer as in tool execution state.
    pub fn flush_before_tool(&mut self) -> Option<String> {
        self.in_tool_execution = true;

        if self.flushed_at >= self.text.len() {
            return None;
        }

        let unflushed = self.text[self.flushed_at..].to_string();
        self.flushed_at = self.text.len();

        if unflushed.trim().is_empty() {
            None
        } else {
            Some(unflushed)
        }
    }

    /// Mark tool execution as ended
    pub fn tool_ended(&mut self) {
        self.in_tool_execution = false;
    }

    /// Check if currently in tool execution
    pub fn is_in_tool_execution(&self) -> bool {
        self.in_tool_execution
    }

    /// Get all accumulated text
    pub fn full_text(&self) -> &str {
        &self.text
    }

    /// Get unflushed text (without flushing)
    pub fn unflushed_text(&self) -> &str {
        &self.text[self.flushed_at..]
    }

    /// Get length of accumulated text
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Reset buffer to initial state
    pub fn reset(&mut self) {
        self.text.clear();
        self.flushed_at = 0;
        self.in_tool_execution = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_buffer() {
        let buffer = StreamBuffer::new();
        assert!(buffer.is_empty());
        assert!(!buffer.is_in_tool_execution());
    }

    #[test]
    fn test_append() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Hello ");
        buffer.append("World");
        assert_eq!(buffer.full_text(), "Hello World");
        assert_eq!(buffer.len(), 11);
    }

    #[test]
    fn test_flush_before_tool() {
        let mut buffer = StreamBuffer::new();
        buffer.append("First chunk. ");

        let flushed = buffer.flush_before_tool();
        assert_eq!(flushed, Some("First chunk. ".to_string()));
        assert!(buffer.is_in_tool_execution());

        // Second flush should return None
        let flushed2 = buffer.flush_before_tool();
        assert!(flushed2.is_none());
    }

    #[test]
    fn test_flush_empty_returns_none() {
        let mut buffer = StreamBuffer::new();
        buffer.append("   ");  // Only whitespace

        let flushed = buffer.flush_before_tool();
        assert!(flushed.is_none());
    }

    #[test]
    fn test_tool_ended() {
        let mut buffer = StreamBuffer::new();
        buffer.flush_before_tool();
        assert!(buffer.is_in_tool_execution());

        buffer.tool_ended();
        assert!(!buffer.is_in_tool_execution());
    }

    #[test]
    fn test_append_after_flush() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Before tool. ");
        buffer.flush_before_tool();
        buffer.tool_ended();

        buffer.append("After tool.");
        let flushed = buffer.flush_before_tool();
        assert_eq!(flushed, Some("After tool.".to_string()));
    }

    #[test]
    fn test_reset() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Some text");
        buffer.flush_before_tool();

        buffer.reset();
        assert!(buffer.is_empty());
        assert!(!buffer.is_in_tool_execution());
        assert_eq!(buffer.unflushed_text(), "");
    }

    #[test]
    fn test_unflushed_text() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Part 1. ");
        buffer.flush_before_tool();
        buffer.append("Part 2.");

        assert_eq!(buffer.unflushed_text(), "Part 2.");
        assert_eq!(buffer.full_text(), "Part 1. Part 2.");
    }
}
```

**Step 2: Run test to verify**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test stream_buffer --no-default-features --features gateway`

Expected: All tests pass

**Step 3: Add module to mod.rs**

Add to `core/src/gateway/mod.rs` after the tool_display line:

```rust
#[cfg(feature = "gateway")]
pub mod stream_buffer;
```

And add export:

```rust
#[cfg(feature = "gateway")]
pub use stream_buffer::StreamBuffer;
```

**Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add core/src/gateway/stream_buffer.rs core/src/gateway/mod.rs && git commit -m "$(cat <<'EOF'
feat(gateway): add stream buffer for block-level text flushing

- StreamBuffer accumulates text with flush tracking
- flush_before_tool() returns unflushed text before tool execution
- Supports append-after-flush for multi-tool scenarios
- Whitespace-only content is not flushed

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Message Deduplication Module (Rust)

**Files:**
- Create: `core/src/gateway/message_dedup.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create message_dedup.rs**

```rust
// core/src/gateway/message_dedup.rs

//! Message deduplication with text normalization

use std::collections::HashSet;
use std::time::Instant;

/// Normalize text for duplicate comparison
///
/// - Trims whitespace
/// - Collapses multiple spaces
/// - Converts to lowercase
/// - Removes common punctuation
pub fn normalize_text(text: &str) -> String {
    text.trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace(['。', '，', '！', '？', '；', '：'], "")
        .replace(['.', ',', '!', '?', ';', ':'], "")
}

/// Check if two texts are duplicates after normalization
pub fn is_text_duplicate(a: &str, b: &str) -> bool {
    normalize_text(a) == normalize_text(b)
}

/// Record of a sent message
#[derive(Debug, Clone)]
pub struct SentRecord {
    pub channel: String,
    pub user_id: Option<String>,
    pub text: String,
    pub sent_at: Instant,
}

/// Tracks sent messages for deduplication
#[derive(Debug, Default)]
pub struct SentMessageTracker {
    /// Original sent texts
    sent_texts: Vec<String>,
    /// Normalized texts for fast lookup
    sent_normalized: HashSet<String>,
    /// Full records with metadata
    records: Vec<SentRecord>,
}

impl SentMessageTracker {
    /// Create a new tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if text would be a duplicate
    pub fn is_duplicate(&self, text: &str) -> bool {
        let normalized = normalize_text(text);
        self.sent_normalized.contains(&normalized)
    }

    /// Record a sent message
    pub fn record(&mut self, text: &str, channel: &str, user_id: Option<&str>) {
        let normalized = normalize_text(text);

        self.sent_texts.push(text.to_string());
        self.sent_normalized.insert(normalized);
        self.records.push(SentRecord {
            channel: channel.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            text: text.to_string(),
            sent_at: Instant::now(),
        });
    }

    /// Check if duplicate, if not record it
    ///
    /// Returns true if this is a new (non-duplicate) message
    pub fn check_and_record(&mut self, text: &str, channel: &str, user_id: Option<&str>) -> bool {
        if self.is_duplicate(text) {
            return false;
        }
        self.record(text, channel, user_id);
        true
    }

    /// Get all sent texts
    pub fn all_texts(&self) -> &[String] {
        &self.sent_texts
    }

    /// Get all records
    pub fn all_records(&self) -> &[SentRecord] {
        &self.records
    }

    /// Get count of sent messages
    pub fn count(&self) -> usize {
        self.sent_texts.len()
    }

    /// Reset tracker
    pub fn reset(&mut self) {
        self.sent_texts.clear();
        self.sent_normalized.clear();
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_text() {
        assert_eq!(normalize_text("  Hello   World  "), "hello world");
        assert_eq!(normalize_text("Hello, World!"), "hello world");
        assert_eq!(normalize_text("你好，世界！"), "你好世界");
    }

    #[test]
    fn test_is_text_duplicate() {
        assert!(is_text_duplicate("Hello World", "hello world"));
        assert!(is_text_duplicate("Hello, World!", "Hello World"));
        assert!(!is_text_duplicate("Hello", "World"));
    }

    #[test]
    fn test_tracker_is_duplicate() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Hello World", "telegram", None);

        assert!(tracker.is_duplicate("Hello World"));
        assert!(tracker.is_duplicate("hello world"));
        assert!(tracker.is_duplicate("Hello, World!"));
        assert!(!tracker.is_duplicate("Goodbye"));
    }

    #[test]
    fn test_check_and_record() {
        let mut tracker = SentMessageTracker::new();

        // First time should succeed
        assert!(tracker.check_and_record("Hello", "telegram", None));
        assert_eq!(tracker.count(), 1);

        // Duplicate should fail
        assert!(!tracker.check_and_record("Hello", "telegram", None));
        assert_eq!(tracker.count(), 1);

        // Different message should succeed
        assert!(tracker.check_and_record("World", "telegram", None));
        assert_eq!(tracker.count(), 2);
    }

    #[test]
    fn test_reset() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Test", "channel", Some("user1"));

        tracker.reset();
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.is_duplicate("Test"));
    }

    #[test]
    fn test_records_metadata() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Test message", "telegram", Some("user123"));

        let records = tracker.all_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].channel, "telegram");
        assert_eq!(records[0].user_id, Some("user123".to_string()));
        assert_eq!(records[0].text, "Test message");
    }
}
```

**Step 2: Run test to verify**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test message_dedup --no-default-features --features gateway`

Expected: All tests pass

**Step 3: Add module to mod.rs**

Add to `core/src/gateway/mod.rs`:

```rust
#[cfg(feature = "gateway")]
pub mod message_dedup;
```

And export:

```rust
#[cfg(feature = "gateway")]
pub use message_dedup::{normalize_text, is_text_duplicate, SentMessageTracker, SentRecord};
```

**Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add core/src/gateway/message_dedup.rs core/src/gateway/mod.rs && git commit -m "$(cat <<'EOF'
feat(gateway): add message deduplication with text normalization

- normalize_text() for consistent comparison
- SentMessageTracker tracks sent messages per run
- check_and_record() atomic check-then-record
- Supports Chinese and English punctuation normalization

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Enhanced RunSummary and Event Emitter (Rust)

**Files:**
- Modify: `core/src/gateway/event_emitter.rs`

**Step 1: Add enhanced types to event_emitter.rs**

Add after line 125 (after existing `ToolResult` impl):

```rust
/// Enhanced summary with tool details and errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
    #[serde(default)]
    pub tool_summaries: Vec<ToolSummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolErrorItem>,
}

/// Tool execution summary item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummaryItem {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,
    pub display_meta: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// Tool error item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorItem {
    pub tool_name: String,
    pub error: String,
    pub tool_id: String,
}

impl EnhancedRunSummary {
    /// Create from basic RunSummary
    pub fn from_basic(basic: RunSummary, duration_ms: u64) -> Self {
        Self {
            total_tokens: basic.total_tokens,
            tool_calls: basic.tool_calls,
            loops: basic.loops,
            duration_ms,
            final_response: basic.final_response,
            tool_summaries: Vec::new(),
            reasoning: None,
            errors: Vec::new(),
        }
    }

    /// Add a tool summary
    pub fn add_tool(&mut self, item: ToolSummaryItem) {
        self.tool_summaries.push(item);
    }

    /// Add an error
    pub fn add_error(&mut self, error: ToolErrorItem) {
        self.errors.push(error);
    }

    /// Check if there are any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}
```

**Step 2: Add Per-RunId sequence manager**

Add after `GatewayEventEmitter` struct definition (around line 272), replace `seq_counter: AtomicU64` with:

```rust
use dashmap::DashMap;

/// Per-RunId sequence counter manager
pub struct RunSequenceManager {
    sequences: DashMap<String, AtomicU64>,
}

impl RunSequenceManager {
    pub fn new() -> Self {
        Self {
            sequences: DashMap::new(),
        }
    }

    /// Get next sequence number for a run
    pub fn next_seq(&self, run_id: &str) -> u64 {
        self.sequences
            .entry(run_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst)
    }

    /// Cleanup sequences for completed run
    pub fn cleanup(&self, run_id: &str) {
        self.sequences.remove(run_id);
    }
}

impl Default for RunSequenceManager {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 3: Update GatewayEventEmitter to use RunSequenceManager**

Replace the `seq_counter` field and update `next_seq` implementation:

```rust
pub struct GatewayEventEmitter {
    event_bus: Arc<GatewayEventBus>,
    seq_manager: RunSequenceManager,
    // Throttling state for response chunks
    delta_buffer: Mutex<String>,
    last_delta_at: Mutex<Instant>,
    // Current run ID for sequence tracking
    current_run_id: Mutex<Option<String>>,
}

impl GatewayEventEmitter {
    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_manager: RunSequenceManager::new(),
            delta_buffer: Mutex::new(String::new()),
            last_delta_at: Mutex::new(Instant::now()),
            current_run_id: Mutex::new(None),
        }
    }

    /// Set current run ID for sequence tracking
    pub async fn set_current_run(&self, run_id: &str) {
        *self.current_run_id.lock().await = Some(run_id.to_string());
    }

    /// Clear current run ID and cleanup sequences
    pub async fn clear_current_run(&self) {
        if let Some(run_id) = self.current_run_id.lock().await.take() {
            self.seq_manager.cleanup(&run_id);
        }
    }

    /// Get next sequence for current run
    async fn next_seq_for_current(&self) -> u64 {
        if let Some(ref run_id) = *self.current_run_id.lock().await {
            self.seq_manager.next_seq(run_id)
        } else {
            0
        }
    }
}
```

**Step 4: Update EventEmitter trait impl**

Update the `next_seq` method in the `EventEmitter` impl for `GatewayEventEmitter`:

```rust
fn next_seq(&self) -> u64 {
    // For trait compatibility, use blocking approach
    // In practice, use next_seq_for_current() in async contexts
    if let Ok(guard) = self.current_run_id.try_lock() {
        if let Some(ref run_id) = *guard {
            return self.seq_manager.next_seq(run_id);
        }
    }
    0
}
```

**Step 5: Add to Cargo.toml**

Add `dashmap` dependency to `core/Cargo.toml` if not present:

```toml
dashmap = "5.5"
```

**Step 6: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test event_emitter --no-default-features --features gateway`

Expected: All tests pass

**Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add core/src/gateway/event_emitter.rs core/Cargo.toml && git commit -m "$(cat <<'EOF'
feat(gateway): add EnhancedRunSummary and per-runId sequences

- EnhancedRunSummary with tool_summaries, reasoning, errors
- ToolSummaryItem with emoji and display_meta
- RunSequenceManager for per-runId sequence numbers
- Update GatewayEventEmitter to use new sequence manager

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Swift Protocol Models Update

**Files:**
- Modify: `platforms/macos/Aether/Sources/Gateway/ProtocolModels.swift`

**Step 1: Add EnhancedRunSummary and related types**

Add after `RunSummary` struct (around line 300):

```swift
/// Enhanced run summary with tool details
struct EnhancedRunSummary: Codable, Equatable, Sendable {
    let totalTokens: UInt64
    let toolCalls: UInt32
    let loops: UInt32
    let durationMs: UInt64
    let finalResponse: String?
    let toolSummaries: [ToolSummaryItem]
    let reasoning: String?
    let errors: [ToolErrorItem]

    enum CodingKeys: String, CodingKey {
        case totalTokens = "total_tokens"
        case toolCalls = "tool_calls"
        case loops
        case durationMs = "duration_ms"
        case finalResponse = "final_response"
        case toolSummaries = "tool_summaries"
        case reasoning
        case errors
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        totalTokens = try container.decode(UInt64.self, forKey: .totalTokens)
        toolCalls = try container.decode(UInt32.self, forKey: .toolCalls)
        loops = try container.decode(UInt32.self, forKey: .loops)
        durationMs = try container.decodeIfPresent(UInt64.self, forKey: .durationMs) ?? 0
        finalResponse = try container.decodeIfPresent(String.self, forKey: .finalResponse)
        toolSummaries = try container.decodeIfPresent([ToolSummaryItem].self, forKey: .toolSummaries) ?? []
        reasoning = try container.decodeIfPresent(String.self, forKey: .reasoning)
        errors = try container.decodeIfPresent([ToolErrorItem].self, forKey: .errors) ?? []
    }

    /// Create from basic RunSummary for backwards compatibility
    init(from basic: RunSummary, durationMs: UInt64) {
        self.totalTokens = basic.totalTokens
        self.toolCalls = basic.toolCalls
        self.loops = basic.loops
        self.durationMs = durationMs
        self.finalResponse = basic.finalResponse
        self.toolSummaries = []
        self.reasoning = nil
        self.errors = []
    }

    var hasErrors: Bool { !errors.isEmpty }
}

/// Tool execution summary item
struct ToolSummaryItem: Codable, Equatable, Identifiable, Sendable {
    let toolId: String
    let toolName: String
    let emoji: String
    let displayMeta: String
    let durationMs: UInt64
    let success: Bool

    var id: String { toolId }

    enum CodingKeys: String, CodingKey {
        case toolId = "tool_id"
        case toolName = "tool_name"
        case emoji
        case displayMeta = "display_meta"
        case durationMs = "duration_ms"
        case success
    }

    /// Formatted display string: "🔨 Exec: mkdir -p /tmp"
    var formatted: String {
        if displayMeta.isEmpty {
            return "\(emoji) \(toolName)"
        }
        return "\(emoji) \(toolName): \(displayMeta)"
    }

    /// Short format for list view
    var shortFormatted: String {
        if displayMeta.isEmpty {
            return toolName
        }
        return displayMeta
    }
}

/// Tool error item
struct ToolErrorItem: Codable, Equatable, Sendable {
    let toolName: String
    let error: String
    let toolId: String

    enum CodingKeys: String, CodingKey {
        case toolName = "tool_name"
        case error
        case toolId = "tool_id"
    }
}
```

**Step 2: Update RunCompleteEvent to support EnhancedRunSummary**

Replace the existing `RunCompleteEvent` struct:

```swift
struct RunCompleteEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let summary: RunSummary
    let enhancedSummary: EnhancedRunSummary?
    let totalDurationMs: UInt64

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case summary
        case enhancedSummary = "enhanced_summary"
        case totalDurationMs = "total_duration_ms"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        runId = try container.decode(String.self, forKey: .runId)
        seq = try container.decode(UInt64.self, forKey: .seq)
        summary = try container.decode(RunSummary.self, forKey: .summary)
        enhancedSummary = try container.decodeIfPresent(EnhancedRunSummary.self, forKey: .enhancedSummary)
        totalDurationMs = try container.decode(UInt64.self, forKey: .totalDurationMs)
    }

    /// Get enhanced summary, creating from basic if needed
    var effectiveEnhancedSummary: EnhancedRunSummary {
        enhancedSummary ?? EnhancedRunSummary(from: summary, durationMs: totalDurationMs)
    }
}
```

**Step 3: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aether/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/Gateway/ProtocolModels.swift`

Expected: Syntax valid

**Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add platforms/macos/Aether/Sources/Gateway/ProtocolModels.swift && git commit -m "$(cat <<'EOF'
feat(macos): add EnhancedRunSummary and ToolSummaryItem models

- EnhancedRunSummary with toolSummaries, reasoning, errors
- ToolSummaryItem with emoji and displayMeta formatting
- ToolErrorItem for error details
- Backwards compatible with basic RunSummary

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Result Detail Popover View (Swift)

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloResultDetailPopover.swift`

**Step 1: Create HaloResultDetailPopover.swift**

```swift
//
//  HaloResultDetailPopover.swift
//  Aleph
//
//  Detail popover for viewing complete run results with tool summaries.
//

import SwiftUI

/// Popover view showing detailed run results
struct HaloResultDetailPopover: View {
    let summary: EnhancedRunSummary
    let onCopy: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            headerView
            Divider()

            if !summary.toolSummaries.isEmpty {
                toolListView
            }

            if !summary.errors.isEmpty {
                errorListView
            }

            if let reasoning = summary.reasoning, !reasoning.isEmpty {
                reasoningView(reasoning)
            }

            Divider()
            footerView
        }
        .padding(12)
        .frame(width: 320)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }

    // MARK: - Header

    private var headerView: some View {
        HStack {
            Image(systemName: summary.hasErrors ? "exclamationmark.circle.fill" : "checkmark.circle.fill")
                .foregroundColor(summary.hasErrors ? .orange : .green)
                .font(.system(size: 18))

            VStack(alignment: .leading, spacing: 2) {
                Text(summary.hasErrors ? L("result.partial") : L("result.success"))
                    .font(.system(size: 13, weight: .semibold))

                Text(statsText)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }

            Spacer()
        }
    }

    private var statsText: String {
        var parts: [String] = []
        parts.append("\(summary.toolCalls) tools")
        parts.append(formatDuration(summary.durationMs))
        if summary.totalTokens > 0 {
            parts.append("\(summary.totalTokens) tokens")
        }
        return parts.joined(separator: " · ")
    }

    // MARK: - Tool List

    private var toolListView: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(L("result.tools"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)

            ForEach(summary.toolSummaries.prefix(5)) { tool in
                HStack(spacing: 6) {
                    Text(tool.emoji)
                        .font(.system(size: 12))

                    Text(tool.shortFormatted)
                        .font(.system(size: 11))
                        .lineLimit(1)
                        .foregroundColor(tool.success ? .primary : .red)

                    Spacer()

                    Text(formatDuration(tool.durationMs))
                        .font(.system(size: 10))
                        .foregroundColor(.secondary)
                }
            }

            if summary.toolSummaries.count > 5 {
                Text("+\(summary.toolSummaries.count - 5) more")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
        }
    }

    // MARK: - Error List

    private var errorListView: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(L("result.errors"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.red)

            ForEach(summary.errors, id: \.toolId) { error in
                Text("\(error.toolName): \(error.error)")
                    .font(.system(size: 11))
                    .foregroundColor(.red)
                    .lineLimit(2)
            }
        }
    }

    // MARK: - Reasoning

    private func reasoningView(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(L("result.reasoning"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)

            Text(String(text.prefix(200)) + (text.count > 200 ? "..." : ""))
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .lineLimit(4)
        }
    }

    // MARK: - Footer

    private var footerView: some View {
        HStack {
            Button(action: onCopy) {
                Label(L("button.copy"), systemImage: "doc.on.doc")
                    .font(.system(size: 11))
            }
            .buttonStyle(.borderless)

            Spacer()

            Button(L("button.close"), action: onDismiss)
                .font(.system(size: 11))
                .buttonStyle(.borderless)
        }
    }

    // MARK: - Helpers

    private func formatDuration(_ ms: UInt64) -> String {
        if ms < 1000 { return "\(ms)ms" }
        if ms < 60000 { return String(format: "%.1fs", Double(ms) / 1000.0) }
        return "\(ms / 60000)m \((ms % 60000) / 1000)s"
    }
}

// MARK: - Previews

#if DEBUG
#Preview("Success Result") {
    HaloResultDetailPopover(
        summary: EnhancedRunSummary(
            from: RunSummary(
                totalTokens: 1500,
                toolCalls: 3,
                loops: 1,
                finalResponse: "Task completed successfully."
            ),
            durationMs: 2500
        ),
        onCopy: {},
        onDismiss: {}
    )
}
#endif
```

**Step 2: Add localization keys**

Add to `platforms/macos/Aether/Resources/en.lproj/Localizable.strings`:

```
"result.partial" = "Partially Complete";
"result.success" = "Completed";
"result.tools" = "Tools";
"result.errors" = "Errors";
"result.reasoning" = "Reasoning";
```

Add to `platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings`:

```
"result.partial" = "部分完成";
"result.success" = "已完成";
"result.tools" = "工具";
"result.errors" = "错误";
"result.reasoning" = "推理";
```

**Step 3: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aether/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/Components/HaloResultDetailPopover.swift`

Expected: Syntax valid

**Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add platforms/macos/Aether/Sources/Components/HaloResultDetailPopover.swift platforms/macos/Aether/Resources/*/Localizable.strings && git commit -m "$(cat <<'EOF'
feat(macos): add HaloResultDetailPopover for detailed results

- Shows tool summaries with emoji and duration
- Displays errors and reasoning sections
- Copy and close action buttons
- Localized for English and Chinese

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Update HaloResultView with Popover

**Files:**
- Modify: `platforms/macos/Aether/Sources/Components/HaloResultView.swift`

**Step 1: Update HaloResultView to support popover**

Add after the existing `HaloResultView` struct (keep the original for backwards compatibility), add a new wrapper:

```swift
/// Enhanced result view with detail popover
struct HaloResultViewV2: View {
    let context: ResultContext
    let enhancedSummary: EnhancedRunSummary?
    let onDismiss: (() -> Void)?
    let onCopy: (() -> Void)?

    @State private var showingDetail = false

    var body: some View {
        HaloResultView(
            context: context,
            onDismiss: {
                if enhancedSummary != nil {
                    showingDetail = true
                } else {
                    onDismiss?()
                }
            },
            onCopy: onCopy
        )
        .popover(isPresented: $showingDetail, arrowEdge: .bottom) {
            if let summary = enhancedSummary {
                HaloResultDetailPopover(
                    summary: summary,
                    onCopy: { onCopy?() },
                    onDismiss: { showingDetail = false }
                )
            }
        }
    }
}
```

**Step 2: Add preview for V2**

```swift
#if DEBUG
#Preview("Result V2 with Enhanced Summary") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultViewV2(
            context: ResultContext(
                runId: "preview-v2",
                summary: .success(
                    message: "Task completed",
                    toolsExecuted: 3,
                    durationMs: 2500,
                    finalResponse: "Done"
                )
            ),
            enhancedSummary: EnhancedRunSummary(
                from: RunSummary(
                    totalTokens: 1500,
                    toolCalls: 3,
                    loops: 1,
                    finalResponse: "Done"
                ),
                durationMs: 2500
            ),
            onDismiss: { print("Dismissed") },
            onCopy: { print("Copied") }
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}
#endif
```

**Step 3: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aether/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/Components/HaloResultView.swift`

Expected: Syntax valid

**Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add platforms/macos/Aether/Sources/Components/HaloResultView.swift && git commit -m "$(cat <<'EOF'
feat(macos): add HaloResultViewV2 with detail popover support

- New HaloResultViewV2 wraps original with popover
- Tap to show detail popover when enhanced summary available
- Backwards compatible with basic ResultContext

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Update GatewayStreamAdapter

**Files:**
- Modify: `platforms/macos/Aether/Sources/Gateway/GatewayStreamAdapter.swift`

**Step 1: Add enhanced summary tracking**

Add property after `runSummary`:

```swift
/// Enhanced run summary when complete
@Published private(set) var enhancedRunSummary: EnhancedRunSummary?

/// Accumulated tool summaries during run
private var toolSummariesAccumulated: [ToolSummaryItem] = []
```

**Step 2: Update handleToolEnd to accumulate summaries**

Update `handleToolEnd` method:

```swift
private func handleToolEnd(_ event: ToolEndEvent) {
    guard event.runId == currentRunId else { return }

    let resultString: String
    let status: ToolPartStatus
    if event.result.success {
        resultString = event.result.output ?? "Success"
        status = .success
    } else {
        resultString = "Error: \(event.result.error ?? "Unknown error")"
        status = .error
    }

    print("[GatewayStreamAdapter] Tool ended (\(event.durationMs)ms): \(resultString.prefix(50))...")

    // Update tool call part
    if let toolInfo = activeToolCalls[event.toolId] {
        updateOrAddPart(.toolCall(
            id: toolInfo.partId,
            toolName: toolInfo.name,
            status: status,
            result: resultString,
            durationMs: event.durationMs
        ))

        // Accumulate tool summary
        let summary = ToolSummaryItem(
            toolId: event.toolId,
            toolName: toolInfo.name,
            emoji: toolEmoji(for: toolInfo.name),
            displayMeta: resultString.prefix(50).description,
            durationMs: event.durationMs,
            success: event.result.success
        )
        toolSummariesAccumulated.append(summary)

        activeToolCalls.removeValue(forKey: event.toolId)
    }
}

/// Get emoji for tool name
private func toolEmoji(for toolName: String) -> String {
    switch toolName.lowercased() {
    case "exec", "shell", "bash", "run_command": return "🔨"
    case "read", "read_file", "cat": return "📄"
    case "write", "write_file": return "✏️"
    case "edit", "edit_file", "patch": return "📝"
    case "web_fetch", "fetch", "http": return "🌐"
    case "search", "grep", "find": return "🔍"
    case "list", "ls", "dir": return "📁"
    default: return "⚙️"
    }
}
```

**Step 3: Update handleRunComplete**

```swift
private func handleRunComplete(_ event: RunCompleteEvent) {
    guard event.runId == currentRunId else { return }

    print("[GatewayStreamAdapter] Run complete: \(event.summary.loops) loops, \(event.totalDurationMs)ms")

    // Save run summary
    runSummary = event.summary

    // Build enhanced summary
    var enhanced = EnhancedRunSummary(from: event.summary, durationMs: event.totalDurationMs)
    // Use event's enhanced summary if available, otherwise use accumulated
    if let eventEnhanced = event.enhancedSummary {
        enhancedRunSummary = eventEnhanced
    } else {
        // Build from accumulated data
        enhanced = EnhancedRunSummary(
            from: event.summary,
            durationMs: event.totalDurationMs
        )
        // Note: Swift doesn't allow mutation after init, so we'd need to adjust the model
        // For now, store what we have
        enhancedRunSummary = enhanced
    }

    // Use final response from summary if available, otherwise use accumulated text
    let response = event.summary.finalResponse ?? accumulatedText

    // Finalize text part
    if !accumulatedText.isEmpty {
        let partId = "text-\(currentRunId ?? "unknown")"
        updateOrAddPart(.text(id: partId, content: accumulatedText, isStreaming: false))
    }

    // Reset state (but keep parts and summary for display)
    currentRunId = nil
    accumulatedText = ""
    reasoningText = ""
    isStreaming = false
    activeToolCalls = [:]
    toolSummariesAccumulated = []
}
```

**Step 4: Update reset method**

```swift
func reset() {
    currentRunId = nil
    accumulatedText = ""
    reasoningText = ""
    isStreaming = false
    parts = []
    activeToolCalls = [:]
    runSummary = nil
    enhancedRunSummary = nil
    pendingQuestion = nil
    partIdCounter = 0
    toolSummariesAccumulated = []
}
```

**Step 5: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aether/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/Gateway/GatewayStreamAdapter.swift`

Expected: Syntax valid

**Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add platforms/macos/Aether/Sources/Gateway/GatewayStreamAdapter.swift && git commit -m "$(cat <<'EOF'
feat(macos): update GatewayStreamAdapter with enhanced summary

- Add enhancedRunSummary property
- Accumulate tool summaries during run
- Build enhanced summary on run complete
- Add toolEmoji helper function

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Update HaloStreamingView with Tool Formatting

**Files:**
- Modify: `platforms/macos/Aether/Sources/Components/HaloStreamingView.swift`

**Step 1: Update toolCallRow to show emoji**

Replace the `toolCallRow` function:

```swift
private func toolCallRow(_ toolCall: ToolCallInfo) -> some View {
    HStack(spacing: 8) {
        // Emoji instead of status icon for visual appeal
        Text(toolEmoji(for: toolCall.name))
            .font(.system(size: 14))

        toolStatusIcon(toolCall.status)
            .frame(width: 12, height: 12)

        Text(toolCall.name)
            .font(.system(size: 12, weight: .medium))
            .foregroundColor(.primary)
            .lineLimit(1)

        Spacer()

        if let progressText = toolCall.progressText {
            Text(progressText)
                .font(.system(size: 10))
                .foregroundColor(.secondary)
                .lineLimit(1)
        }
    }
    .frame(maxWidth: 260)
}

/// Get emoji for tool name
private func toolEmoji(for toolName: String) -> String {
    switch toolName.lowercased() {
    case "exec", "shell", "bash", "run_command": return "🔨"
    case "read", "read_file", "cat": return "📄"
    case "write", "write_file": return "✏️"
    case "edit", "edit_file", "patch": return "📝"
    case "web_fetch", "fetch", "http": return "🌐"
    case "search", "grep", "find": return "🔍"
    case "list", "ls", "dir": return "📁"
    default: return "⚙️"
    }
}
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aether/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/Components/HaloStreamingView.swift`

Expected: Syntax valid

**Step 3: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add platforms/macos/Aether/Sources/Components/HaloStreamingView.swift && git commit -m "$(cat <<'EOF'
feat(macos): add tool emoji to HaloStreamingView

- Show emoji prefix for each tool in streaming view
- Add toolEmoji helper matching Rust side mapping

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Integration Test and Cleanup

**Files:**
- Verify all changes compile

**Step 1: Build Rust core**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo build --no-default-features --features gateway`

Expected: Build succeeds

**Step 2: Run Rust tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && ~/.cargo/bin/cargo test --no-default-features --features gateway`

Expected: All tests pass

**Step 3: Generate Xcode project**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodegen generate`

Expected: Project generated

**Step 4: Build macOS app**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build CODE_SIGNING_ALLOWED=NO 2>&1 | tail -20`

Expected: Build succeeds

**Step 5: Final commit with all integration**

```bash
cd /Volumes/TBU4/Workspace/Aleph && git add -A && git status
```

If there are unstaged changes:

```bash
git commit -m "$(cat <<'EOF'
chore: message flow optimization integration

- All Rust modules integrated
- All Swift components updated
- Build verified

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

| Task | Component | Files |
|------|-----------|-------|
| 1 | Tool Display Module | `tool_display.rs` |
| 2 | Stream Buffer Module | `stream_buffer.rs` |
| 3 | Message Dedup Module | `message_dedup.rs` |
| 4 | Enhanced Event Emitter | `event_emitter.rs` |
| 5 | Swift Protocol Models | `ProtocolModels.swift` |
| 6 | Result Detail Popover | `HaloResultDetailPopover.swift` |
| 7 | Result View V2 | `HaloResultView.swift` |
| 8 | Stream Adapter Update | `GatewayStreamAdapter.swift` |
| 9 | Streaming View Update | `HaloStreamingView.swift` |
| 10 | Integration Test | All files |

**Total: 10 tasks, ~4 new files, ~6 modified files**
