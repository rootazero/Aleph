# DESKTOP_SHELL.md

## NSH macOS Phase10 scope / NSH macOS Phase10 范围

`desktop/nsh-shell/` is the official NSH 0.6.0 alternative host for the same
Leptos/WASM Panel, not a second business UI. Its resident window, tray, summon
hotkey, validated `aleph://open`, notifications and placement have local macOS
evidence. [Phase10 closeout](../superpowers/runs/2026-11-21-phase10-closeout.md)
uses the explicitly approved Quit-only median/p95 ≤ 1.30× amendment; it does
**not** claim faster Quit or a pass under the original limits. Physical display
topology remains user-deferred and NOT TESTED. / NSH 只托管同一 Panel；按用户批准的
退出门槛修订收尾，不声称提速，多屏拓扑仍未测试。

The Tauri host described below remains available and was the G7 comparator.
No default desktop switch, daemon-supervisor migration, release distribution,
Windows/Linux NSH acceptance or Tauri removal follows. NSH uses an existing
Gateway; its root owns native callbacks and tasks, not that Gateway's lifetime.
`ALEPH_SHELL_HOTKEY=disabled` avoids shortcut registration. Notification OS
permission is not a product feature-disable switch; no such switch is supplied.
/ 保留下述 Tauri；NSH 不接管独立 Gateway，热键可禁用，通知权限不等于功能开关。

Phase11 now has a [scoped real Panel/native status proof](../superpowers/runs/2026-11-22-phase11-production-panel-receipt.json): the regenerated canonical WASM performs an empty nonce-isolated credential load, obtains real credentialless Gateway authorization and renders its actual read-only native login-status component. The [QA guide](../../qa/phase11_nsh_panel_status.md) separates this from protected pairing, OS writes, performance and platform/default/dual-track acceptance. / 已验证真实 Panel 的有限功能链路，不是全期或跨平台验收。

A separate [protected Panel checkpoint](../superpowers/runs/2026-11-22-phase11-protected-panel-receipt.json) proves actual ticket submission, native save, distinct-document stored-credential reload, isolated deletion and canonical helper-confirmed Alice binding plus shutdown. Its [guide](../../qa/phase11_nsh_pairing.md) states that the protected-auth helper is not the full production server; TLS/remote, other platforms, service/default/dual-track and removal remain open. / 受保护配对证据独立，不扩张完整生产服务或三平台结论。


## NSH Panel migration / Panel 迁移

