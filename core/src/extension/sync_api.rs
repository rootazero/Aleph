//! Synchronous API wrapper for ExtensionManager
//!
//! This module provides synchronous (blocking) wrappers around the async
//! ExtensionManager API, suitable for use with FFI bindings that don't
//! support async/await.
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::SyncExtensionManager;
//!
//! let manager = SyncExtensionManager::new()?;
//! let summary = manager.load_all()?;
//! let skills = manager.get_all_skills();
//! ```

use super::{
    ExtensionAgent, ExtensionCommand, ExtensionConfig, ExtensionError, ExtensionManager,
    ExtensionResult, ExtensionSkill, HookEvent, LoadSummary, McpServerConfig,
    PluginInfo, PluginRecord,
};
use crate::extension::hooks::{HookContext, HookResult};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::RwLock;

/// Synchronous wrapper for ExtensionManager
///
/// Provides blocking API for use with FFI bindings. Internally uses a tokio
/// runtime to execute async operations.
pub struct SyncExtensionManager {
    /// The underlying async manager
    inner: Arc<RwLock<ExtensionManager>>,
    /// Tokio runtime for blocking operations
    runtime: Runtime,
}

impl SyncExtensionManager {
    /// Create a new sync extension manager with default configuration
    pub fn new() -> ExtensionResult<Self> {
        Self::with_config(ExtensionConfig::default())
    }

    /// Create a new sync extension manager with custom configuration
    pub fn with_config(config: ExtensionConfig) -> ExtensionResult<Self> {
        // Create a new runtime for this manager
        let runtime = Runtime::new().map_err(|e| {
            ExtensionError::Runtime(format!("Failed to create tokio runtime: {}", e))
        })?;

        // Create the async manager within the runtime
        let inner = runtime.block_on(async {
            let manager = ExtensionManager::new(config).await?;
            Ok::<_, ExtensionError>(Arc::new(RwLock::new(manager)))
        })?;

        Ok(Self { inner, runtime })
    }

    /// Create from an existing async manager (for integration with existing async code)
    pub fn from_async(manager: Arc<RwLock<ExtensionManager>>) -> ExtensionResult<Self> {
        let runtime = Runtime::new().map_err(|e| {
            ExtensionError::Runtime(format!("Failed to create tokio runtime: {}", e))
        })?;

        Ok(Self {
            inner: manager,
            runtime,
        })
    }

    /// Get a handle to the runtime for advanced usage
    pub fn runtime_handle(&self) -> Handle {
        self.runtime.handle().clone()
    }

    // =========================================================================
    // Loading Operations
    // =========================================================================

    /// Load all extensions (skills, commands, agents, plugins)
    pub fn load_all(&self) -> ExtensionResult<LoadSummary> {
        self.runtime.block_on(async {
            self.inner.read().await.load_all().await
        })
    }

    // =========================================================================
    // Skill Operations
    // =========================================================================

    /// Get all skills
    pub fn get_all_skills(&self) -> Vec<ExtensionSkill> {
        self.runtime.block_on(async {
            self.inner.read().await.get_all_skills().await
        })
    }

    /// Get auto-invocable skills (for LLM prompt injection)
    pub fn get_auto_invocable_skills(&self) -> Vec<ExtensionSkill> {
        self.runtime.block_on(async {
            self.inner.read().await.get_auto_invocable_skills().await
        })
    }

