//! Shell Tools Module
//!
//! Provides native AgentTool implementations for shell command execution.
//! All tools share a common `ShellContext` for security configuration.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `shell_exec` | Execute shell command | Yes (always) |
//!
//! # Security
//!
//! Shell tools are security-sensitive and implement multiple layers of protection:
//!
//! 1. **Disabled by default**: Shell execution must be explicitly enabled
//! 2. **Command whitelist**: Only whitelisted commands can execute
//! 3. **Command blacklist**: Dangerous commands are always blocked
//! 4. **Directory restrictions**: Limit where commands can run
//! 5. **Timeout protection**: Prevent runaway processes
//! 6. **Confirmation required**: User must approve every execution
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::shell::{ShellConfig, ShellContext, ShellExecuteTool};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create context with allowed commands
//! let config = ShellConfig::with_allowed_commands(vec![
//!     "echo".to_string(),
//!     "ls".to_string(),
//!     "cat".to_string(),
//! ]);
//! let ctx = ShellContext::new(config);
//!
//! // Register tool
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(ShellExecuteTool::new(ctx))).await;
//!
//! // Execute (with confirmation)
//! let result = registry.execute("shell_exec", r#"{"command": "echo hello"}"#).await?;
//! ```
//!
//! # Security Recommendations
//!
//! - Never use `ShellConfig::allow_all()` in production
//! - Keep the allowed commands list as small as possible
//! - Always validate user input before passing to shell
//! - Consider using directory restrictions for additional security

mod config;
mod execute;

pub use config::{ShellConfig, ShellContext};
pub use execute::ShellExecuteTool;

use std::sync::Arc;

use super::AgentTool;

/// Create all shell tools with shared context
///
/// Convenience function to create all shell tools at once.
///
/// # Arguments
///
/// * `config` - Shell security configuration
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations.
/// Note: Returns an empty vector if shell is disabled in config.
pub fn create_all_tools(config: ShellConfig) -> Vec<Arc<dyn AgentTool>> {
    // Only create tools if shell is enabled
    if !config.enabled {
        return vec![];
    }

    let ctx = ShellContext::new(config);
    vec![Arc::new(ShellExecuteTool::new(ctx))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools_enabled() {
        let config = ShellConfig::with_allowed_commands(vec!["echo".to_string()]);
        let tools = create_all_tools(config);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "shell_exec");
    }

    #[test]
    fn test_create_all_tools_disabled() {
        let config = ShellConfig::default();
        let tools = create_all_tools(config);

        // No tools created when disabled
        assert!(tools.is_empty());
    }

    #[test]
    fn test_shell_tool_requires_confirmation() {
        let config = ShellConfig::with_allowed_commands(vec!["echo".to_string()]);
        let tools = create_all_tools(config);

        for tool in &tools {
            assert!(
                tool.requires_confirmation(),
                "{} should always require confirmation",
                tool.name()
            );
        }
    }

    #[test]
    fn test_shell_tool_category() {
        use crate::tools::ToolCategory;

        let config = ShellConfig::with_allowed_commands(vec!["echo".to_string()]);
        let tools = create_all_tools(config);

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Shell,
                "{} should have Shell category",
                tool.name()
            );
        }
    }
}
