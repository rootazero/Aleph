# In-window "Restart to update" Banner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface a staged app update through a non-modal, dismissible in-window top banner with a "Restart to update" button, so users no longer have to find the hidden tray/menu item.

**Architecture:** All changes live in the thin Tauri shell (`desktop/shell/`). When `update.rs::stage()` records a found update, the shell injects a banner into the current webview document via `window.eval`. The banner's buttons call back through a **same-origin sentinel navigation** (`/__aleph-shell/update/apply` | `/dismiss`) that the existing `on_navigation` guard in `main.rs` intercepts and cancels — an origin-independent channel that works for the loopback full app and a remote-pointed Panel-lite shell alike (Tauri IPC is loopback-scoped and cannot be used from a remote origin). Manual "Check for Updates" in the tray/macOS menu is unchanged.

**Tech Stack:** Rust, Tauri v2 (`tauri-plugin-updater`, `WebviewWindow::eval`, `WebviewWindowBuilder::on_navigation` / `on_page_load`), `serde_json` for JS-safe string escaping. No new dependencies.

## Global Constraints

- **Location:** every change is under `desktop/shell/` (crate `aleph-desktop-shell`). Zero `src/` core changes, zero Panel/WASM changes, zero `aleph-server` re-embed.
- **Both variants must compile:** full app (default feature `embedded-core`) **and** Panel-lite (`--no-default-features`). Nothing in this plan is feature-gated.
- **No new dependencies.** Do **not** re-introduce `tauri-plugin-dialog`. No native OS modal dialog.
- **R5 (non-intrusive):** the banner is non-modal, never steals focus, never blocks the window, and is dismissible.
- **R2/R4 (shell chrome only):** the banner is update chrome owned by the shell (like `splash`, `__alephError`, `SHELL_MARKER_JS`, the external-link interceptor). No business UI/logic.
- **`tray.rs` and `menu.rs` are NOT modified** — manual "Check for Updates" and the post-stage "Restart to update" fallback item stay exactly as they are.
- **Reserved path prefix:** `/__aleph-shell/` is shell-owned; the Panel/daemon never serve it.
- **Injection safety:** all values interpolated into injected JS are escaped with `serde_json::to_string`.
- **Commit message format:** `shell: <description>` (English, conventional-commits style).
- **Cargo economy:** the `aleph-desktop-shell` crate is small and separate from the memory-heavy `alephcore` — `cargo test -p aleph-desktop-shell` and `cargo check -p aleph-desktop-shell` are cheap. Prefer targeted test filters; run the lite-variant check once at the end.

---

## File Structure

- `desktop/shell/src/update.rs` — **modified** (the bulk). Adds: the `UpdateControl` enum + `control_action` sentinel parser + reserved-path constants (Task 1); the `banner_script` builder + `BANNER_TEMPLATE` (Task 2); the `Updater.dismissed` session latch + `show_update_banner` / `reinject_banner_if_staged` / `handle_control` / `remove_banner` + the `stage()` injection wire (Task 3). New unit tests live in the existing `#[cfg(test)] mod tests`.
- `desktop/shell/src/main.rs` — **modified** (Task 4). `on_navigation` gains a control-link intercept; `on_page_load(Finished)` re-injects a staged banner after a Panel reload.
- `desktop/shell/src/tray.rs`, `desktop/shell/src/menu.rs` — **unchanged**.
- `docs/reference/DESKTOP_SHELL.md` — **modified** (Task 6). The "Auto-update" section documents the new banner.

---

## Task 1: Sentinel control channel (`UpdateControl` + `control_action`)

