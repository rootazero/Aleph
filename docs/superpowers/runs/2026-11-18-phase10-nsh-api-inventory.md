# Phase10 NSH 0.5 API Inventory
# Phase10 NSH 0.5 API 清单

**Pinned source / 固定源码:** `/Users/zouguojun/.cargo/git/checkouts/nsh-9218b667fc8c8325/f4d68f7`
**Revision / 修订:** `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`
**Workspace version / 工作区版本:** `0.5.0`

This inventory records public and private surfaces actually inspected. It is not permission to depend on private modules. The pinned-upstream audit is historical; see the [local integration checkpoint](2026-11-19-phase10-local-integration-checkpoint.md) for the separate fork-based candidate and its bounded evidence. / 本清单记录实际检查过的公共/私有接口，不构成使用私有模块的授权。上游固定版本的审计是历史记录；另见本地集成检查点，了解 fork 候选及其证据边界。

## 1. Aleph current host / Aleph 当前宿主

### `desktop/nsh-shell/src/main.rs`

Current imports include the public pinned page-control path:

```rust
use nsh_core::prelude::{
    Element, IntoElement, Prop, Props, Styled, component, element_ref, set_timeout, web_view,
};
use nsh_platform_macos::{AppOptions, AppSource, Root, quit_app, run_app};
```

Current flow:

1. `HostSettings::from_inputs` validates the loopback Panel URL and optional QA nonce.
2. `web_view(props.url)` creates the existing external Panel WebView.
3. QA JavaScript is injected before content and reports through `window.nsh.postMessage`.
4. `on_message` validates nonce and run id, records QA events, and currently calls `quit_app()` for `close-requested`.
5. `run_app(AppSource::Rust(root), AppOptions { title: "Aleph", size: (1180, 800), app_id: "com.aleph.nsh-shell", .. })` starts the standalone NSH app.
6. Return from `run_app` emits a synthetic QA `closed` record with `owned_drained`.

The final step proves explicit app quit/engine return only. It does not identify native `windowShouldClose`, `windowWillClose`, tray, hotkey, notification, or URL callback delivery.

### `desktop/nsh-shell/src/shell_host.rs`

`HostSettings::from_inputs(url: Option<&str>, nonce: Option<&str>) -> Result<HostSettings, String>`:

- default: `http://127.0.0.1:18790/` with no QA nonce;
- debug QA: explicit `http://127.0.0.1:<nonzero>/` plus exactly 64 lowercase hexadecimal characters;
- rejects external origins, credentials, non-root paths, query tokens, missing/extra nonce, invalid ports and invalid nonce characters.

## 2. Public macOS app surface / macOS 公共应用接口

### `nsh-platform-macos/src/lib.rs`

Public re-exports include `App`, `AppOptions`, `AppSource`, `AppView`, `WindowDisplay`, `quit_app`, `run_app`, `Root`, `objc2`, `objc2_app_kit`, and `objc2_foundation`.

### `nsh-platform-macos/src/app.rs`

`AppOptions` public fields relevant to this phase:

- `app_id: String`
- `title: String`
- `size: (i32, i32)`
- `display: Option<WindowDisplay>`
- `keep_size: bool`
- `background: bool`
- `activation_policy: ActivationPolicy`
- `keep_alive_on_close: bool`
- `notifications: bool`
- `host: nsh_engine::HostChannel`
- `transparent: bool`

`keep_alive_on_close: true` is a public resident-window option for `run_app`; it hides the standalone window on user close while retaining its engine. Explicit `quit_app()` still stops the app.

Embedding API:

```rust
pub fn new(mtm: MainThreadMarker, options: AppOptions) -> Rc<AppView>;
pub fn host(&self) -> nsh_engine::HostChannel;
pub fn load(&self, app: App);
pub fn stop(&self);
```

The embedding API exposes a host channel but does not expose a public native window/delegate callback registration API in the inspected source. `run_app` does not return `AppView` or a host handle.

### Native callbacks observed in private implementation

`nsh-platform-macos/src/app.rs` internally installs a delegate handling `application:openURLs:`, `applicationDidFinishLaunching:`, `applicationShouldTerminateAfterLastWindowClosed:`, `applicationShouldTerminate:`, `applicationShouldHandleReopen:hasVisibleWindows:`, `windowShouldClose:`, and `windowWillClose:`. `quit_app()` takes the explicit app-termination path and is distinct from user close.

