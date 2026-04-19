# Phase 1–4 Production E2E Report — 2026-04-19

**Environment**
- Build: `just build` → `target/release/aleph-server` (release, 4m 18s)
- Server version banner: `Aleph Gateway v2026.04.18`
- Process: single instance at PID 28302, bound to `0.0.0.0:18790`
- Gateway token: `aleph-9976129a-407d-4893-a96c-6467b24bedac`
- Env: `ALEPH_HARNESS_V2=1 RUST_LOG=info`
- Session backend (runtime observation): **FileSessionStore** at `~/.aleph/data/sessions`
- Main HEAD at time of test: `f2ad56c29` (post cargo-fmt commit, after Phase 4 merge)

## Test Matrix (executed)

| # | Scenario | Agent | Phase focus | Result |
|---|---|---|---|---|
| T1 | Non-streaming smoke chat | `aleph/default` | Phase 2 routing + AgentLoop hot path | ✅ 200, structured JSON response |
| T2 | Elevated tool attempt (`bash`, `code_exec`) | `aleph/coding` | Phase 3 ApprovalGate + Phase 4a H4 | ✅ Auto-denied (no approver wired) — expected safe default |
| T3 | File listing (baseline tool) | `aleph/main` | Phase 2 ToolService façade + Phase 4a H4 exec-class split | ✅ `file_ops → execute`, agent returned real directory contents |
| T4 | Multi-turn context retention | `aleph/main` | AgentLoop turn bookkeeping + memory tool | ✅ R2 correctly recalled "ALPHA-7734" from R1 |
| T5 | Streaming SSE | `aleph/main` | Phase 4a streaming relocation | ✅ `data:` frames + delta chunks + `[DONE]` terminator |
| T6 | Session isolation probe (X-Session-Id header) | `aleph/main` | Gateway session routing | ℹ️ All requests route to one session key `agent:main:peer:openai-api-client` — X-Session-Id header ignored by OpenAI-compatible endpoint. Cross-session recall works via persistent MEMORY.md (design) |

## Phase-by-Phase Verdict

### Phase 1 — SessionService event log
- **Boot evidence**: `ToolService chain assembled (Phase 2)` and `SharedTokenManager: restored persisted HMAC secret from DB`.
- **Runtime observation**: `build_sqlite_session_service()` only wires `SessionService` into `SessionManager` **when the SQLite backend is active**. The production runtime uses `FileSessionStore` (file backend at `~/.aleph/data/sessions`) → **Phase 1 dual-write is not active in this deployment**.
- **Architectural status**: correct per plan; Phase 6 migrates file backend onto `SessionService`. The 9076 library + 2 integration tests (`harness_run_e2e.rs` + `shim.rs` + `tool_trace.rs`) continue to cover Phase 1 behavior.
- **Verdict**: ✅ code shipped, ✅ tests green, ⏸️ production dual-write awaits Phase 6 (expected).

### Phase 2 — ToolService façade
- Boot: `ToolService chain assembled (Phase 2)`, `ExtensionManager: tool registry wired`.
- Runtime: every tool call in the AgentLoop hot path surfaces through `tool_pipeline` — observed for `bash`, `code_exec`, `file_ops`, `file_edit`.
- Middleware chain active: `permission resolved, tool=<name>, hook_decision=none, safety_passed=true, final_action=<confirm|execute>`.
- **Verdict**: ✅ production path confirmed.

### Phase 3 — Sandbox + LayeredPermissionResolver
- Boot: `Sandbox: WorkspaceSandbox rooted at /Users/zouguojun/.aleph/workspaces`.
- Boot: `wired LayeredPermissionResolver into PermissionLayer with global tool permissions (per-agent overrides plug in at Phase 4 session activation), default=Allow, overrides=0`.
- Permission decision visible per tool call. H5 fixes (env/timeout/max_output_bytes) are in the built binary; the test path didn't exercise sandbox execution because elevated tools were denied before `execute()`. This was expected (no approval responder on the OpenAI API channel).
- **Verdict**: ✅ wired and responsive; execution leg not triggered in this harness (requires an approval responder to cross the gate).

