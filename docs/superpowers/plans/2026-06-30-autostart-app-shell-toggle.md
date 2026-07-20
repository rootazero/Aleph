# App Shell Autostart Toggle — Implementation Plan (Track A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the full desktop App a user-facing "launch at login" toggle in Settings → General, give the Panel-only (lite) shell the same control via a tray menu item, and stop force-enabling autostart on first run (default OFF).

**Architecture:** The Tauri shell already bundles `tauri-plugin-autostart`. We expose two shell commands (`get_autostart`/`set_autostart`) over the existing loopback IPC surface; the full App's Panel (served from `127.0.0.1:18790`, the only origin the IPC capability allows) calls them from a `is_native_shell()`-gated General-settings section. The lite shell — whose Panel is a *remote* origin with no IPC — gets a native tray `CheckMenuItem` instead. We delete the first-run `ensure_autostart()` so neither product silently adds itself to login items.

**Tech Stack:** Rust, Tauri v2, `tauri-plugin-autostart`, Leptos 0.8 (CSR/WASM), `wasm-bindgen` / `wasm-bindgen-futures` / `js-sys` / `web-sys`.

## Global Constraints

- **R1 (brain–limb):** platform autostart calls live ONLY in the Tauri shell (`desktop/shell/`), never in `src/`. ✔ by construction.
- **R2 (single UI source):** the toggle UI is Leptos Panel (full App); the lite tray item is a native system control, not business UI. ✔
- **IPC boundary fact:** the IPC capability scopes commands to `windows:["main"]` + `remote.urls:["http://127.0.0.1:18790/*"]` (`desktop/shell/capabilities/default.json`). A remote-origin Panel (lite shell, or full App pointed at a remote server) **cannot** invoke shell commands. The settings section must hide itself when the `get_autostart` probe fails.
- **Default OFF:** remove first-run auto-enable. Do NOT forcibly `disable()` existing users — `is_enabled()` reports real OS state.
- **Commit format:** `<scope>: <description>` (e.g. `desktop: add autostart toggle commands`).
- **Cargo discipline:** compile-check sparingly. Shell: `cargo check -p aleph-desktop-shell` (full) and `… --no-default-features` (lite). Panel: `just wasm`. Confirm the shell crate name from `desktop/shell/Cargo.toml [package] name` before running (`aleph-desktop-shell` expected).
- **Testability note:** Tauri-`AppHandle`-bound code and WASM↔JS interop are not unit-testable without a runtime/browser; those tasks end with a **compile gate + Operator verification**, per "if you can't write a test, say why".

---

### Task A1: Shell autostart commands + remove first-run auto-enable

**Files:**
- Create: `desktop/shell/src/autostart.rs`
- Modify: `desktop/shell/src/main.rs` (add `mod autostart;`; register commands in BOTH `invoke_handler` arms at `:158` and `:165`; delete `ensure_autostart` call at `:211` and the fn at `:911-930`)

**Interfaces:**
- Produces: `#[tauri::command] pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String>` and `#[tauri::command] pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String>` — consumed by the Panel (Task A3) over IPC and conceptually mirrored by the tray (Task A2).

- [ ] **Step 1: Create the autostart command module**

Create `desktop/shell/src/autostart.rs`:

```rust
//! Launch-at-login control, exposed to the Panel over the shell's loopback IPC
//! surface. Thin wrapper over `tauri-plugin-autostart` — it holds no state and
//! makes no policy decision (the user drives it from Settings → General). The
//! plugin maps to a macOS LaunchAgent, the Windows registry Run key, and an XDG
//! autostart `.desktop` entry respectively.
//!
//! Only reachable from an origin the IPC capability allows (loopback Panel /
//! bundled pages). A remote-origin Panel cannot call these; the Panel hides the
//! section when the `get_autostart` probe fails (see settings/desktop_autostart).

use tauri_plugin_autostart::ManagerExt;

/// Whether launch-at-login is currently enabled at the OS level.
#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login. Idempotent: enabling when already enabled
/// (or disabling when already disabled) is a no-op the plugin tolerates.
#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}
```

- [ ] **Step 2: Declare the module and register the commands**

In `desktop/shell/src/main.rs`, add `mod autostart;` next to the other `mod` declarations near the top of the file.

Then add both commands to EACH `invoke_handler` arm. Full app arm (currently `:158-163`):