These facts explain the behavior but are not a supported Aleph host API. Do not import private delegate types or reproduce the implementation in the shell.

## 3. Host channel / HostChannel

### `nsh-engine/src/host_channel.rs`

Public methods inspected:

```rust
pub fn new() -> Self;
pub fn set_params(&self, params: serde_json::Map<String, JsonValue>);
pub fn params(&self) -> serde_json::Map<String, JsonValue>;
pub fn send(&self, name: impl Into<String>, data: JsonValue) -> Result<(), String>;
pub fn handle(&self, name: impl Into<String>, handler: impl Fn(HostCall) + Send + Sync + 'static);
pub fn remove_handler(&self, name: &str);
pub fn handle_open_url(&self, handler: impl Fn(OpenUrlRequest) + Send + Sync + 'static);
pub fn remove_open_url_handler(&self);
```

`send` delivers `host.on(name, data)` events to a running NSH app and queues up to `nsh_core::host_channel::MAX_QUEUED_EVENTS` (256) while no app is running. Handlers run on the engine thread and must return quickly.

`HostChannel` is an embedding-app-to-NSH-app message boundary. It is not an AppKit callback subscription and does not by itself make the current external Panel route owner consume events.

## 4. Root and WebView API / Root 与 WebView 接口

### `nsh-core/src/element/host.rs`

Public constructors/builders inspected:

```rust
pub fn web_view(url: impl IntoReactive<String>) -> WebView;
pub fn web_view_html(html: impl IntoReactive<String>, base_url: Option<&str>) -> WebView;
pub fn injected_java_script_before_content(self, script: impl IntoReactive<String>) -> Self;
pub fn injected_java_script(self, script: impl IntoReactive<String>) -> Self;
pub fn on_message<M>(self, listener: impl IntoListener<str, M>) -> Self;
pub fn on_navigation_state_change<M>(self, listener: impl IntoListener<WebNavigation, M>) -> Self;
pub fn url_scheme(self, scheme: &str, handler: impl Fn(WebResourceRequest) + Send + Sync + 'static) -> Self;
```

No public host-side `evaluate_javascript`, `on_host`, `on_appkit_event`, or root listener for native OS events was found. Generic `Element::on_event` is for typed component events, not an external AppKit callback bridge.

The current Panel uses the WebView's `window.nsh.postMessage` path only for QA reporting. No existing production Panel listener for NSH `host.on(...)` events was found in the inspected source.

## 5. NSH canonical representations / NSH canonical 表示

### `nsh-core/src/desktop.rs`

`DesktopCommand` includes:

```text
OpenWindow
SetAttributes
Request
Quit
Restart
SetMenu
SetMenuBar
PopupMenu
SetTray
RegisterGlobalShortcut
UnregisterGlobalShortcut
```

`Tray` and `Shortcut` are canonical engine representations. Tray click/menu reports use `EventKind::Menu`; global shortcut registration reports `registered: bool` and presses report the shortcut id through menu events.

### Notifications

`nsh-core/src/notifications.rs` defines:

```text
NotificationPermission::{Default, Granted, Denied, Unavailable}
NotificationCommand::{Permission, Show, Close, Listen, SetBadge}
NotificationEvent keys: id, permission, shown, pressed
```

### URL schemes

`nsh-core/src/url_scheme.rs` provides `check_scheme`, `schemes_from_json`, and `check_url`, including URL length and reserved-scheme checks. System-delivered URLs are retained for root listeners and are handled by application logic.

### Single instance

`nsh-core/src/single_instance.rs` defines `SecondInstance { args, cwd }`, `to_json`, `from_json`, and `MAX_PENDING = 16`. Its documentation says macOS bundle launches are already single-instance and URL/file delivery uses URL schemes rather than a second-instance event.

### Window

`nsh-core/src/element/desktop.rs` exposes `main_window() -> WindowRef` and methods such as `show`, `hide`, `focus`, `close`, `restore`, `set_bounds`, `set_placement`, and `on_close_press`. These are engine graph operations inside an NSH app, not a callback surface from standalone `run_app` to the embedding Rust host.

## 6. Feasibility matrix / 可行性矩阵

