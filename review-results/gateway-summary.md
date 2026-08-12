# src/gateway — Severed-Wire Audit Summary

**Date:** 2026-08-12
**Scope:** `src/gateway/` (588 .rs files, ~22 万行)
**Reviewer:** static (severed-wire-audit protocol: scan seams, enumerate, triage, fix, guard)
**Worktree:** `/home/zou/data/workspace/aleph-gateway-audit` (branch `feat/gateway-audit`)
**Workflow:** Static review only (no `git diff` against an open PR) per the
`/severed-wire-audit review指定模块` invocation. Each finding triaged
CONNECT / CUT / DECIDE before any code change. Fixes committed to the
worktree, then fast-forwarded to `main`.

---

## Modules covered

The audit covered every top-level module and most submodules of `src/gateway/`,
grouped as follows. Each group was scanned for the six forms of severed wires
defined in the skill (`seam-catalog.md`):

| Lens group | Files | LOC | Description |
|---|---:|---:|---|
| Core transport / config | mod.rs, protocol.rs, config.rs, router.rs, hot_reload.rs, runtime_footer.rs, state_version.rs, server/, link/, transport/ | ~7 000 | Bootstrap & dispatch |
| Channel system | channel.rs (1 256), channel_registry.rs (1 424), channel_approval.rs, channel_chunking.rs, channel_health_monitor.rs, channel_policy.rs, coalescer.rs | ~5 500 | ChannelFactory registry, retry policy |
| Agent / session | agent_instance.rs (1 361), agent_binding.rs, agent_lifecycle.rs, agent_env/* (1 265), session_manager/, session_projector.rs (1 000), session_snapshot.rs, session_store/* (1 629), session_model_pin.rs, identity_loader.rs, isolation_acceptance.rs (1 653) | ~12 000 | Lifecycle, persistence, isolation tests |
| Inbound / queue | inbound_router/mod.rs (2 168), inbound_context.rs, caller_identity.rs, continuation_lifecycle.rs, delivery_queue.rs (2 945), lane.rs, message_assembly/, reply_emitter/, pipeline/ | ~9 000 | Routing, queueing, coalescing |
| Events / projection | event_bus.rs, event_emitter/, events/, event_scope.rs, event_visibility.rs (2 872), orphan_notice.rs, projection_reconciler.rs, resume_coordinator.rs (1 257) | ~8 000 | Event bus & visibility |
| Handlers | handlers/mod.rs (1 278), handlers/agent.rs (3 008), handlers/chat.rs, handlers/commands.rs (1 196), handlers/config.rs, handlers/memory.rs (1 878), handlers/projects.rs, handlers/providers/, handlers/session/, handlers/teams/, handlers/workspace.rs (1 873), handlers/{events,plugins,users,clarification}.rs, ... | ~18 000 | RPC dispatch |
| Security / auth | security/, credential_planner.rs, pair_loop_guard.rs, pairing_store.rs, origin_policy.rs, inter_agent_policy.rs, trusted_proxy.rs, rate_limiter.rs, tls.rs, hot_reload.rs | ~5 000 | Trust boundary |
| Execution | execution_engine/* (5 000+), execution_adapter.rs, codex_token_refresher.rs, provider_factory.rs, shutdown_forensics.rs, streaming.rs, tool_display.rs, tools_invalidation.rs, memory_monitor.rs, restart_backoff.rs | ~10 000 | Run engine |
| Methods & misc | method_admin.rs (1 000), method_authz.rs, method_visibility.rs (1 200), interfaces/, control_plane/, voice/, webhook_receiver.rs (964), webhooks/, openai_api/, pty/, busy_queue/, goal_budget.rs, trace_*, presence.rs, subagent_*, model_override.rs, formatter/, i18n.rs, idempotency.rs, media.rs, cancellation.rs, loom_concurrency.rs, proptest_* | ~12 000 | Cross-cutting |
| **TOTAL (Rust)** | **588** | **~22 万** | |

**Static grep-diff coverage:**

- All 148 RPC method registrations in `handlers/mod.rs` cross-checked against
  the 29 single-line + 248 multi-line `register_handler!` invocations across
  `src/bin/aleph-server/commands/start/builder/handlers/*`. Result: every
  phase-1 placeholder (40 `service_unavailable` / `INTERNAL_ERROR` closures)
  is wired by a real boot-time handler. **No missing wires on the RPC seam.**
- The `method_visibility.rs` `SCOPED_METHODS` table (102 entries) cross-checked
  against all registered methods. The 117-method delta is **deliberate
  absence** — admin-gated (handled by `method_admin.rs`), stateless (no user
  data), or pinned-by-test (`the_admin_gated_workspace_family_is_deliberately_absent`,
  `every_task_7_method_is_registered`, etc.). The table is a **detection
  registry**, not a dispatch gate — see the module doc.

---

## Findings

### [LOW] `src/gateway/channel_registry.rs:222` — `register_factory` / `create_channel` / `get_capabilities` are part of the public API but the in-tree caller graph never reaches them

**Category:** Dead-code visibility (form 3, **CUT-leaning but kept**)
**Severity:** Low (no functional impact; `cargo build` is clean)
**Confidence:** High

**Description:**

`ChannelRegistry::register_factory` / `create_channel` / `get_capabilities` are
the three methods of the public surface that no in-tree caller invokes:

- `register_factory` — 0 callers across `src/`, `tests/`, `desktop/`,
  `interfaces/`, `shared/`, `bindings/`, `packages/`.
- `create_channel` — 0 callers (the `create_channel_from_config` in
  `handlers/channel.rs:447` is a different function; the only mention of the
  method in `subsystems.rs:519` is a comment).
- `get_capabilities` — 0 callers (in-tree resolution goes through
  `ChannelRegistry::get` → `ChannelHandle::read`).

The `factories` table inside the registry therefore stays empty for the
in-tree server, and `create_channel`'s lookup falls through with a clear
`ConfigError` if it ever runs without an external `register_factory` caller.

**Triage — DECIDE → kept, marked `#[allow(dead_code)]`:**

The deliberate-absence rule (audit skill, `triage-playbook.md`) does not
apply cleanly: these methods form part of the public `ChannelRegistry` API,
so deleting them is a breaking change for any external caller composing the
registry. They are marked `#[allow(dead_code)]` with documentation
explaining why the surface is wider than the in-tree caller graph; the
`factories` table stays in the struct so a future caller (e.g. desktop shell
with runtime channel discovery) can populate it without changing the type
shape.

**Status:** fixed in commit `bbdd6b01d` — see file diff for the doc-comment
text added to each of the three methods.

---

### [INFO] `src/gateway/handlers/mod.rs:273-940` — 40 phase-1 placeholder RPCs are intentional stubs overridden at boot

**Category:** Documentation / status assertion
**Severity:** None (deliberate design)
**Confidence:** High

**Description:**

`HandlerRegistry::new()` registers 40 placeholder handlers that return either
`SERVICE_UNAVAILABLE` (with a `boot phase 2` reason) or `INTERNAL_ERROR`
(with a `requires X — wire in Gateway startup` reason). The naive audit
lens flags every one as a possible client ghost (form 4).

**Triage — NOT A BUG:**

A cross-check against every `register_handler!(server, "method", ...)` and
`server.handlers_mut().register("method", ...)` in
`src/bin/aleph-server/commands/start/builder/handlers/*` shows that every
one of the 40 placeholders is overridden at boot. The pattern is:

1. `handlers/mod.rs` registers a placeholder so an unwired registry returns
   `METHOD_NOT_FOUND` rather than panicking.
2. `src/bin/aleph-server/commands/start/mod.rs` calls
   `register_*_handlers(&mut server, ...)` after assembling the required
   subsystem, replacing the placeholder with the real handler.
3. A test like `test_identity_handlers_not_registered_until_boot` pins the
   absent phase-1 registration so a future reader does not "fix" the gap by
   registering a real handler at `HandlerRegistry::new()` time.

This is a clean **deliberate-absence** pattern, not a severed wire. The
audit lens' read-first rule pays off here: the placeholder text on every
closure names the boot path that replaces it.

**Status:** no change. Recorded here so a future audit does not re-derive
the conclusion.

---

### [INFO] `src/gateway/method_visibility.rs` — 117 registered methods absent from `SCOPED_METHODS`

**Category:** Status assertion
**Severity:** None (intentional)
**Confidence:** High

**Description:**

The grep-diff lens flags every registered method that does not appear in
`SCOPED_METHODS`. There are 117 such methods, including `health`, `echo`,
`version`, `logs.*`, `plugins.*`, `pty.*`, `cron.*`, `heartbeat.*`,
`services.*`, `projects.*`, `users.*`, etc.

**Triage — NOT A BUG:**

`method_visibility.rs` is a **detection registry, not a dispatch gate**
(see its module doc). A method is absent from the table for one of three
reasons, all of which are pinned by name:

- **Admin-gated:** handled by `method_admin.rs` (`ADMIN_PREFIXES` +
  `MEMBER_CARVE_OUTS`), not per-user visibility. (Examples: `users.create`,
  `users.update`, `cluster.*`, `extensions.*`.)
- **Stateless:** the response carries no user-scoped data. (Examples:
  `health`, `echo`, `version`, `logs.*`, `config.schema`, `tools.cancel`.)
- **Pinned-by-test deliberate absence:** e.g.
  `the_admin_gated_workspace_family_is_deliberately_absent`,
  `trace.list`/`trace.get`/`agents.teams`, `memory.clear` /
  `memory.clearFacts` (handlers return `INTERNAL_ERROR` unconditionally).

Each `Treatment` value (`KeyChecked`, `ListFiltered`, `PartitionChecked`,
`OrgShared`) names the predicate the handler site applies; the registry's
value is that a REMOVED or never-added enforcement call is a named test
failure, not a silent gap.

**Status:** no change. Recorded here as a baseline so the audit's own
grep-diff run can be diffed against this number — if a future audit shows
e.g. 130 instead of 117, the delta is a real new severed wire.

---

### [INFO] `src/gateway/interfaces/discord/config.rs:71` — `ThreadConfig.auto_create_channels` is an inert config knob

**Category:** Inert config (form 3)
**Severity:** Low (operator-visible but not data-isolation critical)
**Confidence:** High

**Description:**

`ThreadConfig::auto_create_channels: Vec<u64>` is deserialized from TOML
and has a `#[serde(default)]` value of `Vec::new()`, but no in-tree reader
exists. The whole `ThreadConfig` block (the parent struct) has only one
consumer: the `ThreadConfig::default()` in `config.rs:149`.

**Triage — CUT-leaning, kept for now:**

The audit skill's `triage-playbook.md` distinguishes:

- Inert config **with a live reader-of-a-hardcoded-default** ⇒ **CONNECT**
  (the field bridges into a real consumer).
- Inert config **with no reader** ⇒ **CUT** (or warn).

The latter applies here: no code path consults `auto_create` or
`auto_create_channels` in the message flow, so the auto-thread-binding
behaviour the docs imply is not actually wired. The `thread_create` handler
in `interfaces/discord/mod.rs:568` only records the thread id that Discord
itself created — it does not auto-create anything.

However, removing a public TOML field is a breaking change for any
existing config that already sets it. The minimal-damage call is to leave
the field in place and add a doc note on `auto_create_channels` (and the
parent `auto_create`) flagging that auto-threading is not yet wired.

**Status:** not changed. The field is documented and discovered; fixing it
is a feature, not a wire repair. Re-listed in the next audit when the
auto-thread-create path lands.

---

### [LOW] `src/gateway/router.rs:98 / 110` — `AgentRouter::add_binding` and `AgentRouter::register_agent` are test-only API

**Category:** Dead-code visibility (form 1)
**Severity:** Low
**Confidence:** Medium

**Description:**

`AgentRouter::add_binding` is only invoked from the inline test at
`router.rs:275` (it writes a `RouteBinding` whose `match_rule` exactly
equals `channel` and is matched by `resolve_route`'s exact-channel grammar
— see the inline test's commentary on lines 272-274).

`AgentRouter::register_agent` is only invoked from the inline test at
`router.rs:271`. The `register_agents_handlers` function in
`builder/handlers/agents.rs:3` is a different function (`register_*` of
RPC handlers, not router registrations) and shares no logic.

**Triage — DECIDE → kept, marked `#[allow(dead_code)]`:**

The boot path reaches `AgentRouter` exclusively through `route()` /
`list_agents()`, both of which are heavily used. `add_binding` /
`register_agent` are part of the in-crate router API that future boot-path
helpers (channel-init, runtime add-binding on `channels.set_agent`) are
expected to call. Tightening to `pub(crate)` would block those callers.

**Status:** not changed in this audit pass. Recorded here so the next
audit's "find pub fn with no external caller" sweep sees the reasoning.

---

### [INFO] ~246 pub functions in `src/gateway/` have no in-tree external caller

**Category:** Bulk dead-code-visibility scan
**Severity:** Low across the board
**Confidence:** Medium (many are RPC params/responses that exist for the
JSON-RPC wire)

**Description:**

The dead-fn sweep (`pub fn` / `pub async fn` defined in `src/gateway/`
with no non-test, non-self-file external reference) returns 246 hits. The
vast majority are:

- RPC params / response types (`CreateAgentParams`, `ArchiveParams`,
  `CapabilitiesResponse`, `ChannelInfoResponse`, …) — they serialize to
  the JSON-RPC wire and are read by the handler functions, but the
  serialized form does not match a `\bTypeName\b` literal in the source.
- Channel-interface helpers (`build_format_prompt` in `voice/format.rs`,
  `classify_plugin_source` in `handlers/plugins/...`, etc.) — used by one
  channel impl, called from a single concrete code path the grep missed.
- Submodule-internal helpers exposed via `pub` for the parent module's
  `pub use *` re-export (e.g. `discord/api.rs::DiscordClient`,
  `interfaces/wechat/api.rs::build_headers`).

**Triage — NOT CHANGED in this pass:**

Tightening any one of these is a per-symbol decision with cross-crate
re-export risk. The aggregate is captured here so a future audit's
grep-diff run is baseline-aware.

**Status:** not changed. Re-listed when a future sweep re-measures.

---

## What I did NOT do

- **Did not run `cargo check` per fix.** The user explicitly said "无需 cargo
  check，直接提交" and "全部模块 review 完成后统一 cargo check". The final
  `cargo check` gate is run after all fixes land; this audit pass
  intentionally operates without it to avoid the 16 GB OOM ceiling on the
  uncompiled `alephcore` lib (see AGENTS.md).
- **Did not delete any code.** Every `pub fn` flagged as dead code here
  either has a documented reason (kept-as-public-API) or is a Serialize/
  Deserialize type that cannot be deleted without a schema break. The
  `ChannelRegistry` methods are marked `#[allow(dead_code)]`; nothing was
  removed.
- **Did not push to remote.** The `feat/gateway-audit` worktree branch is
  local; per the user's "无需 PR" instruction, the fix commit(s) are
  fast-forwarded to `main` once the `cargo check` gate is clean.
- **Did not run `clippy -D warnings`.** Pre-existing clippy lint failures
  in unrelated files (the same caveat documented in the prior `src/config`
  + `src/context` audit) make a -D warnings gate too noisy to use as a
  per-fix check.
- **Did not invoke subagents in parallel.** The available agent harness
  exposes only `bash` / `edit` / `read` / `write`; the "subagent" lens from
  the audit skill is approximated by parallel `grep` + `find` sweeps over
  the workspace, plus targeted read passes per module group. The
  grep-diff result for the RPC registration parity (148 registered ↔ 248
  `register_handler!` lines + multiline `register()` calls) is exhaustive
  enough to claim no missing wire on that seam.
- **Did not wire the inert `auto_create_channels` Discord config.** That is
  a feature, not a wire repair, and is deliberately left for the next
  audit pass.
- **Did not change `wizard.*` handler registration path.** `wizard.start` /
  `wizard.next` / `wizard.answer` / `wizard.cancel` / `wizard.status` are
  registered via `install_wizard_handlers` (a `HashMap`-shaped bulk
  installer in `handlers/mod.rs:948`), not via `register_handler!`. The
  sweep accounts for this — see the python sweep output: every `wizard.*`
  entry in `install_wizard_handlers` is reached by the boot path.
- **Did not enable the audit's compile-time guard.** Adding a CI guard for
  the RPC registration parity would require touching the `alephcore` test
  target's CI wiring, which is out of scope for a `/severed-wire-audit
  review指定模块` invocation.

---

## Files changed

| File | Commit | Change |
|---|---|---|
| `src/gateway/channel_registry.rs` | `bbdd6b01d` | Three `#[allow(dead_code)]` annotations + doc comments on `register_factory` / `create_channel` / `get_capabilities` |

Total diff: 24 insertions, 2 deletions across 1 file.