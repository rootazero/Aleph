//! Windows WebView2 TLS-error adapter (Task 9).
//!
//! WebView2 validates server certs internally and, on failure, blocks the
//! navigation (its default error page) — the engine-level analogue of
//! WKWebView silently rejecting a self-signed host. `ICoreWebView2_14`'s
//! `ServerCertificateErrorDetected` event is the WebView2 counterpart of
//! WKWebView's `didReceiveAuthenticationChallenge`: the handler receives the
//! request URI + the presented leaf cert and decides an `Action`
//! (`ALWAYS_ALLOW` / `CANCEL` / `DEFAULT`).
//!
//! We run the shared TOFU decision (`install::resolve`) and either `ALWAYS_ALLOW`
//! (the presented cert matches the pinned fingerprint) or `CANCEL` (unknown /
//! changed — the shared `prompt` has already navigated the webview to the
//! approval page). Fail-closed on every abnormal path: `SetAction(DEFAULT)`
//! leaves WebView2's own block in place, never a blanket accept.

use tauri::webview::PlatformWebview;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, ServerCertificateErrorDetectedEventHandler};
use windows_core::{Interface, PWSTR};

use crate::cert_trust::install::{resolve, HookAction, CERT_TRUST_APP};

/// Default Panel listen port — the store keys on `host:port` (Task 4). Used only
/// as a defensive fallback; http/https URIs always carry a known default port.
const DEFAULT_PORT: u16 = 18790;

/// Reason string surfaced on the approval page for a rejected server cert.
const REASON: &str = "self-signed / untrusted issuer";

/// Install the WebView2 `ServerCertificateErrorDetected` hook on the Panel
/// webview, once. `pview` is Tauri's `PlatformWebview` (Windows: its
/// `controller()` is the live `ICoreWebView2Controller`). Best-effort: any
/// failure to reach the CoreWebView2 or register the handler is logged and
/// leaves the fail-closed default (self-signed hosts simply don't load).
pub(crate) fn install(pview: &PlatformWebview) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| install_inner(pview));
}

fn install_inner(pview: &PlatformWebview) {
    let controller = pview.controller();
    // SAFETY: `controller` is a live `ICoreWebView2Controller` COM interface
    // owned by the Tauri webview; we run inside `with_webview` on the UI thread.
    // Every COM call below returns a `Result` that is checked — nothing is
    // dereferenced blindly.
    unsafe {
        let core = match controller.CoreWebView2() {
            Ok(core) => core,
            Err(e) => {
                tracing::warn!("cert-trust: WebView2 CoreWebView2() unavailable: {e}");
                return;
            }
        };
        // `add_ServerCertificateErrorDetected` lives on `ICoreWebView2_14`
        // (WebView2 Runtime 1.0.1466+). An older runtime fails the
        // QueryInterface → fail-closed (self-signed hosts don't load), never a
        // blanket accept.
        let core: ICoreWebView2_14 = match core.cast() {
            Ok(core) => core,
            Err(e) => {
                tracing::warn!(
                    "cert-trust: WebView2 runtime lacks ServerCertificateErrorDetected: {e}"
                );
                return;
            }
        };
        let handler =
            ServerCertificateErrorDetectedEventHandler::create(Box::new(|_sender, args| {
                if let Some(args) = args {
                    on_cert_error(&args);
                }
                Ok(())
            }));
        let mut token = 0i64;
        if let Err(e) = core.add_ServerCertificateErrorDetected(&handler, &mut token) {
            tracing::warn!("cert-trust: WebView2 add_ServerCertificateErrorDetected failed: {e}");
        } else {
            tracing::info!("cert-trust: WebView2 ServerCertificateErrorDetected hook installed");
        }
    }
}

