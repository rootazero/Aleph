# In-window "Restart to update" banner (窗口内"重启以更新"横幅)

**Date**: 2026-07-14
**Scope**: `desktop/shell/` (aleph-desktop-shell) — full app + Panel-lite, all platforms
**Status**: Design approved, pending implementation plan

---

## 1. Problem (问题)

Today an available update is surfaced **only** through a desktop notification plus a
relabelled tray / macOS-menu item (`update.rs::stage` → "Restart to update to vX.Y.Z").
Applying the update requires the user to *find and click that menu item*. This path is
too hidden — users don't know to open the tray/menu, so staged updates sit unapplied.

**Goal**: make the "restart to update" affordance **impossible to miss** with an
in-window banner + button, uniformly on macOS / Windows / Linux, for both the full app
and the Panel-lite shell. **Manual "Check for Updates" stays in the tray/menu** (it is
the right home for a user-initiated check — unchanged).

**目标**:把"重启以更新"从仅存在于托盘/菜单(隐蔽),升级为窗口内始终可见的顶部横幅 +
按钮,全平台一致,完整版与 Panel 纯壳版通用。手动"检查更新"保留在托盘/菜单,不动。

---

## 2. Constraints & redline compliance (约束与红线合规)

| Redline | Compliance |
|---|---|
| **R5** — 不抢焦点 / 不弹模态对话框 | The banner is a **non-modal** in-window DOM overlay: it never steals focus, never blocks the window, and is dismissible. This is precisely why a non-modal banner is chosen over a native OS modal dialog (which would steal focus and violate R5). |
| **R2 / R4** — UI 唯一源 / Interface 纯 I/O | The banner is **shell chrome**, not business UI. The updater is 100% shell-owned (tray, menu, `tauri-plugin-updater` all live in the shell). The banner is the same class of host→webview injection the shell already does: `splash`, the `__alephError` hook, the `SHELL_MARKER_JS` platform marker, and the external-link interceptor. No business logic, no settings form. |
| **R10** — 薄 harness | Pure I/O in the desktop shell. Zero `src/harness/` changes, zero core changes. |
| **Tech-stack** | Zero new dependencies. `tauri-plugin-dialog` (removed earlier) is **not** re-introduced. |

Also honoured: **cross-platform uniformity** (one wry-level mechanism, no per-OS code
except a small macOS titlebar offset), and **minimal-change** — everything lands in
`desktop/shell/`, no Panel/WASM rebuild, no re-embed of `aleph-server`.

---

## 3. Chosen UX (选定的交互)

Decisions locked during brainstorming:

1. **Form** — In-window **top banner** (non-modal), *not* a corner card, *not* a native
   modal dialog.
2. **Ownership** — **Shell-injected overlay** (host injects via `window.eval`), *not* a
   Panel Leptos component. One codebase, lightest deploy (rebuild the shell only).
3. **Persistence** — **Dismiss per session, re-show next launch**. The `×` hides it for
   the current run; the tray/menu "Restart to update" item stays available as the
   fallback; the next app launch re-finds and re-shows it.

Banner content (self-installing platforms — macOS / Windows / Linux AppImage):

```
┌────────────────────────────────────────────────────┐
│ ⟳ Aleph 26.7.14 is ready   [ Restart to update ]  × │
└────────────────────────────────────────────────────┘
```

On click of **Restart to update**, the banner switches to a disabled progress state
("Updating… Aleph will restart") and the existing `apply_staged_update` runs
(download → stop daemon → install → restart).

---

## 4. Callback channel (回调通道) — the load-bearing decision

The banner button must call back into the shell to apply/dismiss. The mechanism must be
**origin-independent** because the Panel-lite shell can point at a *remote* Gateway.

**Fact** (`capabilities/default.json` + `connection.rs`): Tauri IPC (`invoke`) is scoped
to `http://127.0.0.1:18790/*` (loopback) only — **a remote-origin Panel cannot invoke
shell commands**. So `invoke` is ruled out.

**Chosen channel**: reuse the shell's existing, proven, origin-independent host↔webview
channel — the `WebviewWindowBuilder::on_navigation` guard (`external_link::route`). This
is the same pattern the external-link interceptor already uses.

- The button sets `window.location.href = '/__aleph-shell/update/apply'` (or
  `.../update/dismiss`) — a **same-origin absolute-path navigation**, which reliably
  fires `on_navigation` on every platform and every origin (loopback or remote).
- The `on_navigation` guard recognises the reserved `/__aleph-shell/update/*` path,
  performs the shell action, and returns `false` to **cancel the navigation** — the
  reserved path never actually loads (no 404, no page change).

**Why not the `aleph://` OS deep link**: that round-trips through the OS handler
(re-activates the app, extra hop). Intercepting at `on_navigation` is the most direct.

`/__aleph-shell/` is a reserved prefix the Panel/daemon never serve, so there is no
collision with real Panel routes.

---

## 5. Lifecycle & state (生命周期与状态)

- **Appear** — in `update.rs::stage()` (already runs when a new version is found),
  after the existing notification + menu relabel, **also inject the banner**.
