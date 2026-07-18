//! Install the per-platform cert-trust hook on the Panel webview, plus the
//! shared, platform-agnostic decision helper every adapter funnels through.
//!
//! The native TLS-error hook (see `adapter_macos` and, later, the Linux/Windows
//! adapters) receives only engine-level arguments — no Tauri context. So install
//! stashes the [`AppHandle`] in a process-global [`CERT_TRUST_APP`]; the hook
//! reads it to reach the managed `PendingCert` state and the "main" webview.

use tauri::{AppHandle, Manager, WebviewWindow};

/// Process-global [`AppHandle`] for the native TLS-challenge hook, which is
/// invoked by the platform webview with no Tauri context. Set once at install
/// time (idempotent — a second set is ignored).
pub(crate) static CERT_TRUST_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// Install the platform cert-trust hook on `window`'s webview. Best-effort:
/// any failure to reach the native webview is logged, never fatal — a missing
/// hook just means self-signed hosts fail to load (fail-closed), never a blanket
/// accept.
pub fn install_cert_trust(window: &WebviewWindow) {
    // Stash the AppHandle so the native hook can reach managed state + the webview.
    let _ = CERT_TRUST_APP.set(window.app_handle().clone());

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = window.with_webview(|pview| {
            // macOS `PlatformWebview::inner()` is the `WKWebView` pointer.
            super::adapter_macos::install(pview.inner());
        }) {
            tracing::warn!("cert-trust: could not reach macOS webview for install: {e}");
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = window.with_webview(|pview| {
            // Windows `PlatformWebview::controller()` is the ICoreWebView2Controller.
            super::adapter_windows::install(&pview);
        }) {
            tracing::warn!("cert-trust: could not reach Windows webview for install: {e}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = window.with_webview(|pview| {
            // Linux `PlatformWebview::inner()` is the `webkit2gtk::WebView`.
            super::adapter_linux::install(&pview);
        }) {
            tracing::warn!("cert-trust: could not reach Linux webview for install: {e}");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let _ = window;
}

/// Platform-agnostic verdict for a presented leaf cert. An adapter extracts the
/// leaf DER + `host:port` from its engine's TLS hook and calls this; the shared
/// logic (fingerprint → parse → decide → prompt) lives here so every adapter
/// reuses it. Only the cert/host arrival and the "allow" grant are per-platform.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]
pub(crate) enum HookAction {
    /// The presented cert matches the pinned fingerprint — grant the connection.
    Allow,
    /// Unknown or changed cert — a prompt is now showing; reject THIS load.
    Reject,
}

/// Resolve a presented leaf DER for `host` against the pinned TOFU store.
/// `host` is the `host:port` store key. On an unknown/changed cert this stashes
/// the pending record and navigates the webview to the approval page, returning
/// [`HookAction::Reject`]. Fail-closed: a missing store path rejects.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]
pub(crate) fn resolve(app: &AppHandle, host: &str, leaf_der: &[u8], reason: &str) -> HookAction {
    use crate::cert_trust::{
        decide,
        fingerprint::{fingerprint_sha256, parse_cert_info},
        pending,
        store::TrustStore,
        Decision,
    };

    let Some(path) = pending::store_path() else {
        return HookAction::Reject;
    };
    let fp = fingerprint_sha256(leaf_der);
    let info = parse_cert_info(leaf_der, reason);
    let store = TrustStore::load(&path);
    match decide(host, &fp, info, &store) {
        Decision::Allow => HookAction::Allow,
        Decision::PromptUnknown { fp, info } => {
            prompt(app, host, fp, info, None);
            HookAction::Reject
        }
        Decision::WarnChanged {
            old_fp,
            new_fp,
            info,
        } => {
            prompt(app, host, new_fp, info, Some(old_fp));
            HookAction::Reject
        }
    }
}

/// Stash the pending cert into managed state, latch the supervisor off its
/// relocation tick, and navigate the main webview to the approval page.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]
fn prompt(
    app: &AppHandle,
    host: &str,
    fp: String,
    info: crate::cert_trust::CertInfo,
    changed_from: Option<String>,
) {
    use crate::cert_trust::pending::{set_trust_pending, PendingCert, PendingRecord};

    if let Some(state) = app.try_state::<PendingCert>() {
        *state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PendingRecord {
            host: host.to_string(),
            fp,
            info,
            changed_from,
        });
    }
    set_trust_pending(true);
    if let Some(window) = app.get_webview_window("main") {
        // Absolute engine-level navigation, not a relative `location.href` eval:
        // the failing remote load leaves no usable document base, so a relative
        // target would resolve against the wrong origin (blank page).
        match crate::connection::cert_trust_page_url().parse() {
            Ok(url) => {
                if let Err(e) = window.navigate(url) {
                    tracing::error!("cert-trust: failed to show approval page: {e}");
                }
            }
            Err(e) => tracing::error!("cert-trust: invalid approval-page URL: {e}"),
        }
    }
}
