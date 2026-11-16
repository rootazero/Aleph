# NSH 0.5 Migration Inventory / NSH 0.5 迁移库存

**Date / 日期:** 2026-11-15
**Track / 轨道:** NSH desktop replacement, independent from Capability Phase8
**Current Aleph / 当前 Aleph:** local `main`, HEAD `e08af3789dbf194f02f08461c215eb3c7701557f`
**Pinned upstream / 固定上游:** `https://github.com/longbridge/nsh.git`, annotated tag object `729ea4c6810112160e890684e8a8d34528ec6da9`, peeled release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`

## Status boundary / 状态边界

This inventory compares the pinned NSH 0.5 release with current Aleph. The old local checkout, v0.4 spike, historical plans and current shell tests are not v0.5 proof. No production source, dependency declaration, lockfile or workspace membership was changed. Reproducible upstream paths are recorded in [the source manifest](./2026-11-15-nsh05-upstream-evidence.md); no machine-specific checkout or `/tmp` path is part of this inventory.

本库存只比较固定的 NSH 0.5 发布版本与当前 Aleph。旧 checkout、v0.4 spike、历史计划和当前 shell 测试都不是 v0.5 证明。生成期间没有修改生产源码、依赖声明、锁文件或 workspace 成员。可复现上游路径记录在源码清单中，不使用机器特定路径。

## Evidence anchors / 证据锚点

| Area | Current Aleph evidence | Pinned v0.5 evidence | Result |
| --- | --- | --- | --- |
| Upstream identity | old local checkout `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9`, v0.4 | release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1` | **Gap: old checkout excluded** |
| Workspace/dependencies | `desktop/nsh-shell/Cargo.toml` uses absolute v0.4 paths, Tokio `1.53`, `tokio-tungstenite 0.30` only in dev-dependencies | v0.5 workspace `0.5.0`, Rust `1.97`, resolver 3, Tokio 1, `tokio-tungstenite 0.28` rustls and platform crates | **Gap: no pinned v0.5 dependency** |
| Window API | `desktop/nsh-shell/src/main.rs` uses old `run_app`, `AppOptions`, `AppSource`, `Root` | relative paths in upstream manifest define `Window`, `WindowRef`, `WindowKind`, state and requests/events | **Unverified / migration required** |
| Runtime model | `desktop/nsh-shell/src/runtime.rs` creates private Tokio runtime on worker thread | v0.5 has one shared engine/JS/core and platform `AppView` | **Architecture mapping required** |
| Connection | `desktop/nsh-shell/src/connection.rs` performs hello/subscription/session list/retry | Aleph protocol remains authority; v0.5 does not prove Panel projection | **Partial transport only** |
| Session/UI | `desktop/nsh-shell/src/session.rs` uses local UUIDs, empty history and fabricated assistant text | Tauri Panel sources remain observable baseline | **Gap: replace mock path** |
| Panel | `interfaces/webchat/src/app.rs`, `mode_sidebar.rs`, `workspace_panel.rs` contain current state/routes/pane | NSH has no current parity implementation | **New fixed Chat slice required** |
| AppView/platform | no Aleph proof of v0.5 construction | v0.5 macOS/Linux/iOS/TUI `AppView` relative paths in manifest | **Unverified** |
| macOS | old `run_app`; no v0.5 floor proof | macOS `14.0`, `ActivationPolicy::{Regular, Accessory}` | **Gap: compile/runtime proof** |
| Windows/Linux | no current NSH shell proof | v0.5 platform crates and Aleph floors | **Unimplemented / unverified** |
| Host/media/types | no current exact host channel, optional Tokio handle, media grants or generated `nsh/desktop` types | v0.5 source requires exact mapping | **Inventory gap** |
| Shutdown | connection closes client; supervisor launches detached daemon without owned shutdown | v0.5 native close is platform-owned; daemon lifecycle is separate | **Bounded slice must exclude daemon ownership** |

## Current implementation / 当前实现

