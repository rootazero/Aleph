# Severed-Wire Audit — `src/gateway` Batch 4

**Date:** 2026-08-17
**Method:** PRODUCED − CONSUMED via `rg` across `src/`, `bin/`, `interfaces/`, `shared/`, `tests/`.
**Scope:** session_manager/, session_store/, inbound_router/, reply_emitter/, formatter/, events/, event_emitter/

## Files scanned

```
src/gateway/session_manager/{mod.rs, ops/{crud.rs, emit.rs, identity.rs, mod.rs, modify.rs, query.rs, tests.rs}, tests.rs}
src/gateway/session_store/{error.rs, migration.rs, mod.rs, types.rs,
                           file_backend/{meta.rs, mod.rs}, sqlite_backend/mod.rs}
src/gateway/inbound_router/{mod.rs, agent_resolver.rs, approval_callback.rs, command_handler.rs,
                            dedup.rs, executor.rs, group_chat_handler.rs, permission.rs, types.rs}
src/gateway/reply_emitter/{config.rs, mod.rs, extract.rs, sanitize.rs, tests.rs,
                           emitter/{mod.rs, helpers.rs, streaming.rs}}
src/gateway/formatter/{mod.rs, helpers.rs, markdown_to_platform.rs, platform_to_markdown.rs,
                       splitting.rs, tests.rs}
src/gateway/events/{mod.rs, frame.rs, frame_census.rs}
src/gateway/event_emitter/{mod.rs, artifact_ping.rs, impls.rs, instant_buffer.rs,
                           origin_fanout.rs, redacting.rs, team_fanout.rs, tests.rs, types.rs}
```

## Findings (3 total — all DECIDE)

The batch is **largely healthy**. Every public API I probed (including the entire
`SessionManager` surface, `SessionStore` trait, every formatter helper, every
reply-emitter helper, every fanout emitter, every migration helper,
`ApprovalCallbackSink`, `RoutingError`, `ChannelConfig`, `MessageFormatter`,
`ReplyEmitter`, `GatewayEventFrame`, etc.) has at least one live consumer.

Three symbols are flagged as orphan producers — but each is wrapped in a
deliberate `#[allow(dead_code)]` annotation with an inline "F3 audit note"
explaining that the **wire shape** is consumed by downstream CLI/TUI/Protocol
decoders even though no in-tree producer emits them. Cutting them would
silently break deserialisation in `interfaces/tui/src/tui/app/events.rs` and
`interfaces/cli/src/commands/run_follow.rs`.

| ID | Symbol | Form | Severity | Decision |
|----|--------|------|----------|----------|
| `sw-gateway-17` | `StreamEvent::ReasoningBlock` variant + `reasoning_block` / `reasoning_block_with_confidence` constructors | 1 | low | DECIDE |
| `sw-gateway-18` | `StreamEvent::UncertaintySignal` variant + `uncertainty_signal` constructor | 1 | low | DECIDE |
| `sw-gateway-19` | `UncertaintyAction` enum + `description()` accessor | 1 | low | DECIDE |

## Rationale

### sw-gateway-17 — `ReasoningBlock` (DECIDE)
- `src/gateway/event_emitter/types.rs:209` declares the variant; `:301`, `:322` declare its constructors.
- Annotations explicitly mark them dead-code and reference "F3 audit note".
- No in-tree producer emits it; the From→`GatewayEventFrame::ReasoningBlock` conversion at `events/frame.rs:637-652` exists, and `interfaces/tui/src/tui/app/{events.rs:309, tests.rs:708}` and `interfaces/cli/src/commands/run_follow.rs:274` decode it. **Cutting the wire shape would break those clients.**

### sw-gateway-18 — `UncertaintySignal` (DECIDE)
- `src/gateway/event_emitter/types.rs:234` declares the variant; `:344` its constructor.
- Same story: deliberate dead-code, served by conversion to `GatewayEventFrame::UncertaintySignal` and consumed by `interfaces/tui/src/tui/app/events.rs:318` and `interfaces/cli/src/commands/run_follow.rs:260`. **Wire shape must remain.**

