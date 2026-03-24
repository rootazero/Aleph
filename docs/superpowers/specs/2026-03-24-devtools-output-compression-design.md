# DevTools Tool Output Compression

## Problem

Chrome DevTools MCP tools (take_snapshot, evaluate_script, list_network_requests, etc.) return large outputs that accumulate in conversation history during multi-tool agent loops. A single YouTube page snapshot can exceed 50KB. After 20+ tool calls, context reaches millions of tokens, causing "prompt too long" API errors.

Current defenses are insufficient:
- `truncation.rs` (50KB/2000 lines) only applies to builtin tools, not MCP tools
- `tool_compactor` only compresses "consumed" results (after assistant reply), missing in-progress accumulation
- `enforce_context_limit` is a last-resort that drops all history

## Solution

Add a tool output compressor (`tool_output/compressor.rs`) that runs at the point MCP tool results are written into conversation history (`loop_core.rs:546`). The compressor uses tool name pattern matching to apply type-specific compression strategies for DevTools tools.

Additionally, add 2 lines to BASE_BEHAVIOR prompt to guide the LLM toward more efficient DevTools usage.

## Architecture

```
MCP tool returns result
    ↓
serde_json::to_string(output)
    ↓
compress_tool_output(tool_name, output_text)   ← NEW
    ↓ extract short name from MCP tool name
    ├─ "take_snapshot"         → snapshot_compressor()
    ├─ "take_screenshot"       → screenshot_compressor()
    ├─ "evaluate_script"       → script_compressor()
    ├─ "list_network_requests" → list_compressor(30)
    ├─ "list_console_messages" → list_compressor(50)
    ├─ "get_network_request"   → generic_truncate(8KB)
    ├─ other devtools tool     → generic_truncate(10KB)
    └─ non-devtools tool       → pass through (unchanged)
    ↓
messages.push(UnifiedMessage::tool_result(...))
```

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

### Other DevTools tools (→ ≤10KB)

Generic truncation at 10KB with notice.

### Non-DevTools tools

Pass through unchanged. Existing truncation.rs and tool_compactor handle these.

## Tool Name Pattern Matching

MCP tool names follow the pattern: `mcp__plugin_{server}_{namespace}__{tool_name}`

Example: `mcp__plugin_chrome-devtools-mcp_chrome-devtools__take_snapshot`

Extract short name: split on `__`, take the last segment. Match against known DevTools tool names. If the tool name contains `chrome-devtools` in the namespace portion, treat unrecognized tools as "other DevTools" (10KB truncation).

## File Changes

| Action | File | Change |
|--------|------|--------|
| Create | `core/src/tool_output/compressor.rs` | All compression logic + tests |
| Modify | `core/src/tool_output/mod.rs` | Add `pub mod compressor;` |
| Modify | `core/src/agent_loop/loop_core.rs:546` | Call `compress_tool_output()` before pushing tool result |
| Modify | `core/src/agent_loop/prompt_builder.rs` | Add 2 lines to BASE_BEHAVIOR |

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
- MCP tool definitions or schemas — no changes to tool interfaces

## Testing

- Unit tests for each compressor function (snapshot, screenshot, script, list, generic)
- Unit test for tool name extraction from MCP naming pattern
- Integration: verify compress_tool_output returns passthrough for non-DevTools tools
- Verify existing agent_loop tests still pass
