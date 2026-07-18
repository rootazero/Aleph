# Connection Form By Build — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make connection form build-determined (full app = local-only, lite shell = remote-only, browser = address bar), strip the in-panel local/remote switcher down to a read-only indicator, and drop the `data-shell-variant` marker.

**Architecture:** Panel decides local-vs-remote purely from `location.host` (loopback → local, else → remote) — no shell-injected marker, no Tauri IPC. The full app is made local-only by reconciling any persisted target to `Local` at boot and removing the remote navigation arm; the lite shell's remote-connect flow (connect.html / connect_setup) is untouched.

**Tech Stack:** Rust (Tauri v2 desktop shell), Leptos/WASM panel, JSON i18n.

## Global Constraints

- MSRV 1.95; repo toolchain pinned 1.96.0 — do not change.
- No new dependencies; serde-only serialization; no platform-API crates in `src` (R1).
- Interface stays pure I/O (R4); thin shell, delete dead code (R10).
- English commit messages, format `<scope>: <description>`.
- **极度节制 cargo**: do NOT run per-step cargo. Run `cargo check` only at the two batch boundaries named below (Task 4 end = panel; Task 5 end = shell). One invocation each.
- Do not touch `cluster.rs`, `connect_setup.rs`, `connect.html`, `menu.rs`, `tray.rs`, or any `.dark`/material CSS mirror.
- `docs/superpowers/**` is gitignored — the plan/spec files are local working docs and will not be committed; only source changes get committed.

## ⚠️ Deviation from approved spec (§3.2 E/F) — needs reviewer awareness

The spec said: full handler *removes* `set/get/clear_connection_target` IPC registration (E), and `load_target()` is cfg-gated to `Local` under `embedded-core` (F).

**Planning found both cause cfg-conditional dead-code traps:**
- The command fns are referenced by `menu.rs`, which is `#[cfg(target_os = "macos")]` only. Removing them from the full handler → on **Windows/Linux full builds** the `#[tauri::command]` fns become unreferenced → `dead_code` → fails `-D warnings`. Honoring removal would cascade cfg changes into `menu.rs` (which the design said not to touch).
- cfg-gating `load_target()`'s body risks orphaning its marker helpers under `embedded-core`.

**This plan instead makes the full app local-only via a single boot chokepoint** (Task 5): at startup the embedded-core path reconciles any persisted target to `Local` and drops the remote navigation arm. `load_target()` then returns `Local` everywhere (menu reload / browser-open included), with **zero** cfg dead-code and no `menu.rs` change. The behavioral guarantee ("the full app can never be remote") is identical — arguably stronger (one chokepoint). The `set/get/clear` IPC commands stay registered but are inert in the full app (no UI path calls them; any write is overwritten to `Local` on next boot).

If the reviewer insists on literal IPC removal, that is a separate follow-up that must also cfg-restructure `menu.rs` and the command defs across three platforms.

---

## File Structure

- `interfaces/webchat/src/views/settings/network/connection.rs` — rewritten to a read-only indicator (panel main change).
- `interfaces/webchat/src/components/connection_status.rs` — dashboard chip; `resolve_target_label` simplified to `location.host`.
- `interfaces/webchat/src/api/tauri_bridge.rs` — deleted (all consumers gone); `mod` line removed from `api.rs`.
- `interfaces/webchat/locales/{zh,en}.json` — prune dead keys; reword `description`.
- `desktop/shell/src/main.rs` — drop `SHELL_VARIANT_JS`; embedded-core boot becomes local-only.

---

## Task 1: Panel — read-only ConnectionSection

**Files:**
- Modify (full rewrite): `interfaces/webchat/src/views/settings/network/connection.rs`

**Interfaces:**
- Consumes: i18n keys `settings.network.{section_title,description,connected_label,badge_remote,badge_local}` (kept; reworded in Task 4).
- Produces: `ConnectionSection` component (same name/signature, now read-only). Private pure fns `host_only(&str)->&str`, `is_loopback_host(&str)->bool` retained for tests.

- [ ] **Step 1: Replace the entire file** with the read-only version below.

