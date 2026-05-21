# DevTools Output Compression Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compress Chrome DevTools MCP tool outputs before they enter conversation history, preventing "prompt too long" API errors during browser automation tasks.

**Architecture:** A new `tool_output/compressor.rs` module provides `compress_tool_output(tool_name, output)` which matches tool names against a known DevTools set and applies type-specific compression (snapshot filtering, screenshot replacement, size truncation). Called from `loop_core.rs` between output serialization and message push. Prompt tweak guides LLM to use targeted queries.

**Tech Stack:** Rust, serde_json (for JSON array parsing in list compressors)

**Spec:** `docs/superpowers/specs/2026-03-24-devtools-output-compression-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/tool_output/compressor.rs` | DevTools-aware tool output compression |
| Modify | `src/tool_output/mod.rs` | Add `pub mod compressor;` |
| Modify | `src/agent_loop/loop_core.rs` | Call `compress_tool_output()` on successful results |
| Modify | `src/agent_loop/prompt_builder.rs` | Add 2 lines to BASE_BEHAVIOR |

---

### Task 1: Create compressor module with tool name identification

**Files:**
- Create: `src/tool_output/compressor.rs`
- Modify: `src/tool_output/mod.rs`

- [ ] **Step 1: Write tests for tool name identification and passthrough**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_devtools_tool() {
        assert!(is_devtools_tool("take_snapshot"));
        assert!(is_devtools_tool("evaluate_script"));
        assert!(is_devtools_tool("list_network_requests"));
        assert!(is_devtools_tool("take_screenshot"));
        assert!(!is_devtools_tool("bash"));
        assert!(!is_devtools_tool("web_search"));
        assert!(!is_devtools_tool("memory_store"));
        assert!(!is_devtools_tool(""));
    }

    #[test]
    fn test_passthrough_non_devtools() {
        let output = "some tool output that should not be changed";
        let result = compress_tool_output("bash", output);
        assert_eq!(result, output);
    }

    #[test]
    fn test_passthrough_small_devtools_output() {
        // Small outputs from click, navigate etc. should pass through
        let output = "Clicked element #submit-button";
        let result = compress_tool_output("click", output);
        assert_eq!(result, output);
    }
}
```

- [ ] **Step 2: Implement core structure**

```rust
//! Tool output compression for context-heavy tools.
//!
//! Compresses outputs from Chrome DevTools MCP tools before they enter
//! conversation history. Each tool type gets a tailored strategy that
//! preserves actionable information while drastically reducing token count.

/// Known Chrome DevTools Protocol tool names.
/// These are stable protocol-defined names from the Chrome DevTools MCP server.
const DEVTOOLS_TOOLS: &[&str] = &[
    "take_snapshot", "take_screenshot", "navigate_page", "click",
    "evaluate_script", "list_network_requests", "list_console_messages",
    "get_network_request", "get_console_message", "new_page", "close_page",
    "select_page", "list_pages", "hover", "fill", "fill_form", "type_text",
    "press_key", "drag", "upload_file", "handle_dialog", "wait_for",
    "emulate", "resize_page", "performance_start_trace",
    "performance_stop_trace", "performance_analyze_insight",
    "take_memory_snapshot", "lighthouse_audit",
];

/// Check if a tool name belongs to the Chrome DevTools MCP server.
pub fn is_devtools_tool(name: &str) -> bool {
    DEVTOOLS_TOOLS.contains(&name)
}

/// Compress tool output based on tool type.
///
/// For DevTools tools, applies type-specific compression.
/// For all other tools, returns the output unchanged.
pub fn compress_tool_output(tool_name: &str, output: &str) -> String {
    if !is_devtools_tool(tool_name) {
        return output.to_owned();
    }

    match tool_name {
        "take_snapshot" => compress_snapshot(output),
        "take_screenshot" => compress_screenshot(output),
        "evaluate_script" => compress_generic(output, 8 * 1024),
        "list_network_requests" => compress_network_requests(output),
        "list_console_messages" => compress_console_messages(output),
        "get_network_request" => compress_generic(output, 8 * 1024),
        _ => compress_generic(output, 10 * 1024),
    }
}
```

- [ ] **Step 3: Add `pub mod compressor;` to `tool_output/mod.rs`**

Add after line 11 (`pub mod truncation;`):
```rust
pub mod compressor;
```

And add to the `pub use` block:
```rust
pub use compressor::compress_tool_output;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (stub functions needed — add empty implementations that return `output.to_owned()`)

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib tool_output::compressor`
Expected: PASS

- [ ] **Step 6: Commit**

```
tool_output: add compressor module with DevTools tool identification
```

---

### Task 2: Implement compression strategies

**Files:**
- Modify: `src/tool_output/compressor.rs`

- [ ] **Step 1: Write tests for each compressor**

```rust
#[test]
fn test_compress_screenshot_replaces_base64() {
    let output = r#"{"content":[{"type":"image","data":"iVBORw0KGgoAAAANSUhEUg..."}]}"#;
    let result = compress_tool_output("take_screenshot", output);
    assert!(result.contains("[Screenshot captured"));
    assert!(!result.contains("iVBORw0KGgo"));
}

