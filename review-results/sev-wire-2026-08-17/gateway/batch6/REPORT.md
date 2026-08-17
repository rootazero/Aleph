# Severed-Wire Audit — `src/gateway` Batch 6

**Date:** 2026-08-17
**Method:** PRODUCED − CONSUMED via `rg` across `src/`, `bin/`, `interfaces/`, `shared/`. Read-before-write triage for borderline candidates.
**Scope:** Top-level gateway `.rs` files not covered by batches 1–5 (mod.rs + ~30 files across `busy_queue/`, `control_plane/`, `link/`, `message_assembly/`, `middleware/`, `pipeline/`, `pty/`, `surface/`, plus the listed top-level files).

---

## Summary

11 candidates inspected → **8 findings** (1 critical, 3 high, 4 medium/low). Most modules in scope are heavily wired and CONNECTED. The notable severed wires are concentrated in:

- **`gateway/pipeline/types.rs`** — entire module dead (no external consumer; only `pub mod pipeline;` re-export and a doc-comment reference).
- **`gateway/middleware/context.rs`** — `GatewayRequestContext` + `TraceFlags` declared `pub` but unused outside the file.
- **`gateway/assets.rs`** — empty 0-byte file (not declared in `mod.rs`; leftover on disk).
- **`gateway/event_bus.rs`** — three internal-only types (`TopicSubscription`, `FieldPredicate`, plus pub-helper `topic_matches`) with no external consumer.

---

## Findings Table

| ID | File(s) | Item(s) | Severity | Decision | Form |
|----|---------|---------|----------|----------|------|
| sw-gateway-01 | `src/gateway/pipeline/types.rs` | `MediaCategory`, `LocalMedia`, `UnderstandingType`, `MediaUnderstanding`, `MergedMessage`, `EnrichedMessage`, `PipelineError` | critical | CUT | 1 (no consumer) |
| sw-gateway-02 | `src/gateway/middleware/context.rs` | `GatewayRequestContext`, `TraceFlags` | high | CUT | 1 (no consumer) |
| sw-gateway-03 | `src/gateway/assets.rs` | (empty file, 0 bytes) | high | CUT | 1 (no producer) |
| sw-gateway-04 | `src/gateway/event_bus.rs` | `TopicSubscription`, `FieldPredicate` | medium | CUT | 1 (internal-only) |
| sw-gateway-05 | `src/gateway/surface/r5_router.rs` | `notification_for`, `approval_for` (pub) | medium | DECIDE | 1 (intra-module only) |
| sw-gateway-06 | `src/gateway/link/types.rs` | `LinkConfig`, `LinkId`, `BridgeId`, `LinkRoutingConfig`, `DmPolicyConfig`, `GroupPolicyConfig` | medium | DECIDE | 1 (re-exported; `LinkManager::new` is the only consumer entry) |
| sw-gateway-07 | `src/gateway/channel_policy.rs` | `ChannelAccessConfig`, `E164Number` | low | CUT | 1 (doc claims "consumed by inbound_router" but inbound_router has its own twin types) |
| sw-gateway-08 | `src/gateway/event_bus.rs` | `topic_matches` (re-exported via `gateway::topic_matches`) | low | DECIDE | 1 (re-exported but no external consumer) |

---

## Findings Detail

### sw-gateway-01 — `src/gateway/pipeline/types.rs` (critical, CUT)
All seven `pub` types are unused outside the file. `pub mod pipeline;` is declared in `gateway/mod.rs:93` and re-exports `types::*` (`gateway/pipeline/mod.rs:13`), but no crate code constructs or reads a `MergedMessage` / `EnrichedMessage` / `LocalMedia` / etc. The pipeline is documented as the contract for an inbound-routing merge phase, but the actual merger (`coalescer`) does its own work directly. **Form 1 — no consumer.** Proposal: delete the module; if a pipeline contract is intended, port it into `coalescer` or `inbound_router`.

### sw-gateway-02 — `src/gateway/middleware/context.rs` (high, CUT)
`GatewayRequestContext` and `TraceFlags` are declared `pub` and re-exported via `middleware/mod.rs:43` (`pub use context::{GatewayRequestContext, TraceFlags};`), but no external code constructs or reads them. They appear intended for middleware-chain inter-stage state but nothing currently populates them. **Form 1 — no consumer.** Proposal: delete the file; reintroduce if a middleware pass ever needs request-level state.

### sw-gateway-03 — `src/gateway/assets.rs` (high, CUT)
The file is **0 bytes** (`wc -l` = 0; `ls -la` confirms 0 bytes since 2025-06-24). It is not declared as `pub mod assets;` in `gateway/mod.rs` (verified). Build does not include it, but it lingers on disk. **Form 1 — no producer.** Proposal: `rm src/gateway/assets.rs`.