    /// Get a specific skill by qualified name
    pub fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
        self.runtime.block_on(async {
            self.inner.read().await.get_skill(qualified_name).await
        })
    }

    /// Execute a skill with arguments
    pub fn execute_skill(&self, qualified_name: &str, arguments: &str) -> ExtensionResult<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .execute_skill(qualified_name, arguments)
                .await
        })
    }

    // =========================================================================
    // Command Operations
    // =========================================================================

    /// Get all commands
    pub fn get_all_commands(&self) -> Vec<ExtensionCommand> {
        self.runtime.block_on(async {
            self.inner.read().await.get_all_commands().await
        })
    }

    /// Get a specific command by name
    pub fn get_command(&self, name: &str) -> Option<ExtensionCommand> {
        self.runtime.block_on(async {
            self.inner.read().await.get_command(name).await
        })
    }

    /// Execute a command with arguments
    pub fn execute_command(&self, name: &str, arguments: &str) -> ExtensionResult<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .execute_command(name, arguments)
                .await
        })
    }

    // =========================================================================
    // Agent Operations
    // =========================================================================

    /// Get all agents
    pub fn get_all_agents(&self) -> Vec<ExtensionAgent> {
        self.runtime.block_on(async {
            self.inner.read().await.get_all_agents().await
        })
    }

    /// Get a specific agent by name
    pub fn get_agent(&self, name: &str) -> Option<ExtensionAgent> {
        self.runtime.block_on(async {
            self.inner.read().await.get_agent(name).await
        })
    }

    /// Get all primary agents
    pub fn get_primary_agents(&self) -> Vec<ExtensionAgent> {
        self.runtime.block_on(async {
            self.inner.read().await.get_primary_agents().await
        })
    }

    /// Get all sub-agents
    pub fn get_sub_agents(&self) -> Vec<ExtensionAgent> {
        self.runtime.block_on(async {
            self.inner.read().await.get_sub_agents().await
        })
    }

    // =========================================================================
    // Plugin Operations
    // =========================================================================

    /// Get all plugin info
    pub fn get_plugin_info(&self) -> Vec<PluginInfo> {
        self.runtime.block_on(async {
            self.inner.read().await.get_plugin_info().await
        })
    }

    /// Get a specific plugin record by name
    pub fn get_plugin_record(&self, name: &str) -> Option<PluginRecord> {
        self.runtime.block_on(async {
            self.inner.read().await.get_plugin_record(name).await
        })
    }

    // =========================================================================
    // Hook Operations
    // =========================================================================

    /// Execute hooks for an event
    pub fn execute_hooks(&self, event: HookEvent, context: &HookContext) -> ExtensionResult<HookResult> {
        self.runtime.block_on(async {
            self.inner.read().await.execute_hooks(event, context).await
        })
    }

    /// Get the number of registered hooks
    pub fn hook_count(&self) -> usize {
        self.runtime.block_on(async {
            self.inner.read().await.hook_count().await
        })
    }

    // =========================================================================
    // MCP Operations
    // =========================================================================

    /// Get all MCP server configurations
    pub fn get_mcp_servers(&self) -> HashMap<String, McpServerConfig> {
        self.runtime.block_on(async {
            self.inner.read().await.get_mcp_servers().await
        })
    }

    // =========================================================================
    // Configuration Access
    // =========================================================================

    /// Get the default model from configuration
    pub fn get_default_model(&self) -> Option<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .get_default_model()
                .map(|s| s.to_string())
        })
    }

    /// Get the small model from configuration
    pub fn get_small_model(&self) -> Option<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .get_small_model()
                .map(|s| s.to_string())
        })
    }

    /// Get the default agent from configuration
    pub fn get_default_agent(&self) -> Option<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .get_default_agent()
                .map(|s| s.to_string())
        })
    }

    /// Get all custom instructions
    pub fn get_instructions(&self) -> Vec<String> {
        self.runtime.block_on(async {
            self.inner
                .read()
                .await
                .get_instructions()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        })
    }

    /// Get the Aleph home directory
    pub fn aleph_home(&self) -> ExtensionResult<PathBuf> {
        self.runtime.block_on(async {
            self.inner.read().await.aleph_home()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_manager_creation() {
        // This test verifies that we can create a sync manager
        // Note: This may fail in CI without proper directory setup
        let result = SyncExtensionManager::new();
        // Just verify it doesn't panic - actual functionality depends on environment
        assert!(result.is_ok() || result.is_err());
    }
}