#[test]
fn test_compress_snapshot_filters_interactive() {
    // Build a snapshot > 4KB to trigger compression
    let mut lines = vec![
        "- document [ref=1]".to_string(),
        "  - navigation \"Main\" [ref=2]".to_string(),
        "    - link \"Home\" [ref=3]".to_string(),
        "    - link \"About\" [ref=4]".to_string(),
        "  - main [ref=5]".to_string(),
    ];
    // Add 200 non-interactive nodes to exceed 4KB threshold
    for i in 0..200 {
        lines.push(format!("    - paragraph \"Filler paragraph number {} with extra text to pad size\" [ref={}]", i, 100 + i));
    }
    lines.push("    - button \"Sign Up\" [ref=8]".to_string());
    lines.push("    - textbox \"Email\" [ref=9]".to_string());
    lines.push("    - heading \"Section Title\" [ref=10]".to_string());
    lines.push("    - link \"Learn more\" [ref=11]".to_string());
    let snapshot = lines.join("\n");
    assert!(snapshot.len() > 4096, "test input must exceed 4KB threshold");

    let result = compress_tool_output("take_snapshot", &snapshot);
    // Should keep interactive elements
    assert!(result.contains("link \"Home\""));
    assert!(result.contains("button \"Sign Up\""));
    assert!(result.contains("textbox \"Email\""));
    assert!(result.contains("link \"Learn more\""));
    // Should strip non-interactive
    assert!(!result.contains("paragraph"));
    assert!(!result.contains("heading"));
    // Should have summary
    assert!(result.contains("compressed"));
}

#[test]
fn test_compress_snapshot_passthrough_small() {
    // Small snapshots (< 4KB) pass through unchanged
    let small = "- document [ref=1]\n  - button \"OK\" [ref=2]";
    let result = compress_tool_output("take_snapshot", small);
    assert_eq!(result, small);
}

#[test]
fn test_compress_evaluate_script_truncates_large() {
    let large = "x".repeat(20_000);
    let result = compress_tool_output("evaluate_script", &large);
    assert!(result.len() <= 8 * 1024 + 200); // 8KB + notice
    assert!(result.contains("output truncated"));
}

#[test]
fn test_compress_evaluate_script_passthrough_small() {
    let small = r#"{"title": "YouTube"}"#;
    let result = compress_tool_output("evaluate_script", small);
    assert_eq!(result, small);
}