### sw-gateway-04 — `src/gateway/event_bus.rs` `TopicSubscription` / `FieldPredicate` (medium, CUT)
Both are `pub` and only referenced inside `event_bus.rs` itself (via `TopicFilter::subscriptions`, `where_clause_matches`, etc.). `TopicFilter` is re-exported (`gateway/mod.rs:156`) and that re-export also has no external consumer (see sw-gateway-08). **Form 1 — no consumer.** Proposal: drop `pub` on both types (they're already private to the module's logic); the `TopicFilter` API surface keeps its public contract.

### sw-gateway-05 — `src/gateway/surface/r5_router.rs` `notification_for` / `approval_for` (medium, DECIDE)
Both are `pub fn` but the only caller in the crate is `run()` (lines 92, 98) plus unit tests. The R5 router deliberately exposes these so future shells could call them; today they are test-only utilities. Borderline because: (a) keeping them `pub` is cheap, (b) the doc comments are the architecture source-of-truth. **Form 1 — no external consumer.** Proposal: leave as-is; revisit when a second surface (mobile, voice) is wired.

### sw-gateway-06 — `src/gateway/link/types.rs` (medium, DECIDE)
`LinkConfig`, `LinkId`, `BridgeId`, `LinkRoutingConfig`, `DmPolicyConfig`, `GroupPolicyConfig` are `pub` and re-exported via `link/mod.rs:5` (`pub use types::*;`). `LinkManager::new` is the only external entry point (consumed at `bin/aleph-server/commands/start/builder/subsystems.rs:514`), and `scan_link_configs` is the only public constructor for `Vec<LinkConfig>` — used only inside `link/manager.rs`. The types are part of the LinkManager's external contract (config-file schema) even though no Rust code outside `link/` consumes them by name. **Form 1 — no external consumer.** Proposal: keep; they're the YAML/JSON schema for external bridge plugins.

### sw-gateway-07 — `src/gateway/channel_policy.rs` `ChannelAccessConfig` / `E164Number` (low, CUT)
Module doc claims (`channel_policy.rs:5`): *"types ([`ChannelAccessConfig`] / [`DmPolicy`] / [`GroupPolicy`]) consumed by inbound_router"*. But `inbound_router/types.rs` defines its own `ChannelConfig` / `DmPolicy` / `GroupPolicy` (twin types at `inbound_router/types.rs:222`, `:230`) and never imports `channel_policy::*`. `ChannelAccessConfig` and `E164Number` are unused outside `channel_policy.rs` and its tests. `DmPolicy` / `GroupPolicy` *are* consumed (interfaces/whatsapp — separate finding). **Form 1 — no consumer for `ChannelAccessConfig` / `E164Number`.** Proposal: drop `ChannelAccessConfig` and `E164Number`; fix the misleading doc comment.

### sw-gateway-08 — `src/gateway/event_bus.rs` `topic_matches` (low, DECIDE)
`pub fn topic_matches` is re-exported at `gateway/mod.rs:156` (`pub use event_bus::{topic_matches, TopicEvent, TopicFilter};`). No external code calls it. However, `TopicEvent` *is* consumed externally (`bin/aleph-server/commands/start/mod.rs:2319`, `bin/aleph-server/commands/start/builder/handlers/config.rs:105,121`). The re-export block stays for `TopicEvent`; `topic_matches` is dead. **Form 1 — no consumer.** Proposal: drop `topic_matches` from the `pub use` re-export (keep it as `pub(crate)` or private).

---

## What I Did NOT Cut (verified CONNECTED)