### sw-gateway-19 — `UncertaintyAction` (DECIDE)
- `src/gateway/event_emitter/types.rs:275` enum + `:300 description()`.
- Used by `sw-gateway-18`'s variant, also tagged intentional. **Remove only with the consumer wire shape.**

## Negative space (what was checked and is fine)

- `SessionManager::{new, with_defaults, with_raw_memory_writer, with_event_bus}` — all live consumers across `bin/aleph-server`, `executor/builtin_registry`, `a2a`, `builtin_tools`, etc.
- Every `SessionStore` trait method has the trait impl in `sqlite_backend/mod.rs` and `file_backend/mod.rs`.
- Every `SessionManager` impl method (`get_or_create`, `add_message`, `get_history`, `close_session`, `set_topic`, `set_state`, `patch_session`, `delete_session`, `reset_session`, `compact_session`, etc.) is reached via `SessionStore` or `builtin_tools/sessions/*`.
- Migration utilities (`migration_needed`, `normalize_session_dir_names`, `export_legacy_messages`, `export_legacy_messages_from`) all consumed by `bin/aleph-server/commands/start/helpers.rs` and `tests/sqlite_migration_legacy_null.rs`.
- `event_emitter::artifact_ping::{publish_artifact_ping, publish_artifact_ping_on, artifact_ping_event, ARTIFACT_TOPIC}` — used by `artifact_publish`, `artifact_harvest`, `run_loop/inner.rs`, and webchat.
- `event_emitter::{team_fanout::{set_team_event_bus, team_event_bus, publish_team_event}, origin_fanout::{set_channel_registry, channel_registry}}` — global event-bus accessors used widely (`clarification/ask.rs`, `teams/dispatcher`, `handlers/agent.rs`, `announce_delivery.rs`, `start/mod.rs`, etc).
- `NoOpEventEmitter`, `CollectingEventEmitter`, `OriginFanoutEmitter`, `TeamFanoutEmitter`, `RedactingEmitter`, `InstantBufferingEmitter`, `DynEventEmitter`, `GatewayEventEmitter` — all live producers/consumers.
- `ReplyEmitter::{should_voice, send_as_voice, take_reasoning_buffer, drain_and_send_media, deliver_run_media, send_media_standalone, format_content, send_to_channel*, send_error, start_typing_indicator, react_on_inbound, split_message}` — all called from `emitter/streaming.rs` and tests; public constructor used by feishu and telegram adapters.
- `extract_final_response` / `sanitize_final_response` / `sanitize_llm_output` / `split_reasoning` — all live.
- All formatter helpers (`parse_markdown_blocks`, `BlockElement`, `render_table_aligned`, `replace_*`, `markdown_to_*`, etc.) are connected via `MessageFormatter` which is itself called by every channel interface.
- `ApprovalCallbackSink`/`ApprovalCallbackResult` — wired through `approval/callback_sink.rs`, `cluster_node_approval.rs`, `exec_approval_resolve_loop.rs`.
- `ChannelConfig` / `ChannelPolicyConfig` / `ChannelPermissionLevel` / `DmPolicy` / `GroupPolicy` / `SlashAccessConfig` / `SlashAccessDecision` / `SLASH_COMMAND_MODE_KEY` / `normalize_slash_command_name` / `check_link_access` / `RoutingError` — all consumed.
- `InboundDedupTracker`, `MetaLocks`, `MetaGuard`, `sanitize_key_for_dir`, `SessionSearchResult`, `session_type_str`, `SessionIdentityMeta`, `SessionManagerConfig` — all internal-but-connected.
- Every `GatewayEventFrame` variant — produced by a conversion or pub use site (full enumeration in `server/handler.rs` construction + `From<StreamEvent>`), received by `event_visibility.rs`, `event_scope.rs`, `frame_census.rs`, etc.

## No-cut recommendation

Batch 4 is **good to merge from a wire-parity perspective**. The three DECIDE
items are gatekept by an explicit `#[allow(dead_code)]` + F3 audit note whose
content matches what I see in the consumers. Re-evaluate when either:
1. A producer is added for `ReasoningBlock`/`UncertaintySignal`/`UncertaintyAction` (then promote to CONNECT).
2. CLI/TUI deserialisation support is dropped (then CUT becomes safe).
