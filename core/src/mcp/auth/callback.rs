//! OAuth Callback Server
//!
//! A lightweight HTTP server that receives OAuth authorization code callbacks.
//!
//! The callback server:
//! - Listens on a configurable port (default: 19877)
//! - Receives the authorization code from the OAuth provider
//! - Notifies the main process via a channel
//! - Auto-shuts down after receiving a callback or timeout (5 minutes)

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::timeout;

use crate::error::{AetherError, Result};

/// Default port for the callback server
pub const DEFAULT_CALLBACK_PORT: u16 = 19877;

/// Default timeout for waiting for callback (5 minutes)
pub const DEFAULT_CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// OAuth callback result
#[derive(Debug, Clone)]
pub struct CallbackResult {
    /// Authorization code
    pub code: String,
    /// State parameter (for CSRF verification)
    pub state: String,
}

/// OAuth callback server
///
/// A lightweight HTTP server that handles the OAuth callback.
/// It runs until it receives a callback or times out.
pub struct CallbackServer {
    /// Port to listen on
    port: u16,
    /// Timeout duration
    timeout_duration: Duration,
    /// Shutdown signal sender
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
}

impl CallbackServer {
    /// Create a new callback server with default settings
    pub fn new() -> Self {
        Self {
            port: DEFAULT_CALLBACK_PORT,
            timeout_duration: DEFAULT_CALLBACK_TIMEOUT,
            shutdown_tx: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a callback server with custom port
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Create a callback server with custom timeout
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout_duration = duration;
        self
    }

    /// Get the callback URL
    pub fn callback_url(&self) -> String {
        format!("http://localhost:{}/callback", self.port)
    }

    /// Start the server and wait for a callback
    ///
    /// Returns the callback result when received, or an error on timeout.
    pub async fn wait_for_callback(&self) -> Result<CallbackResult> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));

        let listener = TcpListener::bind(addr).await.map_err(|e| {
            AetherError::IoError(format!(
                "Failed to bind callback server to port {}: {}",
                self.port, e
            ))
        })?;

        tracing::info!(port = self.port, "OAuth callback server started");

        // Channel to receive callback result
        let (result_tx, mut result_rx) = mpsc::channel::<CallbackResult>(1);

        // Shutdown channel
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        {
            let mut tx = self.shutdown_tx.write().await;
            *tx = Some(shutdown_tx);
        }

        // Spawn the server task
        let server_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, _addr)) => {
                                if let Some(result) = handle_connection(stream).await {
                                    let _ = result_tx.send(result).await;
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to accept connection");
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Callback server shutdown requested");
                        break;
                    }
                }
            }
        });

        // Wait for result or timeout
        let result = timeout(self.timeout_duration, result_rx.recv()).await;

        // Clean up
        server_task.abort();

        match result {
            Ok(Some(callback_result)) => {
                tracing::info!("OAuth callback received");
                Ok(callback_result)
            }
            Ok(None) => Err(AetherError::IoError(
                "Callback server closed unexpectedly".to_string(),
            )),
            Err(_) => Err(AetherError::IoError(format!(
                "OAuth callback timeout after {} seconds",
                self.timeout_duration.as_secs()
            ))),
        }
    }

    /// Shutdown the server
    pub async fn shutdown(&self) {
        let mut tx = self.shutdown_tx.write().await;
        if let Some(shutdown_tx) = tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

impl Default for CallbackServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle an incoming HTTP connection
async fn handle_connection(mut stream: tokio::net::TcpStream) -> Option<CallbackResult> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();

    if reader.read_line(&mut request_line).await.is_err() {
        return None;
    }

    // Parse the request line (GET /callback?code=xxx&state=yyy HTTP/1.1)
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        send_error_response(&mut stream, 400, "Bad Request").await;
        return None;
    }

    let path = parts[1];
    if !path.starts_with("/callback") {
        send_error_response(&mut stream, 404, "Not Found").await;
        return None;
    }

    // Parse query parameters
    let query_start = path.find('?');
    if query_start.is_none() {
        send_error_response(&mut stream, 400, "Missing query parameters").await;
        return None;
    }

    let query = &path[query_start.unwrap() + 1..];
    let params: std::collections::HashMap<&str, &str> = query
        .split('&')
        .filter_map(|p| {
            let mut parts = p.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    // Check for error response
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(|s| url_decode(s))
            .unwrap_or_default();

        send_success_response(
            &mut stream,
            &format!(
                "Authorization failed: {} - {}. You can close this window.",
                error, description
            ),
        )
        .await;
        return None;
    }

    // Extract code and state
    let code = match params.get("code") {
        Some(c) => url_decode(c),
        None => {
            send_error_response(&mut stream, 400, "Missing authorization code").await;
            return None;
        }
    };

    let state = match params.get("state") {
        Some(s) => url_decode(s),
        None => {
            send_error_response(&mut stream, 400, "Missing state parameter").await;
            return None;
        }
    };

    // Send success response
    send_success_response(
        &mut stream,
        "Authorization successful! You can close this window and return to Aether.",
    )
    .await;

    Some(CallbackResult { code, state })
}

/// Simple URL decoding (percent-decoding)
fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            // Try to decode percent-encoded character
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            // If decoding failed, keep the original
            result.push('%');
            result.push_str(&hex);
        } else if c == '+' {
            // Plus signs are spaces in form data
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// Send an HTTP error response
async fn send_error_response(stream: &mut tokio::net::TcpStream, status: u16, message: &str) {
    use tokio::io::AsyncWriteExt;

    let body = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Error</title></head>
<body style="font-family: system-ui; text-align: center; padding: 50px;">
<h1>Error {}</h1>
<p>{}</p>
</body>
</html>"#,
        status, message
    );

    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        message,
        body.len(),
        body
    );

    let _ = stream.write_all(response.as_bytes()).await;
}

/// Send an HTTP success response
async fn send_success_response(stream: &mut tokio::net::TcpStream, message: &str) {
    use tokio::io::AsyncWriteExt;

    let body = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Authorization Complete</title></head>
<body style="font-family: system-ui; text-align: center; padding: 50px;">
<h1>Success!</h1>
<p>{}</p>
<script>setTimeout(function() {{ window.close(); }}, 3000);</script>
</body>
</html>"#,
        message
    );

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let _ = stream.write_all(response.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_callback_server_creation() {
        let server = CallbackServer::new();
        assert_eq!(server.port, DEFAULT_CALLBACK_PORT);
        assert_eq!(server.callback_url(), "http://localhost:19877/callback");
    }

    #[test]
    fn test_callback_server_with_port() {
        let server = CallbackServer::new().with_port(8080);
        assert_eq!(server.port, 8080);
        assert_eq!(server.callback_url(), "http://localhost:8080/callback");
    }

    #[test]
    fn test_callback_server_with_timeout() {
        let server = CallbackServer::new().with_timeout(Duration::from_secs(60));
        assert_eq!(server.timeout_duration, Duration::from_secs(60));
    }

    #[test]
    fn test_callback_result_structure() {
        let result = CallbackResult {
            code: "test_code".to_string(),
            state: "test_state".to_string(),
        };

        assert_eq!(result.code, "test_code");
        assert_eq!(result.state, "test_state");
    }

    #[tokio::test]
    async fn test_callback_server_timeout() {
        let server = CallbackServer::new()
            .with_port(0) // Use any available port
            .with_timeout(Duration::from_millis(100));

        // This should timeout quickly
        let result = server.wait_for_callback().await;
        assert!(result.is_err());
    }
}