**Files:**
- Modify: `desktop/shell/src/update.rs` (add imports, constants, enum, function; add tests in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `const APPLY_PATH: &str = "/__aleph-shell/update/apply";`
  - `const DISMISS_PATH: &str = "/__aleph-shell/update/dismiss";`
  - `pub enum UpdateControl { Apply, Dismiss }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn control_action(url: &tauri::Url) -> Option<UpdateControl>` — matches on `url.path()` only (origin-independent); `None` for every non-sentinel URL.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `desktop/shell/src/update.rs` (it already exists with `use super::*;`):

```rust
    #[test]
    fn control_action_recognises_the_apply_sentinel() {
        let url = Url::parse("http://127.0.0.1:18790/__aleph-shell/update/apply").unwrap();
        assert_eq!(control_action(&url), Some(UpdateControl::Apply));
    }

    #[test]
    fn control_action_recognises_the_dismiss_sentinel_on_any_origin() {
        // Remote origin (Panel-lite pointed at a LAN Gateway) must match too —
        // control_action keys off the path, not the host.
        let url = Url::parse("http://box.lan:9000/__aleph-shell/update/dismiss").unwrap();
        assert_eq!(control_action(&url), Some(UpdateControl::Dismiss));
    }

    #[test]
    fn control_action_ignores_ordinary_urls() {
        for u in [
            "http://127.0.0.1:18790/",
            "http://127.0.0.1:18790/chat",
            "https://github.com/rootazero/Aleph/releases/latest",
            "tauri://localhost/index.html",
        ] {
            assert_eq!(control_action(&Url::parse(u).unwrap()), None, "{u}");
        }
    }

    #[test]
    fn control_action_matches_apply_even_with_query() {
        let url = Url::parse("http://127.0.0.1:18790/__aleph-shell/update/apply?v=1").unwrap();
        assert_eq!(control_action(&url), Some(UpdateControl::Apply));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aleph-desktop-shell control_action`
Expected: FAIL — compile error `cannot find function 'control_action'` / `cannot find type 'UpdateControl'` / `cannot find type 'Url'` in this scope.

- [ ] **Step 3: Add the `Url` import**

In `desktop/shell/src/update.rs`, change the existing Tauri import line:

```rust
use tauri::{AppHandle, Manager, Wry};
```

to:

```rust
use tauri::{AppHandle, Manager, Url, Wry};
```

- [ ] **Step 4: Add the constants, enum, and function**

Add near the top of `desktop/shell/src/update.rs`, just after the existing `RELEASES_URL` constant:

```rust
/// Reserved shell-control paths the in-window update banner navigates to. The
/// `on_navigation` guard (`main.rs`) intercepts and cancels these, so they
/// never actually load — the Panel/daemon never serve the `/__aleph-shell/`
/// prefix. Matching on the path (not the host) keeps the callback working
/// whether the Panel is served from loopback (full app) or a remote Gateway
/// (Panel-lite): Tauri IPC is loopback-scoped and unavailable from a remote
/// origin, so this navigation channel is the only origin-independent one.
const APPLY_PATH: &str = "/__aleph-shell/update/apply";
const DISMISS_PATH: &str = "/__aleph-shell/update/dismiss";

/// A banner control signal routed from the webview back to the shell via a
/// sentinel navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateControl {
    /// Apply the staged update and restart.
    Apply,
    /// Hide the banner for this session.
    Dismiss,
}

/// Recognise a banner control link by its path. Returns `None` for ordinary
/// Panel routes and external links, which must pass through to
/// `external_link::route`.
pub fn control_action(url: &Url) -> Option<UpdateControl> {
    match url.path() {
        APPLY_PATH => Some(UpdateControl::Apply),
        DISMISS_PATH => Some(UpdateControl::Dismiss),
        _ => None,
    }
}
```

Note: a transient `dead_code` warning for `DISMISS_PATH` / `UpdateControl` / `control_action` is expected until Task 3/4 wire them. The crate still compiles and the tests pass.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p aleph-desktop-shell control_action`
Expected: PASS (4 new tests green; existing `restart_label` / `manual_update_label` tests unaffected).

- [ ] **Step 6: Commit**

```bash
git add desktop/shell/src/update.rs
git commit -m "shell: add update-banner sentinel control channel (control_action)"
```

---

## Task 2: Banner script builder (`banner_script`)

**Files:**
- Modify: `desktop/shell/src/update.rs` (add `BANNER_TEMPLATE` const + `banner_script` fn; add tests)

**Interfaces:**
- Consumes: `APPLY_PATH`, `DISMISS_PATH`, `RELEASES_URL` (existing).
- Produces: `fn banner_script(version: &str, self_install: bool) -> String` — returns the self-contained JS that injects (idempotently) the `#__aleph-update-banner` element. `self_install == true` → primary button "Restart to update" → `APPLY_PATH`; `false` → "How to update" → `RELEASES_URL` (opened externally by `external_link::route`).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `desktop/shell/src/update.rs`:

```rust
    #[test]
    fn banner_script_self_install_offers_restart_and_sentinels() {
        let js = banner_script("26.7.14", true);
        assert!(js.contains("Aleph v26.7.14 is ready"));
        assert!(js.contains("Restart to update"));
        assert!(js.contains("/__aleph-shell/update/apply"));
        assert!(js.contains("/__aleph-shell/update/dismiss"));
        // Idempotent injection: removes any prior banner by id first.
        assert!(js.contains("__aleph-update-banner"));
    }

    #[test]
    fn banner_script_package_manager_offers_howto_not_restart() {
        let js = banner_script("26.7.14", false);
        assert!(js.contains("How to update"));
        assert!(js.contains(RELEASES_URL));
        // The restart apply-sentinel must NOT be the primary action here.
        assert!(!js.contains("/__aleph-shell/update/apply"));
        // Dismiss still works.
        assert!(js.contains("/__aleph-shell/update/dismiss"));
    }

    #[test]
    fn banner_script_escapes_a_hostile_version() {
        let js = banner_script("1\"; alert(1);//", true);
        // The embedded quote is escaped by serde_json, so it cannot break out
        // of the JS string literal.
        assert!(!js.contains("1\"; alert"));
        assert!(js.contains("1\\\"; alert"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aleph-desktop-shell banner_script`
Expected: FAIL — compile error `cannot find function 'banner_script'`.

- [ ] **Step 3: Add the template constant and the builder**

Add to `desktop/shell/src/update.rs` (after `control_action` from Task 1):

```rust
/// The injected banner as a JS template. Placeholders (`__MSG__`, `__LABEL__`,
/// `__HREF__`, `__DISMISS__`, `__ISRESTART__`) are replaced with JSON-encoded
/// (JS-safe) literals by `banner_script`. Built with `createElement` +
/// `addEventListener` (never inline `onclick`) so a strict Panel CSP cannot
/// block it; the buttons navigate via `location.href`, which is unaffected by
/// script-CSP. On macOS (`data-platform="macos"`) the bar is offset below the
/// overlay-titlebar traffic lights.
const BANNER_TEMPLATE: &str = r#"(function(){
var ID='__aleph-update-banner';
var old=document.getElementById(ID); if(old) old.remove();
var mac=document.documentElement.getAttribute('data-platform')==='macos';
var dark=window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches;
var bar=document.createElement('div'); bar.id=ID; bar.setAttribute('role','status');
bar.style.cssText='position:fixed;left:0;right:0;top:'+(mac?'28px':'0px')+';z-index:2147483000;display:flex;align-items:center;gap:12px;padding:8px 14px;font:13px -apple-system,system-ui,sans-serif;box-shadow:0 1px 4px rgba(0,0,0,.25);'+(dark?'background:#1f2430;color:#e6e9ef;':'background:#f4f6fb;color:#1b2130;');
var msg=document.createElement('span'); msg.style.cssText='flex:1;'; msg.textContent=__MSG__;
var act=document.createElement('button'); act.textContent=__LABEL__;
act.style.cssText='cursor:pointer;border:0;border-radius:6px;padding:5px 12px;font:inherit;font-weight:600;background:#3b82f6;color:#fff;';
act.addEventListener('click',function(){ if(__ISRESTART__){ act.disabled=true; act.textContent='Updating…'; msg.textContent='Updating — Aleph will restart shortly.'; } window.location.href=__HREF__; });
var close=document.createElement('button'); close.setAttribute('aria-label','Dismiss'); close.textContent='×';
close.style.cssText='cursor:pointer;border:0;background:transparent;color:inherit;font-size:18px;line-height:1;padding:0 6px;';
close.addEventListener('click',function(){ window.location.href=__DISMISS__; });
bar.appendChild(msg); bar.appendChild(act); bar.appendChild(close);
(document.body||document.documentElement).appendChild(bar);
})();"#;

/// Build the banner-injection JS for a staged `version`. `self_install`
/// distinguishes platforms that can self-update (macOS / Windows / Linux
/// AppImage — restart-to-apply) from package-manager installs (Linux
/// .deb/.rpm — point the user at the releases page instead).
fn banner_script(version: &str, self_install: bool) -> String {
    let msg = serde_json::to_string(&format!("Aleph v{version} is ready"))
        .unwrap_or_else(|_| "\"Aleph update is ready\"".to_string());
    let (label, href, is_restart) = if self_install {
        ("Restart to update", APPLY_PATH, "true")
    } else {
        ("How to update", RELEASES_URL, "false")
    };
    let json = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    BANNER_TEMPLATE
        .replace("__MSG__", &msg)
        .replace("__LABEL__", &json(label))
        .replace("__HREF__", &json(href))
        .replace("__DISMISS__", &json(DISMISS_PATH))
        .replace("__ISRESTART__", is_restart)
}
```

Note: a transient `dead_code` warning for `banner_script` is expected until Task 3 calls it; the crate still compiles and tests pass.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aleph-desktop-shell banner_script`
Expected: PASS (3 new tests green).

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/update.rs
git commit -m "shell: add update-banner injection script builder"
```

---

## Task 3: Session latch + injection helpers + wire `stage()`

**Files:**
- Modify: `desktop/shell/src/update.rs` (add `Updater.dismissed`; add `show_update_banner` / `reinject_banner_if_staged` / `handle_control` / `remove_banner`; call `show_update_banner` from `stage()`)

**Interfaces:**
- Consumes: `banner_script` (Task 2), `UpdateControl` (Task 1), `updater_can_self_install`, `apply_staged_update`, `Updater.staged` (existing).
- Produces:
  - `pub fn show_update_banner(app: &AppHandle)` — evals `banner_script` on the `"main"` window; no-op when nothing is staged.
  - `pub fn reinject_banner_if_staged(app: &AppHandle)` — re-injects after a reload unless the session was dismissed.
  - `pub fn handle_control(app: &AppHandle, action: UpdateControl)` — `Apply` → `apply_staged_update`; `Dismiss` → set the latch + remove the banner.

- [ ] **Step 1: Add the `dismissed` field to `Updater`**

In `desktop/shell/src/update.rs`, change the `Updater` struct:

```rust
#[derive(Default)]
pub struct Updater {
    /// The version of a found-but-not-yet-applied update, if any.
    staged: Mutex<Option<String>>,
    /// The update menu items (tray, and the macOS app menu) registered by
    /// their builders so the checker can relabel them once an update is
    /// staged. Both surfaces stay in sync.
    update_items: Mutex<Vec<MenuItem<Wry>>>,
    /// Session latch: set when the user dismisses the in-window banner (`×`).
    /// In-memory only, so a fresh launch re-shows the banner (spec §5).
    dismissed: Mutex<bool>,
}
```

- [ ] **Step 2: Add the injection + control helpers**

Add to `desktop/shell/src/update.rs` (after `banner_script`):

```rust
/// Inject (or replace) the update banner in the main window's current
/// document. No-op when nothing is staged or the main window is gone.
pub fn show_update_banner(app: &AppHandle) {
    let version = app
        .state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(version) = version else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.eval(banner_script(&version, updater_can_self_install())) {
        tracing::debug!("could not inject the update banner: {e}");
    }
}

/// Re-inject the banner after a Panel reload wiped the injected DOM — but only
/// if an update is staged and the user has not dismissed it this session.
/// Wired into `main.rs`'s `on_page_load(Finished)` handler.
pub fn reinject_banner_if_staged(app: &AppHandle) {
    let dismissed = *app
        .state::<Updater>()
        .dismissed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if dismissed {
        return;
    }
    show_update_banner(app);
}

/// Perform a banner control action routed from the `on_navigation` guard.
pub fn handle_control(app: &AppHandle, action: UpdateControl) {
    match action {
        UpdateControl::Apply => apply_staged_update(app),
        UpdateControl::Dismiss => {
            *app.state::<Updater>()
                .dismissed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            remove_banner(app);
        }
    }
}

/// Remove the injected banner element from the current document.
fn remove_banner(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval(
            "var b=document.getElementById('__aleph-update-banner');if(b)b.remove();",
        );
    }
}
```

- [ ] **Step 3: Wire `stage()` to inject the banner**

In `desktop/shell/src/update.rs`, change `stage()` to inject after relabelling:

```rust
/// Record a staged update and relabel every registered update item, then
/// surface the in-window banner.
fn stage(app: &AppHandle, version: &str) {
    *app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version.to_string());
    relabel_update_items(app, staged_label(version));
    show_update_banner(app);
}
```

- [ ] **Step 4: Verify it compiles and existing tests still pass**

Run: `cargo test -p aleph-desktop-shell`
Expected: PASS — the crate compiles (the transient `dead_code` warnings from Tasks 1–2 are now gone except `handle_control` / `control_action`, which Task 4 wires) and all unit tests are green.

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/update.rs
git commit -m "shell: inject the update banner on stage and route its controls"
```

---

## Task 4: Wire the guard intercept and reload re-injection in `main.rs`

**Files:**
- Modify: `desktop/shell/src/main.rs` (the `on_navigation` and `on_page_load` closures in `build_main_window`)

**Interfaces:**
- Consumes: `update::control_action`, `update::handle_control`, `update::reinject_banner_if_staged` (Tasks 1 & 3); `external_link::route` (existing).

- [ ] **Step 1: Intercept control links in the `on_navigation` guard**

In `desktop/shell/src/main.rs`, inside `build_main_window`, find:

```rust
        .initialization_script(external_link::CLICK_INTERCEPTOR_JS)
        .on_navigation(external_link::route)
```

Replace the `.on_navigation(external_link::route)` line with:

```rust
        // Intercept the banner's sentinel control links (apply / dismiss)
        // before the external-link guard: perform the shell action and cancel
        // the navigation so the reserved path never loads. Everything else
        // falls through to the normal internal/external routing.
        .on_navigation({
            let handle = app.clone();
            move |url| {
                if let Some(action) = update::control_action(url) {
                    update::handle_control(&handle, action);
                    return false;
                }
                external_link::route(url)
            }
        })
```

- [ ] **Step 2: Re-inject a staged banner after a Panel reload**

In the same `WebviewWindowBuilder` chain, find the `on_page_load` closure:

```rust
        .on_page_load(|window, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let _ = window.eval(SHELL_MARKER_JS);
            }
        });
