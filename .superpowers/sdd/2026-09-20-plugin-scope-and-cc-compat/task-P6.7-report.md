# Task P6.7 — Wire the face into GatewayServer + boot install/decline

Plan: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P6P7-mcp-face.md` § "Task P6.7".
Scope: HEAD 21d1d3f14 (the `mcp_face: Streamable HTTP routes on /mcp with the /ws guards, sessions and SSE` tip before this task).

## Diff (3 files)

- `docs/reference/GATEWAY.md` — `+6` lines. The "Alongside WebSocket, Gateway serves:" bullet list gains the MCP server face row (`POST/GET/DELETE /mcp`, Streamable HTTP, `[mcp_server].expose` whitelist, loopback free / remote bearer, `notifications/tools/list_changed` on plugin transitions, real-machine `qa/mcp_face/run.sh`). R6.8 / G-6 same-commit pointer; the full "MCP 面" section is P8.5.
- `src/gateway/server/mod.rs` — `+166/-1`. New `mcp_face: Option<Arc<McpFace>>` field, `mcp_face: None,` in both constructors, `pub fn set_mcp_face`, `pub fn operator_presence_probe` (reads `connections` live, requires `!first_message && role_is_operator`), and the new `build_router` `match` that mounts `/mcp` only when `Some(face) && Some(device_token_mgr) && Some(security_store)` (otherwise warns and leaves the path to the SPA fallback). Also the two new tests next to `webhook_prefix_is_always_routed_and_404s_when_nothing_is_mounted`.
- `src/bin/aleph-server/commands/start/mod.rs` — `+76`. New MCP face block inserted after the `set_config_broadcaster` block at L2005, before the Panel voice channel. Three arms on `(mcp_server_cfg.enabled, agent_result.tool_registry.as_ref())`: decline with a reason when `enabled == false` or when there is no tool registry (simulated mode), or build the face over the SAME `BuiltinToolRegistry` the run loop dispatches through, G5-warn on `unknown_expose`, print the banner line `MCP server face: /mcp (N tools exposed[, M unknown at boot — see log])` unless `--daemon`, then both `server.set_mcp_face(face.clone())` and `mcp_face::install_mcp_face(face)`. Sibling census test `boot_installs_or_declines_the_mcp_face` (same shape as `boot_installs_the_spend_policy_and_the_spend_ledger`, comment-stripped + CRLF-safe).

## Tests run (all PASS, foreground bounded)

| Command | Result |
|---|---|
| `cargo check -p alephcore --lib --tests --bin aleph-server` | Finished, 0 errors |
| `cargo test -p alephcore --lib gateway::server::tests::mcp_route_is_mounted_only_when_a_face_and_auth_handles_are_set` | ok |
| `cargo test -p alephcore --lib gateway::server::tests::operator_presence_probe_reads_the_connection_table` | ok |
| `cargo test -p alephcore --lib gateway::mcp_face::` | 62 passed, 0 failed |
| `cargo test -p alephcore --lib gateway::server::` | 143 passed, 0 failed |
| `cargo test -p alephcore --lib capability::` | 55 passed, 0 failed |
| `cargo test -p alephcore --bins boot_` | 6 passed: `boot_installs_or_declines_the_mcp_face` + 5 siblings |

Census red→green: the plan predicted `boot_installs_or_declines_the_mcp_face` would FAIL with `install_mcp_face(` not in production text; the new install/decline block made it PASS. Sibling censuses (`boot_installs_the_spend_policy_and_the_spend_ledger`, `boot_installs_or_declines_the_team_background_stores`, `boot_registers_every_users_method_the_default_registry_has`, `boot_marker_is_greppable_and_carries_fields`, `handles_are_installed_before_the_first_extension_load`) all still green — no regression.

## Lint / format / scope guard

| Check | Result |
|---|---|
| `git diff --check HEAD` | clean |
| `rustfmt --check --edition 2021 src/gateway/server/mod.rs src/bin/aleph-server/commands/start/mod.rs` | clean |
| `git diff 21d1d3f14 -- src/harness/` | empty (this task did not touch `src/harness/`) |

## What was NOT done (negative list, per AGENTS.md)

- No P7.1 `qa/mcp_face/{run.sh,drive.py,patch_mcp.py}` — out of P6.7 scope.
- No P6.8 (`packages/pi-aleph/`) — out of scope.
- No P6.9 (`apply_expose` / `live_apply.rs` / `reload_impact.rs`) — out of scope.
- No edits to `src/capability/` census rules; `capability::census` ran green unchanged.
- No `--no-verify`, no `git add -A`, no stash, no `~/.claude/.aleph` writes, no `aleph-server` boot.

## Known gap (plan Step 5 mutation)

The plan's planned mutation step ("drop `!c.first_message &&` from `operator_presence_probe`") was not exercised in this commit; the test exists and pins the predicate. P7.1 will exercise the wired face end-to-end against a booted daemon.