```rust
    #[cfg(feature = "embedded-core")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        connection::get_connection_target,
        connection::set_connection_target,
        connection::clear_connection_target,
        connection::is_lite_shell,
        autostart::get_autostart,
        autostart::set_autostart,
    ]);
```

Lite arm (currently `:164-172`):

```rust
    #[cfg(not(feature = "embedded-core"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        connection::get_connection_target,
        connection::set_connection_target,
        connection::clear_connection_target,
        connection::is_lite_shell,
        autostart::get_autostart,
        autostart::set_autostart,
        connect_setup::discover_servers,
        connect_setup::connect_to,
    ]);
```

- [ ] **Step 3: Remove first-run auto-enable (default OFF)**

In `desktop/shell/src/main.rs`, delete the call at `:209-211`:

```rust
            // A resident assistant should come back after a reboot; enable
            // autostart once, then never fight the user's later choice.
            ensure_autostart(&handle);
```

And delete the entire `ensure_autostart` function (`:911-930`, the `fn ensure_autostart(app: &tauri::AppHandle) { … }` block and its doc comment). Leave `connection::marker_path` untouched — it is still used by `target`/`gateway-token` markers.

- [ ] **Step 4: Compile gate — both variants**

Run: `cargo check -p aleph-desktop-shell`
Expected: compiles; no reference to `ensure_autostart` remains (a leftover call would error `cannot find function`).

Run: `cargo check -p aleph-desktop-shell --no-default-features`
Expected: lite variant compiles; both commands registered.

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/autostart.rs desktop/shell/src/main.rs
git commit -m "desktop: add get/set_autostart commands, drop first-run auto-enable"
```

- [ ] **Step 6: Operator verification (cannot be unit-tested — AppHandle-bound)**

Note in the PR/handoff that runtime verification happens after the full App is rebuilt (Track A is verified together at the end): toggling must call `autolaunch().enable()/disable()` and survive a reboot. No automated test here by design.

---

### Task A2: Lite-shell tray "Launch at Login" toggle

**Files:**
- Modify: `desktop/shell/src/tray.rs` (add a lite-only `CheckMenuItem`, wire its event)

**Interfaces:**
- Consumes: `tauri_plugin_autostart::ManagerExt` (`app.autolaunch()`), same API as Task A1. Independent of A1's commands (the tray calls the plugin directly, no IPC).

- [ ] **Step 1: Build a checked menu item reflecting current state (lite only)**

In `desktop/shell/src/tray.rs`, extend the `use` for `menu` and add the item before `Menu::with_items`. Change the import line `use tauri::{menu::{Menu, MenuItem, PredefinedMenuItem}, …}` to also import `CheckMenuItem`:

```rust
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
```

After the `quit_stop` item is built (`:41`) and before `let menu = Menu::with_items(`, add:

```rust
    // Panel-only (lite) shell: the remote-origin Panel cannot reach the
    // autostart IPC commands, so launch-at-login lives here as a native,
    // shell-local toggle. The full app uses Settings → General instead.
    #[cfg(not(feature = "embedded-core"))]
    let autostart_item = {
        use tauri_plugin_autostart::ManagerExt;
        let enabled = app.autolaunch().is_enabled().unwrap_or(false);
        CheckMenuItem::with_id(
            app,
            "autostart",
            "Launch at Login",
            true,
            enabled,
            None::<&str>,
        )?
    };
    #[cfg(not(feature = "embedded-core"))]
    let autostart_for_event = autostart_item.clone();
```

- [ ] **Step 2: Add the item to the menu (lite only)**

Replace the `let menu = Menu::with_items(app, &[ … ])?;` block (`:42-53`) so the autostart item is appended on lite builds. Because `Menu::with_items` takes a fixed slice, build the slice conditionally:

```rust
    #[cfg(feature = "embedded-core")]
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &update_item,
            &connect_remote,
            &connect_local,
            &separator,
            &quit,
            &quit_stop,
        ],
    )?;
    #[cfg(not(feature = "embedded-core"))]
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &update_item,
            &connect_remote,
            &connect_local,
            &separator,
            &autostart_item,
            &quit,
            &quit_stop,
        ],
    )?;
