//! Tool call sequence compaction.
//!
//! Collapses verbose tool-use / tool-result message pairs into compact
//! summaries, reducing token usage while preserving semantic content.
//!
//! This is a purely deterministic, zero-LLM-call compaction layer that runs
//! synchronously before each LLM invocation.

use crate::providers::message::UnifiedMessage;

use super::context_window::{
    estimate_tokens, estimate_total_tokens, is_tool_result_consumed, partition_fresh_tail,
};

// ---------------------------------------------------------------------------
// Language detection heuristic
// ---------------------------------------------------------------------------

/// Guess a file's language from its content using simple heuristics.
///
/// Returns a short label like `"rust"`, `"python"`, `"json"`, etc.
fn detect_language(content: &str) -> &'static str {
    let trimmed = content.trim_start();

    // JSON
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return "json";
    }
    // Python
    if trimmed.contains("def ") || trimmed.contains("import ") && !trimmed.contains("use ") {
        return "python";
    }
    // Rust
    if trimmed.contains("fn ") || trimmed.contains("use ") || trimmed.contains("impl ") {
        return "rust";
    }
    // TypeScript / JavaScript
    if trimmed.contains("const ") || trimmed.contains("function ") || trimmed.contains("export ") {
        return "typescript";
    }
    // Shell / Bash
    if trimmed.starts_with('#') || trimmed.starts_with("#!/") {
        return "shell";
    }
    // TOML / INI
    if trimmed.contains(" = ") && !trimmed.contains(":") {
        return "toml";
    }
    // YAML
    if trimmed.contains(": ") {
        return "yaml";
    }

    "text"
}

// ---------------------------------------------------------------------------
// Per-type compressors
// ---------------------------------------------------------------------------

/// Compress the output of a file-read tool (`Read`, `Glob`, `read_file`).
fn compress_file_read(content: &str) -> String {
    let lines = content.lines().count();
    let lang = detect_language(content);
    format!("[Read file, {lines} lines, {lang}]")
}

/// Compress the output of a search tool (`Grep`, `Search`, `grep`).
fn compress_search(content: &str) -> String {
    // Count non-empty result lines as "matching lines".
    let n = content.lines().filter(|l| !l.trim().is_empty()).count();
    format!("[Search result, {n} matching lines]")
}

/// Compress the output of a shell/bash tool (`Bash`, `bash`, `shell`).
fn compress_bash(content: &str) -> String {
    let n = content.lines().count();
    format!("[Command output, {n} lines]")
}

/// Compress a web-fetch result — keep first 200 chars.
fn compress_web(content: &str) -> String {
    let head = safe_truncate(content, 200);
    if content.len() > 200 {
        format!(
            "{head}… [truncated, full content was {} chars]",
            content.len()
        )
    } else {
        head.to_owned()
    }
}

/// Generic compressor for unknown tools when content exceeds 500 tokens.
fn compress_generic(content: &str) -> String {
    let head = safe_truncate(content, 200);
    format!(
        "{head}… [truncated, full content was {} chars]",
        content.len()
    )
}