- **当前挂载 / Current mount:** [NSH main](../../desktop/nsh-shell/src/main.rs) hosts the existing Leptos/WASM Panel in a WebView; Tauri remains available. [`panel-modular`](../../interfaces/webchat/Cargo.toml) is opt-in, not a default feature. Its actual `PanelShell` render sites are in [`app.rs`](../../interfaces/webchat/src/app.rs). / NSH 仍托管现有 Panel；模块化面板经 feature 启用，不是另一个生产 UI 权威。
- **S0 基础 / Foundation:** [Gateway `system.hello`](../../src/gateway/handlers/system_hello.rs) on the legacy `/ws` path follows `connect`; it selects a compatible minor and advertises only implemented features. Its legacy limits are compatibility hints, not enforced bounds. / 旧入口的协商值不等于准入，也不代表旧 WS 已部署 IPC 帧上限。
- **S1-A 共享分发 / Shared dispatch:** The existing `/ws` path uses the [common RPC executor](../../src/gateway/server/connection/rpc.rs), and [Client pending calls](../../shared/client/src/connection/requests.rs) belong to one socket. S1-A itself mounted no listener. [Verification receipt](../superpowers/runs/2026-11-23-nsh-panel-s1a-shared-dispatch-verification.md). / 共享分发与请求隔离已验证，旧生产入口不变。
- **S1-B 本地只读 IPC / Read-only Desktop IPC:** An explicit `--desktop-ipc <socket>` mounts the [Unix listener](../../src/gateway/server/connection/local_ipc.rs) using [bounded frames](../../shared/protocol/src/framing.rs), same-account OS authentication, and the instance's live Core owner. The caller supplies a fresh path in its private directory each launch. The dedicated [NSH status command](../../desktop/nsh-shell/src/ipc_status.rs) queries `health` via the shared Client; WebView still uses its existing transport. / 不回退 TCP/WS；仅允许共享的 [`V1_METHODS`](../../src/gateway/server/connection/mod.rs)（S1 时为 `health`、`sessions.list`、`panel.capabilities.snapshot`；S3 与 Memory Nebula 分别增加会话和只读图谱方法，见下）。[S1-B 回执 / receipt](../superpowers/runs/2026-11-23-nsh-panel-s1b-desktop-ipc-verification.md) records macOS native verification; see the independent [Linux native retest](../superpowers/runs/2026-11-23-nsh-panel-s1b-linux-native-retest.md) for Linux. Windows production still refuses both opt-in commands; the [Windows Named Pipe receipt](../superpowers/runs/2026-11-23-nsh-panel-s1b-windows-named-pipe-verification.md) separates candidate evidence from the missing alternate-account gate. / Windows 生产入口仍拒绝，候选结果不代替异账户验收。
- **S1-C 浏览器只读接入 / Read-only Web v1:** The [native TLS listener](../../src/gateway/server/connection/web_v1.rs) mounts explicit same-origin `POST /v1/session` and `GET /v1/ws`. An active paired device or explicitly user-bound one-time ticket yields a short-lived host-only Cookie; it does not change the Core principal. `system.hello` precedes bounded binary calls limited to the same `V1_METHODS` as Desktop IPC (at S1 `health`, `sessions.list`, `panel.capabilities.snapshot`; S3 and Memory Nebula extend the allowlist with chat and read-only graph methods, below); hello carries an opaque per-principal `subject` (HMAC of the user under the shared-token secret) for client-side layout keys. / [S1-C 验证回执](../superpowers/runs/2026-11-23-nsh-panel-s1c-web-v1-verification.md)记录真实 Chrome 证据；旧 `/ws` 和现有 Panel 不换线，未支持代理终止 TLS。
- **S2 可定制 TSX 壳 / Customizable TSX shell (opt-in):** [`interfaces/panel`](../../interfaces/panel/nsh.json) is one TSX source (shell, layout editor, pairing form, Overview / Sessions / Capabilities modules) plus the Rust UI set [`aleph-panel-ui`](../../interfaces/panel/src/lib.rs) over the shared [panel model](../../shared/ui_logic/src/panel/mod.rs). [`PanelLayout`](../../shared/ui_logic/src/panel/layout.rs) persists only stable tab IDs under `aleph.panel.layout.v1:<endpoint>:<subject>`; a tab the snapshot no longer offers shows as unavailable and never runs. Desktop: the shell's `tsx-panel` feature with `ALEPH_SHELL_PANEL=tsx` mounts one embedded `AppView` ([`panel_surface.rs`](../../desktop/nsh-shell/src/panel_surface.rs); a second is refused) whose [host functions](../../desktop/nsh-shell/src/panel_host.rs) reach the Gateway only through the Desktop IPC socket named by `ALEPH_SHELL_PANEL_IPC`, with no WS fallback (macOS only). Web: [`web/host.js`](../../interfaces/panel/web/host.js) speaks the Web v1 framing, and the Gateway serves the package at `/v1/panel/` with a CSP only when `[gateway] panel_web_dir` is set ([`panel_routes`](../../src/gateway/server/connection/web_v1.rs)). Recipes: `just panel-tsx-test`, `panel-tsx-bundle`, `panel-tsx-web`, `shell-tsx-dev <socket>`. [S2 receipt](../superpowers/runs/2026-11-23-nsh-panel-s2-shell-receipt.md) (macOS real-machine Desktop + Chrome). / 默认渲染仍是 Leptos/WASM；TSX 壳需显式启用，DOM-ABI 社区 tab 在其中显示为不可用。
- **记忆星云 / Memory Nebula (opt-in):** Core advertises `aleph:memory-nebula` only when `graph.query`, `graph.search`, and `graph.node_detail` are registered. Desktop IPC and Web v1 expose exactly those graph reads, never graph mutations; registration is not a readiness promise. The [shared TSX module](../../interfaces/panel/modules/MemoryNebula.tsx) renders the same bounded scene and accessible list on both hosts. It does not fall back to legacy `/ws`. / 三个读取方法挂载才公布入口；运行时停用后仍按原权限与错误语义处理。旧 Leptos 图继续使用原入口，不渲染此 TSX 模块。
- **S3 会话纵切 / Chat slice (in the opt-in TSX shell):** `V1_METHODS` adds `chat.history`, `chat.send`, `chat.abort`; v1 writes keep the idempotency cache (`idempotency_key`). The hello feature `live_events` ([`Feature::LiveEvents`](../../shared/protocol/src/system_hello.rs)) is implemented only by the v1 endpoints (`V1_FEATURES`). Once negotiated, the Gateway pushes id-less frames that pass the same [`admit_event`](../../src/gateway/server/connection/event_admission.rs) as legacy `/ws` (ownership, scope, audience, projection), narrowed to `V1_EVENT_TOPICS` (`stream.run_accepted`, `stream.response_chunk`, `stream.run_complete`, `stream.run_error`, `connection.warning`). An oversized frame becomes `connection.warning` and the connection stays; bus lag sends the warning and closes. There is no replay: Chat refetches `chat.history` after a warning, a dropped desktop frame (`connection.stale`) or a reconnect. Both hosts deliver frames as the host event `aleph.event {topic, data}` ([`panel_host.rs`](../../desktop/nsh-shell/src/panel_host.rs), [`web/host.js`](../../interfaces/panel/web/host.js)); modules subscribe with SDK `on(topicPrefix)`. [`Chat.tsx`](../../interfaces/panel/modules/Chat.tsx): history → send (one v4-format key per attempt, built from `Math.random` because the desktop runtime has no Web Crypto) → streamed preview by `run_id` → refetch; Stop → `chat.abort`; a send lost with the connection is reported as unknown and never retried. Sessions rows open their session in Chat. The providerless (simulated) run path now checks session ownership like the real engine. [S3 receipt](../superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.md) (macOS Desktop + Chrome). / v1 端点协商 `live_events` 后推送经同一准入的会话流帧；无重放，断线或告警后重取快照。Linux/Windows、Chrome 内真实输入法、桌面端 Stop 未验证。
- **目标与退场 / Target and retirement:** [Approved design](../superpowers/specs/2026-11-22-nsh-panel-modular-refactor-design.md) targets one shared TSX source, Native Components with Web fallbacks, and Core-owned capability and authorization. Migrating the remaining pages into the TSX shell (after the S3 chat slice), Linux/Windows embedding, the default switch and S4 snapshot/replay remain separate work. The [migration roadmap](../superpowers/plans/2026-11-22-nsh-panel-migration-roadmap.md) retains per-surface replacement proof, three native platforms, and a real common 14-day dual track before Tauri removal. / 未完成对应验证，不切默认、不删旧入口。

