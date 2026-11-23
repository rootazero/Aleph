# Round 4 severed-wire audit — S2 orchestration

## Scope and method

Read the current working tree, without modifying the repository. In scope:

- `src/gateway/control_plane/**`
- `src/gateway/mcp_face/**`
- `src/gateway/pipeline/**`
- `src/gateway/message_assembly/**`

The graph slice at commit `3bba257c` was used only to prioritize searches. Every finding below was re-checked against the current files with `rg` and targeted reads. Round 3's landed fixes were not re-reported.

## Result

| Disposition hypothesis | Count |
|---|---:|
| CONNECT | 2 |
| CUT | 0 |
| DECIDE | 1 |
| DARK | 0 |
| **Total** | **3** |

No additional candidate was promoted merely because a helper had no caller: unused convenience methods with no promised user-facing behavior are listed under exclusions, not presented as product defects.

## Findings

### S2-01 — `mcp_server.expose` has a documented live-apply consumer but no live updater
- Form: 3
- Producer: src/gateway/mcp_face/config.rs:19 — `McpFaceConfig::expose` (also parsed as `Config.mcp_server` at src/config/structs.rs:180)
- Consumer: src/gateway/mcp_face/mod.rs:65 — `McpFace::expose: ArcSwap<BTreeSet<String>>`, initialized once at src/gateway/mcp_face/mod.rs:148 and read by `list_tools` / `call_tool` at src/gateway/mcp_face/mod.rs:389,412
- Live caller search: `rg -n 'apply_expose' src --glob '*.rs'` found only the comment at `src/gateway/mcp_face/mod.rs:63`, not a function or call. `rg -n 'mcp_server\.expose' src --glob '*.rs'` found documentation/comments but no executable updater. `rg -n 'unknown_expose\(' src --glob '*.rs'` found the boot call at `src/bin/aleph-server/commands/start/mod.rs:2209` and tests, not a post-reload call. `src/config/reload_impact.rs:84,119` lists live targets as `behavior`, `execution`, `search`, `policies.spend`, and `policies.terminal`; `src/config/live_apply.rs:56-205` applies only that set. No production live caller/updater exists.
- Symptom: Editing `[mcp_server].expose` does not change the running face's exposure set, emit `notifications/tools/list_changed`, or rerun the unknown-name warning until restart. The reload classifier honestly reports `Restart`, so this is not a completely dead config field; it is a missing implementation behind three explicit live-apply design statements (`src/gateway/mcp_face/config.rs:19`, `src/gateway/mcp_face/mod.rs:63`, and `src/bin/aleph-server/commands/start/mod.rs:2207-2209`).
- Hypothesis: CONNECT — the parsed field, swappable `ArcSwap`, per-request readers, warning helper, and notification API all exist; the reload classification/application arm is the missing middle. If restart-only behavior is now intentional, the contradictory live-apply comments must instead be corrected.
- Confidence: high

### S2-02 — Upstream MCP catalog mutations update the face's tool source but never notify face clients
- Form: 4
- Producer: src/mcp/manager/actor.rs:475,493 — `McpManagerEvent::ToolsChanged` is constructed and broadcast; src/mcp/tool_bridge.rs:189-191 consumes `ServerStarted | ToolsChanged` and calls `sync_server`, mutating the canonical registry that the face reads
- Consumer: src/gateway/mcp_face/mod.rs:467 — `McpFace::notify_tools_list_changed`, broadcasting `notifications/tools/list_changed` to the same session streams used by `tools/list`
- Live caller search: `rg -n 'notify_tools_list_changed' src --glob '*.rs'` found the definition, tests, and exactly one production caller, `src/extension/lifecycle.rs:1793`, after plugin transitions. `rg -n 'McpManagerEvent::ToolsChanged|notify_tools_list_changed' src/mcp src/gateway/mcp_face src/bin/aleph-server/commands/start --glob '*.rs'` found no manager/bridge-to-face call. The sibling manager subscriber at `src/bin/aleph-server/commands/start/mod.rs:1593-1644` publishes only gateway `tools.changed`; it never calls the MCP-face notifier. Thus a live caller exists for plugin changes, but not for upstream MCP server changes.
- Symptom: After an upstream server sends its own tools `list_changed` (or starts with a new catalog), Aleph's face can serve the new registry contents on a fresh `tools/list`, but connected downstream MCP clients receive no notification and retain stale cached tool lists. Newly added tools remain undiscoverable to such clients; removed tools can remain advertised client-side until another refresh trigger.
- Hypothesis: CONNECT — bridge the existing manager/registry mutation event to the existing face notifier (with registry synchronization ordered before notification), rather than adding a second catalog or notification protocol.
- Confidence: high

