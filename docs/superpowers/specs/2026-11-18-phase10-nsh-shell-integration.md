# Phase10 NSH macOS Shell Integration Specification
# Phase10 NSH macOS 桌面壳集成规格

**Status / 状态:** Historical NSH 0.5 specification. The user superseded the patched candidate on 2026-11-20; its fork proposal, [final SPEC report](../../.phase10-evidence/reviews/phase10-spec-final-2026-11-20.md), and [integration checkpoint](../runs/2026-11-19-phase10-local-integration-checkpoint.md) are retained as historical evidence only. The [current implementation plan](../plans/2026-11-18-phase10-nsh-shell-integration.md) uses the installed official NSH `0.6.0 (5541637)` and crates.io `0.6.0`. / 本文为 NSH 0.5 历史规格；旧补丁候选已作废，原规格与验收证据仅供追溯。当前计划以本机安装的官方 NSH 0.6.0 为基准。

**Date / 日期:** 2026-11-18

**Baseline / 基线:** Local `main` at `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7`. The original Phase9 implementation and plan commits remain as objects and `runtime-proof` points to `81e8ceda2bfde52d1c9ae96cabc6c2967cab9953`; current `main` contains rewritten equivalents `6e3004985` and `0e4e1d4eb`. These identities must remain distinct in evidence.

## 1. Goal / 目标

Make the existing NSH 0.5 host a resident macOS shell around the existing Leptos/WASM Panel, with independently provable window lifecycle, tray and instance routing, summon hotkey, deep links, notifications, and window-state restoration. The Panel remains the only business UI, session state, navigation projection, and Gateway transport authority. / 在现有 NSH 0.5 宿主外补齐可独立证明的 macOS 常驻桌面壳能力；Panel 仍是唯一业务 UI、会话状态、导航投影和 Gateway transport 权威。

Phase10 is a continuation after accepted Phase9. It is not the historical NSH master Phase5 Plugin Runtime, not native Chat UI, and not Tauri removal. / Phase10 是 Phase9 之后的受限延续，不是旧 master 的 Phase5 Plugin Runtime、原生 Chat UI 或 Tauri 移除。

## 2. Architecture decision / 架构决定

### 2.1 Authorities / 权威边界

- `desktop/nsh-shell` owns macOS OS I/O, one NSH window/WebView host, shell callbacks, registrations, and their teardown.
- The existing Panel owns business state, Chat/Artifacts/Settings UI, Gateway/WebSocket transport, route projection, and notification intent content.
- Rust Core remains platform-independent. No AppKit/CoreGraphics/Vision or native OS API enters `src/`.
- The shell does not create a second Gateway client, session reducer, tool registry, ownership map, daemon, or business persistence store.
- The existing `aleph-server` singleton lock and managed Gateway lifecycle remain outside the shell. Quitting NSH must not stop an externally managed Gateway.

### 2.2 Intent path / 意图路径

Every OS-originated intent must follow one path:

```text
macOS callback or launch input
  -> shell-owned validation and lifecycle owner
  -> one narrow host/Panel ingress
  -> existing Panel route/state/transport owner
```

The shell may validate syntax, origin, length, nonce, and lifecycle state. It must not decide Chat/session/tool behavior. A rejected or unsupported intent is observable as a rejection and never becomes a navigation or business event.

### 2.3 API feasibility result / API 可行性结果

The pinned NSH revision is `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`, version 0.5.0.

The public standalone API provides:

- `run_app(AppSource, AppOptions)`;
- `AppOptions::keep_alive_on_close`, which hides the standalone window on user close while retaining the engine;
- `AppOptions::host: nsh_engine::HostChannel`;
- `quit_app()` for explicit application termination;
- `AppView::new`, `AppView::host`, `AppView::load`, and `AppView::stop` for embedding.

The public API does **not** provide a Rust callback from `run_app` for `windowShouldClose`, `windowWillClose`, tray menu events, global shortcut presses, notification presses, or AppKit URL delivery. The platform's tray, global shortcut, navigation, notification, and window-state modules are private. `HostChannel::send` delivers `host.on(name, data)` events into an NSH app, but the current external `web_view(url)` Panel has no existing documented Panel listener for those events.

