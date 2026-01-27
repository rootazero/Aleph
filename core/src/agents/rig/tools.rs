//! Built-in tools configuration and creation

use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::{
    BashExecTool, CodeExecTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool, SearchTool,
    WebFetchTool, YouTubeTool,
};
use crate::tools::{AetherToolServer, AetherToolServerHandle};
use std::sync::{Arc, RwLock};
use tracing::info;

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
/// Returns an `AetherToolServerHandle` that can be shared across threads.
pub fn create_builtin_tool_server(config: Option<&BuiltinToolConfig>) -> AetherToolServerHandle {
    let search_tool = if let Some(cfg) = config {
        SearchTool::with_api_key(cfg.tavily_api_key.clone())
    } else {
        SearchTool::new()
    };

    let mut server = AetherToolServer::new()
        .tool(search_tool)
        .tool(WebFetchTool::new())
        .tool(YouTubeTool::new())
        .tool(FileOpsTool::new())
        .tool(BashExecTool::new())
        .tool(CodeExecTool::new())
        .tool(PdfGenerateTool::new());

    // Add image generation tool if generation registry is available
    if let Some(cfg) = config {
        if let Some(ref registry) = cfg.generation_registry {
            // Log the number of providers in the registry
            let provider_count = registry.read().map(|r| r.len()).unwrap_or(0);
            info!(
                provider_count = provider_count,
                "ImageGenerateTool registered with generation provider registry"
            );
            server = server.tool(ImageGenerateTool::new(Arc::clone(registry)));
        } else {
            info!("No generation_registry provided, ImageGenerateTool not registered");
        }
    } else {
        info!("No builtin tool config provided, using default tools only");
    }

    server.handle()
}

/// Create initial registered tools list
pub fn create_builtin_tools_list() -> Vec<String> {
    BUILTIN_TOOLS.iter().map(|s| s.to_string()).collect()
}