### Phase 4a — six relocation PRs
| ID | Fix | Runtime evidence | Status |
|---|---|---|---|
| H1 | `ApprovalRequester` adapter over `ChannelApprovalBridge` | Adapter compiled in; production wiring deliberately NOT done (revert commit `f62385f93` kept the adapter decoupled until Phase 5). Boot warn `ApprovalGate has no ApprovalRequester wired` is the expected marker. | ⏸️ Adapter shipped, production threading deferred |
| H2 | `SESSION_ID` scope propagation | **0** occurrences of `no active session context` across all test traffic | ✅ |
| H4 | Exec-class exclude-list on `LayeredPermissionResolver` | `bash/code_exec → final_action=confirm`, `file_ops → final_action=execute` — exec vs. baseline cleanly split | ✅ |
| Retry | `providers/llm_retry.rs` relocation | Two transient-error retries observed: `minimax → chatgpt → moonshot` fallback chain fired | ✅ |
| Safety | `session/ingress_safety.rs` relocation | Compiled and loaded; no misfires during clean chat traffic | ✅ compile-time |
| Streaming | `session/streaming.rs` relocation | SSE emitted proper `data:` chunks and `[DONE]` sentinel | ✅ |

### Phase 4b — Harness Think→Act
- **Harness code**: compiled into release binary (`strings` shows `ALEPH_HARNESS_V2` literals present; `tests/harness_run_e2e.rs` + `src/harness/tests/*` passed during build).
- **Discoverability warn**: 🐛 **did not fire** — see "Bugs found" §1.
- **Production driver swap**: not performed (correct for this phase; Phase 5 wires the orchestrator bridge).
- **Verdict**: ✅ library + integration tests cover Harness; ⚠️ one minor ordering bug in the discoverability log-line.

## Bugs found

### 1. `ALEPH_HARNESS_V2` discoverability warn never emits
- **Location**: `src/bin/aleph-server/commands/start/mod.rs:384–399`
- **Cause**: the env-var read + `tracing::warn!` block runs inside `start_server()` at line 389. `initialize_tracing(args)` is only called later at line 415. The warn therefore writes to a no-op subscriber and is lost. `strings` confirms the message is compiled in; `grep` on the log confirms it never lands.
- **Severity**: Low. The flag is discoverability-only; no runtime behavior depends on it.
- **Suggested fix**: move lines 384–399 to after line 415 (or fold them into the post-`initialize_tracing` boot section). One-line move, no test change.

### 2. Pre-existing: legacy `sessions.db` migration Null column
- `Warning: Session migration failed: Configuration/Database error: Row error: Invalid column type Null at index: 11, name: input_tokens`
- Not introduced by Phase 1–4; legacy SQLite → file migration. Logged as warning, non-fatal.

### 3. Pre-existing: external embedding provider 4xx
- T8Star `https://ai.t8star.cn/v1/embeddings` — `error sending request for url` — notes retrieval falls back to `skeleton_fallback_v1`. External dependency, not Phase 4.

## Non-regression anchors
- Baseline (pre-Phase 4): 9067 / 2 known-fail / 20 ignored
- Post-Phase 4 (commit `77a5b9061`): 9076 / 2 known-fail / 20 ignored (+9 Harness/driver tests)
- Integration tests: `tests/harness_run_e2e.rs` 2 passing, `tests/session_scope_propagation.rs` 1 passing
- Cargo build clean on commit `f2ad56c29` (style) `Finished release profile [optimized] target(s) in 4m 18s`

## What this E2E DID NOT cover (scope limits)

1. **Phase 3 WorkspaceSandbox `execute()`** — needs an ApprovalRequester on the channel so the gate can be crossed. The OpenAI-compatible peer channel has no human approver in loop.
2. **Phase 1 dual-write session_events writes** — production uses FileSessionStore; Phase 6 wires the file backend onto SessionService.
3. **Cron / heartbeat paths (H2 deeper coverage)** — exercised only indirectly via boot-time propagation; no scheduled job was triggered during the 20-minute window.
4. **MCP external server tool calls** — no MCP servers were configured/listed during the test.
5. **Channel-based interfaces** (Telegram / Discord / Matrix etc.) — out of scope; tested only through OpenAI HTTP API, which is the most direct path to exercise the agent hot loop.

## Recommendation

- Phase 1–4 architectural refactor ships and behaves as designed in production traffic (~6 chat completions, 1 SSE stream, 4+ tool invocations, retry fallback chain, multi-turn memory recall).
- Merge `main` is ready for release (`just release YYYY.MM.DD`) modulo the one-line fix for Bug #1 above, which is strictly a log cleanup.
- Production driver swap, full approval round-trip, and FileSessionStore event log all land in Phase 5 + 6 as scheduled.
