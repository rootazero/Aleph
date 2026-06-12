//! `aleph open` — open the Panel in the system browser.
//!
//! LAN-trust model: the gateway serves the Panel at its HTTP root with no
//! authentication, so there is no nonce to issue and no token to carry. This
//! command simply derives the Panel's HTTP URL from the configured WebSocket
//! endpoint and shells out to the platform browser launcher.

use crate::output::{self, icon, theme};
use aleph_client::CliResult;

/// Derive the Panel's HTTP base URL from the gateway WebSocket URL.
///
/// `ws://127.0.0.1:18790/ws` → `http://127.0.0.1:18790/`
/// `wss://host:443/ws`       → `https://host:443/`
///
/// Falls back to returning the input unchanged if it has no recognized WS
/// scheme (the caller may already have passed an HTTP URL).
fn panel_url(server_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = server_url.strip_prefix("wss://") {
        ("https://", rest)
    } else if let Some(rest) = server_url.strip_prefix("ws://") {
        ("http://", rest)
    } else if server_url.starts_with("http://") || server_url.starts_with("https://") {
        return server_url.trim_end_matches("/ws").to_string();
    } else {
        // Unknown scheme: assume plain host:port, default to http.
        ("http://", server_url)
    };
    // Drop the `/ws` JSON-RPC path; the Panel is served at the root.
    let host = rest.trim_end_matches("/ws").trim_end_matches('/');
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
        assert_eq!(panel_url("wss://example.com:443/ws"), "https://example.com:443/");
    }

    #[test]
    fn http_url_is_normalized_to_root() {
        assert_eq!(
            panel_url("http://127.0.0.1:18790/ws"),
            "http://127.0.0.1:18790"
        );
    }

    #[test]
    fn bare_host_port_defaults_to_http() {
        assert_eq!(panel_url("127.0.0.1:18790"), "http://127.0.0.1:18790/");
    }
}