### S2-03 — `notifications/initialized` writes handshake state that production never reads
- Form: 1
- Producer: src/gateway/mcp_face/protocol.rs:89-91 — the notification handler calls `SessionTable::mark_initialized`; src/gateway/mcp_face/session.rs:206 sets `SessionView.initialized = true`
- Consumer: `no consumer found` (grepped: `\.initialized\b`, `initialized:`, and `mark_initialized` under `src/gateway/mcp_face`; production hits are construction/write only). The only reads/assertions are tests at `src/gateway/mcp_face/session.rs:367,419`, `src/gateway/mcp_face/protocol.rs:389`, and `src/gateway/mcp_face/http.rs:705`.
- Live caller search: `rg -n 'mark_initialized|\.initialized\b|initialized:' src/gateway/mcp_face --glob '*.rs'` found the notification producer, field initialization/write, and the tests above. `tools/list` and `tools/call` dispatch at `src/gateway/mcp_face/protocol.rs:119,126` without consulting the flag; no other production module reads it.
- Symptom: Completing the client handshake has no operational effect: the stored bit is not exposed, logged, used for gating, or otherwise consumed. Requests before `notifications/initialized` follow exactly the same production path as requests after it.
- Hypothesis: DECIDE — either connect the state to the intended handshake policy or remove the write-only bookkeeping. The missing consumer is proven, but this audit does not claim a mandatory protocol violation; whether a lenient server is desirable is a product/protocol decision.
- Confidence: medium

## Verified connections / non-candidates

- **MCP route and method parity:** `src/gateway/server/mod.rs:1187-1205` mounts `mcp_routes` when the face and required auth/security dependencies exist. `src/gateway/mcp_face/http.rs` registers GET/POST/DELETE `/mcp`; protocol dispatch has real handlers for `initialize`, `ping`, `tools/list`, `tools/call`, `notifications/initialized`, and `notifications/cancelled`. No client-only method ghost was found.
- **MCP boot composition:** `src/bin/aleph-server/commands/start/mod.rs:2179-2229` reads `app_config.mcp_server`, constructs the canonical-source face, installs its server/global handles, and declines it cleanly when disabled.
- **MCP session safety mechanisms:** principal checks, rate limiting, request/session ownership, cancellation tokens, pending-request removal, DELETE cleanup, and session-expiry cleanup have real production consumers. Cancellation is not a success-only stub.
- **Control plane:** `create_control_plane_router` (`src/gateway/control_plane/server.rs:17`) is mounted by `src/gateway/server/mod.rs:1042`; `serve_static_asset` (`src/gateway/control_plane/server.rs:159`) delegates to real embedded/static serving and has a live panel fallback caller at `src/gateway/server/panel_asset_route.rs:247`. Embedded index and asset lookups are connected.
- **Message assembly:** `MessageAssembler` methods are called by `src/gateway/execution_engine/event_drain.rs:68,73,117,274,279`; assembly/scrubbing implementations in `src/gateway/message_assembly/assembler.rs` are operational rather than stubs.
- **Pipeline:** `src/gateway/pipeline/mod.rs` contains architecture documentation and explicitly records removal of the old orchestrator as dead code. There is no production symbol in this slice requiring registration or dispatch.
- **Stub/cfg sweep:** no production `TODO`, `todo!()`, `unimplemented!()`, empty success handler, or non-test `#[cfg(feature = ...)]` far end was found in the scoped files. The MCP protocol-version header check is explicitly documented as deliberately unimplemented in `src/gateway/mcp_face/http.rs`, so it was not mislabeled as a newly discovered severed wire.
- **Excluded dead helpers:** `McpCaller::is_operator`, `SessionTable::len/is_empty`, and `McpFace::is_enabled` lack meaningful production call sites, but the associated production behavior uses equivalent fields/predicates directly or needs no helper. They do not independently demonstrate a broken feature and were not promoted to candidates. Write-only client/protocol metadata was likewise not promoted because no runtime behavior is promised for it.

## Seams not covered

- No runtime MCP client interoperability, browser, or HTTP integration test was run; this was a static audit only.
- `src/mcp/**` was inspected only along the manager-event/tool-bridge boundary needed for S2-02, not audited as a whole.
- Embedded WebChat frontend internals and the separately maintained Panel repository were not audited; only their server-side asset/fallback seams were checked.
- Tool implementations and every canonical-registry mutation outside the assigned slice were not exhaustively audited.
- Round-3-owned gateway modules were not re-audited except where necessary to establish a live caller or mount point.
