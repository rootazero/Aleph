# aleph-desktop-shell

A thin **Tauri v2** native shell that turns the Aleph Panel into a real
desktop app: a native window, a system tray, daemon lifecycle, OS
notifications, launch-at-login, background auto-update, an `aleph://` URL
scheme, a global summon hotkey, and a macOS application menu.

## What it is — and is not

This crate is the "last-mile native shell". It hosts the existing Leptos
Panel (`interfaces/webchat/`, served by `aleph-server` at
`http://127.0.0.1:18790`) inside a native window. It contains **no business
logic and no business UI** — those live in the Panel (R2) and the daemon
(R1/R3). The shell is pure I/O and OS integration, and must stay that way
(R10/P6).

| Concern | Owner |
|---|---|
| Window, tray, notifications, autostart, daemon lifecycle | this crate |
| Auto-update, `aleph://` links, global hotkey, macOS menu | this crate |
| All product UI (chat, memory, settings, …) | Panel (`interfaces/webchat`) |
| All reasoning / tools / state | `aleph-server` daemon |

## How it works

1. On launch the shell shows a native splash (`splash/index.html`).
2. The background worker checks whether `aleph-server` is up; if not it
   launches it **detached** (the daemon outlives the shell — R5/R6).
3. It polls the daemon's `GET /ready` probe; once green, the webview
   navigates to the Panel.
4. Closing the window only **hides** it — the shell stays in the tray.
   The tray offers *Quit (keeps the daemon running)* and
   *Quit & Stop Aleph*.
5. A best-effort WebSocket bridge subscribes to the daemon's EventBus and
   raises native notifications for `approval.requested` / `agent.ask.user`.
6. For the shell's lifetime: a daemon health supervisor relaunches the
   daemon if it dies, and a background checker watches GitHub Releases for
   updates (staged for the user to apply — never restarted under them).
7. Entry points beyond the tray: a global summon hotkey
   (`CmdOrCtrl+Shift+A`), the `aleph://` URL scheme, and — on macOS — the
   system menu bar.

## Daemon discovery

`aleph-server` is bundled **inside the app** as a Tauri `externalBin`, so it
sits next to the shell executable. The shell looks for it in this order:

1. next to the shell executable (the bundled copy, or a `cargo run` build),
2. anything on `PATH` (covers unusual dev setups).

If none is found the splash shows an actionable error.

On first launch — and after every app update — the shell forces any stale
daemon offline (and removes the pre-app bash-installer autostart service) so
the `aleph-server` bundled in this app always wins.

## Notifications & authentication

The notification bridge connects to `ws://127.0.0.1:18790/ws`. If the
Gateway requires authentication, set `ALEPH_GATEWAY_TOKEN` so the shell can
authenticate; otherwise notifications degrade silently and the rest of the
shell is unaffected. Wiring proactive heartbeat events to a dedicated
EventBus topic is a planned follow-up.

## Build & run

```sh
just shell-dev      # run in dev mode (rebuilds the daemon first)
just shell-build    # produce installers (.app/.dmg, .msi, .deb, …)
just check-shell    # cargo check
just clippy-shell   # clippy
```

`cargo tauri build` cannot cross-compile; each platform's installer is
produced on its own CI runner (see `.github/workflows/aleph-server-release.yml`).

## Environment variables

| Variable | Effect |
|---|---|
| `ALEPH_SHELL_LOG` | log filter, e.g. `debug` (default `info`) |
| `ALEPH_GATEWAY_TOKEN` | token used by the notification bridge to authenticate |
| `ALEPH_SHELL_HOTKEY` | override the global summon shortcut (default `CmdOrCtrl+Shift+A`) |