```

Replace it with:

```rust
        .on_page_load(|window, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let _ = window.eval(SHELL_MARKER_JS);
                // A daemon-recovery reload re-navigates the Panel and wipes the
                // injected banner; put it back if an update is still staged and
                // the user has not dismissed it this session.
                update::reinject_banner_if_staged(window.app_handle());
            }
        });
```

- [ ] **Step 3: Verify the full app compiles**

Run: `cargo check -p aleph-desktop-shell`
Expected: PASS — no errors, and the `dead_code` warnings for `control_action` / `handle_control` / `reinject_banner_if_staged` are now gone (all are wired).

- [ ] **Step 4: Verify the Panel-lite variant compiles**

Run: `cargo check -p aleph-desktop-shell --no-default-features`
Expected: PASS — the lite shell compiles the same banner path (nothing is feature-gated).

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/main.rs
git commit -m "shell: intercept banner control links and re-inject on Panel reload"
```

---

## Task 5: Final verification (tests, both variants, lint)

**Files:**
- None (verification only; commit only if `fmt`/`clippy` produce changes)

- [ ] **Step 1: Run the full shell test suite**

Run: `cargo test -p aleph-desktop-shell`
Expected: PASS — all unit tests green (Task 1: 4, Task 2: 3, plus the pre-existing `update.rs` and `main.rs`/`external_link.rs`/`connection.rs`/`menu.rs` tests).

