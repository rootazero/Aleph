//! Desktop Bridge JSON-RPC 2.0 protocol types.
//!
//! Shared between Rust Core (`SwiftBridge` client) and the Swift `AlephBridge`
//! helper subprocess. Rust side is the single source of truth; Swift handwrites
//! matching `Codable` structs validated by golden fixtures (see Task 0.10).

pub mod envelope;
pub mod errors;
pub mod methods;

pub use envelope::{ErrorResponse, Message, Notification, Request, Response, RpcError};
pub use errors::*;
