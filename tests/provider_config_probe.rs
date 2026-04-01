//! Layer 1: Rust logic probes for provider configuration.
//!
//! Pure logic tests — serialization, config struct manipulation, edge cases.
//! No handler calls, no file I/O, no server startup.

mod provider_config_probe {
    pub mod config_crud_tests;
    pub mod edge_case_tests;
    pub mod serde_tests;
}