- [ ] **Step 2: Confirm both variants build**

Run: `cargo check -p aleph-desktop-shell && cargo check -p aleph-desktop-shell --no-default-features`
Expected: PASS for both.

- [ ] **Step 3: Format and lint the touched files**

Run: `rustfmt --edition 2021 --check desktop/shell/src/update.rs desktop/shell/src/main.rs`
Expected: no diff. If it reports formatting changes, apply them: `rustfmt --edition 2021 desktop/shell/src/update.rs desktop/shell/src/main.rs`

Run: `cargo clippy -p aleph-desktop-shell`
Expected: no new warnings introduced by this change.

- [ ] **Step 4: Commit any formatting fixes (only if Step 3 changed files)**

```bash
git add desktop/shell/src/update.rs desktop/shell/src/main.rs
git commit -m "shell: rustfmt update-banner changes"
```

---

## Task 6: Document the banner in DESKTOP_SHELL.md

**Files:**
- Modify: `docs/reference/DESKTOP_SHELL.md` (the "Auto-update" section)

- [ ] **Step 1: Update the Auto-update description**

In `docs/reference/DESKTOP_SHELL.md`, find the "### Auto-update" section. Replace this sentence:

```
It never restarts under the
user (R5): a found update is *staged*, surfaced through a desktop
notification and the tray's update item (relabelled "Restart to update to
vX.Y.Z"), and applied only when the user picks it.
```