| Requirement | Public pinned API found | Current Aleph delivery | Status |
| --- | --- | --- | --- |
| Resident close/hide | `AppOptions::keep_alive_on_close` | Enabled in Aleph `run_app` | Actual close-button hide and residence observed with a local HTML fixture; reopen, focus, and Quit unproven |
| Explicit quit | `quit_app`, `DesktopCommand::Quit` | Current QA close path uses `quit_app` | Available; already Phase9-scoped |
| Rust user-close callback | None from `run_app` | No callback owner | API gap |
| Reopen/focus callback | Internal delegate only; engine window methods | No shell callback | API gap for host-side evidence |
| Tray callback | Private backend + engine `EventKind::Menu` | No Panel host listener | Boundary gap |
| Global hotkey callback | Private backend + engine menu event | No shell host listener | Boundary gap |
| URL delivery | Internal app/root listener and `HostChannel::handle_open_url` for app-opened URLs | Current external Panel route owner not connected | Delivery gap |
| Notification click/permission | Internal notification command/event path | No host-side route | Boundary gap |
| Window state | Internal state watcher and engine projection | No host-side callback | Boundary gap |
| Host-to-app event | `HostChannel::send` | No production Panel listener found | Candidate, unproved |
| Page-control handle | `element_ref()` + `Element::element_ref()` + `ElementRef::post_message` | Compile spike passed; isolated native QA witness captured one matching `rust-webview-message` delivery; independent receipt recorded | QA-only delivery sub-result PASS; production ingress and contract review remain open |
| Host-side JS evaluation | `ElementRef::evaluate` exists after attaching a ref | Not used for the Phase10 ingress; no delivery/teardown contract | Public page control, not approved event boundary |
| Approved boundary | Maintained public fork API: typed `AppEvent`/`AppEventSink` plus checked bounded WebView ingress | Initial commit `6bbf9c96`; follow-up passes offline Rust-only macOS compilation and the native fixture | Independent re-review and Aleph integration remain open |

## 7. Approved boundary follow-up / 已批准边界后续

The pinned upstream lacks the complete Phase10 boundary. The fork exports `nsh_platform_macos::{AppEvent, AppEventSink}` and `AppOptions::event_sink`, and adds `ElementRef::post_message_json`. The initial commit received an independent QUALITY verdict of not ready: cold URLs could miss the host, notification presses could be lost or duplicated, and registration did not prove system delegate ownership.

The follow-up passes offline Rust-only macOS compilation, the checked-message queue regression and a native fixture that uses the AppKit delegate and a real WKWebView. Independent SPEC returned BLOCK on the first follow-up. The revised fixture verifies observable startup overflow and a host-origin message; production Panel ingress and re-review remain open. See `docs/superpowers/runs/2026-11-19-phase10-nsh-fork-api-receipt.json`. Aleph still uses pinned upstream. / 首次跟进补丁独立 SPEC 未通过。修订夹具已验证启动溢出和宿主发起投递，生产入口及复审仍未关闭，Aleph 尚未切换依赖。

- The fork's typed sink emits owned close/reopen/quit, URL, tray, shortcut, notification registration/press, and window-state facts through existing NSH owners.
- The checked WebView path returns `NotMounted`, `TooLarge`, `QueueFull`, or `Serialization`, caps serialized JSON at 64 KiB and pending checked commands at 256, and reuses the existing ordered platform dispatch.
- Legacy `ElementRef::post_message` remains unchanged and is intentionally outside the checked quota.

The shell must consume only these public APIs after the fork compiles and the real delivery contract is reviewed. The Panel remains the sole business/navigation/session/Gateway owner. Private NSH imports, copied delegates, secret-bearing URLs, second transports/reducers, and detached daemons remain prohibited.

**Not done / 未做:** No Aleph dependency switch, independent follow-up review verdict, production Panel ingress, LaunchServices or notification-click proof, OS permission change, or G1-G7 acceptance. The earlier Phase9-schema launcher run remains NOT_ACCEPTED. / 未切换 Aleph 依赖、未取得独立复审结论、未接入生产 Panel、未验证系统 URL 投递或通知点击、未改变系统权限、未验收 G1-G7。使用 Phase9 schema 的先前测试整轮仍未通过。
