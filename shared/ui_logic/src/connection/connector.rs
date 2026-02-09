use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use futures::Stream;
use std::pin::Pin;

#[derive(Error, Debug, Clone)]
pub enum ConnectionError {
    #[error("Failed to connect: {0}")]
    ConnectFailed(String),
    #[error("Connection lost: {0}")]
    ConnectionLost(String),
    #[error("Send failed: {0}")]
    SendFailed(String),
    #[error("Receive failed: {0}")]
    ReceiveFailed(String),
    #[error("Url parse error: {0}")]
    UrlError(String),
}

#[async_trait(?Send)]
pub trait AlephConnector {
    /// Connect to the gateway
    async fn connect(&mut self, url: &str) -> Result<(), ConnectionError>;

    /// Disconnect from the gateway
    async fn disconnect(&mut self) -> Result<(), ConnectionError>;

    /// Send a message
    async fn send(&mut self, message: Value) -> Result<(), ConnectionError>;

    /// Receive messages as a stream
    fn receive(&mut self) -> Pin<Box<dyn Stream<Item = Result<Value, ConnectionError>>>>;

    /// Check if connected
    fn is_connected(&self) -> bool;
}
