//! WebSocket transport layer
//!
//! Handles low-level WebSocket connection management, message I/O,
//! and connection state tracking.

use crate::{ClientError, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

#[cfg(feature = "tracing")]
use tracing::{debug, error, info};

/// Type alias for WebSocket stream
pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Type alias for WebSocket write half
pub type WsWriter = Arc<Mutex<futures_util::stream::SplitSink<WsStream, Message>>>;

/// Type alias for WebSocket read half
pub type WsReader = futures_util::stream::SplitStream<WsStream>;

/// WebSocket connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// WebSocket transport
pub struct Transport {
    url: String,
    state: Arc<std::sync::atomic::AtomicU8>,
}

impl Transport {
    /// Create a new transport instance
    pub fn new(url: String) -> Self {
        #[cfg(feature = "tracing")]
        debug!("Creating transport for {}", url);

        Self {
            url,
            state: Arc::new(std::sync::atomic::AtomicU8::new(
                ConnectionState::Disconnected as u8,
            )),
        }
    }

    /// Get the WebSocket URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get current connection state
    pub fn state(&self) -> ConnectionState {
        match self
            .state
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Reconnecting,
            _ => ConnectionState::Disconnected,
        }
    }

    /// Set connection state
    fn set_state(&self, state: ConnectionState) {
        self.state
            .store(state as u8, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.state() == ConnectionState::Connected
    }

    /// Connect to WebSocket server
    ///
    /// Returns split read and write halves
    pub async fn connect(&self) -> Result<(WsWriter, WsReader)> {
        #[cfg(feature = "tracing")]
        info!("Connecting to {}", self.url);

        self.set_state(ConnectionState::Connecting);

        let (ws_stream, _) = connect_async(&self.url)
            .await
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;

        let (write, read) = ws_stream.split();

        self.set_state(ConnectionState::Connected);

        #[cfg(feature = "tracing")]
        info!("Connected to Gateway");

        Ok((Arc::new(Mutex::new(write)), read))
    }

    /// Send a text message
    pub async fn send(&self, writer: &WsWriter, text: String) -> Result<()> {
        let mut write = writer.lock().await;
        write
            .send(Message::Text(text))
            .await
            .map_err(|e| ClientError::WebSocketError(e.to_string()))?;
        Ok(())
    }

    /// Close the connection
    pub async fn close(&self, writer: &WsWriter) -> Result<()> {
        #[cfg(feature = "tracing")]
        info!("Closing WebSocket connection");

        let mut write = writer.lock().await;
        write
            .send(Message::Close(None))
            .await
            .map_err(|e| ClientError::WebSocketError(e.to_string()))?;

        self.set_state(ConnectionState::Disconnected);
        Ok(())
    }
}

/// Message type for read loop
#[derive(Debug)]
pub enum TransportMessage {
    /// Text message received
    Text(String),
    /// Connection closed by server
    Close,
    /// Ping received (auto-responded with Pong by tungstenite)
    Ping,
    /// WebSocket error
    Error(String),
}

/// Read messages from WebSocket stream
///
/// This is a utility function that can be called in a spawned task
pub async fn read_messages(mut reader: WsReader) -> Vec<TransportMessage> {
    let mut messages = Vec::new();

    while let Some(msg) = reader.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                #[cfg(feature = "tracing")]
                debug!("Received text message: {} bytes", text.len());
                messages.push(TransportMessage::Text(text));
            }
            Ok(Message::Close(_)) => {
                #[cfg(feature = "tracing")]
                info!("Server closed connection");
                messages.push(TransportMessage::Close);
                break;
            }
            Ok(Message::Ping(_)) => {
                #[cfg(feature = "tracing")]
                debug!("Received ping");
                messages.push(TransportMessage::Ping);
            }
            Err(e) => {
                #[cfg(feature = "tracing")]
                error!("WebSocket error: {}", e);
                messages.push(TransportMessage::Error(e.to_string()));
                break;
            }
            _ => {}
        }
    }

    messages
}
