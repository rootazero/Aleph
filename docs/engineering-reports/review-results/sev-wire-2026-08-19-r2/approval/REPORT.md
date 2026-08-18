# Severed-wire audit — `src/approval/` (2026-08-19 round)

Scope: `src/approval/` (12 .rs files, ~3.9k LoC: `mod.rs`,
`adapters.rs`, `audit.rs`, `callback_sink.rs`, `config.rs`,
`guardian_requester.rs`, `mod.rs`, `node_requester.rs`,
`operator_requester.rs`, `policy.rs`, `session_route.rs`,
`tool_call.rs`, `types.rs`). Strict cross-crate budget.

Method: skill methodology — 7 seam lenses (registration parity,
call-vs-handler, classifier-vs-handler, event emit-vs-subscribe,
config-reader, path/route, stub sweep). Read-first triage per
`triage-playbook.md`. All 12 files were read in full; cross-module
consumers in `src/builtin_tools/`, `src/exec/`, `src/gateway/`,
`src/harness/`, `src/tools/scoped/`, `src/bin/aleph-server/`, and
`src/config/` were grep-verified.

## Module map

Approval is the human-in-the-loop decision flow. The module has four
sub-roles:

- **Config policy** — `ConfigApprovalPolicy` (defaults / allowlist /
  blocklist / inheritance), consulted by `policy::check()`.
- **Decision storage** — `ApprovalDecision::Allow | Deny | Ask` enum,
  applied through `OperatorApprovalRequester` / `NodeApprovalRequester` /
  `GuardianApprovalRequester` / `FallbackApprovalRequester` adapters.
- **Notification surface** — `ManagerCallbackSink` + channel-bridge
  adapters, wired through `bin/aleph-server/commands/start/mod.rs`
  (lines 2875–3106) into `approval_gate.set_requester`,
  `set_confirmation_requester`, `set_config_approval_requester`, and
  `inbound_router.with_approval_callback_sink`.
- **Identity tracking** — `CallIdentity` / `current_call_identity` /
  `current_tool_call_id` / `with_call_identity`, used by
  `harness/agent/act.rs`, `tools/scoped/dispatch.rs` + `mod.rs`,
  `builtin_tools/bash_exec.rs`, `exec/manager.rs`.

## Findings

**Total: 0 CUT, 0 CONNECT, 0 DECIDE**

The module is fully wired and well-tested. Specifically verified:

| Seam lens | State |
|---|---|
| `ActionType` variants (24) | All referenced both inside and outside `src/approval/`; exhaustive test `curated_default_covers_every_action_type` pins the curated map. |
| `ApprovalDecision` (Allow / Deny / Ask) | All variants dispatched; no fallthrough. |
| `ConfigApprovalPolicy` (defaults / allowlist / blocklist / inheritance) | All fields read in `check()`. The `inherited_from()` rename-preservation invariant is guarded by `inheritance_is_one_level_and_acyclic`. |
| `matches_glob` | Two live callers (`config.rs` + `config/types/policies/tool_permissions.rs`). |
| `audit_identity` | 8 live callers across browser / desktop / pim / media / automation / system / hooks / channel_bridge. |
| `run_node_approval` | Called by `gateway/server/handler.rs:750`. |
| `ChannelApprovalBridgeAdapter` / `FallbackApprovalRequester` / `GuardianApprovalRequester` / `OperatorApprovalRequester::new` / `::for_config_tier` / `ManagerCallbackSink` | All wired in `bin/aleph-server/commands/start/mod.rs` (lines 2875–3106). |
| `CallIdentity` / `current_call_identity` / `current_tool_call_id` / `with_call_identity` | Wired in `harness/agent/act.rs`, `tools/scoped/dispatch.rs` + `mod.rs`, `builtin_tools/bash_exec.rs`, `exec/manager.rs`. |
| `GatewayEventFrame::ApprovalRequested / Resolved / Expired` | Emitted by `operator_requester.rs` + `node_requester.rs`; consumed by `gateway/surface/r5_router.rs::approval_for`, `gateway/event_visibility.rs::session_identity_of`, `gateway/event_scope.rs`, `gateway/surface/delivery.rs`. |
| `channel_route` (`pub(crate)`) | Single live caller in `adapters.rs`; well-tested recursive resolver. |
| Stubs | No `todo!()` / `unimplemented!()` / `// TODO` in `src/approval/`. |

## Cross-cutting concerns

None. The audit was verification-only.

## Almost-cut but kept

None. After verification, every candidate either had a live consumer
(kept) or didn't surface at all (no candidate to triage). The module
is a healthy example of: (a) compile-time guarded defaults, (b)
exhaustive tests pinning both ends of the seam, (c) explicit
documentation of the seam splits (config-tier vs. fallback leg,
parent-key rename preservation, Panel-turn routing).

## Outcome

Honest summary: this module is well-engineered and fully wired across
all 7 seam lenses. Zero safe-and-reversible findings.