with:

```
It never restarts under the
user (R5): a found update is *staged* and surfaced three non-intrusive ways —
a desktop notification, the tray/macOS-menu update item (relabelled "Restart
to update to vX.Y.Z"), and a **non-modal in-window top banner** with a
"Restart to update" button. The banner is injected into the webview by the
shell (`update.rs::show_update_banner`); its button calls back through a
same-origin sentinel navigation (`/__aleph-shell/update/apply` | `/dismiss`)
that the `on_navigation` guard intercepts — an origin-independent channel that
works for the loopback full app and a remote-pointed Panel-lite alike. The
banner is dismissible per session (`×`) and re-appears on the next launch; the
tray/menu item is the always-available fallback. Applying (any surface)
downloads, installs, and restarts the app — and with it the bundled
`aleph-server`.
```

- [ ] **Step 2: Commit**

```bash
git add docs/reference/DESKTOP_SHELL.md
git commit -m "docs: document the in-window restart-to-update banner"
```

---

## Manual verification (post-implementation, not automated)

The banner cannot be exercised by unit tests (it needs a real staged update + a live webview). After the tasks land, verify manually on at least Windows and macOS:

1. Build and run the shell (`just shell-dev`, or a full `just shell-build` + install).
2. Force a staged update: temporarily lower the local `VERSION` (or point the updater at a test `latest.json`) so the checker finds a "newer" release; wait for the ~90 s first check (or trigger a manual check from the tray/menu).
3. Confirm the top banner appears, non-modal, without stealing focus.
4. Click **Restart to update** → banner shows "Updating…", the app downloads, stops the daemon, installs, and restarts into the new version.
5. Re-stage, click `×` → banner disappears; the tray/menu "Restart to update" item still works; relaunch → banner re-appears.
6. On macOS, confirm the banner clears the traffic-light inset (28 px offset).

