//! MCP Registrar — collect-then-batch capability registration for MCP plugins

/// Collects capability declarations from an MCP plugin's initialization
/// and registers them in a single batch via `CapabilityApi`.
pub struct McpRegistrar {
    /// The plugin ID this registrar acts on behalf of
    pub plugin_id: String,
}