- `desktop/nsh-shell/src/main.rs` is macOS-only and uses v0.4-style APIs; non-macOS returns success.
- `desktop/nsh-shell/src/runtime.rs` owns a private multi-thread Tokio runtime; this is not v0.5 host proof.
- `desktop/nsh-shell/src/connection.rs` performs `system.hello`, `events.subscribe`, `sessions.list`, retries and client close but discards events instead of projecting Panel state.
- `desktop/nsh-shell/src/supervisor.rs` probes port 18790 and launches `aleph-server --daemon start` detached. It has no coordinated owned daemon shutdown protocol.
- `desktop/nsh-shell/src/session.rs` is explicitly mock and cannot be product evidence.
- Native input/message components are conceptual v0.4-derived components; local tests do not prove v0.5 runtime or Panel parity.

## Phase 0–6 reconciliation / Phase 0–6 对账

The historical phase labels describe the old v0.4 workstream. This matrix reports current proof against the pinned v0.5 migration target; a historical completion claim does not become v0.5 evidence.

| Phase | Current status | Evidence and exact limitation |
| --- | --- | --- |
| Phase 0 compatibility spike | **PARTIAL** | `desktop/nsh-spike/FINDINGS.md` proves only the v0.4 macOS build/window/`system.info` loopback and Tokio 1.53.3; it excludes events/session/reconnect/snapshot-replay/WSS/non-macOS and used SIGTERM. v0.5 compatibility remains NOT_PROVEN. |
| Phase 1 protocol/transport | **PARTIAL** | Historical `desktop/nsh-shell/src/connection.rs`/Phase1 plan prove old `system.hello`, subscription and detached supervisor intent, but the client drains events and the plan explicitly lacked a hello gate and durable replay. Pinned v0.5 host/API integration is NOT_PROVEN. |
| Phase 2 connection | **PARTIAL** | Historical plan records reconnect and status transitions, but no UI/event projection, no `interfaces/nsh-desktop`, no non-mac proof and old v0.4 dependencies. Current canonical Panel state is NOT_PROVEN. |
| Phase 3 UI core | **PARTIAL** | Historical technical-debt report records 23/23 local tests, while `desktop/nsh-shell/src/session.rs` still uses local UUIDs, no-op history and fabricated responses. Real Server RPC and canonical Chat state are NOT_PROVEN. |
| Phase 4 capability/parity | **NOT_PROVEN** | The historical Phase4 document is a plan for API verification, Capability alignment, real RPC and UI parity; no pinned v0.5 runtime or state-by-state Tauri parity evidence exists. |
| Phase 5 PluginRuntime | **DEFERRED** | It depends on unresolved ADR-006/008 and is outside the fixed Chat/Artifacts slice. |
| Phase 6 Tauri/WebChat removal | **DEFERRED** | The master design requires Phase4 parity plus a two-week dual run before removal; neither exists for v0.5. |

## ADR-003–009 reconciliation / ADR-003–009 对账

The historical ADR document itself is the status authority for these records; it is architecture guidance, not crate/runtime proof.

| ADR | Recorded status | Current migration implication |
| --- | --- | --- |
| ADR-003 plugin UI Hybrid | **Accepted** | Compatible with keeping Server capability authority and a separately hosted UI, but no current NSH v0.5 plugin UI implementation is proven. |
| ADR-004 third-party Rust plugin WASM/process isolation | **Accepted** | Remains a security boundary; no PluginRuntime implementation is in the first slice. |
| ADR-005 restricted Agent TSX | **Accepted** | Requires owner/scope/resource limits and is outside the fixed Chat/Artifacts route. |
| ADR-006 WASM Runtime selection | **Proposed** | Must remain reversible; no concrete runtime or irreversible dependency may be introduced by NSH migration planning. |
| ADR-007 event persistence and Snapshot/Replay | **Proposed** | Current live capability broadcast is not durable replay; any Panel reconnect mapping must use existing Aleph snapshot/history contracts until this ADR is accepted. |
| ADR-008 plugin trust/signing/revocation | **Proposed** | No plugin trust root or revocation behavior is implied by NSH v0.5 host capabilities. |
| ADR-009 dynamic UI package/cache/rollback | **Proposed** | No package distribution, cache or rollback surface is part of the first slice. |

## Exact v0.5 mapping summary / 精确 v0.5 映射摘要