```rust
//! Section 1 — 服务连接:只读反映本 Panel 当前连接的 Aleph 核心(本地 / 远程)。
//!
//! 连接形态由「构建」决定,不在面板里切换:完整版 App 恒连内嵌 loopback 核心、
//! 纯壳 Panel 恒连远程、浏览器取决于地址栏。三者一律由 `location.host`(权威、
//! 永远新鲜)判定本地/远程 —— 无壳注入标记,无 IPC 依赖(R4:Interface 纯 I/O)。

use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// The `host:port` of the core this Panel is served by (and talks to) — the
/// authoritative, always-fresh answer to "which core am I connected to".
/// Empty string if unavailable.
fn current_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default()
}

/// Strip the port from a `host[:port]`, handling IPv6 literals (`[::1]:port`).
/// Pure.
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}

/// Whether a `host[:port]` names a loopback core. Pure.
fn is_loopback_host(host: &str) -> bool {
    let h = host_only(host);
    h.eq_ignore_ascii_case("localhost") || h == "::1" || h.starts_with("127.")
}

#[component]
pub fn ConnectionSection() -> impl IntoView {
    let i18n = use_i18n();

    // Computed once: the origin never changes for the life of a page. A full app
    // is always served from its loopback embedded core, a lite shell from its
    // remote core, a browser from whatever the user typed — so the origin alone
    // is an honest local/remote signal. No signals, no IPC, no shell marker.
    let host = current_host();
    let host_present = !host.is_empty();
    let remote = host_present && !is_loopback_host(&host);

    view! {
        <section class="space-y-4">
            <div>
                <h2 class="text-lg font-semibold text-text-primary mb-1">
                    {t!(i18n, settings.network.section_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.network.description)}
                </p>
            </div>

            <Show when=move || host_present>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <div class="flex items-center gap-2 text-sm">
                        <span class="text-text-secondary">
                            {t!(i18n, settings.network.connected_label)}
                        </span>
                        <span class="font-mono text-text-primary">{host.clone()}</span>
                        <span class=move || {
                            if remote {
                                "px-2 py-0.5 rounded-full text-xs bg-warning/15 text-warning"
                            } else {
                                "px-2 py-0.5 rounded-full text-xs bg-success/15 text-success"
                            }
                        }>
                            {if remote {
                                view! { {t!(i18n, settings.network.badge_remote)} }.into_any()
                            } else {
                                view! { {t!(i18n, settings.network.badge_local)} }.into_any()
                            }}
                        </span>
                    </div>
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::{host_only, is_loopback_host};

    #[test]
    fn host_only_strips_port_and_handles_ipv6() {
        assert_eq!(host_only("127.0.0.1:18790"), "127.0.0.1");
        assert_eq!(host_only("box.lan"), "box.lan");
        assert_eq!(host_only("[::1]:18790"), "::1");
        assert_eq!(host_only("[fe80::1]"), "fe80::1");
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1:18790"));
        assert!(is_loopback_host("127.5.6.7"));
        assert!(is_loopback_host("localhost:18790"));
        assert!(is_loopback_host("LocalHost"));
        assert!(is_loopback_host("[::1]:18790"));
        assert!(!is_loopback_host("172.245.43.211:18790"));
        assert!(!is_loopback_host("core.example:18790"));
    }
}
```

