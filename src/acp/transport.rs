//! NDJSON stdio transport for ACP communication.
//!
//! Wraps a child process's stdin/stdout for newline-delimited JSON messaging.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tracing::{debug, warn};

use crate::acp::incoming::IncomingHandler;
use crate::acp::protocol::{AcpRequest, AcpResponse};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;

const MAX_NOTIFICATIONS: usize = 1024;

/// Cloneable handle to the child stdin writer.
///
/// Held inside the transport AND handed out as a `cancel_handle` so cancel
/// notifications can be written while another task holds the session mutex
/// for an in-flight prompt (see `acp::manager::SessionEntry`).
pub type SharedStdin = Arc<AsyncMutex<ChildStdin>>;

/// NDJSON stdio transport for communicating with an ACP child process.
///
/// Reads newline-delimited JSON from the child's stdout in a background task
/// and provides methods to send requests and receive responses.
pub struct StdioTransport {
    stdin: SharedStdin,
    event_rx: mpsc::Receiver<Result<AcpResponse>>,
    /// Services agent→client requests (`fs/*`, `session/request_permission`).
    /// `None` for transports created without a handler (e.g. unit tests) — in
    /// that case incoming requests fall through to the notification path,
    /// preserving the pre-bidirectional behavior.
    handler: Option<Arc<IncomingHandler>>,
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    /// Like [`new`](Self::new) but wires an [`IncomingHandler`] so agent→client
    /// requests received mid-prompt are answered instead of dropped.
    #[must_use]
    pub fn with_handler(
        stdin: ChildStdin,
        stdout: ChildStdout,
        handler: Option<Arc<IncomingHandler>>,
    ) -> Self {
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
                                "ACP stdout read error: {e}"
                            ))))
                            .await;
                        break;
                    }
                }
            }
        });

        Self {
            stdin: Arc::new(AsyncMutex::new(stdin)),
            event_rx: rx,
            handler,
            _reader_handle: handle,
        }
    }

    /// Hand out a shared writer to the child's stdin.
    ///
    /// Used by `AcpSession::cancel_handle()` so the manager can send
    /// `session/cancel` notifications without blocking on the per-session
    /// mutex held by an in-flight prompt.
    #[must_use]
    pub fn stdin_handle(&self) -> SharedStdin {
        Arc::clone(&self.stdin)
    }

    /// Serialize a request to JSON, append newline, write to stdin, and flush.
    pub async fn send(&self, req: &AcpRequest) -> Result<()> {
        write_request(&self.stdin, req).await
    }
}

/// Write a JSON-RPC request through a `SharedStdin`. Extracted so cancel
/// handles can reuse the same NDJSON framing without owning the transport.
pub async fn write_request(stdin: &SharedStdin, req: &AcpRequest) -> Result<()> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    let mut guard = stdin.lock().await;
    guard
        .write_all(line.as_bytes())
        .await
        .map_err(|e| AlephError::IoError(format!("ACP stdin write error: {e}")))?;
    guard
        .flush()
        .await
        .map_err(|e| AlephError::IoError(format!("ACP stdin flush error: {e}")))?;
    debug!("ACP sent: method={} id={}", req.method, req.id);
    Ok(())
}

/// Write a raw JSON-RPC value (a response to an agent→client request) through a
/// `SharedStdin` with NDJSON framing.
pub async fn write_value(stdin: &SharedStdin, value: &serde_json::Value) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    let mut guard = stdin.lock().await;
    guard
        .write_all(line.as_bytes())
        .await
        .map_err(|e| AlephError::IoError(format!("ACP stdin write error: {e}")))?;
    guard
        .flush()
        .await
        .map_err(|e| AlephError::IoError(format!("ACP stdin flush error: {e}")))?;
    Ok(())
}

impl StdioTransport {
    fn protocol_err(err: &crate::acp::protocol::AcpError) -> crate::error::AlephError {
        crate::acp::protocol::AcpOperationError::with_remote(
            crate::acp::protocol::AcpErrorCode::ProtocolError { code: err.code },
            format!("ACP error {}: {}", err.code, err.message),
            err.clone(),
        )
        .into()
    }

