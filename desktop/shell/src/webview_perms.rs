//! Per-platform microphone grant for the Panel webview.
//!
//! The Panel's voice-input button calls `getUserMedia`, whose success hinges on
//! the *host* webview granting media capture — a native, platform-specific
//! concern (R1: the shell is the limb, the WASM panel only speaks standard Web
//! APIs). The three webview engines diverge:
//!
//! - **macOS / iOS (`WKWebView`)**: wry's UI delegate already auto-grants the
//!   capture request; the only remaining gate is the TCC usage string in
//!   `Info.plist`. Nothing to do here — [`grant_microphone`] is a no-op.
//! - **Windows (`WebView2`)**: wry registers a `PermissionRequested` handler that
//!   only grants the clipboard; microphone falls through to `WebView2`'s default
//!   (a prompt, at best). We attach our own handler granting the mic.
//! - **Linux (`WebKitGTK`)**: wry installs no permission handler at all, and
//!   `enable-media-stream` defaults off, so `getUserMedia` is silently denied.
//!   We enable the setting, then grant a `UserMediaPermissionRequest` only when
//!   it is audio-only (no camera) and originates from the Panel surface; every
//!   other media request is explicitly denied and logged.

use tauri::WebviewWindow;

/// Grant the Panel webview microphone access on platforms where wry does not.
/// Best-effort: failures are logged, never fatal — a missing grant just means
/// the voice button surfaces a permission error (visible in the Panel).
pub fn grant_microphone(window: &WebviewWindow) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        if let Err(e) = window.with_webview(|pview| {
            #[cfg(target_os = "linux")]
            grant_linux(&pview);
            #[cfg(target_os = "windows")]
            grant_windows(&pview);
        }) {
            tracing::warn!("could not reach platform webview for mic grant: {e}");
        }
    }

    // macOS / iOS: wry auto-grants, Info.plist covers TCC. No work needed.
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = window;
}

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

#[cfg(target_os = "windows")]
fn grant_windows(pview: &tauri::webview::PlatformWebview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{take_pwstr, PermissionRequestedEventHandler};
    use windows_core::PWSTR;

    let controller = pview.controller();
    // `controller` is a live `ICoreWebView2Controller` COM interface owned by
    // the Tauri webview; the COM calls below operate on that valid interface,
    // and each returns a `Result` so any failure is handled rather than dereferenced.
    // SAFETY: the COM interface is valid and all calls return a `Result`.
    unsafe {
        let core = match controller.CoreWebView2() {
            Ok(core) => core,
            Err(e) => {
                tracing::warn!("WebView2 CoreWebView2() unavailable: {e}");
                return;
            }
        };
        let mut token = 0i64;
        let handler = PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            if kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                return Ok(());
            }
            // Only the Panel's own origin may auto-grant the mic. Reuse the
            // navigation SSOT so there is one definition of "Panel origin"
            // (loopback daemon / tauri.localhost / configured remote).
            let mut uri = PWSTR::null();
            // SAFETY: `Uri` writes an owned PWSTR (freed by `take_pwstr`);
            // `args` is the live event-args COM interface for this callback.
            args.Uri(&mut uri)?;
            if uri.is_null() {
                // A successful `Uri()` yielding a null pointer is a COM anomaly:
                // the request carries no origin, so the grant is withheld. Log it
                // rather than let the mic request vanish silently.
                tracing::warn!(
                    "WebView2 microphone permission request arrived with an unresolvable origin; withholding grant"
                );
                return Ok(());
            }
            let origin_ok = tauri::Url::parse(&take_pwstr(uri))
                .ok()
                .is_some_and(|u| crate::external_link::is_internal(&u));
            if origin_ok {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
            Ok(())
        }));
        if let Err(e) = core.add_PermissionRequested(&handler, &mut token) {
            tracing::warn!("WebView2 add_PermissionRequested failed: {e}");
        }
    }
}