The **desktop shell** (`desktop/shell/`, crate `aleph-desktop-shell`) is the
"last-mile native shell" — a thin Tauri v2 app that turns the existing Leptos
Panel into a real desktop application. It is **not** the Panel and **not** the
daemon; it only hosts them.

> Plan of record: `~/.claude/plans/whimsical-herding-bee.md`. Background:
> memory `project_desktop_app_flagship_plan.md`.

## Why it exists

Aleph's daemon (`aleph-server`) is production-grade and its Panel
(`interfaces/webchat/`) is a mature product UI — but the Panel lived as a web
page at `localhost:18790`. Using Aleph felt like running a dev server. The
shell closes that last-mile gap: a native window, a system tray, daemon
lifecycle, OS notifications, and launch-at-login.

## Red-line compliance

| Red line | How the shell complies |
|---|---|
| R1 — brain/limb separation | Lives in `desktop/`, never in `src/`. Phase 1 made **zero** `src/` changes. |
| R2 — single source of UI truth | Hosts the webview only; no business UI. All product UI stays in the Panel. |
| R3 / R10 / P6 — thin, dumb | A handful of small modules, pure I/O + OS bridging. No reasoning, no business logic. |
| macOS Swift Bridge | Untouched — orthogonal to the shell. |

## Architecture

