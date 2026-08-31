# DESKTOP_SHELL.md

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
│  daemon.rs   locate / launch / probe aleph-server; relaunch if it dies  │
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

1. The window opens showing the bundled splash (`splash/index.html`), at the
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
