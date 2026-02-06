//! JSON-RPC 2.0 client implementation
//!
//! Handles request/response matching and RPC protocol details.

use crate::{ClientError, Result};
use serde_json::Value;

#[cfg(feature = "tracing")]
use tracing::debug;

/// JSON-RPC client
pub struct RpcClient {
    // TODO: Add request tracking and response matching
}

impl RpcClient {
    /// Create a new RPC client
    pub fn new() -> Self {
        #[cfg(feature = "tracing")]
        debug!("Creating RPC client");

        Self {}
    }

    // TODO: Add call(), send_request(), handle_response() methods
}

impl Default for RpcClient {
    fn default() -> Self {
        Self::new()
    }
}
