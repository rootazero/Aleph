//! Native WebSocket connector implementation using tokio-tungstenite

use super::connector::{AlephConnector, ConnectionError};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::pin::Pin;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Native WebSocket connector using tokio-tungstenite
///
/// This implementation uses Tokio's async runtime and tokio-tungstenite
/// for WebSocket communication. It's suitable for native environments
/// (desktop, server, CLI).
///
/// ## Architecture
///
/// The connector spawns a background task that manages the WebSocket stream.
/// Communication with this task happens through channels:
/// - Send channel: for outgoing messages
/// - Receive channel: for incoming messages
///
/// ## Features
///
/// - Async/await based
/// - Automatic message serialization/deserialization
/// - Stream-based message receiving
/// - Thread-safe (Send + Sync)
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::connection::NativeConnector;
///
/// #[tokio::main]
/// async fn main() {
///     let mut connector = NativeConnector::new();
///     connector.connect("ws://127.0.0.1:18789").await.unwrap();
///
///     // Send message
///     connector.send(json!({"type": "req"})).await.unwrap();
///
///     // Receive messages
///     let mut stream = connector.receive();
///     while let Some(Ok(msg)) = stream.next().await {
///         println!("Received: {:?}", msg);
///     }
/// }
/// ```
pub struct NativeConnector {
    send_tx: Option<mpsc::UnboundedSender<Value>>,
    recv_rx: Option<mpsc::UnboundedReceiver<Result<Value, ConnectionError>>>,
    is_connected: bool,
}

impl NativeConnector {
    /// Create a new native connector
    pub fn new() -> Self {
        Self {
            send_tx: None,
            recv_rx: None,
            is_connected: false,
        }
    }

    /// Spawn a task to manage the WebSocket stream
    ///
    /// This task handles both sending and receiving messages.
    fn spawn_manager_task(
        ws_stream: WsStream,
        mut send_rx: mpsc::UnboundedReceiver<Value>,
        recv_tx: mpsc::UnboundedSender<Result<Value, ConnectionError>>,
    ) {
        use futures::SinkExt;

        tokio::task::spawn(async move {
            let (mut write, mut read) = ws_stream.split();

            loop {
                tokio::select! {
                    // Handle outgoing messages
                    Some(value) = send_rx.recv() => {
                        // Serialize and send
                        match serde_json::to_string(&value) {
                            Ok(text) => {
                                if let Err(e) = write.send(Message::Text(text)).await {
                                    let _ = recv_tx.send(Err(ConnectionError::SendFailed(e.to_string())));
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = recv_tx.send(Err(ConnectionError::Serialization(e)));
                            }
                        }
                    }

                    // Handle incoming messages
                    Some(msg_result) = read.next() => {
                        match msg_result {
                            Ok(Message::Text(text)) => {
                                // Parse JSON message
                                match serde_json::from_str::<Value>(&text) {
                                    Ok(value) => {
                                        if recv_tx.send(Ok(value)).is_err() {
                                            // Receiver dropped, stop task
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = recv_tx.send(Err(ConnectionError::Serialization(e)));
                                    }
                                }
                            }
                            Ok(Message::Binary(data)) => {
                                // Try to parse binary as JSON
                                match serde_json::from_slice::<Value>(&data) {
                                    Ok(value) => {
                                        if recv_tx.send(Ok(value)).is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = recv_tx.send(Err(ConnectionError::Serialization(e)));
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => {
                                // Connection closed
                                let _ = recv_tx.send(Err(ConnectionError::ConnectionFailed(
                                    "Connection closed by server".to_string(),
                                )));
                                break;
                            }
                            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {
                                // Ignore ping/pong/frame messages
                                continue;
                            }
                            Err(e) => {
                                let _ = recv_tx.send(Err(ConnectionError::ReceiveFailed(e.to_string())));
                                break;
                            }
                        }
                    }

                    // Both channels closed
                    else => break,
                }
            }
        });
    }
}

impl Default for NativeConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl AlephConnector for NativeConnector {
    fn connect(
        &mut self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>> {
        let url = url.to_string(); // Clone URL to avoid lifetime issues
        Box::pin(async move {
            // Connect to WebSocket
            let (ws_stream, _) = connect_async(&url)
                .await
                .map_err(|e| ConnectionError::ConnectionFailed(e.to_string()))?;

            // Create channels
            let (send_tx, send_rx) = mpsc::unbounded_channel();
            let (recv_tx, recv_rx) = mpsc::unbounded_channel();

            // Spawn manager task
            Self::spawn_manager_task(ws_stream, send_rx, recv_tx);

            // Store channels
            self.send_tx = Some(send_tx);
            self.recv_rx = Some(recv_rx);
            self.is_connected = true;

            Ok(())
        })
    }

    fn disconnect(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>> {
        Box::pin(async move {
            // Drop the channels (this will cause the manager task to exit)
            self.send_tx = None;
            self.recv_rx = None;
            self.is_connected = false;

            Ok(())
        })
    }

    fn send(
        &mut self,
        message: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>> {
        Box::pin(async move {
            if let Some(send_tx) = &self.send_tx {
                send_tx
                    .send(message)
                    .map_err(|_| ConnectionError::SendFailed("Channel closed".to_string()))?;
                Ok(())
            } else {
                Err(ConnectionError::NotConnected)
            }
        })
    }

    fn receive(&mut self) -> Pin<Box<dyn Stream<Item = Result<Value, ConnectionError>> + '_>> {
        if let Some(recv_rx) = self.recv_rx.take() {
            Box::pin(futures::stream::unfold(recv_rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            }))
        } else {
            // Return empty stream if not connected
            Box::pin(futures::stream::empty())
        }
    }

    fn is_connected(&self) -> bool {
        self.is_connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_connector() {
        let connector = NativeConnector::new();
        assert!(!connector.is_connected());
    }

    #[test]
    fn test_default() {
        let connector = NativeConnector::default();
        assert!(!connector.is_connected());
    }

    // Note: Integration tests with actual WebSocket server
    // should be in tests/ directory
}
