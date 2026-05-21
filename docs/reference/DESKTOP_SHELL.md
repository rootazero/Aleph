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
| R3 / R10 / P6 — thin, dumb | ~4 small modules, pure I/O + OS bridging. No reasoning, no business logic. |
| macOS Swift Bridge | Untouched — orthogonal to the shell. |

## Architecture

```
┌──────────────────── aleph-desktop-shell (Tauri v2) ────────────────────┐
│  main.rs    builder, window, plugins, run-loop, window-close = hide     │
│  daemon.rs  locate aleph-server → launch detached → poll /ready         │
│  tray.rs    system tray icon + menu                                     │
│  notify.rs  ws://…/ws  → subscribe → OS notifications  (best-effort)     │
└────────────────────────────────────────────────────────────────────────┘
        │ hosts webview                       │ launches / probes
        ▼                                     ▼
  http://127.0.0.1:18790  ◄─── served by ───  aleph-server daemon
  (the Leptos Panel)                          (resident, outlives the shell)
```

### Startup flow

1. The window opens showing the bundled splash (`splash/index.html`).
2. A background thread (its own Tokio runtime) checks whether the daemon is
   up; if not, it launches `aleph-server` **detached** and polls
   `GET /ready`.
3. Once ready, the webview navigates to the Panel.
4. Closing the window only **hides** it — the shell stays in the tray. The
   tray offers *Quit (keeps the daemon running)* and *Quit & Stop Aleph*.

### Daemon lifecycle

The daemon is **not** a child of the shell. On Unix the shell runs
`aleph-server --daemon start`, which double-forks and detaches itself; on
Windows it spawns `aleph-server start` with `DETACHED_PROCESS`. Either way the
daemon outlives the shell (R5/R6). The OS-level `flock` singleton guarantees
only one daemon ever runs.

`aleph-server` is bundled inside the app (Tauri `externalBin`), so it resolves
as a sibling of the shell executable; `PATH` is a dev-only fallback.

On first launch — and after every app update, gated by a per-version marker
file — the shell **reconciles** the daemon: it removes the pre-app
bash-installer autostart service (the keep-alive launchd / systemd / Task
Scheduler entry that would otherwise resurrect a stale `aleph-server`) and
stops whatever daemon is running, so the version bundled in this app wins.

### OS notifications

`notify.rs` opens its own WebSocket to the daemon's EventBus, sends the
mandatory `connect` handshake, subscribes to `approval.requested` and
`agent.ask.user`, and raises a native notification per event. It is the last
mile of R5 ("AI comes to you") — pure I/O, forwarding events without
interpreting them.

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

## Panel native-aesthetics pass (Phase 2)

Phase 2 is a CSS/theme-layer pass on `interfaces/webchat/`:

- **Vibrant theme** — a fourth theme mode (System / Light / Dark / Vibrant).
  Vibrant pairs the dark palette with translucent surfaces. It engages only
  inside the macOS shell (`data-platform="macos"`, injected by the shell);
  elsewhere it degrades silently to the solid dark theme.
- **Transparent titlebar** — the shell injects `data-shell` / `data-platform`
  flags; the Panel's `.app-titlebar` leaves room for the macOS traffic lights.
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

Tauri cannot cross-compile: `.github/workflows/aleph-server-release.yml`
builds the daemon and bundles the app on a per-platform matrix
(macOS / Linux / Windows), attaching each installer to the release.

## Follow-ups (not in this cycle)

- **Heartbeat → EventBus topic.** True proactive heartbeat notifications would
  benefit from a dedicated EventBus topic emitted by the heartbeat service.
  The plan pre-authorized a minimal `src/` topic for this; it was deferred
  because the existing topics already satisfy Phase 1's verification.
- **Window drag region.** Native titlebar drag works; extending the drag
  region across the whole top bar needs a Tauri drag handler on the external
  Panel page.
- **Code signing / notarization.** Required for warning-free `.dmg` / `.msi`
  distribution. The bundled `externalBin` daemon must be signed with the same
  identity as the app, or Gatekeeper blocks it — a one-time account/cert
  cost, not a code change.