---

## Self-Review

**1. Spec coverage** (against `docs/superpowers/specs/2026-07-14-restart-to-update-banner-design.md`):
- §3 in-window top banner, shell-injected, dismiss-per-session/re-show-next-launch → Tasks 2 (script), 3 (`show_update_banner` + `dismissed` latch + `stage()` wire), 4 (`reinject_banner_if_staged`). ✓
- §4 origin-independent sentinel callback intercepted by `on_navigation` → Task 1 (`control_action`), Task 4 (guard intercept). ✓
- §5 lifecycle: appear on stage (Task 3), restart click / apply (Task 3 `handle_control` + existing `apply_staged_update`), dismiss latch (Task 3), reload re-inject (Task 4), re-show next launch (in-memory state — inherent), package-manager "How to update" variant (Task 2 `self_install=false`). ✓
- §6 files: `update.rs` (Tasks 1–3), `main.rs` (Task 4), `tray.rs`/`menu.rs` unchanged (Global Constraints). ✓
- §7 testing: pure unit tests for `banner_script` + `control_action` (Tasks 1–2), both-variant compile (Tasks 4–5), manual E2E (Manual verification section). ✓
- §8 edge cases: macOS 28 px offset, CSP-safe `addEventListener`, serde_json escaping, idempotent-by-id → all in `BANNER_TEMPLATE` / `banner_script` (Task 2) with a dedicated escaping test. ✓
- §9 non-goals honoured: no dialog plugin, no Panel/WASM change, no check-cadence change, manual check unchanged (Global Constraints). ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows the assertions and the exact command + expected result. ✓

**3. Type consistency:** `control_action(&Url) -> Option<UpdateControl>` (Task 1) is consumed with the same signature in Task 4. `banner_script(&str, bool) -> String` (Task 2) is called as `banner_script(&version, updater_can_self_install())` in Task 3. `handle_control(&AppHandle, UpdateControl)` / `reinject_banner_if_staged(&AppHandle)` (Task 3) match their call sites in Task 4. The `dismissed` field name is identical in Task 3's struct and its helpers. ✓

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-14-restart-to-update-banner.md`.
