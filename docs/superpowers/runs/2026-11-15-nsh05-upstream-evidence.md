# NSH 0.5 Upstream Evidence Manifest / NSH 0.5 上游证据清单

**Status / 状态:** source locator only; no local dependency or production change
**Date / 日期:** 2026-11-15
**Repository / 仓库:** `https://github.com/longbridge/nsh.git`
**Release / 发布:** annotated tag object `729ea4c6810112160e890684e8a8d34528ec6da9`; peeled commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`

## Reproduction / 重现

Use a disposable checkout or archive outside the Aleph repository:

```text
git clone https://github.com/longbridge/nsh.git /path/to/nsh-v05
git -C /path/to/nsh-v05 checkout --detach f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1
```

The old local checkout at `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9` is v0.4 and is excluded from v0.5 evidence. This manifest deliberately contains no `/tmp` or machine-specific checkout path.

## Pinned relative source paths / 固定相对源码路径

All paths below are relative to the pinned NSH tree at `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`:

- `crates/nsh-core/src/desktop/window.rs`: `WindowKind`, `WindowMode`, `CloseReason`, `WindowCloseCause`, `WindowAttributes`, request/event split.
- `crates/nsh-core/src/desktop/window_state.rs`: `WindowPlacement`, `WindowState`, display/bounds state.
- `crates/nsh-platform-macos/src/window.rs`: `NshWindow`/`NshPanel`, `FIRST_FRAME_MS = 250`, `CLOSE_GRACE_MS = 250`.
- `crates/nsh-cli/src/apple.rs`: `MACOS_MINIMUM = "14.0"`, `ActivationPolicy::{Regular, Accessory}`, `macos.activationPolicy`, `LSUIElement`.
- `tests/compat/cases/desktop/window-open.tsx`: `Window`, `WindowRef`, `currentWindow`, `windows` compatibility case.
- `tests/compat/cases/desktop/window-ref.tsx`: `WindowRef` state/set behavior.
- `crates/nsh-platform-macos/src/app.rs`, `crates/nsh-platform-linux/src/app.rs`, `crates/nsh-platform-ios/src/view.rs`, `crates/nsh-platform-tui/src/app.rs`: platform `AppView` declarations.

## Exact v0.5 API table / 精确 v0.5 API 表

The following symbols were checked at the pinned commit. They are source evidence for planning only; they do not prove that Aleph currently adapts them.

| Contract | Pinned relative path and symbol | Meaning for Aleph migration |
| --- | --- | --- |
| Host channel | `crates/nsh-engine/src/host_channel.rs`: `HostChannel`, `HostChannel::new`, `set_params`, `params`, `send`, `handle`, `remove_handler`, `handle_open_url`, `remove_open_url_handler` | One `HostChannel` per view; launch parameters are read when an app starts, later host events queue up to the bounded channel policy, and handlers run on the engine thread but may answer later from another thread. |
| Host shutdown | `crates/nsh-engine/src/host_channel.rs`: crate-private `request_shutdown`, `shutdown`, `attach`; `HostAttachment` `Drop` | Shutdown is owned by the platform embedding path. `request_shutdown` lets JS quit listeners run; `shutdown` sets the stop flag and sends the shutdown message; queued events remain recoverable for the next app. Aleph must not call these crate-private functions directly without a verified public v0.5 host seam. |
| macOS host/view | `crates/nsh-platform-macos/src/app.rs`: `AppOptions`, `AppView::new(mtm: MainThreadMarker, options: AppOptions) -> Rc<AppView>`, `AppView::host() -> nsh_engine::HostChannel`, `AppView::load(&self, app: App)`, `run_app(source: AppSource, options: AppOptions) -> std::process::ExitCode` at `:971` | `AppOptions` carries `host`, `keep_alive_on_close`, `activation_policy`, `notifications`, `limits`, cache/data/storage and window options. `AppView::load` builds `EngineConfig` with `runtime: None`, `host: options.host.clone()`, and app access/media policy. |
| Windows host/view | `crates/nsh-platform-windows/src/app.rs`: `AppView` at `:264`, `AppView::new(parent: Option<HWND>, options: AppOptions) -> windows::core::Result<Rc<AppView>>` at `:275`, `show` at `:339`, `run_app` at `:556`; `HostChannel::set_params` use around `:402` | Windows construction and show/exit behavior require a separate platform proof; no Aleph Windows integration is claimed by this slice. |
| Linux host/view | `crates/nsh-platform-linux/src/app.rs`: `AppView` at `:232`, `AppView::new(options: AppOptions) -> Rc<AppView>` at `:248`, `run_app` at `:497` | Linux construction requires the pinned GTK/WebKit platform floor; no Aleph Linux integration is claimed by this slice. |
| Shared engine/runtime | `crates/nsh-engine/src/lib.rs:924` `EngineConfig`; public `EngineConfig::runtime: Option<tokio::runtime::Handle>` at `:961`; macOS `crates/nsh-platform-macos/src/app.rs` `AppView::load` currently sets `runtime: None` | v0.5 does expose an optional Tokio handle field on `EngineConfig`, but the standard macOS AppView path does not inject one. Aleph must choose and document whether its adapter passes a verified handle or preserves `None`; the current private Tokio worker is not proof by itself. |
| Media roots and grants | `crates/nsh-engine/src/lib.rs`: `app_access(app) -> Access`, `media_files(config, access) -> nsh_core::access::MediaFiles` around `:3987–4010`; `crates/nsh-core/src/access.rs`: `Capabilities`, `Access`, `MediaFiles`; `crates/nsh-core/src/image/loader.rs`: `ImageLoaderContext.media_files` and access checks | JS bundles receive the intersection of declared and host-granted capabilities. `media_files` derives allowed roots from access, app data, bundle system paths and native OS files; Rust apps use default media files. Network/webview/openUrl/host/audio/speech/clipboard/device and filesystem-related grants remain capability checks, not implicit adapter authority. |
| Window/secondary windows | `crates/nsh-core/src/desktop/window.rs`: `WindowKind`, `WindowAttributes`, request/event split; `crates/nsh-core/src/desktop/window_state.rs`: `WindowState`, `WindowPlacement`; `crates/nsh-js/src/window.rs`: `WindowRef`, `currentWindow`, `windows`, state/set methods; `tests/compat/cases/desktop/window-open.tsx`, `window-ref.tsx` | The first slice proves only the main Chat window. Secondary window behavior is a compatibility inventory item and remains deferred until the v0.5 API is wired and tested. |
| Platform close ordering | `crates/nsh-platform-macos/src/window.rs`: `FIRST_FRAME_MS = 250`, `CLOSE_GRACE_MS = 250`; `crates/nsh-core/src/desktop/window.rs`: `CloseReason`, `WindowCloseCause` | Measure first-frame and close-settlement at the platform boundary. Do not equate NSH view close with termination of Aleph's detached daemon. |
| TypeScript desktop surface | `crates/nsh-core/src/desktop/window.rs:818` `window::declarations() -> String`; `crates/nsh-js/src/declarations.rs:382` maps module `"nsh/desktop"` to `nsh_core::desktop::window::declarations`; `crates/nsh-js/src/desktop.rs:1088` emits the `WindowRef` interface; compatibility cases `tests/compat/cases/desktop/window-open.tsx`, `window-ref.tsx` import `Window`, `type WindowRef`, `currentWindow`, `windows` from `nsh/desktop` | The generated desktop declarations come from Rust types plus the JS desktop module, not an Aleph-maintained handwritten type file. Aleph must run the pinned release compatibility/type-generation path before adding a dependency; no invented `nsh/desktop` adapter is accepted. |
| Version/platform floors | workspace `Cargo.toml`: version `0.5.0`, `rust-version = "1.97"`; `crates/nsh-cli/src/apple.rs`: `MACOS_MINIMUM = "14.0"`, `ActivationPolicy::{Regular, Accessory}`; platform crates in the manifest | Current Aleph v0.4 paths/Tokio `1.53` are a migration gap. Windows/Linux floors and runtime proof stay out of the first macOS slice. |

## Aleph Create/Modify/Test mapping / Aleph 变更映射

| Step | Aleph path | Planned evidence |
| --- | --- | --- |
| Pin and expose v0.5 dependency | `desktop/nsh-shell/Cargo.toml` | Create/modify only after approval; remove absolute v0.4 paths and record feature/version resolution. |
| Host adapter | `desktop/nsh-shell/src/main.rs`, `desktop/nsh-shell/src/runtime.rs`, `desktop/nsh-shell/src/connection.rs` | Replace old `run_app`/private-runtime assumptions with the verified v0.5 `AppOptions.host`/`AppView` seam; prove hello, subscription, snapshot and reconnect. |
| Canonical Panel data path | `desktop/nsh-shell/src/connection.rs`, proposed `desktop/nsh-shell/src/panel_state.rs`; source authorities `interfaces/webchat/src/api/artifacts.rs:131-138`, `interfaces/webchat/src/components/artifacts/mod.rs:251-257,271-279`, `src/gateway/events/frame.rs:769-773,830-833` | Map `sessions.list` and history snapshot to state; map internal run topics and `stream.*` wire methods into the shared `StreamEvent`; treat `session.artifact` as invalidation then call `artifacts.list`; record deduplication and reconnect rebuild rules. |
| v0.5 compatibility test | proposed `desktop/nsh-shell/tests/nsh_v05_window_compat.rs` | Create/modify only after approval; test `WindowKind`, `WindowRef`, host close/exit settlement and no fabricated local session. |
| Fixed Panel route test | proposed `desktop/nsh-shell/tests/nsh_panel_chat_slice.rs` | Create/modify only after approval; test Chat `/`, Artifacts empty/loading/populated, real history/stream/refusal/reconnect and no duplicate session. |
| Managed daemon boundary | existing `desktop/nsh-shell/src/supervisor.rs` | First slice does not modify detached daemon ownership; it connects to an already-managed server and owns only client/view workers. A later lifecycle contract must specify launch owner and shutdown. |

This table closes the Task3 source-symbol audit. Remaining implementation evidence is intentionally deferred to the approved slice and cannot be inferred from this manifest.