- [ ] **Step 2: Commit** (compile is verified in Task 4's batched check).

```bash
git add interfaces/webchat/src/views/settings/network/connection.rs
git commit -m "panel: make Service Connection a read-only indicator (location.host)"
```

---

## Task 2: Panel — connection_status.rs uses location.host only

**Files:**
- Modify: `interfaces/webchat/src/components/connection_status.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `resolve_target_label()` becomes a sync `fn` (no longer `async`, no Tauri IPC).

- [ ] **Step 1: Replace `resolve_target_label` and delete `host_of`.** Replace the existing `async fn resolve_target_label()` (and the whole `host_of` fn) with this single sync fn:

```rust
/// Resolve the human-readable label for the live core. The origin that served
/// this Panel IS the core it talks to — loopback (the full app's embedded core)
/// collapses to `Local`, a remote origin (lite shell / browser) shows its host.
fn resolve_target_label() -> String {
    let host = web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default();
    if host.is_empty() || is_loopback_host(&host) {
        "Local".to_string()
    } else {
        host
    }
}
```

- [ ] **Step 2: Update the caller** in `ConnectionStatus`. Replace this block:

```rust
    let target = RwSignal::new(String::new());
    spawn_local(async move {
        target.set(resolve_target_label().await);
    });
```

with:

```rust
    let target = RwSignal::new(resolve_target_label());
```

- [ ] **Step 3: Remove the now-unused import** `use leptos::task::spawn_local;` (top of file).

- [ ] **Step 4: Delete the two `host_of` tests** in the `#[cfg(test)] mod tests` block (`host_of_strips_scheme_and_path`, `host_of_collapses_loopback_to_local`). Keep `loopback_detection`. After this, `mod tests`'s `use super::*;` still resolves (`is_loopback_host` remains).

- [ ] **Step 5: Commit.**

```bash
git add interfaces/webchat/src/components/connection_status.rs
git commit -m "panel: resolve live-core label from location.host, drop shell IPC"
```

---

## Task 3: Panel — delete orphaned tauri_bridge module

**Files:**
- Delete: `interfaces/webchat/src/api/tauri_bridge.rs`
- Modify: `interfaces/webchat/src/api.rs:40` (remove `pub mod tauri_bridge;`)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. (After Tasks 1+2, all four public fns — `is_shell`, `get_connection_target`, `set_connection_target`, `normalize_endpoint_preview` — have zero consumers.)

- [ ] **Step 1: Verify no remaining references** (must print nothing):

```bash
grep -rn "tauri_bridge" interfaces/webchat/src/ | grep -v "interfaces/webchat/src/api.rs:40:"
```
Expected: empty output (Tasks 1 & 2 removed all call sites).

- [ ] **Step 2: Delete the file.**

```bash
git rm interfaces/webchat/src/api/tauri_bridge.rs
```

- [ ] **Step 3: Remove the module declaration.** In `interfaces/webchat/src/api.rs`, delete the line `pub mod tauri_bridge;` (line 40).

- [ ] **Step 4: Commit.**

```bash
git add interfaces/webchat/src/api.rs
git commit -m "panel: remove orphaned tauri_bridge IPC module"
```

---

## Task 4: i18n — prune dead keys, reword description

**Files:**
- Modify: `interfaces/webchat/locales/zh.json`
- Modify: `interfaces/webchat/locales/en.json`

**Interfaces:**
- Consumes/Produces: i18n key set. After this both files must have identical key sets (project invariant).

- [ ] **Step 1: In `zh.json` `settings.network`**, delete these keys: `local_service`, `remote_service`, `browser_only`, `preview`, `apply`, `local_target`, `confirm_switch`, `confirm_switch_action`, `local_stays_running`, `remote_readonly_lite`, `remote_readonly_full`, `remote_readonly_hint`. Keep `section_title`, `connected_label`, `badge_remote`, `badge_local`. Set `description` to:

```json
      "description": "本 Panel 当前连接的 Aleph 服务。完整版 App 固定连接本地核心,纯壳 Panel 连接远程核心。",
```

- [ ] **Step 2: In `en.json` `settings.network`**, delete the same key set (by name). Set `description` to:

```json
      "description": "The Aleph service this Panel is connected to. The full app is fixed to its local core; the panel-only app connects to a remote core.",
```

- [ ] **Step 3: Verify key parity** between the two files' `settings.network` blocks (same keys, no trailing-comma JSON errors):

```bash
python3 -c "import json; z=json.load(open('interfaces/webchat/locales/zh.json'))['settings']['network']; e=json.load(open('interfaces/webchat/locales/en.json'))['settings']['network']; print('OK' if set(z)==set(e) else ('MISMATCH '+str(set(z)^set(e))))"
```
Expected: `OK`

- [ ] **Step 4: BATCHED PANEL COMPILE CHECK** (covers Tasks 1–4 — the single panel cargo invocation):

```bash
cargo check -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: PASS, no warnings. (If a leptos `i18n` macro errors on a missing key, a kept view references a pruned key — re-check Task 1/4.)

- [ ] **Step 5: Commit.**

```bash
git add interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git commit -m "panel: prune connection-switch i18n keys, reword description"
```

---

## Task 5: Shell — drop variant marker + full app local-only

**Files:**
- Modify: `desktop/shell/src/main.rs` (constants ~85-98; `build_main_window` init script ~291; `bring_target_online` embedded-core ~380-434)

**Interfaces:**
- Consumes: `connection::{load_target, save_target, ConnectionTarget}`, `external_link::set_remote_host`, `daemon::{reconcile_for_version, ensure_ready}`, `reveal_panel`, `show_daemon_error` (all exist).
- Produces: full-app boot path that always serves the loopback core.

- [ ] **Step 1: Delete the `SHELL_VARIANT_JS` constant** — both cfg arms (the `#[cfg(feature = "embedded-core")]` and `#[cfg(not(feature = "embedded-core"))]` definitions and their doc comment, ~lines 85-98). Keep `SHELL_MARKER_JS` (still used for `data-shell`/`data-platform`).

- [ ] **Step 2: Remove the marker injection** in `build_main_window`. Delete this line (~291):

```rust
        .initialization_script(SHELL_VARIANT_JS)
```
Keep the surrounding `.initialization_script(SHELL_MARKER_JS)` and `.initialization_script(external_link::CLICK_INTERCEPTOR_JS)` lines.

- [ ] **Step 3: Rewrite the embedded-core `bring_target_online`** (the `#[cfg(feature = "embedded-core")]` one, ~380-434). Replace its body with the local-only version below (drops the `Remote` arm; reconciles any legacy persisted target to `Local` so menu reload / browser-open also see local):

```rust
#[cfg(feature = "embedded-core")]
async fn bring_target_online(handle: &tauri::AppHandle) -> bool {
    let version = handle.package_info().version.to_string();
    // The full app is local-only: it always serves its embedded loopback core
    // and never honors a remote target. Reconcile any legacy persisted remote
    // (from a previous build that allowed switching) back to Local so every
    // surface — menu reload, open-in-browser — agrees on local.
    if !matches!(connection::load_target(), connection::ConnectionTarget::Local) {
        let _ = connection::save_target(&connection::ConnectionTarget::Local);
    }
    external_link::set_remote_host(None);
    // First launch / post-update: force any stale daemon offline so the
    // `aleph-server` bundled in this app takes over.
    daemon::reconcile_for_version(&version).await;
    match daemon::ensure_ready().await {
        Ok(()) => {
            reveal_panel(handle);
            true
        }
        Err(e) => {
            tracing::error!("daemon did not become ready: {e}");
            show_daemon_error(handle, &e);
            false
        }
    }
}
```

- [ ] **Step 4: BATCHED SHELL COMPILE CHECK** (the single shell cargo invocation; default features = full/embedded-core):

```bash
cargo check -p aleph-desktop-shell
```
Expected: PASS, no warnings. (If `SHELL_VARIANT_JS` is reported unused/undefined, a reference was missed in Step 1/2.)

- [ ] **Step 5: Commit.**

```bash
git add desktop/shell/src/main.rs
git commit -m "shell: drop data-shell-variant marker; full app is local-only"
```

---

## Deploy (after all tasks)

Not part of task commits — run when integrating:
- Panel: `just wasm` → rebuild `aleph-server` (rust_embed embeds the panel at compile time).
- Shell: `just shell-build` (full) **and** `just shell-build-lite` (lite). Both `.app`s must be rebuilt + reinstalled — the original bug was a stale shell binary.
- Lite-variant compile is exercised by `just shell-build-lite`; the lite-only code paths (connect_setup, lite handler) are untouched by Task 5.

## Self-Review (done)

- **Spec coverage:** A(3.1)→T1; B(3.1)→T2; C(3.1)→T3; D(3.1 i18n)→T4; E/F(3.2)→T5 (via the documented deviation: boot reconcile instead of handler removal); marker drop(3.2 E)→T5. ✅
- **Placeholder scan:** none — every code/edit step shows exact content. ✅
- **Type consistency:** `host_only`/`is_loopback_host` signatures identical across T1; `resolve_target_label` sync in T2 matches its caller change; `connection::{load_target,save_target,ConnectionTarget}` match the read signatures in connection.rs:210-230. ✅
