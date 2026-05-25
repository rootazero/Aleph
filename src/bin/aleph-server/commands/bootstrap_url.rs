//! `bootstrap-url` subcommand — issue a one-shot bootstrap nonce against
//! a running daemon and print the loopback URL the desktop shell should
//! open in the system browser.
//!
//! Lives next to `bootstrap-token` and follows the same threat model
//! (same-UID gate via DB-read of the shared token), but unlike that
//! sibling this one **requires the daemon to be running**: it opens a
//! WebSocket to the daemon, authenticates via the shared token's
//! existing connect path, calls `gateway.bootstrap.issue`, and prints
//! the resulting `url` field.
//!
//! Output contract: one line on stdout (the URL, newline-terminated)
//! on success. Any failure exits with EX_USAGE (64) and a stderr
//! diagnostic — the shell shells out to this binary and parses stdout.

use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
use serde::Deserialize;
use serde_json::{json, Value};
use std::error::Error;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Daemon's loopback WebSocket endpoint. Matches Phase-1 `DAEMON_*`
/// constants in `desktop/shell/src/daemon.rs`. The port is hard-coded
/// because this CLI ships next to the daemon binary in the bundle and
/// the daemon always listens on 18790 in production.
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:18790/ws";
/// Cap the whole exchange so a stuck daemon cannot hang the shell.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct IssueResult {
    url: String,
}

/// Handle `aleph-server bootstrap-url`.
pub async fn handle_bootstrap_url() -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path =
        paths::get_security_db_path().map_err(|e| format!("resolve security DB path: {e}"))?;
    let data_dir = db_path
        .parent()
        .ok_or("security DB has no parent directory")?
        .to_path_buf();
    let vault_path = data_dir.join("secrets.vault");

    let token = {
        let store = Arc::new(SecurityStore::open(&db_path).map_err(|e| format!("open store: {e}"))?);
        let mgr = SharedTokenManager::new(store, vault_path);
        match mgr.try_load_token_from_db() {
            Some(t) => t,
            None => {
                eprintln!(
                    "aleph-server: no shared token provisioned yet — start the \
                     server once (`aleph-server start`) to generate one."
                );
                std::process::exit(64);
            }
        }
    };

    match tokio::time::timeout(REQUEST_TIMEOUT, issue_nonce_url(&token)).await {
        Ok(Ok(url)) => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            writeln!(handle, "{url}")?;
            Ok(())
        }
        Ok(Err(e)) => {
            eprintln!("aleph-server: bootstrap-url failed: {e}");
            std::process::exit(64);
        }
        Err(_) => {
            eprintln!(
                "aleph-server: bootstrap-url timed out talking to the daemon ({}s)",
                REQUEST_TIMEOUT.as_secs()
            );
            std::process::exit(64);
        }
    }
}

/// Open a single WS connection, call `connect` with the shared token,
/// then call `gateway.bootstrap.issue`, return the `url` field.
async fn issue_nonce_url(token: &str) -> Result<String, String> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(DEFAULT_WS_URL)
        .await
        .map_err(|e| format!("connect {DEFAULT_WS_URL}: {e}"))?;

    // 1. connect (auth)
    let connect_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "connect",
        "params": {
            "shared_token": token,
            "device_name": "aleph-shell-bootstrap",
            "device_type": "cli",
        }
    });
    ws.send(Message::Text(connect_req.to_string().into()))
        .await
        .map_err(|e| format!("send connect: {e}"))?;

    // Drain frames until we see the response to id=1. Notifications
    // (e.g. `hello`) may arrive interleaved.
    wait_for_id(&mut ws, 1).await?;

    // 2. issue bootstrap nonce
    let issue_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "gateway.bootstrap.issue",
    });
    ws.send(Message::Text(issue_req.to_string().into()))
        .await
        .map_err(|e| format!("send issue: {e}"))?;

    let resp = wait_for_id(&mut ws, 2).await?;
    let result = resp
        .get("result")
        .ok_or_else(|| format!("issue response had no result: {resp}"))?;
    let issued: IssueResult = serde_json::from_value(result.clone())
        .map_err(|e| format!("parse issue result: {e}"))?;

    // Best-effort close.
    let _ = ws.close(None).await;
    Ok(issued.url)
}

/// Read WS frames until we see a JSON-RPC response with the requested
/// id. Ignores notifications and other ids.
async fn wait_for_id(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
) -> Result<Value, String> {
    while let Some(frame) = ws.next().await {
        let frame = frame.map_err(|e| format!("ws read: {e}"))?;
        let text: String = match frame {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => std::str::from_utf8(b.as_ref())
                .map_err(|e| format!("binary frame not utf-8: {e}"))?
                .to_string(),
            Message::Close(_) => return Err("ws closed before response".to_string()),
            // Ping/Pong/Frame are handled by tungstenite; loop.
            _ => continue,
        };
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("parse ws frame as json: {e} (raw: {text})"))?;
        if let Some(got) = value.get("id").and_then(|v| v.as_u64()) {
            if got == id {
                if let Some(err) = value.get("error") {
                    return Err(format!("rpc error for id={id}: {err}"));
                }
                return Ok(value);
            }
        }
        // Notification or wrong id — keep reading.
    }
    Err("ws stream ended before response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_result_parses_url() {
        let v = json!({"url": "http://127.0.0.1:18790/auth/bootstrap?nonce=abc"});
        let r: IssueResult = serde_json::from_value(v).unwrap();
        assert!(r.url.contains("/auth/bootstrap?nonce="));
    }
}