```
┌──────────────────── aleph-desktop-shell (Tauri v2) ────────────────────┐
│  main.rs     builder, window, plugins, run-loop, daemon supervisor      │
│  daemon.rs   legacy reconcile / explicit stop; shared startup reexports │
│  tray.rs     system tray icon + menu                                    │
│  notify.rs   ws://…/ws  → subscribe → OS notifications  (best-effort)    │
│  update.rs   background GitHub-Releases auto-update    (best-effort)     │
│  deeplink.rs aleph:// URL scheme → focus window + forward to the Panel   │
│  hotkey.rs   global summon shortcut (CmdOrCtrl+Shift+A, configurable)    │
│  menu.rs     macOS application menu                       (macOS only)  │
└────────────────────────────────────────────────────────────────────────┘
        │ hosts webview                       │ launches / probes
        ▼                                     ▼
  http://127.0.0.1:18790  ◄─── served by ───  aleph-server daemon
  (the Leptos Panel)                          (resident, outlives the shell)
```

### Startup flow

1. The window opens showing the bundled splash (`desktop/shell/splash/index.html`), at the
   size and position it had last run.
2. A background thread (its own Tokio runtime) probes the daemon port. If a
   foreign process holds it the shell fails fast with a clear message;
   otherwise it launches `aleph-server` **detached** when needed and polls
   `GET /ready`.
3. Once ready, the webview navigates to the Panel.
4. A daemon **health supervisor** then runs for the rest of the shell's
   lifetime (see below).
5. Closing the window only **hides** it — the shell stays in the tray. The
   tray offers *Quit (keeps the daemon running)* and *Quit & Stop Aleph*.

### One window, one instance

A second launch of the app does not open a second window: the
single-instance plugin routes it to the already-running shell and focuses
that. The window's size and position are remembered across restarts (the
window-state plugin); the shell still owns visibility itself.

### Window & process lifecycle (cross-platform)

The operating model is identical on every platform: **the daemon's life is
decoupled from the shell's.** What differs is only the *native gesture* each
OS uses to re-enter — the lifecycle itself never changes.

There are two distinct "close" actions, and both leave the daemon running:

| Action | Effect on shell | Effect on daemon |
|---|---|---|
| **Close the window** (✕ / Cmd+W) | Window is **hidden, not destroyed** — `CloseRequested` calls `window.hide()` + `prevent_close()`; the shell process stays resident in the tray. | Untouched, keeps serving. |
| **"Quit (Aleph keeps running)"** (tray / macOS menu) | Shell process **exits** (`app.exit(0)`). | Untouched — it was spawned detached (Unix double-fork + `setsid`; Windows `DETACHED_PROCESS`), so it is **not** a child of the shell and survives. |
| **"Quit & Stop Aleph"** (tray / macOS menu) | Shell process exits. | **Stopped** — `aleph-server stop` then `app.exit(0)`. The only full teardown. |

**Re-opening always reconnects, never restarts.** A freshly launched (or
re-revealed) shell probes `/ready`, finds the daemon already on its port
(`DaemonReady`), and reveals the Panel against the live daemon instead of
relaunching it (`reconcile` only forces a stale daemon offline on a version
change). So "continue working where you left off" holds whether you closed
the window or quit the shell entirely.

**The re-entry gesture is the only platform-specific part.** The lifecycle
above is uniform; how you bring a closed-to-tray window back is per-OS native
convention:

- **macOS** — click the dock icon (fires `RunEvent::Reopen`, which calls
  `focus_window`; this is the canonical reopen behaviour and the reason the
  app must handle that event), **or** the tray icon, **or** the menu bar's
  *Show Aleph*.
- **Windows / Linux** — `window.hide()` removes the taskbar button entirely,
  so there is nothing to click there. Re-entry is the **tray icon** (left
  click → `focus_window`) or relaunching the app (single-instance focuses the
  running shell).

Likewise the menu-bar items (*Show Aleph*, *Quit & Stop Aleph*, …) are
**macOS-only** (`#[cfg(target_os = "macos")] mod menu`); on the chromeless
Windows/Linux builds the same actions live in the **system-tray menu**. Same
capabilities, native entry points.

