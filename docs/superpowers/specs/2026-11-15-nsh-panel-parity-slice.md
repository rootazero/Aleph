# NSH Panel Parity Slice Specification / NSH Panel Parity 切片规格

**Status / 状态:** proposed; requires R2 decision and separate human approval
**Date / 日期:** 2026-11-15
**Track / 轨道:** NSH 0.5 desktop replacement, independent from Capability Phase8

## Purpose / 目的

Prove a small NSH 0.5 macOS slice against the actual Aleph Panel contract while preserving NSH's architecture and performance properties. The existing Tauri/Leptos Panel is the observable behavior and information-architecture baseline. Restoration is selective: preserve what fits, redesign conflicts around NSH's shared-engine/window/lifecycle model, and defer unverified surfaces. Performance is the first constraint; pixel identity is not.

This specification does not authorize production changes, dependency migration, Tauri/WebChat removal, or a silent R2 change. The R2 decision is recorded separately in [the decision proposal](./2026-11-15-nsh-r2-native-ui-decision.md).

## Fixed first slice / 固定首个切片

The first slice is fixed to the **default Chat route (`PanelMode::Chat`, path `/`)** with the **WorkspacePanel `Artifacts` body**. It includes these observable state fields:

- connection state: connecting, connected, reconnecting and terminal error;
- session list and selected session id from `sessions.list` and the existing session contract;
- message history and streaming assistant text from real Server events;
- active run id and run status from `run.accepted`, `agent.run.complete`, `agent.run.error` and the existing run event mapping;
- refusal/error state for a rejected `chat.send` and a clean shutdown state;
- sidebar selected Chat mode, section navigation and the Artifacts empty/loading/populated state without fabricated artifacts.

The first slice must use `system.hello`, `events.subscribe`, `sessions.list` and `chat.send` through the existing `shared/client` and `shared/protocol` contracts. It must project events into this route; merely receiving and discarding them is a failure.

### State and data-path mapping / 状态与数据路径映射

| Observable state | Snapshot/request authority | Event/receive path | Reconnect rule |
| --- | --- | --- | --- |
| Session list and selected session | `sessions.list`; existing session/history contract used by the current Panel | session snapshot plus the existing session event mapping | re-run hello, create a fresh subscription, fetch `sessions.list`, then reconcile by canonical session id; never mint a local UUID |
| Message history | existing canonical session/message history request used by `ChatState` | message events from the shared event stream; no fabricated assistant text | replace or merge by canonical message id/sequence after reconnect, then resume streaming only for the current run |
| Active run and streaming text | `chat.send` response metadata plus existing run snapshot/lookup | `run.queued` and `stream.run_queued` establish the queued state, carry `run_id`, `session_key` and queue position (`ahead`); `run.accepted` transitions the same run to active. Internal run topics are translated at the wire boundary to `stream.*` methods and the shared client `StreamEvent`; `run.accepted`, `agent.run.complete`, `agent.run.error` update the route state | discard the old receiver, re-handshake and rebuild run state from the new snapshot before accepting new deltas; deduplicate by run/event identity; preserve queued→accepted→complete/error transitions and the session key |
| Artifacts empty/loading/populated | `interfaces/webchat/src/api/artifacts.rs:131-138` `artifacts.list` is the read authority | `interfaces/webchat/src/components/artifacts/mod.rs:250-257` handles session-change refetch; `:271-279` filters the `session.artifact` invalidation ping and refetches | after reconnect, fetch `artifacts.list` for the selected session; do not treat the invalidation ping as artifact content |
| Refusal/error | addressed `chat.send` result and current refusal/error contract | scoped refusal/error event if the Server emits one; audit observation is not UI authorization | clear only after a new addressed operation succeeds or the server provides a fresh state |

The current source also exposes queued state: `src/gateway/events/frame.rs:29-34` defines the `RunQueued` frame with `run_id`, `session_key` and `ahead`, `:772` maps the already-defined variant to the `run.queued` topic, and `:833` maps the wire `stream.run_queued` form. `interfaces/webchat/src/platform/wide/views/chat/events.rs:914-947` binds queued runs, while `:1130-1144` handles queued events before the accepted/complete/error terminal transitions. The slice must preserve this state rather than collapsing it into generic streaming.

The internal run topics, `stream.*` wire methods and shared `StreamEvent` are distinct layers. The implementation plan must name the translation at the existing client/protocol boundary and record the session/run association; a test that only sees a raw topic or only sees a fabricated `StreamEvent` is insufficient. The mapping anchors are `src/gateway/events/frame.rs:769-773` for `run.queued` and `:830-833` for `stream.run_queued`.

## Concrete artifacts / 具体文件

| Purpose | Proposed path | Required named evidence |
|---|---|---|
| v0.5 compatibility proof | `desktop/nsh-shell/tests/nsh_v05_window_compat.rs` | `window_kind_and_ref_follow_pinned_v05`, `close_and_exit_settle_owned_host_work` |
| real Chat route integration | `desktop/nsh-shell/tests/nsh_panel_chat_slice.rs` | `chat_route_projects_sessions_messages_runs_and_refusal`, `chat_route_reconnect_rebuilds_state_without_duplicate_session` |
| parity fixture and results | `docs/superpowers/runs/2026-11-15-nsh-panel-parity-evidence.md` | state-by-state Tauri/NSH matrix, performance receipts, preserve/redesign/deferred decisions |
| upstream API source | [NSH v0.5 evidence manifest](../runs/2026-11-15-nsh05-upstream-evidence.md) | pinned commit and relative source paths only |

