//! Built-in Tool Registration Methods
//!
//! This module provides builder methods for registering built-in tools
//! during AlephToolServer construction.
//!
//! Note: Most built-in tools require complex configuration and dependencies.
//! For production use, prefer using `executor::builtin_registry::create_tool_boxed()`
//! which handles all configuration automatically.
//!
//! These methods are provided for simple cases and testing.

use super::server::AlephToolServer;
use crate::builtin_tools::*;

impl AlephToolServer {
    /// Register the bash execution tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_bash();
    /// ```
    pub fn with_bash(self) -> Self {
        self.tool(BashExecTool::new())
    }

    /// Register the file operations tool
    ///
    /// This includes: read, write, edit, move, delete, and other file operations.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_file_ops();
    /// ```
    pub fn with_file_ops(self) -> Self {
        self.tool(FileOpsTool::new())
    }

    /// Register the web search tool
    ///
    /// Uses TAVILY_API_KEY from environment if not provided.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_search();
    /// ```
    pub fn with_search(self) -> Self {
        self.tool(SearchTool::new())
    }

    /// Register the web fetch tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_web_fetch();
    /// ```
    pub fn with_web_fetch(self) -> Self {
        self.tool(WebFetchTool::new())
    }

    /// Register the YouTube tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_youtube();
    /// ```
    pub fn with_youtube(self) -> Self {
        self.tool(YouTubeTool::new())
    }

    /// Register the code execution tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_code_exec();
    /// ```
    pub fn with_code_exec(self) -> Self {
        self.tool(CodeExecTool::new())
    }

    /// Register the PDF generation tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_pdf_generate();
    /// ```
    pub fn with_pdf_generate(self) -> Self {
        self.tool(PdfGenerateTool::new())
    }

    // Advanced tools with dependencies - these require parameters

    /// Register the atomic operations tool
    ///
    /// Provides atomic search, replace, and move operations powered by Atomic Engine.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::path::PathBuf;
    /// let server = AlephToolServer::new()
    ///     .with_atomic_ops(PathBuf::from("/workspace"));
    /// ```
    pub fn with_atomic_ops(self, workspace_root: std::path::PathBuf) -> Self {
        self.tool(AtomicOpsTool::new(workspace_root))
    }

    /// Register the invalid tool (fallback handler)
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_invalid(vec!["search".to_string(), "web_fetch".to_string()]);
    /// ```
    pub fn with_invalid(self, available_tools: Vec<String>) -> Self {
        self.tool(InvalidTool::new(available_tools))
    }

    /// Register the memory search tool
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// let server = AlephToolServer::new()
    ///     .with_memory_search(database);
    /// ```
    pub fn with_memory_search(
        self,
        database: crate::memory::store::MemoryBackend,
        embedder: std::sync::Arc<dyn crate::memory::EmbeddingProvider>,
    ) -> Self {
        self.tool(MemorySearchTool::new_with_embedder(database, embedder))
    }

    /// Register MCP resource reading tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_mcp_read_resource(mcp_handle);
    /// ```
    pub fn with_mcp_read_resource(
        self,
        mcp_handle: crate::mcp::manager::McpManagerHandle,
    ) -> Self {
        self.tool(McpReadResourceTool::new(mcp_handle))
    }

    /// Register MCP prompt retrieval tool
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_mcp_get_prompt(mcp_handle);
    /// ```
    pub fn with_mcp_get_prompt(
        self,
        mcp_handle: crate::mcp::manager::McpManagerHandle,
    ) -> Self {
        self.tool(McpGetPromptTool::new(mcp_handle))
    }

    /// Register the browser automation tool (Chromium via CDP).
    ///
    /// The tool manages its own browser lifecycle. When no browser is running,
    /// action calls return a friendly message instead of an error.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_browser();
    /// ```
    pub fn with_browser(self) -> Self {
        self.tool(BrowserTool::new())
    }

    /// Register the desktop bridge tool (requires macOS App running).
    ///
    /// When the macOS App is not running, all tool calls return a friendly
    /// message instead of an error, allowing the agent to degrade gracefully.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = AlephToolServer::new()
    ///     .with_desktop();
    /// ```
    pub fn with_desktop(self) -> Self {
        self.tool(DesktopTool::new())
    }

    /// Register the config read tool (read-only config inspection).
    ///
    /// Allows the LLM to read current configuration with sensitive fields masked.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use tokio::sync::RwLock;
    /// use alephcore::config::Config;
    ///
    /// let config = Arc::new(RwLock::new(Config::default()));
    /// let server = AlephToolServer::new()
    ///     .with_config_read(config);
    /// ```
    pub fn with_config_read(self, config: std::sync::Arc<tokio::sync::RwLock<crate::config::Config>>) -> Self {
        self.tool(ConfigReadTool::new(config))
    }

    /// Register the config update tool (write with validation + secret vault).
    ///
    /// Allows the LLM to update configuration. Requires user confirmation.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use alephcore::config::ConfigPatcher;
    ///
    /// let patcher = Arc::new(patcher);
    /// let server = AlephToolServer::new()
    ///     .with_config_update(patcher);
    /// ```
    pub fn with_config_update(self, patcher: std::sync::Arc<crate::config::ConfigPatcher>) -> Self {
        self.tool(ConfigUpdateTool::new(patcher))
    }

    /// Register the vision tool (image understanding + OCR).
    ///
    /// Requires a [`VisionPipeline`](crate::vision::VisionPipeline) configured
    /// with one or more providers (e.g. Claude Vision, Platform OCR).
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use alephcore::vision::VisionPipeline;
    ///
    /// let pipeline = Arc::new(VisionPipeline::new());
    /// let server = AlephToolServer::new()
    ///     .with_vision(pipeline);
    /// ```
    pub fn with_vision(self, pipeline: std::sync::Arc<crate::vision::VisionPipeline>) -> Self {
        self.tool(VisionTool::new(pipeline))
    }
}