### Daemon lifecycle

The daemon is **not** a child of the shell. On Unix the shell runs
`aleph-server --daemon start`, which double-forks and detaches itself; on
Windows it spawns `aleph-server start` with `DETACHED_PROCESS`. Either way the
daemon outlives the shell (R5/R6). The OS-level `flock` singleton guarantees
only one daemon ever runs.

`aleph-server` is bundled inside the app (Tauri `externalBin`), so it resolves
as a sibling of the shell executable; `PATH` is a dev-only fallback.

The shell tells *its* daemon apart from a stranger by the `/ready` probe:
`aleph-server` answers it the moment it binds the port (`200` ready, `503`
booting). Any other reply means a foreign process holds the port — the shell
then refuses to wait on it as if it were a daemon mid-boot, and surfaces an
actionable error instead of silently burning the readiness timeout.

On first launch — and after every app update, gated by a per-version marker
file — the shell **reconciles** the daemon: it removes the pre-app
bash-installer autostart service (the keep-alive launchd / systemd / Task
Scheduler entry that would otherwise resurrect a stale `aleph-server`) and
stops whatever daemon is running, so the version bundled in this app wins.

Switching the Panel to a **remote** Gateway at runtime (Settings → 服务连接)
does **not** stop the local daemon — only the webview navigates away. The
daemon stays resident, still serving any other channels it hosts (CLI, bots,
the dream daemon), consistent with its decoupled lifecycle. The Panel's switch
dialog says as much; **Quit & Stop Aleph** remains the one deliberate way to
stop the local daemon.

### Daemon health supervision

The full Tauri shell delegates startup, status-only readiness probes and relaunch I/O to the unpublished `aleph-desktop-host` library in `desktop/host/src/local_server.rs`. Opt-in NSH `embedded-core` uses the same launcher but checks canonical `/health` and `/ready` documents for the local target before mounting the Panel. Only connection refusal permits launching the bundled sibling; incompatible listeners are left untouched. That compatibility check does not authenticate a process. NSH has no persistent supervisor or stop action yet; version takeover, legacy autostart removal and explicit stop remain in `desktop/shell/src/daemon.rs`. See the [scoped admission receipt](../superpowers/runs/2026-11-22-phase11-gateway-admission-receipt.json). / NSH 仅显式启用的本地 full 路径执行严格启动准入；不接管现有进程，尚未完成监督、停服与三平台验收。

The daemon can crash, be killed, or be stopped out from under the shell. A
supervisor task probes `GET /ready` every few seconds for the shell's whole
lifetime. After a short run of failures it declares the daemon down,
relaunches it (only when the port is genuinely free — a foreign occupant is
left untouched), and once `/ready` is green again it silently reloads the
Panel webview. It never shows or focuses the window (R5); it only keeps the
plumbing connected. A failed *initial* boot is handled by the same machinery:
the supervisor simply keeps retrying until the daemon comes up.

### OS notifications