These are proposed test/evidence artifacts, not current implementation proof. The tests must call the pinned v0.5 APIs discovered in Step 1; they must not invent adapters around v0.4 names.

## Source contracts / 源码契约

The source authority is NSH release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`. Use the relative paths in the evidence manifest for `WindowKind`, `WindowAttributes`, `WindowRef`, `WindowState`, AppView, activation policy, `HostChannel`, `EngineConfig.runtime: Option<tokio::runtime::Handle>`, media grants, secondary windows and shutdown. Current Aleph sources remain the authority for Panel behavior:

- `interfaces/webchat/src/app.rs`: contexts, ChatState, session/workspace/canvas/browser/voice state, run-event dispatch and reconnect repair;
- `interfaces/webchat/src/components/mode_sidebar.rs`: `PanelMode` and sidebar/section navigation;
- `interfaces/webchat/src/components/workspace_panel.rs`: split pane and `Artifacts` body;
- `interfaces/webchat/src/api/artifacts.rs:131`: `artifacts.list` reads populated artifact state;
- `interfaces/webchat/src/components/artifacts/mod.rs:250-257`: session-change/refetch trigger; `:271-279` handles the `session.artifact` invalidation ping;
- `src/gateway/events/frame.rs:820`: internal event frame/run topic layer that must be mapped to the shared client/protocol stream layer.

## Selective parity rules / 选择性 parity 规则

| Classification | Rule for this slice |
| --- | --- |
| Preserve | Chat route, selected session, streaming/refusal/reconnect states, sidebar intent and Artifacts body state can be represented with current ownership and performance. |
| NSH redesign | Host/window composition or event scheduling differs while the observable state, ownership and user intent remain equivalent. Record the reason and measured effect. |
| Deferred | Media, voice/camera, i18n, other routes, unverified v0.5 APIs, non-macOS runtime, PluginRuntime, or Phase4-wide parity. |

## Daemon and shutdown boundary / Daemon 与 shutdown 边界

The current `desktop/nsh-shell/src/supervisor.rs` launches `aleph-server --daemon start` detached and has no owned daemon shutdown protocol. The first slice must **not launch or claim ownership of that detached daemon**. It uses an already-managed loopback server and owns only the NSH client, event receiver, UI worker and v0.5 host resources created by the slice. Acceptance requires those owned resources to settle on close; daemon termination remains a separate approved lifecycle contract.

## Performance gate / 性能闸门

Run Tauri and NSH on the same macOS 14+ machine, using the same `debug-qa` profile for the first controlled comparison, the same deterministic loopback server/provider, with the Tauri baseline first and NSH second for each scenario. This first gate is a relative integration/performance signal; it does not claim release performance. Before each scenario reset the server/session to the same canonical fixture, clear the selected session and reconnect state, close prior windows, and record that no stale receiver or artifact cache was reused. Discard the first 10 warm-up runs; collect 30 measured runs per implementation in the fixed order Tauri(30), reset, NSH(30), then repeat the whole block once if a process or provider fault invalidates a run. A release-profile comparison requires a separately reviewed release-safe QA confinement design.

Use these timing observations: startup-to-first-frame starts immediately before `run_app`/NSH AppView launch and ends at the first rendered frame receipt; Chat route transition starts after the route command is accepted and ends at the route's first stable rendered frame; input-to-`chat.send` starts at user input dispatch and ends at the addressed request receipt; reconnect repair starts when the connection is marked reconnecting and ends after the replacement snapshot plus state projection is rendered; close settlement starts at the close request and ends when the NSH-owned client, receiver, UI worker and host attachment have joined or reached their documented terminal state. Record peak RSS and live thread count for the shell process plus its NSH renderer/engine child processes, excluding the externally managed daemon; sample RSS/thread count every 100 ms from launch through close settlement and retain the maximum.

For each metric sort the 30 measured values ascending and define p95 as the value at rank `ceil(0.95 * 30) = 29` using one-based indexing. Compare medians, p95, peak RSS and maximum live threads against the paired Tauri baseline. The proposed eligibility rule is: NSH median ≤ Tauri +10%, p95 ≤ +20%, peak RSS ≤ +10%, and maximum live threads ≤ baseline +2. A run is invalid if the controlled provider/server receipt differs; rerun the invalid pair after resetting both implementations.

## Acceptance cases / 验收案例

1. **Window contract:** pinned v0.5 main window opens with the selected kind, reports state, handles close causes and settles owned host work.
2. **Canonical connection:** hello, subscription and session retrieval use existing contracts; reconnect re-handshakes and rebuilds state without a duplicate local session.
3. **Canonical Chat:** real session/message/run events drive the fixed Chat route; a mock response, local UUID session or discarded event stream fails.
4. **Selective parity:** sidebar Chat selection, message/stream/refusal/reconnect states and Artifacts body states match the Tauri observable contract; each difference is Preserve, NSH redesign or Deferred.
5. **Performance:** the stated same-machine protocol and thresholds are met or the case is explicitly recorded as not accepted.
6. **Owned shutdown:** NSH-owned client, receiver, UI worker and v0.5 host resources settle; detached daemon termination is outside this slice and is not claimed.
7. **R2:** one approved choice from the R2 decision proposal is recorded before implementation. No silent native business-UI authority is introduced.

## Explicit non-goals / 明确不做

- No Tauri/WebChat/workspace-member removal before Phase4 parity and a two-week dual run.
- No NSH v0.4 compatibility claim, all-platform claim, GPUI revival, or native media/i18n implementation.
- No invented host channel, optional Tokio handle, media grant or shutdown API.
- No PluginRuntime/Phase5 work before its ADR prerequisites, especially ADR-006/008, are resolved.
