# Task P6.5 Report — MCP method dispatch: initialize/ping/tools list+call with three-version negotiation

## Status

- **Status**: complete
- **Base SHA** (HEAD of this worktree before this commit): `97bbf8a54` (full `97bbf8a5496eb5080f1abe3efe1f738b4e566db0`)
- **Branch**: `worktree-plugin-scope-round`
- **Commit** (this task): `mcp_face: initialize/ping/tools list+call dispatch with three-version negotiation`
- **Model**: DeepSeek V4 Pro (task trailer)

## What was implemented

### `src/gateway/mcp_face/protocol.rs` — the JSON-RPC dispatcher (was a `//! (P6.5)` stub)

HTTP-neutral: takes an already-authorized `McpCaller`, an already-resolved session (or `None`, for `initialize`) and one `JsonRpcRequest`, answers an `Outcome`. `http.rs` (P6.6) owns status codes/headers; this file owns the JSON-RPC semantics.

- `pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]` — newest first; the oldest is the client stack's own `"2025-03-26"` so the two ends cannot disagree. The sessionless `2026-07-28` (`modern::MCP_MODERN_PROTOCOL_VERSION`) is deliberately absent.
- `pub const SERVER_NAME: &str = "aleph"` — the name hosts use to prefix tools (`aleph_<tool>` / `mcp__aleph__<tool>`).
- `pub const INSTRUCTIONS: &str` — short (pi never shows it; dsh does not consume it).
- `pub fn negotiate(requested: &str) -> &'static str` — accept the client's version if spoken, else answer the newest (`[0]`).
- `pub fn requires_session(method: &str) -> bool` — `method != "initialize"`.
- `pub enum Outcome { Reply { response: JsonRpcResponse, new_session: Option<SessionView> }, Accepted }` — `new_session` is `Some` exactly on a successful `initialize`.
- `pub async fn handle_message(face, caller, session: Option<&SessionView>, msg) -> Outcome`:
  - `msg.validate()` first → `INVALID_REQUEST` on failure.
  - no `id` ⇒ notification ⇒ never answered; `notifications/initialized` marks the session initialized; any other notification is accepted and ignored → `Outcome::Accepted`.
  - `initialize` → `negotiate` + `sessions().create(...)` + `InitializeResult` (serverInfo `aleph` + `env!("ALEPH_VERSION")`, `tools.listChanged = true`, `resources`/`prompts` omitted).
  - session required for everything else, else `INVALID_REQUEST` ("missing or unknown Mcp-Session-Id").
  - `ping` → `{}`; `tools/list` → `ToolsListResult` (no `nextCursor`); `tools/call` → `ToolCallParams` (missing/unparseable params → `INVALID_PARAMS`, `Err(UnknownTool)` → `INVALID_PARAMS` "Unknown tool: {name}"); any other method → `METHOD_NOT_FOUND`.

### Tests (16, bottom of the file, from the plan verbatim)

`negotiate` acceptance + newest fallback (incl. `"2026-07-28"`), legacy-constant pin, initialize on all three versions, unsupported-version→newest, initialize-without-clientInfo→INVALID_PARAMS + no session minted, `notifications/initialized` marks + other notifications accepted, no-id ⇒ no reply, `ping` empty object + string-id echo, `tools/list` filtered by expose + inputSchema + no nextCursor, `tools/call` text content + `isError=false`, no-arguments dispatch, failing tool `isError=true` (not a protocol error), unexposed tool INVALID_PARAMS, unknown methods METHOD_NOT_FOUND, sessionless non-initialize INVALID_REQUEST, wrong `jsonrpc` version INVALID_REQUEST.

## Test evidence (every `cargo test -p alephcore --lib` invocation read its `test result:` line)

| Suite | Result | Notes |
|---|---|---|
| `gateway::mcp_face::protocol::tests` | **16 passed; 0 failed** | all 16 new tests |
| `gateway::mcp_face` (full) | **49 passed; 0 failed** | 33 prior (P6.1–P6.4) + 16 new |
| `capability::census::tests` | **21 passed; 1 failed** | see Census |

Totals for the gated run: `gateway::mcp_face` 49/49, `capability::census` 21/22.

## Mutation step (red → revert)

Changed `negotiate`'s fallback from `SUPPORTED_PROTOCOL_VERSIONS[0]` to `SUPPORTED_PROTOCOL_VERSIONS[2]`:

```
failures:
    gateway::mcp_face::protocol::tests::an_unsupported_version_is_answered_with_the_newest_and_a_session
    gateway::mcp_face::protocol::tests::negotiation_accepts_every_supported_version_and_answers_the_newest_otherwise
test result: FAILED. 0 passed; 2 failed; 0 ignored
```

Exactly the two expected reds from the plan's Step 5. Reverted; full suite back to green.

## Census — the P6.4 decline red is still the only red

`capability::census::tests::every_decline_wrapper_has_a_production_caller` **still fails by exactly one** with `["decline_mcp_face"]`. This is P6.4's intentional state: the `decline_mcp_face` production caller lands in P6.7 (boot wiring). Per the lead ruling, P6.5 neither forwards P6.7 nor masks the census rule, and the wrapper is not deleted.

Note: `every_installed_global_is_a_capability_slot` (the other census red at `census.rs:823` per constraints.md §6, "red by exactly one on main") is **green in this worktree** — the P6.4 roster move (`52 → 53`) resolved it. Only the `decline_mcp_face` red remains.

## Static checks

- `rustfmt --check --edition 2021 src/gateway/mcp_face/protocol.rs`: **clean, exit 0** (after `rustfmt --edition 2021` on the touched file; the verbatim plan test block needed reformatting).
- `git diff --check`: **clean** (no output).
- `git diff 97bbf8a54 -- src/harness/`: **empty** (0 lines; R10 satisfied).

## Deviations from the plan

- **Borrow fix in the plan's own test.** `tools_list_is_filtered_by_expose_and_carries_schemas` as written in the plan has a move error: `response.result.unwrap()["tools"]...` moves `response.result`, then `response.result.as_ref().unwrap().get("nextCursor")` borrows after move (E0382). Fixed minimally to `response.result.as_ref().unwrap()["tools"]...` — no assertion changed.
- **Trailer override.** The plan's Step 6 body uses `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`; the task instruction overrides it to `Co-Authored-By: DeepSeek V4 Pro <noreply@deepseek.com>`.

## Out of scope (per task; not done)

- **`http.rs` (P6.6) untouched** — `/mcp` routes, `Mcp-Session-Id`, 401/404/426/429 framing are P6.6. `handle_message` is the core it will call.
- **`decline_mcp_face` caller (P6.7) untouched** — the census red is the design.
- **No MCP route is wired**; no `cargo fmt` was run (only `rustfmt` on the single touched file per constraints §8).

## Negatives

- `protocol.rs` trusts `serde_json::from_value` for `InitializeParams` / `ToolCallParams` rather than hand-validating every field; a structurally-wrong-but-deserializable body (e.g. `clientInfo.capabilities` present but malformed) passes serde and is accepted. The HTTP layer's spec framing is where stricter validation belongs; this is not exercised here.
- `to_value` falls back to `Value::Null` on a serialization failure rather than surfacing an internal error (P7's concern; the current payload types cannot fail).
- A sessionless `initialize` with a well-formed body but unknown version still mints a session (spec-correct: the server answers the newest and lets the client decide), so a client probing with a bogus version can still allocate session rows; the session table's idle sweep (P6.2) is the backstop.
