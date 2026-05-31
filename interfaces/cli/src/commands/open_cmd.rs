//! `aleph open` — issue a bootstrap nonce and open the system browser.
//!
//! Mirrors the desktop app's "Open in Browser" menu. Requires the daemon
//! to be running and the caller to be authenticated (the CLI client
//! already carries the shared token on connect).

use serde::{Deserialize, Serialize};

use crate::output::{self, icon, theme};
use aleph_client::{AlephClient, CliResult};

#[derive(Debug, Deserialize, Serialize)]
struct BootstrapIssueResult {
    url: String,
    expires_in_secs: u64,
    #[serde(default)]
    nonce: Option<String>,
}

/// Issue a one-shot bootstrap nonce via JSON-RPC and open the resulting
/// URL in the system browser. With `--json` only prints the URL +
/// expiry; otherwise also shells out to the platform `open` command.
pub async fn run(server_url: &str, json: bool) -> CliResult<()> {
    let issued: BootstrapIssueResult = {
        let _spin = (!json).then(|| output::Spinner::start("Requesting bootstrap nonce"));
        let result = async {
            let (client, _events) = AlephClient::connect(server_url).await?;
            let issued: BootstrapIssueResult =
                client.call("gateway.bootstrap.issue", None::<()>).await?;
            client.close().await?;
            Ok::<BootstrapIssueResult, aleph_client::CliError>(issued)
        }
        .await;
        if let Some(s) = _spin {
            s.stop().await;
        }
        result?
    };

    if json {
        output::print_json(&serde_json::json!({
            "url": issued.url,
            "expires_in_secs": issued.expires_in_secs,
        }));
        return Ok(());
    }

    println!(
        "{} Opening {} {}",
        theme::paint(theme::Style::Info, icon::arrow()),
        theme::paint(theme::Style::Bold, &issued.url),
        theme::paint(
            theme::Style::Muted,
            &format!("(valid for {}s)", issued.expires_in_secs)
        )
    );
    if let Err(e) = open_url(&issued.url) {
        // Don't fail hard — print the URL so the user can copy/paste.
        eprintln!(
            "{} Could not launch system browser: {e}",
            theme::paint(theme::Style::Warning, icon::warn())
        );
        eprintln!(
            "{} Open this URL manually: {}",
            theme::paint(theme::Style::Info, icon::info()),
            issued.url
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
    fn bootstrap_result_roundtrip() {
        let json = serde_json::json!({
            "nonce": "abc",
            "url": "http://127.0.0.1:18790/auth/bootstrap?nonce=abc",
            "expires_in_secs": 60u64,
        });
        let parsed: BootstrapIssueResult = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.expires_in_secs, 60);
        assert!(parsed.url.contains("/auth/bootstrap?nonce="));
        assert_eq!(parsed.nonce.as_deref(), Some("abc"));
    }

    #[test]
    fn bootstrap_result_omits_optional_nonce() {
        // Server may evolve to drop the nonce from the payload (URL already
        // carries it); the client tolerates that.
        let json = serde_json::json!({
            "url": "http://127.0.0.1:18790/auth/bootstrap?nonce=xyz",
            "expires_in_secs": 60u64,
        });
        let parsed: BootstrapIssueResult = serde_json::from_value(json).unwrap();
        assert!(parsed.nonce.is_none());
    }
}
