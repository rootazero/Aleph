//! Layer 2: RPC integration probe tests for provider configuration.
//!
//! Spawns a real Aleph server process and exercises the providers.*
//! JSON-RPC endpoints over WebSocket. Validates end-to-end behavior
//! including config persistence, error paths, and concurrent access.

mod provider_rpc_probe {
    pub mod harness;
    pub mod endpoint_tests;
    pub mod error_tests;
    pub mod robustness_tests;
}
