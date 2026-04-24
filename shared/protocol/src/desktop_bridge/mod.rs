//! Desktop Bridge JSON-RPC 2.0 protocol types.
//!
//! Shared between Rust Core (`SwiftBridge` client) and the Swift `AlephBridge`
//! helper subprocess. Rust side is the single source of truth; Swift handwrites
//! matching `Codable` structs validated by golden fixtures (see Task 0.10).

pub mod envelope;
pub mod errors;
pub mod methods;

pub use envelope::{ErrorResponse, Message, Notification, Request, Response, RpcError};
pub use envelope::{BridgeErrorResponse, BridgeRequest, BridgeRpcError, BridgeSuccessResponse};
pub use errors::*;

// Socket path retained for one release of backward compatibility with any
// caller still referencing it; removed in Stage 6 (the helper now uses stdio).
pub fn default_socket_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".aleph").join("bridge.sock")
}