/// Handle one `ServerCertificateErrorDetected` event: read the URI + presented
/// leaf cert, run the shared TOFU decision, and set the resulting `Action`.
/// Fail-closed (`SetAction(DEFAULT)`) on every early return.
fn on_cert_error(args: &ICoreWebView2ServerCertificateErrorDetectedEventArgs) {
    // SAFETY: `args` is the live event-args COM interface for this callback,
    // valid for its duration; every COM method returns a checked `Result`, and
    // exactly one `SetAction` runs on every path (fail-closed on error).
    unsafe {
        let Some(host_key) = read_request_host_key(args) else {
            let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_DEFAULT);
            return;
        };
        let Some(leaf_der) = read_server_cert_der(args) else {
            let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_DEFAULT);
            return;
        };
        let Some(app) = CERT_TRUST_APP.get() else {
            tracing::warn!("cert-trust: no AppHandle — failing {host_key} closed");
            let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_DEFAULT);
            return;
        };

        let action = resolve(app, &host_key, &leaf_der, REASON);
        tracing::info!(
            "cert-trust: WebView2 TLS error {host_key} -> {}",
            match action {
                HookAction::Allow => "ALLOW (pinned)",
                HookAction::Reject => "PROMPT (unknown/changed)",
            }
        );
        let verdict = match action {
            // Pinned match: ignore the warning and proceed. WebView2 caches this
            // for the host+cert within the session; our store stays the source
            // of truth (a changed cert re-fires the event → re-resolve).
            HookAction::Allow => COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW,
            // Unknown/changed: the approval prompt is now showing (shared
            // `prompt` navigated the webview); cancel THIS errored load. On
            // approval the reroute re-navigates and the pinned cert yields Allow.
            HookAction::Reject => COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL,
        };
        if let Err(e) = args.SetAction(verdict) {
            tracing::warn!("cert-trust: WebView2 SetAction failed: {e}");
        }
    }
}

/// Read the event's `RequestUri` and reduce it to the `host:port` store key.
///
/// # Safety
/// `args` must be the live event-args COM interface for the current callback.
unsafe fn read_request_host_key(
    args: &ICoreWebView2ServerCertificateErrorDetectedEventArgs,
) -> Option<String> {
    let mut uri = PWSTR::null();
    // SAFETY: `RequestUri` writes an owned PWSTR (freed by `take_pwstr`).
    unsafe { args.RequestUri(&mut uri).ok()? };
    if uri.is_null() {
        return None;
    }
    let uri = take_pwstr(uri);
    host_key_from_uri(&uri)
}

/// Read the presented leaf cert's DER via `ServerCertificate.ToPemEncoding`.
///
/// # Safety
/// `args` must be the live event-args COM interface for the current callback.
unsafe fn read_server_cert_der(
    args: &ICoreWebView2ServerCertificateErrorDetectedEventArgs,
) -> Option<Vec<u8>> {
    // SAFETY: `ServerCertificate` returns the leaf `ICoreWebView2Certificate`.
    let cert = unsafe { args.ServerCertificate().ok()? };
    let mut pem = PWSTR::null();
    // SAFETY: `ToPemEncoding` writes an owned PWSTR (freed by `take_pwstr`).
    unsafe { cert.ToPemEncoding(&mut pem).ok()? };
    if pem.is_null() {
        return None;
    }
    let pem = take_pwstr(pem);
    pem_to_der(&pem)
}

/// Parse a WebView2 `RequestUri` into the TOFU store key `host:port`. Pure (no
/// COM) so it is unit-testable. A URI that does not parse, or carries no host,
/// yields `None` (the caller then fails closed).
fn host_key_from_uri(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    let host = url.host_str()?;
    // `port_or_known_default` recovers the real TCP port (e.g. 443 for https
    // with no explicit port), matching the macOS adapter's protection-space
    // port. The Aleph target always carries an explicit port (18790).
    let port = url.port_or_known_default().unwrap_or(DEFAULT_PORT);
    Some(format!("{host}:{port}"))
}

/// Decode a single PEM-encoded leaf certificate to DER. Pure; unit-testable.
/// `None` on malformed PEM (the caller then fails closed).
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).ok()?;
    Some(parsed.contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_from_uri_keeps_explicit_port() {
        assert_eq!(
            host_key_from_uri("https://172.245.43.211:18790/").as_deref(),
            Some("172.245.43.211:18790")
        );
        assert_eq!(
            host_key_from_uri("https://box.lan:9000/panel").as_deref(),
            Some("box.lan:9000")
        );
    }

    #[test]
    fn host_key_from_uri_rejects_non_url() {
        assert!(host_key_from_uri("not a url").is_none());
        assert!(host_key_from_uri("").is_none());
    }

    #[test]
    fn pem_to_der_round_trips_leaf_der() {
        // rcgen is a dev-dep already used by fingerprint.rs tests. The PEM from
        // `cert.pem()` must decode to exactly the same bytes as `cert.der()`.
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(vec!["172.245.43.211".to_string()]).unwrap();
        let der = pem_to_der(&cert.pem()).expect("valid PEM decodes");
        assert_eq!(der.as_slice(), cert.der().as_ref());
    }

    #[test]
    fn pem_to_der_on_garbage_is_none_not_panic() {
        assert!(pem_to_der("-----BEGIN CERTIFICATE-----\nnot base64\n").is_none());
        assert!(pem_to_der("plain text").is_none());
    }
}