The complete source-symbol table and Aleph Create/Modify/Test mapping are in [the upstream evidence manifest](./2026-11-15-nsh05-upstream-evidence.md). The critical anchors are: `crates/nsh-engine/src/host_channel.rs` `HostChannel` and crate-private shutdown/attachment lifecycle; `crates/nsh-engine/src/lib.rs:924` `EngineConfig` with public `runtime: Option<tokio::runtime::Handle>` at `:961`; `crates/nsh-platform-macos/src/app.rs` `AppOptions`, `AppView::new`, `host`, `load`, `run_app`; `crates/nsh-engine/src/lib.rs:3987–4010` access/media derivation; `crates/nsh-core/src/desktop/window.rs:818` TypeScript declarations; `crates/nsh-js/src/declarations.rs:382` `nsh/desktop` mapping; and `crates/nsh-js/src/desktop.rs:1088` `WindowRef` declarations. These are pinned v0.5 source facts, not current Aleph compatibility proof.


`interfaces/webchat/src/app.rs` is the state composition authority; `interfaces/webchat/src/components/mode_sidebar.rs` defines `PanelMode`; `interfaces/webchat/src/components/workspace_panel.rs` defines the split pane and body tabs. The fixed first slice is `PanelMode::Chat` at `/` with `WorkspacePanel` body `Artifacts`, driven by real session/message/run/refusal/reconnect state. Preserve observable intent where it fits NSH; redesign host composition when NSH ownership/performance requires it; defer unverified surfaces. Pixel or implementation identity is not required.

## Pinned v0.5 contract inventory / 固定 v0.5 契约库存

Use only the relative source paths in `2026-11-15-nsh05-upstream-evidence.md` for Window/WindowRef/window kinds, WindowState/Placement, macOS mapping and activation policy, AppView, generated types, host channel, optional Tokio handle, media grants, secondary windows and shutdown. The old v0.4 names are not placeholders.

## ADR/R2 state / ADR/R2 状态

`docs/research/NSH_ADR-001~009.md` is historical mixed-maturity guidance, not current crate proof. No current `docs/adr/` index was found. Existing R2 requires complex business UI in Leptos/WASM and bridge OS/API I/O. The proposed first-slice decision keeps R2 and treats NSH as native host/lifecycle/transport while Panel/Leptos/WASM remains business UI authority; the separate decision proposal must be approved before implementation.

## Migration gaps and gates / 迁移缺口与闸门

1. **Version/API:** prove pinned v0.5 dependencies and exact symbols before changing Cargo declarations.
2. **Window:** compile/open/state/set/close/secondary-window behavior with `Window`/`WindowRef` and owned host settlement.
3. **Host/lifecycle:** prove host channel, optional Tokio integration, reconnect, cancellation and owned close; detached daemon is outside first slice.
4. **Fixed Panel slice:** real Chat route `/`, `Artifacts` body, real session/message/run/refusal/reconnect state; no mock UUID or fabricated text.
5. **Performance:** same macOS host/provider, 10 warm-up + 30 measured runs; proposed gate NSH median ≤ Tauri +10%, p95 ≤ +20%, RSS ≤ +10%, threads ≤ +2, subject to Task5 approval.
6. **Platforms:** Rust `1.97`, macOS `14.0`, Windows `10 1809+`, Linux GTK `4.14+`/WebKitGTK `6.0` (`2.40+`); non-macOS remains NOT_PROVEN.
7. **Removal:** Phase4 parity plus two-week dual run before removing Tauri/WebChat/workspace members. Swift media/i18n remain separate.

## Historical exclusions / 历史排除

`desktop/nsh-spike/FINDINGS.md` proves only the v0.4 macOS spike and `system.info` loopback; it excludes events/session/reconnect/snapshot-replay/WSS/non-macOS and uses SIGTERM. Historical `nsh-*` plans and current shell tests are not v0.5 or canonical chat evidence.

## Decision boundary / 决策边界

Task4 has produced the fixed slice spec/plan and R2 decision proposal. Task5 must approve the exact scope, thresholds and R2 choice before dependency or production changes. This inventory is not implementation authorization.
