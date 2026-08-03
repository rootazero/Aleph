use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum ConnectionError {
    #[error("Failed to connect: {0}")]
    ConnectFailed(String),
    /// The socket fired `error`/`close` *before* reaching OPEN, as opposed to
    /// going silent. Something reacted — a `403`/`426`/`503` on the upgrade, or
    /// a TCP refusal because nothing is listening.
    ///
    /// This variant deliberately does **not** say which: browsers withhold the
    /// upgrade's HTTP status from script and report both cases identically
    /// (`error` + `close(1006)`, empty reason). Deciding between them needs an
    /// independent probe of the origin and is the classifier's job — see
    /// `failure::OriginLiveness`. Keep this message factual so it cannot
    /// contradict the verdict rendered above it.
    #[error("Connection failed before open: {0}")]
    FailedBeforeOpen(String),
    #[error("Connection lost: {0}")]
    ConnectionLost(String),
    #[error("Send failed: {0}")]
    SendFailed(String),
    #[error("Receive failed: {0}")]
    ReceiveFailed(String),
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
