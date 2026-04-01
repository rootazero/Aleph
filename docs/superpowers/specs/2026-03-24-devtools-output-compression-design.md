# DevTools Tool Output Compression

## Problem

Chrome DevTools MCP tools (take_snapshot, evaluate_script, list_network_requests, etc.) return large outputs that accumulate in conversation history during multi-tool agent loops. A single YouTube page snapshot can exceed 50KB. After 20+ tool calls, context reaches millions of tokens, causing "prompt too long" API errors.

Current defenses are insufficient:
- `truncation.rs` (50KB/2000 lines) only applies to builtin tools, not MCP tools
- `tool_compactor` only compresses "consumed" results (after assistant reply), missing in-progress accumulation
- `enforce_context_limit` is a last-resort that drops all history

## Solution

Add a tool output compressor (`tool_output/compressor.rs`) that runs at the point tool results are written into conversation history (`loop_core.rs`, between `output_text` construction and `messages.push()`). The compressor uses a hardcoded set of known DevTools tool names to apply type-specific compression strategies.

Additionally, add 2 lines to BASE_BEHAVIOR prompt to guide the LLM toward more efficient DevTools usage.

## Architecture

```
Tool execution returns result
    ↓
serde_json::to_string(output)  →  output_text
    ↓
compress_tool_output(tool_name, output_text)   ← NEW
    ↓ match tool_name against DEVTOOLS_TOOLS set
    ├─ "take_snapshot"         → snapshot_compressor()
    ├─ "take_screenshot"       → screenshot_compressor()
    ├─ "evaluate_script"       → script_compressor()
    ├─ "list_network_requests" → list_compressor(30)
    ├─ "list_console_messages" → list_compressor(50)
    ├─ "get_network_request"   → generic_truncate(8KB)
    ├─ other known devtools    → generic_truncate(10KB)
    └─ non-devtools tool       → pass through (unchanged)
    ↓
messages.push(UnifiedMessage::tool_result(...))
```

Compression only applies to successful results (`is_error == false`). Error messages pass through unchanged.

## Tool Name Identification

In the agent loop (`loop_core.rs`), `tc.name` contains the **short tool name** as returned by the LLM (e.g., `take_snapshot`, not the long MCP prefix format). The `LoopTool` trait does not expose server/source metadata.

**Strategy**: Use a hardcoded `HashSet` of known Chrome DevTools tool names. These names are defined by the Chrome DevTools Protocol and are stable:

```rust
const DEVTOOLS_TOOLS: &[&str] = &[
    "take_snapshot", "take_screenshot", "navigate_page", "click",
    "evaluate_script", "list_network_requests", "list_console_messages",
    "get_network_request", "get_console_message", "new_page", "close_page",
    "select_page", "list_pages", "hover", "fill", "fill_form", "type_text",
    "press_key", "drag", "upload_file", "handle_dialog", "wait_for",
    "emulate", "resize_page",
];
```

Only tools in this set get DevTools-specific compression. All other tools pass through unchanged.

**Why hardcoded**: Adding a `source()` method to the `LoopTool` trait would require modifying the trait and all implementors (high churn for minimal gain). The DevTools tool names are Protocol-defined constants that rarely change. If a new tool is added to the MCP server, the worst case is it gets no compression (same as today) until we add it to the set.

## Compression Strategies

### take_snapshot (~50KB+ → ~5KB)

The accessibility tree is a line-based text format. Scan lines and keep only interactive elements: links (`link`), buttons (`button`), inputs (`textbox`, `input`), textareas, selects, checkboxes, radio buttons. Preserve indentation hierarchy but strip pure text/decoration nodes (headings, paragraphs, static text, images without alt-action).

Append a notice: `[Snapshot compressed: kept N interactive elements out of M total nodes]`

### take_screenshot (~200KB → ~30 bytes)

LLMs cannot parse base64-encoded image data in text form. Replace the entire base64 payload with:
`[Screenshot captured successfully]`

Note: If the screenshot was requested for visual analysis, the LLM should use evaluate_script to query specific visual properties instead.

### evaluate_script (variable → ≤8KB)

Keep the first 8KB of output. If truncated, append:
`\n[... output truncated, showing first 8192 bytes of N total]`

### list_network_requests (variable → ~3KB)

Parse the JSON array. Keep first 30 entries, extract only: `method`, `url`, `status`. Format as:
```
GET https://example.com/api/data → 200
POST https://example.com/submit → 302
[... showing 30 of N total requests]
```

### list_console_messages (variable → ~5KB)

Keep last 50 messages. If truncated, prepend:
`[... showing last 50 of N total console messages]`

### Other known DevTools tools (→ ≤10KB)

Generic truncation at 10KB with notice. Applies to: navigate_page, click, hover, fill, etc. These tools typically return small outputs but the cap protects against edge cases.

### Non-DevTools tools

Pass through unchanged. Existing truncation.rs and tool_compactor handle these.

## Interaction with Existing Systems

**truncation.rs**: Independent. It handles builtin tool file overflow storage. MCP tools don't go through it. No interaction.

**tool_compactor.rs**: The compressor runs first (at write time), tool_compactor runs later (before each LLM call, on consumed results). Double-compression is harmless — if the compressor already shrunk the output, the compactor's `> 500 token` threshold won't trigger, making it a no-op.

**enforce_context_limit()**: Stays as last-resort safety net. With the compressor in place, it should rarely trigger for DevTools workflows.

**wrap_external_content() (content sanitizer)**: MCP tool results may be wrapped with security boundary markers by `content_sanitizer::wrap_external_content()` before reaching the agent loop. The compressor receives the wrapped content. Compression strategies operate on the inner content — for `take_screenshot`, pattern-match the base64 prefix; for `take_snapshot`, scan the accessibility tree lines within the wrapper.

## File Changes

| Action | File | Change |
|--------|------|--------|
| Create | `src/tool_output/compressor.rs` | All compression logic + tests |
| Modify | `src/tool_output/mod.rs` | Add `pub mod compressor;` |
| Modify | `src/agent_loop/loop_core.rs` | Call `compress_tool_output()` on successful results before pushing to messages |
| Modify | `src/agent_loop/prompt_builder.rs` | Add 2 lines to BASE_BEHAVIOR |

## Prompt Change (BASE_BEHAVIOR)

Add to the behavior rules section:
```
When using Chrome DevTools tools, prefer targeted CSS selectors over full-page snapshots.
Use evaluate_script with specific queries rather than dumping entire page content.
```

## What This Does NOT Change

- `truncation.rs` — independent responsibility (builtin tool file overflow)
- `tool_compactor.rs` — independent responsibility (post-hoc history compression)
- `enforce_context_limit()` — stays as last-resort safety net
- `LoopTool` trait — no new methods added
- MCP tool definitions or schemas — no changes to tool interfaces

## Testing

- Unit tests for each compressor function (snapshot, screenshot, script, list, generic)
- Unit test for `is_devtools_tool()` against known and unknown tool names
- Integration: verify `compress_tool_output` returns passthrough for non-DevTools tools
- Verify existing agent_loop tests still pass