```

- [ ] **Step 3: Make the event closure `move` and handle the toggle (lite only)**

Change `.on_menu_event(|app, event| match event.id.as_ref() {` to `.on_menu_event(move |app, event| match event.id.as_ref() {` and add a new arm before the `_ => {}` arm (`:91`):

```rust
            // Toggle launch-at-login and reflect the new state in the checkmark.
            #[cfg(not(feature = "embedded-core"))]
            "autostart" => {
                use tauri_plugin_autostart::ManagerExt;
                let mgr = app.autolaunch();
                let now = mgr.is_enabled().unwrap_or(false);
                let res = if now { mgr.disable() } else { mgr.enable() };
                match res {
                    Ok(()) => {
                        let _ = autostart_for_event.set_checked(!now);
                    }
                    Err(e) => tracing::warn!("toggle autostart failed: {e}"),
                }
            }
```

(The `move` closure captures `autostart_for_event` only on lite builds; on the full build the cfg-gated capture and arm are both compiled out, so `move` captures nothing.)

- [ ] **Step 4: Compile gate — both variants**

Run: `cargo check -p aleph-desktop-shell --no-default-features`
Expected: lite compiles; tray shows the new item.

Run: `cargo check -p aleph-desktop-shell`
Expected: full app compiles unchanged (no autostart tray item).

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/tray.rs
git commit -m "desktop: lite shell tray gets Launch at Login toggle"
```

- [ ] **Step 6: Operator verification note**

Runtime check (after lite shell rebuild): the tray shows "Launch at Login" with a checkmark matching OS state; clicking flips both the OS login item and the checkmark. AppHandle-bound — no unit test.

---

### Task A3: Panel General-settings autostart section (full App)

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/settings/desktop_autostart.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/mod.rs` (declare the module)
- Modify: `interfaces/webchat/src/platform/wide/views/settings/general.rs` (render the section after `<ConfigReloadSection />`)

**Interfaces:**
- Consumes: shell commands `get_autostart` / `set_autostart` (Task A1) via `window.__TAURI__.core.invoke`; `is_native_shell()` from `crate::platform::wide::views::voice::audio`.
- Produces: `#[component] pub fn DesktopAutostartSection() -> impl IntoView`.

- [ ] **Step 1: Create the section + its Tauri-IPC helpers**

Create `interfaces/webchat/src/platform/wide/views/settings/desktop_autostart.rs`:

```rust
//! "This device" autostart toggle — desktop-shell only.
//!
//! Launch-at-login is a property of the *local* Tauri shell, not the (possibly
//! remote) server this Panel talks to, so it bypasses the gateway JSON-RPC and
//! calls the shell's `get_autostart`/`set_autostart` commands directly over
//! Tauri IPC (`window.__TAURI__.core.invoke`, available because the shell sets
//! `withGlobalTauri: true`). The section renders only when (a) we are inside the
//! native shell AND (b) the IPC probe succeeds — which is false for a
//! remote-origin Panel (the lite shell, or a full App pointed at a remote
//! server), where the IPC capability blocks the call. Those users toggle
//! autostart from the lite shell's tray instead.

use crate::platform::wide::views::voice::audio::is_native_shell;
use js_sys::{Function, Object, Promise, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Resolve `window.__TAURI__.core.invoke` and call it with `(cmd, args)`.
/// Returns the resolved JS value, or a string error if Tauri IPC is unavailable
/// (remote origin / plain browser) or the command rejected.
async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window")?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| "no __TAURI__".to_string())?;
    if tauri.is_undefined() || tauri.is_null() {
        return Err("Tauri IPC unavailable".to_string());
    }
    let core =
        Reflect::get(&tauri, &JsValue::from_str("core")).map_err(|_| "no core".to_string())?;
    let invoke_fn =
        Reflect::get(&core, &JsValue::from_str("invoke")).map_err(|_| "no invoke".to_string())?;
    let invoke_fn: Function = invoke_fn
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;
    let ret = invoke_fn
        .call2(&core, &JsValue::from_str(cmd), &args)
        .map_err(|e| format!("invoke threw: {e:?}"))?;
    let promise: Promise = ret
        .dyn_into()
        .map_err(|_| "invoke did not return a Promise".to_string())?;
    JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))
}

async fn get_autostart() -> Result<bool, String> {
    invoke("get_autostart", JsValue::NULL)
        .await?
        .as_bool()
        .ok_or_else(|| "get_autostart did not return a bool".to_string())
}

async fn set_autostart(enabled: bool) -> Result<(), String> {
    let args = Object::new();
    Reflect::set(
        &args,
        &JsValue::from_str("enabled"),
        &JsValue::from_bool(enabled),
    )
    .map_err(|_| "could not build args".to_string())?;
    invoke("set_autostart", args.into()).await.map(|_| ())
}

/// Launch-at-login toggle for the local desktop shell. Self-hiding: renders
/// nothing unless we are in the native shell and the IPC probe succeeds.
#[component]
#[must_use]
pub fn DesktopAutostartSection() -> impl IntoView {
    // `available`: probe resolved AND we are in the shell. `enabled`: current state.
    let (available, set_available) = signal(false);
    let (enabled, set_enabled) = signal(false);

    if is_native_shell() {
        spawn_local(async move {
            if let Ok(state) = get_autostart().await {
                set_enabled.set(state);
                set_available.set(true);
            }
            // On error (remote origin / no IPC): stay unavailable → section hidden.
        });
    }

    let on_toggle = move |ev| {
        let want = event_target_checked(&ev);
        set_enabled.set(want); // optimistic
        spawn_local(async move {
            if set_autostart(want).await.is_err() {
                // Revert on failure so the checkbox never lies about OS state.
                if let Ok(actual) = get_autostart().await {
                    set_enabled.set(actual);
                }
            }
        });
    };

    move || {
        available.get().then(|| {
            view! {
                <div class="border border-border rounded-lg p-4">
                    <div class="flex items-start justify-between gap-4">
                        <div>
                            <h3 class="font-medium text-text-primary">"This device"</h3>
                            <p class="text-sm text-text-secondary mt-1">
                                "Launch Aleph automatically when you log in to this computer."
                            </p>
                        </div>
                        <input
                            type="checkbox"
                            class="mt-1"
                            prop:checked=move || enabled.get()
                            on:change=on_toggle
                        />
                    </div>
                </div>
            }
        })
    }
}
```

- [ ] **Step 2: Declare the module**

In `interfaces/webchat/src/platform/wide/views/settings/mod.rs`, add alongside the other section/view declarations:

```rust
pub mod desktop_autostart;
```

- [ ] **Step 3: Render the section in General settings**

In `interfaces/webchat/src/platform/wide/views/settings/general.rs`, add the import near the top (after line 5):

```rust
use super::desktop_autostart::DesktopAutostartSection;
```

Then render it immediately after `<ConfigReloadSection />` (`:126`):

```rust
                            <ConfigReloadSection />

                            <DesktopAutostartSection />
```

- [ ] **Step 4: Build the Panel (WASM)**

Run: `just wasm`
Expected: WASM build succeeds. (No unit test: the section is JS-interop + Tauri-runtime bound; `invoke` cannot run under `wasm-bindgen-test` without a Tauri host.)

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/desktop_autostart.rs \
        interfaces/webchat/src/platform/wide/views/settings/mod.rs \
        interfaces/webchat/src/platform/wide/views/settings/general.rs
git commit -m "panel: add This-device launch-at-login toggle to General settings"
```

- [ ] **Step 6: Operator verification (end-to-end, after full App rebuild)**

Rebuild the full App so the new WASM is embedded (`just wasm` → rebuild `aleph-server` → rebuild shell, per DESKTOP_SHELL.md). Then:
1. Full App, local server: Settings → General shows "This device" with a checkbox matching OS login-items state; toggling it adds/removes the login item; survives reboot.
2. Full App pointed at a **remote** server (Settings → connect remote): the "This device" section is **hidden** (IPC probe fails). ✔ desired.
3. Plain browser against the server: section hidden (`is_native_shell()` false). ✔
4. Lite shell: section never appears (remote origin); autostart lives in the tray (Task A2). ✔

---

## Self-Review

**Spec coverage:** §3.1 shell commands → A1; §3.1 remove first-run enable → A1 Step 3; §3.2 Panel "本机" section gated by `is_native_shell` via Tauri IPC → A3; revised decision (lite → tray) → A2. ✔
**Placeholder scan:** none — every step has complete code or an exact command.
**Type consistency:** `get_autostart`/`set_autostart` signatures identical across shell (A1), tray usage (A2 calls the plugin directly, not the commands — intentional), and Panel wrappers (A3). `DesktopAutostartSection` defined in A3 Step 1, declared A3 Step 2, used A3 Step 3. ✔
**Known limitation surfaced:** A3's section is self-hiding on IPC failure — this is the documented handling for remote-origin full App, not a gap.