- **Restart click** — the injected click handler first swaps the banner into the
  "Updating…" disabled state, then triggers the `apply` sentinel navigation →
  existing `apply_staged_update` (unchanged download/stop/install/restart flow).
- **Dismiss (`×`)** — hides the banner and sets an in-memory `Updater.dismissed`
  latch for the session. The tray/menu "Restart to update" item remains the fallback.
- **Re-inject on Panel reload** — when the daemon recovers the shell re-navigates the
  Panel, wiping the injected DOM. The existing `on_page_load(Finished)` hook (already
  re-asserts `SHELL_MARKER_JS`) also calls `reinject_banner_if_staged`: if staged **and
  not dismissed**, re-inject so the banner survives reloads.
- **Re-show next launch** — `staged` and `dismissed` are in-memory process state
  (fresh `Default` each launch). A new process starts clean; the checker re-stages
  ~90 s in and re-injects. This is exactly the chosen persistence behaviour.
- **Package-manager installs** (Linux `.deb` / `.rpm`, `updater_can_self_install() ==
  false`) — the banner's primary button reads **"How to update"** and its action opens
  the GitHub releases page (external), matching the existing menu-item behaviour; no
  self-restart is attempted.

`Updater` state additions: `dismissed: Mutex<bool>` (session latch). Existing
`staged: Mutex<Option<String>>` and `update_items` are unchanged.

---

## 6. Files touched (改动文件清单) — all under `desktop/shell/`

Both the full app (`embedded-core`) and lite (`--no-default-features`) compile this;
nothing here is feature-gated.

### `src/update.rs` (the bulk)
- Add `dismissed: Mutex<bool>` to `Updater`.
- Pure, unit-testable helpers:
  - `banner_script(version: &str, self_install: bool) -> String` — the injected JS
    (idempotent by element id; inline styles; `addEventListener`, not inline `onclick`;
    version escaped via `serde_json`).
  - `control_action(url: &Url) -> Option<UpdateControl>` — maps
    `/__aleph-shell/update/apply` → `Apply`, `/__aleph-shell/update/dismiss` → `Dismiss`,
    everything else (real Panel routes, external links) → `None`.
- Side-effecting helpers:
  - `show_update_banner(app)` — evals `banner_script` on the main window.
  - `reinject_banner_if_staged(app)` — called from `on_page_load`.
  - `handle_control(app, action)` — `Apply` → `apply_staged_update`; `Dismiss` → set
    `dismissed` + eval-remove the banner element.
- `stage()` calls `show_update_banner` at the end (alongside the existing notify +
  relabel).

### `src/main.rs`
- In `build_main_window`, replace `.on_navigation(external_link::route)` with a closure
  that captures `handle`, first checks `update::control_action(url)` (→
  `handle_control` + return `false` on match), else falls through to
  `external_link::route(url)`.
- In `.on_page_load(… Finished)`, after the `SHELL_MARKER_JS` eval, add
  `update::reinject_banner_if_staged(...)`.

### `src/tray.rs` / `src/menu.rs`
- **Unchanged.** Manual "Check for Updates" and the post-stage "Restart to update"
  fallback item both stay as they are.

---

## 7. Testing (测试计划)

- **Unit tests** (pure functions, no window; `cargo test -p aleph-desktop-shell` is
  cheap — small crate, unlike the memory-heavy `alephcore`):
  - `banner_script` contains the version, `"Restart to update"`, and the `apply`
    sentinel path; the non-self-install variant contains `"How to update"` + the
    releases URL.
  - A version string containing quotes is escaped (serde_json) — no literal breakout.
  - `control_action` maps apply/dismiss correctly and returns `None` for ordinary
    Panel routes and external links (no mis-interception).
- **Compile check** — `cargo check -p aleph-desktop-shell` for **both** default (full
  app) and `--no-default-features` (lite).
- **Manual E2E** (listed, not automated — needs a real staged update): temporarily
  lower the version / point the updater at a test manifest, observe the banner appear
  → click **Restart to update** → app restarts into the new version. Verify on at least
  Windows + macOS.

---

## 8. Edge cases (边缘情况)

- **macOS overlay titlebar** — under `data-platform="macos"` the banner gets a ~28 px
  top offset so it clears the overlay traffic-light inset.
- **CSP** — the banner is built with `document.createElement` + `addEventListener`
  (no inline `onclick`), so a strict Panel CSP cannot block it; `location.href`
  same-origin navigation is unaffected by script-CSP.
- **Injection safety** — the version is escaped with `serde_json` (same technique as
  `deeplink.rs::deep_link_script`).
- **Idempotency** — inject de-dupes by element id (replace, never stack).
- **Splash / connect pages** — the reserved-path intercept is origin-agnostic, so the
  banner and its callback work even if injected on the `tauri://` splash / connect page
  (rare — staging happens ≥90 s in, after the Panel loads).

---

## 9. Non-goals (非目标)

- No change to the update *check* cadence, signing, or `latest.json` manifest flow.
- No native modal dialog; no `tauri-plugin-dialog` re-introduction.
- No Panel/Leptos/WASM changes; no `aleph-server` re-embed.
- Manual "Check for Updates" placement is unchanged (stays in tray/menu).
