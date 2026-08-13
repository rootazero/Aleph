# Review Report — Batch 6 (Modern protocol, sampling, tool_bridge, tool_sanitize, context_injector, presets)

**Scope:** `src/mcp/modern/mod.rs`, `src/mcp/modern/discover.rs`, `src/mcp/modern/cache.rs`,
`src/mcp/modern/headers.rs`, `src/mcp/modern/mrtr.rs`, `src/mcp/sampling.rs`,
`src/mcp/sampling_bridge.rs`, `src/mcp/tool_bridge.rs`, `src/mcp/tool_sanitize.rs`,
`src/mcp/context_injector.rs`, `src/mcp/presets/mod.rs`
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-mcp-audit` (branch `mcp-audit`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 2 |
| Medium   | 4 |
| Low      | 3 |

The modern protocol layer is the most recent and the most spec-heavy; the
tests are dense and the constants are careful. The two High findings are
about (a) the `mcp/headers.rs` `walk_schema` recursion blowing the stack on
deeply-nested schemas, and (b) the `tool_sanitize` pass *not* being applied
to the embedded `PromptContentItem::Resource.resource` text payloads, which
are a place where a server can ship an unbounded string into the agent's
context.

## Findings

### [HIGH] src/mcp/modern/headers.rs:240 — `walk_schema` recursively descends into arrays and composition keywords, with no depth cap
**Category:** Security (resource exhaustion / stack overflow)
**Confidence:** High

`walk_schema` (line 240) is called from `collect_param_headers` (line 222) on
every tool's `inputSchema`. A server that emits a schema like:

```json
{
  "type": "object",
  "properties": {
    "a": {"anyOf": [{"anyOf": [{"anyOf": [{"anyOf": [{"anyOf": [...]}]}]}]}
  }
}
```

…with a recursion depth of ~10,000 drives the recursive Rust call through
~10,000 stack frames and panics with a stack overflow. The panic is caught
by the manager actor's `tokio::select!` and the connection is dropped, but
the next tool's `tools/list` re-attempts the same parse and panics again.

The `UNREACHABLE_KEYS` loop (line 296) iterates `patternProperties` and
`$defs`/`definitions` as maps, which is also unbounded.

**Failure scenario:** a malicious MCP server sends a tool with a 65,000-deep
`anyOf` chain. `collect_param_headers` panics on the first handshake. The
manager's `start_server_internal` returns the panic as `Err`, the
`ServerCrashed` event fires, the bridge's `sync_server` returns early, and
the server is never registered. The user sees a blank `mcp.list`.

**Suggested fix:** add a `MAX_DEPTH: usize = 32` (or whatever the spec's
practical limit is) and short-circuit on overflow with a `Truncated` variant
of `ParamHeaderError`. Tests exist for the rejection paths; the depth path
should too.

### [HIGH] src/mcp/tool_sanitize.rs:39 — `normalize_tool_schema` is applied to `tools/list` results, but the tool *description* is not length-bounded and the embedded `PromptContentItem::Resource.resource.text` payloads from `prompts/get` are not normalized at all
**Category:** Security (DoS via prompt context)
**Confidence:** High

`normalize_tool_schema` (line 39) repairs malformed schemas. The schema
normalization is bounded by the schema's size, which is bounded by the
server's response — capped by the transport's response size (`B2-02`).

The description is *not* bounded. A server can emit a 10 MB description
string. `scan_description_for_injection` does `to_lowercase()` on the
whole string (line 318), allocating a 20 MB `String`. The agent's prompt
layer then renders the description into the system prompt verbatim.

`PromptContentItem::Resource.resource.text` (in `external/connection.rs`)
is also unbounded. The `get_prompt` path (line 1319) maps the wire-level
resource to the internal `PromptContent::Resource { uri, text }` without
any size cap or sanitization. A server that returns a `prompts/get` response
with a 100 MB `text` field consumes the agent's full context window.

**Failure scenario:** a malicious MCP server includes a 100 MB description
on every tool. The agent's request budget is consumed in the prompt — a
single tool call costs the user $X in tokens, with no functional benefit.

**Suggested fix:** add a `MAX_DESCRIPTION_BYTES: usize = 8 * 1024` (and a
corresponding `MAX_RESOURCE_TEXT_BYTES`) and truncate with a `[… truncated]`
suffix. The transport's `B2-02` cap is the upstream bound; this is the
downstream bound that survives a properly-bounded transport.

### [MEDIUM] src/mcp/modern/headers.rs:303 — `walk_schema` iterates `UNREACHABLE_KEYS` for every key in the schema, but the inner `if let Some(child) = object.get(*key)` runs even when the value is `null` — `null` is treated as a schema node
**Category:** Logic (false positive)
**Confidence:** Medium

Line 303: `match child { … Value::Array(items) => { for item in items { walk_schema(item, …) } } Value::Object(map) if matches!(key, "$defs" | "definitions" | "patternProperties") => { for sub in map.values() { walk_schema(sub, …) } } other => walk_schema(other, path, false, found)?, }`

The `other` arm matches `Value::Null`, `Value::Bool`, `Value::Number`,
`Value::String`. Each is sent through `walk_schema` with `reachable: false`
and `path` unchanged. The outer `if let Some(object) = node.as_object()`
short-circuits on `null`, so the recursive call is a no-op. But the call
itself happens for every `null` value in the schema, which is the
innermost constant of a 1000-property schema. The recursion is bounded by
the schema's *structure*, not its *depth*, so this is a perf concern, not a
correctness one.

**Suggested fix:** skip the `other` arm for `Value::Null` (the most common
non-schema value). A `match child { Value::Array(items) => …, Value::Object(_) if matches!(key, "$defs" | "definitions" | "patternProperties") => …, Value::Object(_) => walk_schema(child, path, false, found)?, _ => {} }`.

### [MEDIUM] src/mcp/modern/mrtr.rs:180 — `retry_params` inserts `inputResponses` and `requestState` after a `cloned().unwrap_or_default()` — the original params are *cloned* on every retry, so a verbose `arguments` payload is doubled in memory each round
**Category:** Quality (memory)
**Confidence:** Medium

`retry_params` (line 180) does:

```rust
let mut params = original.as_object().cloned().unwrap_or_default();
```

Four MRTR rounds = four clones of the original params. The `arguments`
field, which can be a base64 image, dominates the size. For a tool call
with a 4 MB image, four rounds = 16 MB of duplicate params.

**Suggested fix:** copy only the keys that may change (`inputResponses`,
`requestState`) and leave the rest of the original `Value` borrowed via
`Value::Object(BTreeMap::from([...]))` for non-conflicting keys. The cost
is a small refactor; the win is bounded memory under MRTR retry.

### [MEDIUM] src/mcp/tool_bridge.rs:128 — `spawn_tool_bridge` subscribes to events *before* the reconcile, but the reconcile runs `resync_all` which calls `list_servers` — a bridge that starts while the manager is in the middle of `auto_start_servers` sees a half-populated `clients` map
**Category:** Logic (startup race)
**Confidence:** High

`spawn_tool_bridge` (line 128) is called by the builder *after* the manager
actor is spawned. The actor's `run` (line 161) starts auto-started servers
and emits `ServerStarted` events *before* the bridge subscribes. The bridge's
`resync_all` (line 165) calls `manager.list_servers()` which goes through the
actor's command loop. The actor's command loop is *not* entered until after
`auto_start_servers` finishes (line 178). So the bridge's `resync_all` sees
the complete set.

The defect is that the bridge's `list_servers` returns `McpServerInfo` with
`tool_count: 0` for every server — the manager's `list_servers` (line 752 of
actor.rs) reads `client.list_tools().await.len()` for each. For a server
whose `list_tools()` does network I/O, the bridge's `resync_all` blocks for
the duration of every server's full tool list. A 10-server deployment with
slow servers takes 10 × `list_tools` time to reconcile.

**Suggested fix:** have the manager's `list_servers` return a *snapshot* of
the tool count from `McpServerConnection::cached_tools` (without an `await`),
or carry the count in the `ServerStarted` event. The bridge's reconcile
can then be O(n) without O(n) network calls.

### [MEDIUM] src/mcp/sampling_bridge.rs:73 — `serve_sampling` does not handle the case where `provider.process(payload)` returns an `AiProvider` response that contains a tool-call request — the sampling call returns `text_content()` only, dropping the tool call
**Category:** Logic (silent loss)
**Confidence:** High

`serve_sampling` (line 73) calls `provider.process(payload).await?` and then
`response.text_content()`. If the provider's response contains a tool call,
the tool call is silently dropped. The sampling server expected a model
response (possibly with a tool call), and the model responded with one — but
Aleph returns only the text. The server then sees a "complete" response
without the tool call it expected.

**Failure scenario:** An MCP server's `sampling/createMessage` is a request
to "decide whether to call tool X with args Y". The model calls tool X. The
sampling response returns only the model's pre-tool-call text, missing the
tool call itself. The server retries, gets the same answer, and the loop
runs forever.

**Suggested fix:** surface the tool call in `SamplingResponse.content`. The
spec's `SamplingResponse` is `{ role, content, model, stopReason }` with
`content` being a single block of `Text | Image | Audio`. Aleph would need
to extend the protocol locally (or pass through the tool call as a
text-typed payload). Out of scope for a quick fix; the right answer is a
new `SamplingResponse::ToolCall` variant.

### [LOW] src/mcp/sampling.rs:106 — `set_client` is never called by the manager actor (the actor owns the client, not the sampling handler), so the `client` field is dead state
**Category:** Quality
**Confidence:** High

`SamplingHandler::set_client` (line 52) is `pub async`, but the actor
(`manager/actor.rs`) does not call it. The `client` field is used in
`handle_request` (line 110) via `ContextInjector::gather_context(&client, …)`.
The handler is constructed in `McpClient::new()` (line 91 of client.rs) and
shared by every server connection. The `set_client` is never called.

**Suggested fix:** either wire the call from `McpClient::start_external_server`
to `sampling_handler.set_client(self.clone())`, or remove the dead field.

### [LOW] src/mcp/context_injector.rs:80 — `format_as_system_message` interpolates tool descriptions into the prompt with no length cap
**Category:** Quality (prompt bloat)
**Confidence:** Low

`format_as_system_message` (line 80) builds a `parts` vector with every
resource's URI/name/description and every tool's name/description, and
returns a single `SamplingMessage`. The total length is bounded by the
number of resources/tools, but a single tool with a 100 KB description
(line 102) consumes the prompt. `B6-02` covers the prompt path; this finding
is the context-injection-specific flavour.

**Suggested fix:** covered by `B6-02`'s `MAX_DESCRIPTION_BYTES`.

### [LOW] src/mcp/presets/mod.rs:55 — `catalog()` unwraps the JSON parse with `expect("bundled MCP preset catalog.json must be valid")` — a malformed catalog.json panics at startup
**Category:** Quality
**Confidence:** Low

`catalog()` (line 55) is `OnceLock::get_or_init` with a panic-on-parse. The
catalog is bundled at compile time. A bad edit during development crashes
the daemon at startup. The error is loud, but the daemon start path is
common.

**Suggested fix:** log the error and return an empty catalog. The Hub primer
that projects the catalog into the cache will simply have nothing to project.

## Architecture compliance (Batch 6)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. |
| R3 | clean — uses `tokio::sync`, `serde_json`, `reqwest`. |
| R4 | clean — the bridge is a wire translator; the integrator is a runtime shim. |
| R7 | clean — `serve_sampling` is the LLM boundary; everything else is data. |
| R10 | clean — no regex in the modern protocol layer. |

## Cross-file note

The `tool_bridge` and `manager/actor` share a hand-rolled `Box<dyn Fn>`
notification handler that captures `cmd_tx`. The lifetime issue (`B4-03`)
applies here too. The bridge should hold a `Weak` to the manager's
command channel and refuse to send if the manager is gone.