`notify.rs` opens its own WebSocket to the daemon's EventBus, sends the
mandatory `connect` handshake, subscribes to `surface.notify` and
`surface.approval`, and raises a native notification per event. It is the last
mile of R5 ("AI comes to you") — pure I/O, forwarding events without
interpreting them. Remote `wss://` targets validate with the platform TLS
roots (native-tls); the bridge does **not** take part in the workspace TOFU
pin (`cert_trust::TrustStore` hooks only the webview's certificate challenge),
so a self-signed remote Gateway fails this bridge's handshake while the Panel
itself connects fine — CA-issued certificates work.

**Design note — deviation from the plan's tentative wording.** The plan
floated having the Panel itself call a Tauri notification plugin, and flagged
the daemon possibly needing a new "proactive notification" EventBus topic.
Exploration showed a thinner path: the daemon **already** exposes
notification-worthy topics (`approval.requested`, `agent.ask.user`), so the
shell subscribes to those directly. This keeps the Panel 100% untouched,
needs no remote-IPC capability config, keeps working when the window is
closed but the tray shell is alive, and — crucially — required **no `src/`
core changes at all**. See *Follow-ups* for the heartbeat case.

### Authentication

The notification bridge is best-effort. If the Gateway requires
authentication, set `ALEPH_GATEWAY_TOKEN` so the shell can authenticate;
otherwise notifications degrade silently and nothing else is affected.

### Auto-update

The shell checks GitHub Releases for a newer Aleph in the background — once
about 90 s after launch, then every six hours. It never restarts under the
user (R5): a found update is *staged* and surfaced three non-intrusive ways —
a desktop notification, the tray/macOS-menu update item (relabelled "Restart
to update to vX.Y.Z"), and a **non-modal in-window top banner** with a
"Restart to update" button. The banner is injected into the webview by the
shell (`update.rs::show_update_banner`); its button calls back through a
sentinel navigation (`/__aleph-shell/update/apply` | `/dismiss`)
that the `on_navigation` guard intercepts — a channel that works for the
loopback full app and a remote-pointed Panel-lite alike, but which is honored
**only from the origin the Panel is actually served from**
(`ConnectionTarget::serves_origin`): a foreign page forging the path (e.g. a
rendered markdown link to `http://evil.com/__aleph-shell/update/apply`) is
refused, while the banner's own relative navigations resolve to exactly the
serving origin. The
banner is dismissible per session (`×`) and re-appears on the next launch; the
tray/menu item is the always-available fallback. Applying (any surface)
downloads, installs, and restarts the app — and with it the bundled
`aleph-server`.
The macOS menu's *Check for Updates…* and the tray item also run a manual
check, which always reports its outcome.

Updates are verified against a minisign public key embedded in
`tauri.conf.json`. The checker is best-effort — an unreachable or
unconfigured endpoint is logged and the shell carries on unaffected. CalVer
versions (`YY.M.D`) are valid semver, so the updater's version comparison
works unchanged; see *Build & release* for how CI signs and publishes the
update manifest.

`desktop/host/src/updater.rs::verify_artifact` is the pure, source-compatible
minisign adapter for the NSH migration; it has no default trust key or I/O and
is not yet mounted as an NSH updater. It verifies the global signature before
using the signed version. Existing configs leave `requireSignedVersion` at the
pinned plugin's false default, so authenticated old artifacts without a signed
version still do not bind the announced version. This is not complete rollback
protection. / NSH 迁移现阶段只有纯验签器；未接下载或安装，不改变现有信任和兼容策略，也不冒称完整防回滚。

### `aleph://` deep links

The shell registers the `aleph://` URL scheme. Opening an `aleph://…` link
— from a browser, another app, an email — brings the window forward and
hands the raw URL to the Panel as an `aleph:deep-link` DOM event; the Panel
(R2/R4) decides what to do with it. The shell adds no semantics of its own.
A link opened while the shell is already running is routed to it by the
single-instance plugin's `deep-link` feature on Windows/Linux, and directly
by the OS on macOS.

### Global summon hotkey

A single system-wide shortcut — `CmdOrCtrl+Shift+A` by default — summons
Aleph from anywhere: it shows and focuses the window, or hides it again if
it is already in front. It is the keyboard form of R5. The combination can
be overridden with the `ALEPH_SHELL_HOTKEY` environment variable; a
combination already claimed by another app is logged and skipped, leaving
the tray and window as the other ways in.

### macOS application menu

macOS shows an app menu in the system menu bar regardless of the window's
chrome. The shell builds a tailored one — **Aleph** (About, Show Aleph,
Check for Updates…, Hide, Quit), **Edit**, **Window**. The Edit submenu
uses Tauri's predefined items so the webview keeps native Cmd+C / V / X / A
text editing. Quit is an app-owned item, not the predefined macOS Quit,
which would call `NSApplication terminate` and bypass the close-to-tray
lifecycle. Windows and Linux keep their chromeless look with no menu.

## Panel native-aesthetics pass (Phase 2)

Phase 2 is a CSS/theme-layer pass on `interfaces/webchat/`:

- **Theme modes** — `ThemeMode` is `System / Light / Dark`, and that is the
  whole set (`appearance.rs::ThemeMode::ALL`). This bullet used to describe a
  fourth mode, *Vibrant*, that paired the dark palette with translucent
  surfaces and engaged only inside the macOS shell. Vibrant was superseded by
  *Glass*, and Glass has since been retired too; both survive only as legacy
  `localStorage` values (`"translucent"`, `"glass"`) that
  `legacy_glass_migration` rewrites to dark + liquid material on first boot.
  Translucency is now a material choice, not a mode.
- **Transparent titlebar** — the shell injects `data-shell` / `data-platform`
  flags; the Panel's `.app-titlebar` leaves room for the macOS traffic lights.
  **The two flags are not interchangeable.** `data-platform` answers *which
  WebView engine is rendering*, and `baseline-probe.js` resolves it from the
  user agent when no host declared one — so it reads `macos` in a plain browser
  on a Mac exactly as it does in the shell. Only `data-shell="aleph-tauri"`
  says the document is inside a Tauri window. Window chrome (the overlay
  traffic lights, the transparent window behind the vibrancy, the `<main>`
  drag band) therefore keys on **both**:
  `html[data-shell="aleph-tauri"][data-platform="macos"]`. Keying it on
  `data-platform` alone is what put the sidebar collapse toggle 72 px from the
  left of a browser window that has no traffic lights to clear;
  `platform_host.rs::stylesheet_never_keys_window_chrome_on_the_os_alone`
  keeps the pair together — over every `interfaces/webchat/styles/*.css`,
  derived from the directory, because naming one file left `ios.css` unscanned.

  **`data-shell` is there in time, and that was measured, not reasoned.**
  Gating chrome on a second attribute is only safe if the attribute arrives
  before first paint, and for a panel-only shell pointed at a *remote* Gateway
  the source comments claimed it did not — that `initialization_script` runs
  "only for same-origin pages" and a foreign origin waits for the
  `on_page_load` re-assert. That claim entered the tree in `4c31bfea4`, a
  commit that fixed a real undraggable-window bug with **two** changes at once;
  only the other one (`grant_remote_drag`, which grants the remote origin the
  `core:window:allow-start-dragging` capability — the ACL *is* origin-scoped)
  was load-bearing. The injection is not gated: it becomes a
  `WKUserScript(AtDocumentStart)` on the webview's user content controller, and
  a user script has no origin concept. Measured 2026-08-31 against a genuinely
  non-loopback origin: `data-shell` and `data-platform` are both set when the page's own
  first inline `<script>` runs. Mutating the init-script leg away leaves them
  unset through `window.onload` (`PageLoadEvent::Finished` maps to
  `didFinishNavigation`, which fires after `load`), which is what the flash
  would have looked like had the claim been true.
  Pinned by `qa/webview_compat/run.sh macos`, scenario `marker-origin`.
  Windows and Linux take the same unconditional path in wry but have not been run.
- **Typography** — an explicit `-apple-system` / SF system font stack.
- **Motion** — one converged ease-out curve; `prefers-reduced-motion` honored.
- **Depth** — soft, layered shadow tokens.
- **Workspace pane and the drag band** — the chat's right-hand workspace pane
  is a **pane of bodies** (`WorkspaceBody { Artifacts, Deliverables, Tasks,
  Canvas }`, `state/layout.rs`; the whiteboard canvas has been one of them
  since 2026-09-12 and has no route or tab of its own). The only piece of it
  the shell touches is the `LayoutToggle` parked inside the `<main>` top drag
  band (`app.rs::ChatBandChrome`, `aleph-no-drag` +
  `data-tauri-drag-region="false"`): its right offset reads
  `--aleph-workspace-w`, which the pane's left-edge resizer publishes on the
  **document root** so that the band chrome — outside `ChatView`'s subtree —
  follows the dragged width. Mechanism, keep-alive rule and auto-reveal live in
  [CANVAS.md §6](CANVAS.md); nothing here is shell-specific.

The shell sets DOM flags via an `initialization_script`; the Panel only
*reacts* to them in CSS. No business logic crosses the boundary.

## Build & release

The app **is** the build artifact: `aleph-server` (and, on macOS, the Swift
bridge) is staged as a Tauri `externalBin` and bundled inside the app, so a
freshly installed `.dmg` / `.msi` / `.deb` is fully self-contained — there is
no separate bash-script install path.

| Command | Effect |
|---|---|
| `just shell-dev` | Run the app in dev mode (rebuilds + stages the daemon) |
| `just shell-build` | Produce installers (`.dmg`, `.msi`, `.deb`, …), daemon bundled in |
| `just check-shell` / `just clippy-shell` | Compile / lint the crate |

The bundle version is the `VERSION` file value (`YY.M.D`, e.g. `26.5.7`),
injected via `cargo tauri build --config`. That form is valid semver and
satisfies the Windows MSI version constraints.

Tauri cannot cross-compile: `.github/workflows/aleph-app-release.yml`
builds the daemon and bundles the app on a per-platform matrix
(macOS / Linux / Windows), attaching each installer to the release.

**Auto-update signing.** Update bundles must be signed with a minisign key
whose public half lives in `tauri.conf.json` (`plugins.updater.pubkey`).
The release workflow produces and signs the updater artifacts — and
assembles the `latest.json` manifest the shell polls — only when the
`TAURI_SIGNING_PRIVATE_KEY` repository secret (and, if the key has one,
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) is configured. Without the secret the
build is unchanged and simply ships no auto-update artifacts that release.
Auto-update is therefore an additive, opt-in layer over the CalVer release
model, not a change to it.

## Panel ↔ Daemon 资源嵌入链与刷新

Panel UI（`interfaces/webchat/dist/*`）通过 `rust_embed`（`src/gateway/control_plane/assets.rs`）在 **`aleph-server` 编译时** 静态嵌入到二进制；运行中的 daemon **不会** 从磁盘读取 dist/*。改完 panel 源码后看不到效果，几乎都是漏了重编 binary 这一步。

完整刷新链：

1. `just wasm` — 重建 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}`
2. `cargo build --release -p alephcore --bin aleph-server` — 让 `rust_embed` 把新 dist 烧进 binary
3. 替换正在跑的 binary，让 supervisor relaunch：
   - **dev** (`cargo run` 启动的 daemon)：`./target/release/aleph-server stop` → `cargo run --release -p alephcore --bin aleph-server start`（Windows 同理，二进制为 `aleph-server.exe`）
   - **.app daemon (macOS)**：`mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak}` → `cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/` → `kill <pid>`（Tauri shell 的 supervisor 会自动 relaunch 新 binary 并 reload webview）
   - **安装版 App daemon (Windows)**：先停（`aleph-server stop` 或 `taskkill /IM aleph-server.exe /F`）——**Windows 无法覆盖正在运行的 exe，必须先停**——再 `Copy-Item target\release\aleph-server.exe "$env:LOCALAPPDATA\Aleph\aleph-server.exe" -Force`（NSIS 默认按用户安装；旧 `aleph-server.exe` 与 `Aleph.exe` 同目录），最后重启 `Aleph.exe`，supervisor 会拉起新 binary 并 reload webview

单跑 `just wasm` / `just dev` **不够**——前者只更磁盘，后者只在 daemon 还没启动时有效。已经在跑的 daemon 必须替换 binary 才能感知 panel 改动。

## Follow-ups (not in this cycle)

- **Heartbeat → EventBus topic.** True proactive heartbeat notifications would
  benefit from a dedicated EventBus topic emitted by the heartbeat service.
  The plan pre-authorized a minimal `src/` topic for this; it was deferred
  because the existing topics already satisfy Phase 1's verification.
- **Window drag region.** Native titlebar drag works; extending the drag
  region across the whole top bar needs a Tauri drag handler on the external
  Panel page.
- **Panel-side deep-link routing.** The shell forwards every `aleph://` URL
  to the Panel as an `aleph:deep-link` DOM event, but the Panel does not yet
  listen for it — today a deep link reliably summons the window, and routing
  to a specific view awaits a Panel-side handler. A link that cold-starts
  the app may also arrive before the Panel finishes loading.
- **Code signing / notarization.** Required for warning-free `.dmg` / `.msi`
  distribution. The bundled `externalBin` daemon must be signed with the same
  identity as the app, or Gatekeeper blocks it — a one-time account/cert
  cost, not a code change.
