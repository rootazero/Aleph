//! Built-in tools configuration and creation
//!
//! This module creates tool server instances using the unified builtin registry.
//! All tool definitions come from rig_tools::builtin_registry for consistency.

use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolsConfig,
};
use crate::tools::{AetherToolServer, AetherToolServerHandle};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Built-in tool names
pub const BUILTIN_TOOLS: &[&str] = &[
    "search",
    "web_fetch",
    "youtube",
    "file_ops",
    "bash",
    "code_exec",
    "generate_image",
    "pdf_generate",
];

/// Configuration for built-in tools
///
/// DEPRECATED: Use rig_tools::builtin_registry::BuiltinToolsConfig instead.
/// This type is kept for backward compatibility.
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation
    /// Wrapped in Arc<RwLock<>> for thread-safe access
    pub generation_registry: Option<Arc<RwLock<GenerationProviderRegistry>>>,
}

impl std::fmt::Debug for BuiltinToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinToolConfig")
            .field(
                "tavily_api_key",
                &self.tavily_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "generation_registry",
                &self
                    .generation_registry
                    .as_ref()
                    .map(|_| "[GenerationProviderRegistry]"),
            )
            .finish()
    }
}

/// Create a tool server handle with built-in tools
///
/// This function uses the unified builtin registry to ensure consistency
/// with BuiltinToolRegistry used by Agent Loop.
///
/// Returns an `AetherToolServerHandle` that can be shared across threads.
pub fn create_builtin_tool_server(config: Option<&BuiltinToolConfig>) -> AetherToolServerHandle {
    // Convert to unified config
    let unified_config = config.map(|cfg| BuiltinToolsConfig {
        tavily_api_key: cfg.tavily_api_key.clone(),
        generation_registry: cfg.generation_registry.clone(),
    });

    let mut server = AetherToolServer::new();

    // Register all builtin tools from unified registry
    for name in get_builtin_tool_names() {
        if let Some(tool) = create_tool_boxed(&name, unified_config.as_ref()) {
            server = server.tool_boxed(tool);
            info!(tool = name, "Registered builtin tool in AetherToolServer");
        } else {
            // Tool requires config that wasn't provided (e.g., generate_image needs registry)
            warn!(
                tool = name,
                "Skipped builtin tool registration (missing required config)"
            );
        }
    }

    server.handle()
}

/// Create initial registered tools list
///
/// This function uses the unified builtin registry.
pub fn create_builtin_tools_list() -> Vec<String> {
    get_builtin_tool_names()
}