| Module | Consumer(s) |
|--------|-------------|
| `codex_token_refresher` | `bin/aleph-server/commands/start/mod.rs:1762`; `execution_engine/run_loop/inner.rs:1473,1475` |
| `agent_lifecycle::AgentLifecycleEvent` | `builtin_tools/agent_manage/{create,delete}.rs`; `agent_binding.rs`; `handlers/agents.rs:234` |
| `continuation_lifecycle::*` | `resume_coordinator.rs:703`; `handlers/session/db_handlers/{modify,create}.rs`; `inbound_router/command_handler.rs:314` |
| `coalescer::MessageCoalescer` | `inbound_router/mod.rs:124,301,374-414`; `bin/aleph-server/.../subsystems.rs:640` |
| `channel_approval::ChannelApprovalCapability` | `interfaces/telegram/{approval,mod}.rs` |
| `channel_chunking::WhatsAppChunker` / `ChunkMode` | `interfaces/wechat/mod.rs:168`; `interfaces/whatsapp/config.rs` |
| `channel_health_monitor::ChannelHealthMonitor` | `bin/aleph-server/commands/start/mod.rs:3116`; `channel_registry.rs:757,890`; `config.rs:208` |
| `channel_policy::{clamp_tier_for_channel, channel_permission_level_from_role, set_channel_config_snapshot, wait_for_channel_config_snapshot, system_continuation_identity}` | widely used in `execution_engine/*`, `resume_coordinator`, `inbound_router`, `bin/aleph-server` |
| `channel_registry::ChannelRegistry` | `clarification/ask.rs`, `builtin_tools/{ask_user,channel_directory,channel_message,channel_outbox,channel_manage}`, `executor/builtin_registry`, `tasks/cron`, `teams/*` |
| `event_bus::GatewayEventBus`, `ConfigChangedEvent`, `RuntimeInstallProgressEvent`, `GatewayEvent`, `TopicEvent` | canvas, executor, webchat, `bin/aleph-server` |
| `event_scope::{EventScopeGuard, scope_for_role, is_superuser_scope}` | `server/{handler,probe,mod}.rs`; `handlers/users.rs` |
| `event_visibility::{EventVisibilityIndex, session_identity_of, SessionIdentity}` | `execution_engine/{execute,session_run_registry}`; `server/handler.rs`; `approval/operator_requester.rs` |
| `delivery_queue::{DeliveryStore, DeliveryQueueConfig, spawn_drain, take_media_custody, now_secs, ...}` | `bin/aleph-server/.../subsystems.rs:172-254`; `builtin_tools/channel_outbox.rs`; `channel_registry.rs` |
| `credential_planner::build_credential_plan` | `handlers/gateway_credentials.rs` |
| `link::LinkManager` | `bin/aleph-server/commands/start/builder/subsystems.rs:514` |
| `message_assembly::MessageAssembler` | `execution_engine/event_drain.rs:32` |
| `middleware::MiddlewareChain`, `AuthLayer`, `RateLimitLayer`, `TraceLayer`, `RedactQueryLayer`, `MetricsLayer`, `HandlerLayer`, `ValidateLayer` | `server/{handler,probe,mod}.rs` (heavily used) |
| `middleware::request_state::{RequestState, RequestStateRegistry, ...}` | `server/metrics_endpoint.rs`, `handlers/request_state.rs`, `middleware/{chain,metrics}.rs` |
| `middleware::latency::{LatencyHistogram, get_global_latency, global_latency_or_init}` | `server/metrics_endpoint.rs`, `middleware/metrics.rs` |
| `pty::{PtyManager, attach_event_bus, SpawnOptions, ...}` | `handlers/pty.rs`; `server/mod.rs:708` |
| `surface::{DeliverySurface, SurfaceRegistry, SurfaceNotification, SurfaceApproval, OutboundInteraction, DeliveryError, audience_allows, DesktopSurface, run}` | `bin/aleph-server/commands/start/mod.rs:2992-2998` |
| `control_plane::{create_control_plane_router, ControlPlaneAssets, serve_static_asset}` | `server/mod.rs:757` |
| `busy_queue::{register, deliver_with_ticket, spawn_queued_run, ...}` | `inbound_router/{executor,command_handler}.rs`; `execution_engine/gate.rs`; `bin/aleph-server/server_init.rs` |
| `caller_identity::{current_caller_role, current_caller_user, caller_is_member, current_caller_is_loopback, caller_may_act_as_agent, caller_may_choose_directory}` | `scope/{mod,carried}.rs`; `teams/broadcast/mod.rs`; `gateway/visibility.rs`; `resume_coordinator.rs` |
| `cancellation::CancellationToken` | `gateway/channel.rs:35,570,582,628` (sole but real consumer; `ChannelState::cancel`) |
| `context::GatewayContext` | `teams/{dispatcher,broadcast}`; `executor/builtin_registry/*` |
| `agent_binding::{bind_channel_agent, unbind_channel_agent, BindError, BindOutcome}` | `builtin_tools/agent_manage/{switch,unbind}.rs` |
| `agent_instance::{AgentRegistry, AgentInstance, AgentInstanceConfig, MessageRole, get_or_create_session, list_sessions}` | `teams/*`; `bin/aleph-server`; `memory/session_compactor` |
| `config::{GatewayConfig, GatewayServerConfig, AgentConfig, SandboxConfig, ToolsConfig, ChromeConfig, CronConfig, WebhookConfig, NetworkMode, GatewayTlsConfig, TrustedProxyConfig}` | `bin/aleph-server/commands/start/*` |
| `announce_delivery::{subscribe, deliver, Announcement}` | `process_announce.rs`, `subagent_announce.rs` |

---

## Verification

- `rg` searches executed across `src/`, `bin/`, `interfaces/`, `shared/`.
- For each CUT candidate, the absence of consumers was verified by full-tree greps for the type names, the module path (`crate::gateway::<path>::`), and the re-export path (`gateway::<name>` / `alephcore::gateway::<name>`).
- Borderline candidates (sw-05, sw-06, sw-08) were marked DECIDE because the items remain part of the architecture source-of-truth (doc comments / re-exports) and removal would weaken the public contract more than the LOC savings justify.

## Existing review references

None for batch6.