Pinned v0.5 does expose a public page-control handle: `nsh_core::prelude::element_ref()` can be attached to `web_view(url)` with `Element::element_ref`, and `ElementRef::post_message` sends a `message` event to the loaded page. The compatibility compile spike now proves that constructor/attachment path against the pinned revision. A separate isolated native QA witness captured one matching delivery into the existing external WebView, but this remains a QA-only page-control result, not a production ingress contract: the current public method has no delivery result, explicit queue bound, invalid-event validation, or post-teardown error semantics. The independent receipt records the delivery sub-result while retaining the whole launcher run as NOT_ACCEPTED.

The native OS callback sink remains an API gap. `ElementRef` does not expose AppKit callbacks or convert NSH's private engine events into a shell-owned typed event stream.

### Approved narrow-boundary proposal / 已批准的窄边界提案

The user approved a narrow public boundary, subject to a separate API review and pinned implementation. The boundary has two parts and must remain one-way at the business edge:

1. **Public NSH OS-event sink.** NSH owns AppKit, Carbon, notification, URL, tray, and window-state implementation. A public `AppOptions`/`run_app` callback sink reports typed, serialized events to the embedding shell: `WindowCloseRequested`, `WindowHidden`, `WindowReopened`, `QuitRequested`, `OpenUrls`, `TrayAction`, `GlobalShortcut`, `Notification`, and `WindowState`. Events are delivered in native callback order, never after teardown, and the callback must not block or re-enter NSH. Registration failure is an explicit result; disabled features create no registration or worker. These names are contract labels, not claims about the current v0.5 Rust surface.

2. **External WebView host-event ingress.** The existing `web_view(url)` must gain one documented, one-way method for sending a bounded JSON host event into the loaded page, or an equivalent public `HostChannel` adapter that is proven to reach the external WebView. The method must return a delivery/queue error, preserve event order, reject oversized or invalid names, and stop accepting events after WebView teardown. The Panel will expose one production ingress that forwards only validated shell intents to its existing route/notification owner; it will not become a second reducer or Gateway client. No token, credential, or secret-bearing URL is permitted in this event payload.

These are deliberately separate contracts: the OS sink reports native facts to the shell; the WebView ingress forwards a validated intent to the existing Panel. NSH must not interpret Aleph session/tool behavior, and the shell must not copy NSH's private AppKit implementation.

The API review must settle:

- exact public Rust types, callback thread and reentrancy rules;
- event ordering, bounded queue size, delivery failure and teardown semantics;
- whether `AppOptions` remains clonable and how callback ownership is released;
- the WebView JavaScript namespace/message schema and how the default WASM avoids QA-only globals;
- a compile spike plus one real Panel delivery spike against the pinned dependency or its reviewed replacement;

The constructor/attachment compile spike for `element_ref()` plus `Element::element_ref()` has passed in `desktop/nsh-shell/tests/nsh_v05_window_compat.rs`, using the pinned revision and a new `.phase10-target`. A separate isolated native QA witness also captured one matching `ElementRef::post_message` delivery into the existing external WebView; the result is recorded in `docs/superpowers/runs/2026-11-18-phase10-webview-delivery-receipt.json`. The launcher whole-run remains NOT_ACCEPTED because the immutable Phase9 schema rejects the new event kind and the run contains an existing history-replay QA error. This does not close the production ingress contract or the typed OS-event sink gate.
- compatibility and source/lock identity updates.

No Aleph production module may depend on the proposed names until the API review produces a public implementation that compiles and the delivery spike passes.

The approved boundary does not authorize private-module imports, copied delegates, a second business reducer/transport/registry, or a new daemon.

Therefore implementation has two explicit gates:

1. **Boundary implementation gate:** the reviewed public NSH event sink and external WebView ingress must exist at the pinned/reviewed dependency and pass compile plus real delivery spikes.
2. **Aleph integration gate:** the shell and Panel must connect those contracts while retaining the authority and evidence boundaries below.

Until both gates are satisfied, G1's resident behavior may be implemented with `keep_alive_on_close`, but G2-G7 must not be advertised as implemented. This is deliberate: a compile-only `HostChannel` assertion is not native callback evidence.