#[test]
fn test_compress_network_requests_limits_entries() {
    // Build a JSON array with 100 request objects
    let entries: Vec<String> = (0..100)
        .map(|i| format!(r#"{{"method":"GET","url":"https://example.com/api/{}","status":200}}"#, i))
        .collect();
    let json = format!("[{}]", entries.join(","));
    let result = compress_tool_output("list_network_requests", &json);
    assert!(result.contains("showing 30 of 100"));
    // Should not contain entry 50
    assert!(!result.contains("/api/50"));
}

#[test]
fn test_compress_console_messages_keeps_last() {
    let entries: Vec<String> = (0..200)
        .map(|i| format!("console message {}", i))
        .collect();
    let output = entries.join("\n");
    let result = compress_tool_output("list_console_messages", &output);
    assert!(result.contains("console message 199")); // last kept
    assert!(result.contains("console message 150")); // near end kept
    assert!(!result.contains("console message 0"));   // old dropped
    assert!(result.contains("showing last 50"));
}

#[test]
fn test_compress_generic_devtools_truncates() {
    let large = "y".repeat(20_000);
    let result = compress_tool_output("click", &large);
    assert!(result.len() <= 10 * 1024 + 200);
}
```

- [ ] **Step 2: Implement `compress_screenshot`**

```rust
/// Replace base64 screenshot data with a brief notice.
fn compress_screenshot(_output: &str) -> String {
    "[Screenshot captured successfully]".to_owned()
}
```

- [ ] **Step 3: Implement `compress_snapshot`**

```rust
const SNAPSHOT_COMPRESS_THRESHOLD: usize = 4 * 1024; // Only compress if > 4KB

/// Interactive element types to keep in snapshot compression.
const INTERACTIVE_ROLES: &[&str] = &[
    "link", "button", "textbox", "input", "textarea", "select",
    "checkbox", "radio", "combobox", "menuitem", "tab", "switch",
    "searchbox", "spinbutton", "slider",
];

/// Compress accessibility tree snapshot by keeping only interactive elements.
fn compress_snapshot(output: &str) -> String {
    if output.len() < SNAPSHOT_COMPRESS_THRESHOLD {
        return output.to_owned();
    }

    let mut kept = Vec::new();
    let mut total_nodes = 0usize;

    for line in output.lines() {
        let trimmed = line.trim_start_matches(|c: char| c == ' ' || c == '-' || c == ' ');
        total_nodes += 1;

        // Keep lines containing interactive role keywords
        let is_interactive = INTERACTIVE_ROLES.iter().any(|role| {
            trimmed.starts_with(role)
                || trimmed.starts_with(&format!("{} \"", role))
        });

        if is_interactive {
            kept.push(line);
        }
    }

    if kept.is_empty() {
        // Fallback: if no interactive elements found, use generic truncation
        return compress_generic(output, 5 * 1024);
    }

    let mut result = kept.join("\n");
    result.push_str(&format!(
        "\n[Snapshot compressed: kept {} interactive elements out of {} total nodes]",
        kept.len(),
        total_nodes,
    ));
    result
}
```

- [ ] **Step 4: Implement `compress_network_requests`**

```rust
const MAX_NETWORK_ENTRIES: usize = 30;

/// Compress network request list to summary format.
fn compress_network_requests(output: &str) -> String {
    // Try to parse as JSON array
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return compress_generic(output, 3 * 1024);
    };
    let Some(arr) = value.as_array() else {
        return compress_generic(output, 3 * 1024);
    };

    let total = arr.len();
    if total <= MAX_NETWORK_ENTRIES {
        // Small enough, but still simplify format
        let lines: Vec<String> = arr.iter().map(format_network_entry).collect();
        return lines.join("\n");
    }

    let mut lines: Vec<String> = arr.iter().take(MAX_NETWORK_ENTRIES).map(format_network_entry).collect();
    lines.push(format!("[... showing {} of {} total requests]", MAX_NETWORK_ENTRIES, total));
    lines.join("\n")
}

fn format_network_entry(entry: &serde_json::Value) -> String {
    let method = entry.get("method").and_then(|v| v.as_str()).unwrap_or("?");
    let url = entry.get("url").and_then(|v| v.as_str()).unwrap_or("?");
    let status = entry.get("status").and_then(|v| v.as_u64()).map(|s| s.to_string()).unwrap_or_else(|| "?".into());
    format!("{} {} → {}", method, url, status)
}
```

- [ ] **Step 5: Implement `compress_console_messages`**

```rust
const MAX_CONSOLE_MESSAGES: usize = 50;

/// Keep last N console messages.
fn compress_console_messages(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= MAX_CONSOLE_MESSAGES {
        return output.to_owned();
    }

    let total = lines.len();
    let kept = &lines[total - MAX_CONSOLE_MESSAGES..];
    format!(
        "[... showing last {} of {} total console messages]\n{}",
        MAX_CONSOLE_MESSAGES,
        total,
        kept.join("\n"),
    )
}
```

- [ ] **Step 6: Implement `compress_generic`**

```rust
/// Generic truncation at a byte limit, UTF-8 safe.
fn compress_generic(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_owned();
    }

    let total = output.len();
    // Find a valid UTF-8 boundary
    let mut end = max_bytes;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }

    let truncated = &output[..end];
    format!(
        "{}\n[... output truncated, showing first {} bytes of {} total]",
        truncated, end, total,
    )
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib tool_output::compressor`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```
tool_output: implement DevTools-specific compression strategies
```

---

### Task 3: Wire compressor into agent loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Add compression call**

In `loop_core.rs`, between `output_text` construction (line 568) and `messages.push()` (line 570), add compression for successful results only:

```rust
// After line 568 (end of match block producing output_text):

// Compress verbose tool outputs (especially DevTools MCP tools)
let output_text = if !is_error {
    crate::tool_output::compressor::compress_tool_output(
        &tc.name, &output_text,
    )
} else {
    output_text
};
```

Note: `output_text` was previously immutable `let`. Change to `let` followed by reassignment, or shadow it with a new `let`.

- [ ] **Step 2: Run existing agent loop tests**

Run: `cargo test -p alephcore --lib agent_loop::loop_core`
Expected: ALL PASS (compression is passthrough for non-DevTools tools, existing tests use `echo`/mock tools)

- [ ] **Step 3: Commit**

```
agent_loop: wire tool output compressor before message push
```

---

### Task 4: Add prompt optimization

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`

- [ ] **Step 1: Add DevTools guidance to BASE_BEHAVIOR**

At the end of `BASE_BEHAVIOR` (line 58, before the closing `";`), add:

```rust
\n\
- **DEVTOOLS EFFICIENCY.** When using Chrome DevTools tools, prefer targeted CSS selectors (click, fill) over full-page snapshots (take_snapshot). Use evaluate_script with specific queries (e.g. `document.querySelector('.title').textContent`) rather than dumping entire page content (e.g. `document.body.innerText`).";
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: No regressions

- [ ] **Step 4: Commit**

```
prompt: add DevTools efficiency guidance to BASE_BEHAVIOR
```