/// Truncate `s` to at most `n` *bytes* at a valid UTF-8 char boundary.
fn safe_truncate(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    // Walk back from byte `n` to find a valid char boundary.
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compress a single tool result by dispatching to the appropriate compressor.
///
/// `tool_name` is matched case-insensitively against known tool families.
/// The returned string is the compact summary that replaces the original.
pub fn compress_tool_result(tool_name: &str, content: &str) -> String {
    let name = tool_name.to_ascii_lowercase();

    if name.contains("read") || name.contains("glob") {
        compress_file_read(content)
    } else if name.contains("grep") || name.contains("search") {
        compress_search(content)
    } else if name.contains("bash") || name.contains("shell") {
        compress_bash(content)
    } else if name.contains("webfetch") || name.contains("web_fetch") || name.contains("fetch") {
        compress_web(content)
    } else {
        // Generic: only compress if content is large (> 500 estimated tokens at ratio 3.5)
        let tok = estimate_tokens(content, 3.5);
        if tok > 500 {
            compress_generic(content)
        } else {
            content.to_owned()
        }
    }
}

/// Compress tool results in-place to keep the message list within a token budget.
///
/// # Parameters
/// - `messages`         – mutable slice of the current conversation
/// - `token_budget`     – hard limit in tokens (e.g. model context window)
/// - `threshold`        – fraction of `token_budget` that triggers compaction (e.g. 0.75)
/// - `ratio`            – chars-per-token estimate (e.g. 3.5)
/// - `fresh_tail_count` – number of recent messages that must not be touched
///
/// # Behaviour
/// 1. If estimated tokens < threshold × budget → return immediately (no work).
/// 2. Identify the fresh-tail partition; messages before it are candidates.
/// 3. Iterate candidates oldest-first; for each consumed ToolResult, compress it.
/// 4. Stop as soon as total tokens drop below threshold × budget.
pub fn compact_if_needed(
    messages: &mut [UnifiedMessage],
    token_budget: u64,
    threshold: f64,
    ratio: f64,
    fresh_tail_count: usize,
) {
    let target = (token_budget as f64 * threshold) as usize;
    let total = estimate_total_tokens(messages, ratio);
    if total < target {
        return;
    }

    let limit = target;
    tracing::info!(target: "session_compactor", total_tokens = total, threshold = limit, "tool_compact");

    let partition = partition_fresh_tail(messages, fresh_tail_count);

    // Collect indices of consumed tool results in the compressible zone.
    let candidates: Vec<usize> = (0..partition)
        .filter(|&i| messages[i].is_tool_result() && is_tool_result_consumed(messages, i))
        .collect();

    // Compress oldest first until we are back under target.
    for idx in candidates {
        if estimate_total_tokens(messages, ratio) < target {
            break;
        }

        // Extract info, compress, and replace — borrow checker requires this order.
        let (name, old_content) = match messages[idx].tool_result_info() {
            Some((n, c)) => (n.to_owned(), c),
            None => continue,
        };

        let compressed = compress_tool_result(&name, &old_content);
        // Only bother replacing if we actually shrank the content.
        if compressed.len() < old_content.len() {
            messages[idx].replace_tool_result_content(compressed);
        }
    }

    let current_total = estimate_total_tokens(messages, ratio);
    tracing::info!(target: "session_compactor", saved = total.saturating_sub(current_total), "tool_compact_done");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- compress_tool_result ---

    #[test]
    fn test_compress_file_read_by_name() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let result = compress_tool_result("Read", content);
        assert!(result.starts_with("[Read file,"), "got: {result}");
        assert!(result.contains("3 lines"), "got: {result}");
        assert!(result.contains("rust"), "got: {result}");
    }

    #[test]
    fn test_compress_glob_by_name() {
        let content = "src/main.rs\nsrc/lib.rs\n";
        let result = compress_tool_result("Glob", content);
        assert!(result.starts_with("[Read file,"), "got: {result}");
    }

    #[test]
    fn test_compress_read_file_underscore_name() {
        let content = "line1\nline2\n";
        let result = compress_tool_result("read_file", content);
        assert!(result.starts_with("[Read file,"), "got: {result}");
    }

    #[test]
    fn test_compress_grep_by_name() {
        let content = "src/foo.rs:10: let x = 1;\nsrc/bar.rs:5: let y = 2;\n";
        let result = compress_tool_result("Grep", content);
        assert!(result.starts_with("[Search result,"), "got: {result}");
        assert!(result.contains("2 matching lines"), "got: {result}");
    }

    #[test]
    fn test_compress_search_by_name() {
        let content = "result A\nresult B\nresult C\n";
        let result = compress_tool_result("Search", content);
        assert!(result.starts_with("[Search result,"), "got: {result}");
        assert!(result.contains("3 matching lines"), "got: {result}");
    }

    #[test]
    fn test_compress_bash_by_name() {
        let content = "line1\nline2\nline3\nline4\n";
        let result = compress_tool_result("Bash", content);
        assert!(result.starts_with("[Command output,"), "got: {result}");
        assert!(result.contains("4 lines"), "got: {result}");
    }

    #[test]
    fn test_compress_shell_by_name() {
        let content = "ok\n";
        let result = compress_tool_result("shell", content);
        assert!(result.starts_with("[Command output,"), "got: {result}");
    }

    #[test]
    fn test_compress_web_fetch_keeps_first_200_chars() {
        let long_content = "x".repeat(300);
        let result = compress_tool_result("WebFetch", &long_content);
        // First 200 'x' chars, then truncation note.
        assert!(result.starts_with(&"x".repeat(200)), "got: {result}");
        assert!(result.contains("truncated"), "got: {result}");
    }

    #[test]
    fn test_compress_web_fetch_short_content_unchanged() {
        let short = "short content";
        let result = compress_tool_result("web_fetch", short);
        assert_eq!(result, short);
    }

    #[test]
    fn test_compress_other_small_content_unchanged() {
        // < 500 tokens → not compressed
        let small = "hello world";
        let result = compress_tool_result("CustomTool", small);
        assert_eq!(result, small);
    }

    #[test]
    fn test_compress_other_large_content_truncated() {
        // > 500 tokens at ratio 3.5 → need > 1750 chars
        let large = "a".repeat(2000);
        let result = compress_tool_result("CustomTool", &large);
        assert!(result.contains("truncated"), "got: {result}");
        assert!(result.len() < large.len());
    }

    // --- detect_language ---

    #[test]
    fn test_detect_language_json() {
        assert_eq!(detect_language(r#"{"key": "value"}"#), "json");
        assert_eq!(detect_language("[1, 2, 3]"), "json");
    }

    #[test]
    fn test_detect_language_rust() {
        assert_eq!(detect_language("fn main() { }"), "rust");
        assert_eq!(detect_language("use std::collections::HashMap;"), "rust");
    }

    #[test]
    fn test_detect_language_python() {
        assert_eq!(detect_language("def foo():\n    pass"), "python");
    }

    // --- compact_if_needed: no change when under threshold ---

    #[test]
    fn test_compact_if_needed_under_threshold_no_change() {
        let mut messages = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::tool_result("c1", "Bash", "ok", false),
            UnifiedMessage::assistant("done"),
        ];
        let original_len = messages.len();
        // budget = 100_000, threshold = 0.75 → target = 75_000 tokens — way above actual usage
        compact_if_needed(&mut messages, 100_000, 0.75, 3.5, 2);
        assert_eq!(messages.len(), original_len);
        // Tool result content unchanged
        let (_, content) = messages[1].tool_result_info().unwrap();
        assert_eq!(content, "ok");
    }

    // --- compact_if_needed: compresses when over threshold ---

    #[test]
    fn test_compact_if_needed_over_threshold_compresses_old_results() {
        // Build a conversation where tool results are large enough to trigger compaction.
        let large_output = "x".repeat(2000); // ~571 tokens at ratio 3.5
        let mut messages = vec![
            UnifiedMessage::user("request"),
            UnifiedMessage::tool_result("c1", "Bash", &large_output, false),
            UnifiedMessage::assistant("I processed it."),
        ];

        // total chars ≈ 2000 + small overhead; set a tiny budget to force compaction.
        // budget=100 tokens, threshold=0.01 → target=1 token → always triggers
        compact_if_needed(&mut messages, 100, 0.01, 3.5, 0);

        let (_, content) = messages[1].tool_result_info().unwrap();
        assert!(content.starts_with("[Command output,"), "got: {content}");
    }

    // --- compact_if_needed: fresh tail is untouched ---

    #[test]
    fn test_compact_if_needed_fresh_tail_untouched() {
        let large_output = "x".repeat(2000);
        let mut messages = vec![
            UnifiedMessage::user("old request"),
            UnifiedMessage::tool_result("c1", "Bash", "old small result", false),
            UnifiedMessage::assistant("old reply"),
            // Fresh tail (last 4 messages)
            UnifiedMessage::user("new request"),
            UnifiedMessage::tool_result("c2", "Bash", &large_output, false),
            UnifiedMessage::assistant("new reply"),
        ];

        // Force compaction with fresh_tail_count=4 → only [0..2] are compressible
        compact_if_needed(&mut messages, 100, 0.01, 3.5, 4);

        // The fresh-tail tool result (index 4) must be unchanged
        let (_, tail_content) = messages[4].tool_result_info().unwrap();
        assert_eq!(tail_content, large_output, "fresh tail was modified");
    }

    // --- compact_if_needed: unconsumed tool result not touched ---

    #[test]
    fn test_compact_if_needed_unconsumed_tool_result_not_touched() {
        // Tool result at the end with no assistant message after it → unconsumed
        let large = "x".repeat(2000);
        let mut messages = vec![
            UnifiedMessage::user("do something"),
            UnifiedMessage::tool_result("c1", "Bash", &large, false),
            // No assistant message follows → unconsumed
        ];

        compact_if_needed(&mut messages, 100, 0.01, 3.5, 0);

        // Should not be compressed because it is unconsumed
        let (_, content) = messages[1].tool_result_info().unwrap();
        assert_eq!(content, large, "unconsumed tool result was modified");
    }

    // --- safe_truncate ---

    #[test]
    fn test_safe_truncate_ascii() {
        assert_eq!(safe_truncate("hello world", 5), "hello");
    }

    #[test]
    fn test_safe_truncate_multibyte() {
        // "日本語" is 9 bytes (3 bytes per char); truncating at 5 must not cut mid-char
        let s = "日本語";
        let result = safe_truncate(s, 5);
        assert!(s.is_char_boundary(result.len()));
        assert!(result.len() <= 5);
    }

    #[test]
    fn test_safe_truncate_shorter_than_n() {
        let s = "abc";
        assert_eq!(safe_truncate(s, 100), "abc");
    }
}
