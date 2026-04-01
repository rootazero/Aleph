//! ACP (Agent Client Protocol) probe tests.
//!
//! Integration tests for ACP harness management, configuration,
//! lifecycle, RPC handlers, and tool execution.

#[allow(dead_code)]
mod acp_probe {
    pub mod mock_harness;
    pub mod p1_config_and_presets;
    pub mod p2_manager_lifecycle;
    pub mod p3_custom_harness;
    pub mod p4_rpc_handlers;
    pub mod p5_tool_execution;
    pub mod p6_error_paths;
    pub mod p7_rpc_server_probe;
}
