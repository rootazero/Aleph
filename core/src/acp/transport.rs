//! NDJSON stdio transport for ACP communication.
//!
//! Wraps a child process's stdin/stdout for newline-delimited JSON messaging.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::acp::protocol::{AcpRequest, AcpResponse};
use crate::error::{AlephError, Result};

/// NDJSON stdio transport for communicating with an ACP child process.
///
/// Reads newline-delimited JSON from the child's stdout in a background task
/// and provides methods to send requests and receive responses.
pub struct StdioTransport {
    stdin: ChildStdin,
    event_rx: mpsc::Receiver<Result<AcpResponse>>,
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    /// Create a new transport wrapping a child process's stdin/stdout.
    ///
    /// Spawns a background tokio task that reads lines from stdout,
    /// parses them as `AcpResponse`, and sends via an mpsc channel (capacity 256).
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let (tx, rx) = mpsc::channel(256);

        let handle = tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<AcpResponse>(&line) {
                            Ok(resp) => {
                                if tx.send(Ok(resp)).await.is_err() {
                                    debug!("ACP reader: receiver dropped, stopping");
                                    break;
                                }
                            }
                            Err(e) => {
                                // Truncate line for logging (UTF-8 safe)
                                let truncated: String = line.chars().take(200).collect();
                                warn!(
                                    "ACP reader: failed to parse line: {} — content: {}",
                                    e, truncated
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        debug!("ACP reader: EOF on stdout");
                        break;
                    }
                    Err(e) => {
                        warn!("ACP reader: I/O error: {}", e);
                        let _ = tx
                            .send(Err(AlephError::IoError(format!(
                                "ACP stdout read error: {}",
                                e
                            ))))
                            .await;
                        break;
                    }
                }
            }
        });

        Self {
            stdin,
            event_rx: rx,
            _reader_handle: handle,
        }
    }

    /// Serialize a request to JSON, append newline, write to stdin, and flush.
    pub async fn send(&mut self, req: &AcpRequest) -> Result<()> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AlephError::IoError(format!("ACP stdin write error: {}", e)))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| AlephError::IoError(format!("ACP stdin flush error: {}", e)))?;
        debug!("ACP sent: method={} id={}", req.method, req.id);
        Ok(())
    }

    /// Receive the next parsed event from the reader channel.
    pub async fn recv(&mut self) -> Option<Result<AcpResponse>> {
        self.event_rx.recv().await
    }

    /// Send a request and wait for a response with matching `id`.
    ///
    /// Collects any notifications received while waiting. Returns the matching
    /// response and all collected notifications. Errors on timeout, connection
    /// closed, or ACP error response.
    pub async fn request(
        &mut self,
        req: &AcpRequest,
        timeout: Duration,
    ) -> Result<(AcpResponse, Vec<AcpResponse>)> {
        self.send(req).await?;
        let expected_id = req.id;
        let mut notifications = Vec::new();

        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.event_rx.recv().await {
                    Some(Ok(resp)) => {
                        // Check if this is the response we're waiting for
                        if resp.id == Some(expected_id) {
                            // Check for ACP error
                            if let Some(ref err) = resp.error {
                                return Err(AlephError::tool(format!(
                                    "ACP error {}: {}",
                                    err.code, err.message
                                )));
                            }
                            return Ok((resp, notifications));
                        }
                        // Otherwise it's a notification or response for a different id
                        notifications.push(resp);
                    }
                    Some(Err(e)) => return Err(e),
                    None => {
                        return Err(AlephError::tool("ACP connection closed while waiting for response"));
                    }
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_elapsed) => Err(AlephError::tool(format!(
                "ACP request timed out after {:?}",
                timeout
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::protocol::AcpRequest;

    #[test]
    fn test_serialize_ndjson_line() {
        let req = AcpRequest::initialize();
        let line = serde_json::to_string(&req).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains("initialize"));
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"content":"hello"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text_content(), Some("hello".to_string()));
    }

    #[test]
    fn test_deserialize_error_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"fail"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
    }
}
