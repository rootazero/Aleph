//! Linux WebKitGTK TLS-error adapter (Task 8).
//!
//! WebKitGTK validates the server cert internally and, on failure, emits
//! `load-failed-with-tls-errors` (and, unhandled, shows a default error page) —
//! the engine-level analogue of WKWebView silently rejecting a self-signed
//! host. That signal is the WebKitGTK counterpart of WKWebView's
//! `didReceiveAuthenticationChallenge` / WebView2's `ServerCertificateErrorDetected`:
//! the handler receives the failing URI + the presented leaf cert and returns
//! whether it handled the failure.
//!
//! We run the shared TOFU decision (`install::resolve`). A pinned match allows
//! the cert for that host (`webkit_web_context_allow_tls_certificate_for_host`)
//! and reloads the URI; unknown/changed leaves the shared `prompt`'s navigation
//! to the approval page in place and just suppresses the default error page.
//! Fail-closed: any failure to read the URI/cert/state returns `false`, letting
//! WebKitGTK show its own TLS-error page (the load stays blocked, never a
//! blanket accept).

use tauri::webview::PlatformWebview;
use webkit2gtk::gio;
use webkit2gtk::gio::prelude::TlsCertificateExt;
use webkit2gtk::{WebContextExt, WebViewExt};

use crate::cert_trust::install::{resolve, HookAction, CERT_TRUST_APP};

/// Default Panel listen port — the store keys on `host:port` (Task 4). Used only
/// as a defensive fallback; http/https URIs always carry a known default port.
const DEFAULT_PORT: u16 = 18790;

/// Reason string surfaced on the approval page for a rejected server cert.
const REASON: &str = "self-signed / untrusted issuer";

/// Install the WebKitGTK `load-failed-with-tls-errors` hook on the Panel
/// webview, once. `pview` is Tauri's `PlatformWebview` (Linux: its `inner()` is
/// the `webkit2gtk::WebView`). Best-effort: a missing hook just means
/// self-signed hosts fail to load (fail-closed), never a blanket accept.
pub(crate) fn install(pview: &PlatformWebview) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| install_inner(pview));
}

fn install_inner(pview: &PlatformWebview) {
    let web_view = pview.inner();
    web_view.connect_load_failed_with_tls_errors(|web_view, failing_uri, cert, _errors| {
        on_tls_error(web_view, failing_uri, cert)
    });
    tracing::info!("cert-trust: WebKitGTK load-failed-with-tls-errors hook installed");
}

/// Handle one `load-failed-with-tls-errors` signal. Returns `true` when handled
/// (suppress WebKitGTK's default error page), `false` to let it show its own
/// (fail-closed).
fn on_tls_error(
    web_view: &webkit2gtk::WebView,
    failing_uri: &str,
    cert: &gio::TlsCertificate,
) -> bool {
    let Some((host, host_key)) = host_and_key(failing_uri) else {
        return false;
    };
    // `certificate()` is the DER-encoded leaf (the "certificate" GByteArray
    // property); `glib::ByteArray` derefs to `&[u8]` for the shared decision.
    let Some(der) = cert.certificate() else {
        return false;
    };
    let Some(app) = CERT_TRUST_APP.get() else {
        tracing::warn!("cert-trust: no AppHandle — failing {host_key} closed");
        return false;
    };

    let action = resolve(app, &host_key, &der, REASON);
    tracing::info!(
        "cert-trust: WebKitGTK TLS error {host_key} -> {}",
        match action {
            HookAction::Allow => "ALLOW (pinned)",
            HookAction::Reject => "PROMPT (unknown/changed)",
        }
    );
    match action {
        HookAction::Allow => {
            // The signal fires only on the failed load, so pin the cert for the
            // host and retry the URI — the reload then succeeds without a TLS
            // error (WebKitGTK matches its allow-list by host, hence the bare
            // host, not the `host:port` store key).
            if let Some(context) = web_view.web_context() {
                context.allow_tls_certificate_for_host(cert, &host);
                web_view.load_uri(failing_uri);
            }
            true
        }
        // The approval prompt is now showing (shared `prompt` navigated the
        // webview); swallow the default error page. On approval the reroute
        // re-navigates and the pinned cert yields Allow.
        HookAction::Reject => true,
    }
}

/// Parse the failing URI into `(host, "host:port")`. The bare host feeds
/// WebKitGTK's per-host allow-list; the `host:port` key feeds the shared TOFU
/// store (matching the other adapters). Pure; unit-testable. `None` on a URI
/// that does not parse or carries no host (the caller then fails closed).
fn host_and_key(uri: &str) -> Option<(String, String)> {
    let url = url::Url::parse(uri).ok()?;
    let host = url.host_str()?.to_string();
    // `port_or_known_default` recovers the real TCP port (443 for https with no
    // explicit port), matching the other adapters. The Aleph target always
    // carries an explicit port (18790).
    let port = url.port_or_known_default().unwrap_or(DEFAULT_PORT);
    let key = format!("{host}:{port}");
    Some((host, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_key_splits_host_and_keyed_port() {
        let (host, key) = host_and_key("https://172.245.43.211:18790/").unwrap();
        assert_eq!(host, "172.245.43.211");
        assert_eq!(key, "172.245.43.211:18790");
        let (h2, k2) = host_and_key("https://box.lan:9000/panel").unwrap();
        assert_eq!(h2, "box.lan");
        assert_eq!(k2, "box.lan:9000");
    }

    #[test]
    fn host_and_key_rejects_non_url() {
        assert!(host_and_key("not a url").is_none());
        assert!(host_and_key("").is_none());
    }
}
