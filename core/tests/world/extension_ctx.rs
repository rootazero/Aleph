//! Extension system test context
//!
//! Contains state for extension plugin registry BDD tests.

use alephcore::extension::{DiagnosticLevel, PluginRegistry, RegistryStats};

/// Context for extension system tests
#[derive(Debug, Default)]
pub struct ExtensionContext {
    /// The plugin registry under test
    pub registry: Option<PluginRegistry>,

    /// Cached plugin count
    pub plugin_count: usize,

    /// Cached active plugin count
    pub active_count: usize,

    /// Cached tool count
    pub tool_count: usize,

    /// Cached hook count
    pub hook_count: usize,

    /// Cached channel count
    pub channel_count: usize,

    /// Cached provider count
    pub provider_count: usize,

    /// Cached gateway method count
    pub gateway_method_count: usize,

    /// Cached http route count
    pub http_route_count: usize,

    /// Cached http handler count
    pub http_handler_count: usize,

    /// Cached CLI command count
    pub cli_command_count: usize,

    /// Cached service count
    pub service_count: usize,

    /// Cached in-chat command count
    pub command_count: usize,

    /// Cached diagnostic count
    pub diagnostic_count: usize,

    /// Registry statistics snapshot
    pub stats: Option<RegistryStats>,

    /// Last operation success flag
    pub last_op_success: bool,

    /// Expected diagnostic level for verification
    pub expected_diagnostic_level: Option<DiagnosticLevel>,
}
