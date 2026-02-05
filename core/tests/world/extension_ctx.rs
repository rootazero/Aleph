//! Extension system test context
//!
//! Contains state for extension plugin registry BDD tests.

use alephcore::extension::{
    DiagnosticLevel, ExtensionError, PluginManifest, PluginRegistry, RegistryStats, ServiceInfo,
    ServiceState,
};
use tempfile::TempDir;

/// Context for extension system tests
#[derive(Default)]
pub struct ExtensionContext {
    // ═══ Plugin Registry Fields ═══
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

    // ═══ V2 Manifest Parsing Fields ═══
    /// Temporary directory for test isolation
    pub temp_dir: Option<TempDir>,

    /// Raw TOML content for parsing
    pub toml_content: Option<String>,

    /// Raw JSON content for parsing
    pub json_content: Option<String>,

    /// Parsed manifest result
    pub manifest: Option<PluginManifest>,

    /// Parse error if parsing failed
    pub parse_error: Option<String>,

    // ═══ Plugin Runtime Fields ═══
    /// Flag indicating if manager was created (we don't store it due to Debug requirement)
    pub manager_created: bool,

    /// Flag indicating if loader was created
    pub loader_created: bool,

    /// Loaded plugin count for loader tests
    pub loaded_plugin_count: usize,

    /// Is any runtime active (for loader tests)
    pub any_runtime_active: bool,

    /// Is nodejs runtime active (for loader tests)
    pub nodejs_runtime_active: bool,

    /// Is wasm runtime active (for loader tests)
    pub wasm_runtime_active: bool,

    /// Tool execution result
    pub tool_result: Option<serde_json::Value>,

    /// Hook execution result
    pub hook_result: Option<serde_json::Value>,

    /// Last extension error
    pub extension_error: Option<ExtensionError>,

    // ═══ Service Serialization Fields ═══
    /// ServiceInfo for serialization tests
    pub service_info: Option<ServiceInfo>,

    /// ServiceState for serialization tests
    pub service_state: Option<ServiceState>,

    /// Serialized JSON string
    pub serialized_json: Option<String>,
}

// Manual Debug impl to avoid requiring Debug on ExtensionManager/PluginLoader
impl std::fmt::Debug for ExtensionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionContext")
            .field("registry", &self.registry.is_some())
            .field("plugin_count", &self.plugin_count)
            .field("active_count", &self.active_count)
            .field("tool_count", &self.tool_count)
            .field("hook_count", &self.hook_count)
            .field("temp_dir", &self.temp_dir.as_ref().map(|t| t.path()))
            .field("manifest", &self.manifest.is_some())
            .field("parse_error", &self.parse_error)
            .field("manager_created", &self.manager_created)
            .field("loader_created", &self.loader_created)
            .field("extension_error", &self.extension_error)
            .finish()
    }
}
