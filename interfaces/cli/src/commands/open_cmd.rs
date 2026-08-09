//! `aleph open` — open the Panel in the system browser.
//!
//! The static Panel assets are served unauthenticated — the login wall lives on
//! the WebSocket, not the HTTP root — so there is no nonce to issue and no token
//! to carry here. This command derives the Panel's HTTP URL from the configured
//! WebSocket endpoint and shells out to the platform browser launcher; the
//! Panel then presents (or is asked for) its own credential when it connects.

use crate::output::{self, icon, theme};
use aleph_client::CliResult;

/// Derive the Panel's HTTP base URL from the gateway endpoint.
///
/// Every input shape — ws/wss/http/https scheme (or bare host:port), with or
/// without the `/ws` JSON-RPC path, with or without a trailing slash —
/// converges to the Panel root: `<scheme>://<host[:port]>/`.
///
/// `ws://127.0.0.1:18790/ws`   → `http://127.0.0.1:18790/`
/// `wss://host:443/ws/`        → `https://host:443/`
/// `http://127.0.0.1:18790/ws` → `http://127.0.0.1:18790/`
fn panel_url(server_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = server_url.strip_prefix("wss://") {
        ("https://", rest)
    } else if let Some(rest) = server_url.strip_prefix("ws://") {
        ("http://", rest)
    } else if let Some(rest) = server_url.strip_prefix("https://") {
        ("https://", rest)
    } else if let Some(rest) = server_url.strip_prefix("http://") {
        ("http://", rest)
    } else {
        // Unknown scheme: assume plain host:port, default to http.
        ("http://", server_url)
    };
    // Normalize to the root. Trim order matters: strip any trailing '/'
    // FIRST (so a `…/ws/` endpoint still loses its `/ws` path), then the
    // `/ws` JSON-RPC path, then any slash that exposed — and re-append
    // exactly one.
    let host = rest
        .trim_end_matches('/')
        .trim_end_matches("/ws")
        .trim_end_matches('/');
    format!("{scheme}{host}/")
}

/// Open the Panel URL in the system browser. With `--json` only prints the URL.
pub async fn run(server_url: &str, json: bool) -> CliResult<()> {
    let url = panel_url(server_url);

    if json {
        output::print_json(&serde_json::json!({ "url": url }));
        return Ok(());
    }

    println!(
        "{} Opening {}",
        theme::paint(theme::Style::Info, icon::arrow()),
        theme::paint(theme::Style::Bold, &url)
    );
    if let Err(e) = open_url(&url) {
        // Don't fail hard — print the URL so the user can copy/paste.
        eprintln!(
            "{} Could not launch system browser: {e}",
            theme::paint(theme::Style::Warning, icon::warn())
        );
        eprintln!(
            "{} Open this URL manually: {}",
            theme::paint(theme::Style::Info, icon::info()),
            url
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(url).status()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(url).status()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .status()?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_url(_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no system browser launcher for this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_becomes_http_panel_root() {
        assert_eq!(
            panel_url("ws://127.0.0.1:18790/ws"),
            "http://127.0.0.1:18790/"
        );
    }

    #[test]
    fn wss_url_becomes_https_panel_root() {
        assert_eq!(
            panel_url("wss://example.com:443/ws"),
            "https://example.com:443/"
        );
    }

    #[test]
    fn http_url_is_normalized_to_root() {
        // Same trailing-slash shape as the ws branches — callers can rely on
        // `<scheme>://<host[:port]>/` regardless of the configured scheme.
        assert_eq!(
            panel_url("http://127.0.0.1:18790/ws"),
            "http://127.0.0.1:18790/"
        );
        assert_eq!(panel_url("https://example.com/ws"), "https://example.com/");
    }

    #[test]
    fn trailing_slash_after_ws_path_is_stripped() {
        // `…/ws/` must not leave a `/ws` residue (trim order boundary).
        assert_eq!(
            panel_url("ws://127.0.0.1:18790/ws/"),
            "http://127.0.0.1:18790/"
        );
        assert_eq!(
            panel_url("http://127.0.0.1:18790/ws/"),
            "http://127.0.0.1:18790/"
        );
    }

    #[test]
    fn trailing_slash_without_ws_path_normalizes() {
        assert_eq!(
            panel_url("ws://127.0.0.1:18790/"),
            "http://127.0.0.1:18790/"
        );
        assert_eq!(
            panel_url("https://example.com:8443/"),
            "https://example.com:8443/"
        );
    }

    #[test]
    fn bare_host_port_defaults_to_http() {
        assert_eq!(panel_url("127.0.0.1:18790"), "http://127.0.0.1:18790/");
    }
}