    fn session_dead_err() -> crate::error::AlephError {
        crate::acp::protocol::AcpOperationError::new(
            crate::acp::protocol::AcpErrorCode::SessionDead,
            "ACP connection closed while waiting for response",
        )
        .into()
    }

    fn timeout_err(timeout: Duration) -> crate::error::AlephError {
        crate::acp::protocol::AcpOperationError::new(
            crate::acp::protocol::AcpErrorCode::Timeout,
            format!("ACP request timed out after {timeout:?}"),
        )
        .into()
    }

    /// Answer an agent→client request via the configured [`IncomingHandler`],
    /// writing the JSON-RPC response back to the child's stdin. If no handler
    /// is wired the request is dropped (legacy half-duplex behavior).
    async fn dispatch_incoming(&self, resp: &AcpResponse) {
        let Some(handler) = self.handler.as_ref() else {
            debug!(method = ?resp.method, "ACP incoming request dropped (no handler)");
            return;
        };
        let Some((id, method, params)) = resp.as_incoming_request() else {
            return;
        };
        debug!(method, id, "ACP handling agent→client request");
        let outcome = handler.handle(method, params).await;
        let response = outcome.into_jsonrpc(id);
        if let Err(e) = write_value(&self.stdin, &response).await {
            warn!(method, "ACP incoming: failed to write response: {}", e);
        }
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
                        // Agent→client request (has both method + id): answer it
                        // and keep waiting. Checked BEFORE id matching because
                        // the agent's request-id namespace overlaps ours.
                        if resp.is_incoming_request() {
                            self.dispatch_incoming(&resp).await;
                            continue;
                        }
                        // Check if this is the response we're waiting for
                        if resp.id == Some(expected_id) {
                            // Check for ACP error
                            if let Some(ref err) = resp.error {
                                return Err(Self::protocol_err(err));
                            }
                            return Ok((resp, notifications));
                        }
                        // Otherwise it's a notification or response for a different id
                        if notifications.len() >= MAX_NOTIFICATIONS {
                            return Err(crate::acp::protocol::AcpOperationError::new(
                                crate::acp::protocol::AcpErrorCode::ProtocolError { code: -32603 },
                                "ACP notification buffer overflow: too many out-of-band messages",
                            )
                            .into());
                        }
                        notifications.push(resp);
                    }
                    Some(Err(e)) => return Err(e),
                    None => return Err(Self::session_dead_err()),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => {
                // Timeout: drain any stale responses so the next request
                // doesn't receive a mismatched id.
                while self.event_rx.try_recv().is_ok() {
                    debug!("ACP transport: drained stale response after timeout");
                }
                Err(Self::timeout_err(timeout))
            }
        }
    }

    /// Send a request and stream notifications via callback as they arrive.
    ///
    /// Unlike `request()`, notifications are forwarded immediately to the callback
    /// instead of being collected. Only the final response (matching id) is returned.
    ///
    /// Use this for `session/prompt` where real-time streaming matters.
    /// Use `request()` for `initialize` and `session/new` where it doesn't.
    pub async fn request_streaming(
        &mut self,
        req: &AcpRequest,
        timeout: Duration,
        on_notification: impl Fn(&AcpResponse),
    ) -> Result<AcpResponse> {
        self.send(req).await?;
        let expected_id = req.id;

        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.event_rx.recv().await {
                    Some(Ok(resp)) => {
                        // Agent→client request: answer it inline (this is the
                        // hot path — agents issue fs/permission requests while
                        // streaming a prompt turn).
                        if resp.is_incoming_request() {
                            self.dispatch_incoming(&resp).await;
                            continue;
                        }
                        if resp.id == Some(expected_id) {
                            if let Some(ref err) = resp.error {
                                return Err(Self::protocol_err(err));
                            }
                            return Ok(resp);
                        }
                        // Notification — forward immediately
                        on_notification(&resp);
                    }
                    Some(Err(e)) => return Err(e),
                    None => return Err(Self::session_dead_err()),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => {
                // Timeout: drain any stale responses so the next request
                // doesn't receive a mismatched id.
                while self.event_rx.try_recv().is_ok() {
                    debug!("ACP transport: drained stale response after timeout");
                }
                Err(Self::timeout_err(timeout))
            }
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