## 3. Observable contract / 可观察合同

### G1 Window lifecycle / 窗口生命周期

- Start exactly one shell-owned NSH window and existing Panel.
- User close transitions the same window to resident-hidden state; it does not quit the app or create a new WebView.
- Reopen/show/focus returns the same Panel instance.
- Explicit Quit terminates the app and settles all shell-owned resources.
- Close, reopen, and explicit quit are distinct events in evidence.
- Repeated close/reopen does not create duplicate windows, receivers, or Panel state.

Required evidence: native close/reopen/quit sequence, public callback or documented equivalent, actual process identity, WebView/engine settlement, and managed Gateway health after shell quit.

### G2 Tray and single instance / 托盘与单实例

- Tray Show uses G1's lifecycle owner.
- Tray Quit uses explicit Quit.
- A secondary launch never becomes a second resident host. It forwards one validated launch intent to the primary and exits.
- Secondary failure is visible; no detached fallback daemon or broad process cleanup is allowed.

Required evidence: two real same-user launches, one resident window, primary/secondary terminal states, callback cleanup, and no duplicate Panel.

### G3 Summon hotkey / 唤起热键

- Enabled mode registers one global shortcut owned by the shell lifecycle.
- A press shows/focuses the existing window.
- Conflict or registration failure is reported as failure, not treated as success.
- Disabled mode registers no shortcut and starts no optional worker.
- Quit releases the registration; re-launch can register again.

Use NSH's canonical `Shortcut` representation where the supported path exposes it. Do not create another shortcut grammar or registry.

### G4 Deep links / Deep link

- Reuse the existing supported URI grammar and canonical route owner.
- Validate scheme, URL length, origin/authority, reserved forms, and supported route before delivery.
- Cover cold start, warm primary, and secondary-instance delivery.
- Deliver accepted input exactly once to the existing Panel navigation owner.
- Malformed, unsupported, secret-bearing, or foreign-origin input is rejected before navigation.

The shell does not parse Chat/session semantics and does not call Gateway APIs directly.

### G5 Notifications / 通知

- Panel-owned notification intent is presented through the existing NSH/OS notification path.
- Permission is queried and represented as `Default`, `Granted`, `Denied`, or `Unavailable`.
- A notification press focuses the current Panel and routes through its existing owner.
- Permission denial is honest evidence; no private API, policy bypass, or alternate hidden channel is allowed.

### G6 Window state / 窗口状态

- Restore supported normal-frame and visibility state through the public NSH window-state path or a narrowly approved host adapter.
- Reject non-finite, zero/negative, or wholly off-screen bounds.
- Reconcile changed displays and constrain restored bounds to a visible display.
- Presentation state is shell state only and never becomes session or business state.

### G7 Ownership and regression / 所有权与回归

- Every callback, worker, receiver, shortcut, tray item, notification delegate, and WebView has one owner and one teardown path.
- Teardown is observed, not inferred solely from `run_app` return or an `owned_drained` field.
- Existing Phase9 reducer, thresholds, Panel transport, and default WASM asset policy remain unchanged.
- External managed Gateway remains alive after shell quit.
- Native OS evidence is separate from Phase9's existing performance/reducer evidence.

## 4. Files and ownership / 文件与所有权

Current owners:

- `desktop/nsh-shell/src/main.rs`: shell entrypoint, existing Panel WebView, QA-only clock and message validation.
- `desktop/nsh-shell/src/shell_host.rs`: launch URL and QA nonce validation.
- `desktop/nsh-shell/tests/nsh_v05_window_compat.rs`: pinned public API compile checks only.
- `interfaces/webchat/src/{app,context.rs}` and `shared/ui_logic/src/connection/wasm.rs`: Panel state and transport; do not move ownership into NSH.
- `desktop/shell/src/{tray,hotkey,deeplink,notify}.rs`: behavior reference only; Tauri plugins and daemon assumptions must not be copied wholesale.

Proposed NSH files, only after the API gate passes:

- `desktop/nsh-shell/src/lifecycle.rs`: one shell lifecycle owner, if a separate file remains justified.
- `desktop/nsh-shell/src/tray.rs`: OS tray adapter only.
- `desktop/nsh-shell/src/hotkey.rs`: OS registration adapter only.
- `desktop/nsh-shell/src/deeplink.rs`: syntax/authority validation and one ingress call.
- `desktop/nsh-shell/src/notifications.rs`: permission/presentation adapter only.
- `desktop/nsh-shell/src/window_state.rs`: host presentation-state validation only.

Avoid creating a module whose only purpose is to rename a one-use call. Keep helpers local unless they own a real boundary or are reused.

## 5. Verification contract / 验证合同

Before every Cargo command:

```text
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
```

Proceed only on `ALLOW`; use one Cargo owner, `--offline --locked --jobs 1`, and a new `CARGO_TARGET_DIR=.phase10-target`. Never overwrite `.phase9-target`.

Required native matrix:

- G1: user close -> hidden -> reopen/focus -> explicit quit, repeated once.
- G2: primary/secondary launch, tray Show/Quit, secondary exit.
- G3: successful registration, conflict, disabled mode, teardown/re-registration.
- G4: cold, warm, secondary accepted URL; malformed and unsupported rejection.
- G5: permission result, real delivery when granted, honest denial/unavailable case, press routing.
- G6: normal restore, changed display, invalid/off-screen rejection.
- G7: startup failure and quit settlement; managed Gateway remains healthy.

A native debug proof may be PASS only when the relevant callback/registration or honest permission outcome is observed and raw process/identity/settlement evidence is retained. Compile coverage, synthetic QA messages, old Phase9 receipts, or `owned_drained` alone are insufficient.

## 6. Out of scope / 不在范围

Release packaging/notarization/auto-update installation, Windows/Linux acceptance, full embedded-core bundle ownership, detached server takeover, Core execution redesign, Plugin Runtime, native Chat/Artifacts/Settings UI, Swift camera/voice/screenshot, i18n migration, Tauri/WebChat removal, two-week rollout, remote push/release, and constitution/ADR changes.

## 7. Trade-offs / 取舍

This design preserves the R2 authority boundary and avoids a second business runtime, but it makes OS integration dependent on a narrow NSH public contract. A maintained fork is authorized; initial commit `6bbf9c96` and its follow-up carry source synchronization and release-maintenance cost. The follow-up passed the Rust-only macOS check and native fixture, but independent review and production Panel delivery are still required before adoption. That cost is preferable to private-module coupling or a shell-owned duplicate Panel state machine.

## 8. Spec review checklist / SPEC 审查清单

- [ ] The approved boundary has exact public types, callback thread, reentrancy, queue/error, and teardown semantics before Aleph consumes it. Independent SPEC returned BLOCK on the initial follow-up. A later fixture passes observable overflow and host-origin delivery; the production validated Panel ingress and ordered re-review remain open.
- [x] The external WebView delivery spike reached the existing external WebView through the public pinned `ElementRef::post_message` page-control path; the isolated witness and its NOT_ACCEPTED whole-run boundary are recorded in `docs/superpowers/runs/2026-11-18-phase10-webview-delivery-receipt.json`. This does not approve production Panel ingress, whose queue/error/ordering/teardown contract remains open.
- [ ] The fork boundary is compiled, integrated reproducibly into Aleph, and reaches the existing external Panel in a native delivery spike. The pinned upstream QA-only witness remains separate and does not satisfy this gate.
- [ ] Every G1-G7 behavior has an observable native or honest-denial evidence requirement.
- [ ] No historical master Phase0-6 claim is promoted to Phase10 completion.
- [ ] No private NSH module is treated as a public API.
- [ ] The Panel remains the sole business/session/Gateway authority.
- [ ] The API gate and user approval are explicit; approval is not confused with implementation or acceptance.
- [ ] Resource ownership and teardown are stated for both enabled and disabled modes.

**Not done / 未做:** No independent follow-up review verdict, reproducible Aleph dependency integration, production Panel ingress, OS permission change, release artifact, system notification-click proof, or G1-G7 acceptance. / 尚无独立复审结论、可复现 Aleph 依赖集成、生产 Panel 入口、OS 权限变更、发布物、系统通知点击证明或 G1-G7 验收。
