# Linux Webview Mic Gate (#7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the Linux `webview_perms` gap (#7) — `grant_linux` currently grants *any* `UserMediaPermissionRequest` (camera included, no origin check); gate it to audio-only capture from the Panel origin.

**Architecture:** Rewrite the single `connect_permission_request` callback inside `grant_linux` to apply two gates before allowing a media request — (1) audio-only (`is_for_audio_device() && !is_for_video_device()`), (2) origin (`WebViewExt::uri` → parse → `external_link::is_internal`, the same Panel-origin SSOT the Windows leg reuses). Any failure explicitly `deny()`s and logs, mirroring the Windows leg's withhold-and-log. This is the Linux counterpart of the already-landed Windows `grant_windows` mic-origin gate (Task 3 of the Windows batch).

**Tech Stack:** Rust, `webkit2gtk` 2.0.2 (`UserMediaPermissionRequestExt`, `WebViewExt`, `PermissionRequestExt`), Tauri webview shell (`aleph-desktop-shell`).

## Global Constraints

- **Single file, single function**: only `grant_linux` in `desktop/shell/src/webview_perms.rs` changes. No new files, no new dependencies (`webkit2gtk` already in the tree).
- **R1 (brain-limb)**: this is the shell/limb performing a native, platform-specific webview grant — legitimate. The WASM Panel only speaks standard Web APIs.
- **DRY / P2 / P5**: reuse `crate::external_link::is_internal(&Url)` as the sole definition of "Panel origin". Do NOT introduce a second origin allowlist.
- **Platform gating**: `grant_linux` is already `#[cfg(target_os = "linux")]`; it compiles only on Linux. This plan MUST be executed on a Linux machine (the reason #7 was deferred here).
- **Gate-fail behavior (locked in spec §⑦)**: any failing gate → explicit `request.deny()` + `tracing::warn!`. A `None` uri folds into `origin_ok = false`. Never `return false` to WebKitGTK's version-dependent default for a security property.
- **No faked test coverage**: a GTK permission callback needs a live WebKitGTK webview + a real permission event; it cannot be meaningfully unit-tested in a headless harness. Verification is compile-check + manual smoke, not an automated test. Do not write a fake/no-op unit test to satisfy a TDD checkbox.
- **Minimal cargo**: exactly one `cargo check` for this change (project convention — cargo calls are a system burden).

---

### Task 1: Gate the Linux webview mic grant to audio-only + Panel origin (#7)

**Files:**
- Modify: `desktop/shell/src/webview_perms.rs` — the module doc comment (lines 14-16) and the `grant_linux` function body (lines 41-63)

**Interfaces:**
- Consumes:
  - `webkit2gtk::UserMediaPermissionRequestExt::is_for_audio_device(&self) -> bool`
  - `webkit2gtk::UserMediaPermissionRequestExt::is_for_video_device(&self) -> bool`
  - `webkit2gtk::WebViewExt::uri(&self) -> Option<glib::GString>`
  - `webkit2gtk::PermissionRequestExt::allow(&self)` / `deny(&self)`
  - `crate::external_link::is_internal(url: &tauri::Url) -> bool` (Panel-origin SSOT)
- Produces: no new public surface — `grant_linux` keeps its signature `fn grant_linux(pview: &tauri::webview::PlatformWebview)`.

- [ ] **Step 1: Update the module doc comment for the Linux engine**

In `desktop/shell/src/webview_perms.rs`, replace the Linux bullet in the module doc (currently lines 14-16):

```rust
//! - **Linux (`WebKitGTK`)**: wry installs no permission handler at all, and
//!   `enable-media-stream` defaults off, so `getUserMedia` is silently denied.
//!   We enable the setting and allow `UserMediaPermissionRequest`.
```

with the gated description:

```rust
//! - **Linux (`WebKitGTK`)**: wry installs no permission handler at all, and
//!   `enable-media-stream` defaults off, so `getUserMedia` is silently denied.
//!   We enable the setting, then grant a `UserMediaPermissionRequest` only when
//!   it is audio-only (no camera) and originates from the Panel surface; every
//!   other media request is explicitly denied and logged.
```

- [ ] **Step 2: Rewrite `grant_linux` with the two-gate handler**

In `desktop/shell/src/webview_perms.rs`, replace the entire `grant_linux` function (currently lines 41-63):

```rust
#[cfg(target_os = "linux")]
fn grant_linux(pview: &tauri::webview::PlatformWebview) {
    use webkit2gtk::glib::object::Cast;
    use webkit2gtk::{PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, WebViewExt};

    let webview = pview.inner();

    // getUserMedia is unavailable unless the media-stream feature is enabled.
    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_media_stream(true);
    }

    webview.connect_permission_request(|_webview, request| {
        if request
            .downcast_ref::<UserMediaPermissionRequest>()
            .is_some()
        {
            request.allow();
            return true; // handled — stop further emission
        }
        false // defer everything else to default handling
    });
}
```

with:

```rust
#[cfg(target_os = "linux")]
fn grant_linux(pview: &tauri::webview::PlatformWebview) {
    use webkit2gtk::glib::object::Cast;
    use webkit2gtk::{
        PermissionRequestExt, SettingsExt, UserMediaPermissionRequest,
        UserMediaPermissionRequestExt, WebViewExt,
    };

    let webview = pview.inner();

    // getUserMedia is unavailable unless the media-stream feature is enabled.
    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_media_stream(true);
    }

    webview.connect_permission_request(|webview, request| {
        let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() else {
            return false; // not a media request — defer to default handling
        };
        // Gate 1 — audio-only: the Panel voice button never needs a camera, so
        // any request that includes a video device is refused wholesale.
        let audio_only = media.is_for_audio_device() && !media.is_for_video_device();
        // Gate 2 — origin: reuse the navigation SSOT for "is this the Panel
        // surface?" (loopback daemon / tauri.localhost / configured remote).
        // A `None` uri (unresolvable origin) folds into origin_ok = false.
        let origin_ok = WebViewExt::uri(webview)
            .and_then(|u| tauri::Url::parse(&u).ok())
            .is_some_and(|u| crate::external_link::is_internal(&u));
        if audio_only && origin_ok {
            request.allow();
        } else {
            // Withhold camera / foreign-origin capture explicitly rather than
            // relying on WebKitGTK's version-dependent unhandled default.
            tracing::warn!(
                audio_only,
                origin_ok,
                "webview UserMedia request denied (not audio-only from Panel origin)"
            );
            request.deny();
        }
        true // handled — stop further emission
    });
}
```

Notes for the implementer:
- The closure's first parameter is now bound (`webview`, was `_webview`) because Gate 2 reads its uri. It shadows the outer `webview` binding — intentional and fine; the callback signature is `Fn(&WebView, &PermissionRequest) -> bool`.
- `request.allow()` / `request.deny()` are called on the `&PermissionRequest` via `PermissionRequestExt` (already imported).
- `is_for_audio_device` / `is_for_video_device` come from `UserMediaPermissionRequestExt` — the newly added import. Without it the build fails with "no method named `is_for_audio_device`".
- `tauri::Url` is `url::Url` re-exported; the Windows leg already passes `tauri::Url` into `is_internal`, so the type lines up.

- [ ] **Step 3: Compile-verify on Linux**

Run: `cargo check -p aleph-desktop-shell`
Expected: PASS (no errors). This is the real verification — the whole reason #7 was deferred to a Linux machine.

If it fails with an unresolved-import or trait-method error, re-check the `use` list in Step 2 (most likely `UserMediaPermissionRequestExt` missing).

- [ ] **Step 4: Manual smoke (record result inline; this replaces automated coverage)**

No automated test exists for this callback (see Global Constraints). Perform and record:
1. Launch the desktop shell dev app on Linux (`just shell-dev`).
2. In the Panel, press the voice-input button → browser mic prompt/capture succeeds (audio still works — no regression). Expect NO camera indicator.
3. (Best-effort) Confirm no `webview UserMedia request denied` warning appears in the shell logs for the normal audio-only Panel request.

If `just shell-dev` cannot run in the current environment, note that explicitly in the commit body / hand-off rather than claiming the smoke passed.

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/webview_perms.rs
git commit -m "shell: gate Linux webview mic grant to audio-only Panel origin (#7)

grant_linux allowed any UserMediaPermissionRequest (camera included, no
origin check). Gate it: allow only audio-only capture from the Panel
origin (external_link::is_internal SSOT, same as the Windows leg);
explicitly deny + warn otherwise. Compile-verified on Linux via
cargo check -p aleph-desktop-shell.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015yfKbTtgFBh2VKVLHTQLr4"
```

---

### Final verification

- [ ] **Step 1: Confirm scope did not leak into the deferred item**

`desktop/shared/src/perception/screen_record.rs` (#6 macOS `setSourceRect`) MUST be untouched by this cycle — it stays deferred (no macOS env). Confirm `git show --stat HEAD` lists only `desktop/shell/src/webview_perms.rs`.

- [ ] **Step 2: Confirm the spec matches reality**

`docs/superpowers/specs/2026-07-20-review-followup-fixes-design.md` §⑦（已落地）already documents this change; no further spec edit needed. Only #6 remains under §延后.
