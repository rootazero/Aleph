//! Shell Tools Configuration
//!
//! Shared configuration for shell tools, including security controls.

use std::sync::Arc;

use crate::error::{AetherError, Result};

/// Shared configuration for shell tools
///
/// Shell tools are security-sensitive and require explicit configuration.
/// By default, shell execution is disabled.
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Whether shell execution is enabled (default: false for security)
    pub enabled: bool,

    /// Command timeout in seconds (default: 30)
    pub timeout_seconds: u64,

    /// Allowed commands whitelist
    ///
    /// Only commands in this list can be executed.
    /// - Empty list means NO commands are allowed (most secure)
    /// - Use "*" to allow all commands (least secure, not recommended)
    pub allowed_commands: Vec<String>,

    /// Blocked commands blacklist
    ///
    /// Commands in this list are always blocked, even if whitelisted.
    /// Use this to block dangerous commands like "rm -rf /".
    pub blocked_commands: Vec<String>,

    /// Working directory restriction
    ///
    /// If set, commands can only run in directories under these paths.
    /// Empty means no directory restriction.
    pub allowed_directories: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_seconds: 30,
            allowed_commands: vec![],
            blocked_commands: vec![
                // Dangerous commands that should never be allowed
                "rm -rf /".to_string(),
                "rm -rf /*".to_string(),
                "mkfs".to_string(),
                "dd if=/dev/zero".to_string(),
                ":(){:|:&};:".to_string(), // Fork bomb
            ],
            allowed_directories: vec![],
        }
    }
}

impl ShellConfig {
    /// Create a new configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a configuration with specific allowed commands
    pub fn with_allowed_commands(commands: Vec<String>) -> Self {
        Self {
            enabled: true,
            allowed_commands: commands,
            ..Default::default()
        }
    }

    /// Create a configuration that allows all commands (DANGEROUS!)
    ///
    /// This is intended for development/testing only.
    /// Use with extreme caution in production.
    pub fn allow_all() -> Self {
        Self {
            enabled: true,
            allowed_commands: vec!["*".to_string()],
            ..Default::default()
        }
    }

    /// Enable shell execution
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Set timeout in seconds
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Add an allowed command
    pub fn add_allowed_command(&mut self, command: &str) {
        self.allowed_commands.push(command.to_string());
    }

    /// Add a blocked command
    pub fn add_blocked_command(&mut self, command: &str) {
        self.blocked_commands.push(command.to_string());
    }

    /// Check if a command is allowed
    ///
    /// A command is allowed if:
    /// 1. Shell execution is enabled
    /// 2. The command (or "*") is in the allowed list
    /// 3. The command is NOT in the blocked list
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if !self.enabled {
            return false;
        }

        // Check blocked list first (takes priority)
        if self.is_command_blocked(command) {
            return false;
        }

        // If allowed list is empty, nothing is allowed
        if self.allowed_commands.is_empty() {
            return false;
        }

        // Extract the program name (first word)
        let program = command.split_whitespace().next().unwrap_or("");

        self.allowed_commands.iter().any(|allowed| {
            allowed == program || allowed == "*"
        })
    }

    /// Check if a command is blocked
    fn is_command_blocked(&self, command: &str) -> bool {
        self.blocked_commands.iter().any(|blocked| {
            command.contains(blocked)
        })
    }

    /// Check if a working directory is allowed
    pub fn is_directory_allowed(&self, dir: &str) -> bool {
        // If no restrictions, all directories are allowed
        if self.allowed_directories.is_empty() {
            return true;
        }

        self.allowed_directories.iter().any(|allowed| {
            dir.starts_with(allowed)
        })
    }

    /// Validate command and return error if not allowed
    pub fn validate_command(&self, command: &str) -> Result<()> {
        if !self.enabled {
            return Err(AetherError::PermissionDenied {
                message: "Shell execution is disabled".to_string(),
                suggestion: Some("Enable shell execution in configuration".to_string()),
            });
        }

        if !self.is_command_allowed(command) {
            let program = command.split_whitespace().next().unwrap_or("(empty)");
            return Err(AetherError::PermissionDenied {
                message: format!("Command not allowed: {}", program),
                suggestion: Some("Add the command to allowed_commands list".to_string()),
            });
        }

        Ok(())
    }

    /// Validate working directory and return error if not allowed
    pub fn validate_directory(&self, dir: &str) -> Result<()> {
        if !self.is_directory_allowed(dir) {
            return Err(AetherError::PermissionDenied {
                message: format!("Working directory not allowed: {}", dir),
                suggestion: Some("Add the directory to allowed_directories list".to_string()),
            });
        }

        Ok(())
    }
}

/// Shell tools context
///
/// Provides shared access to shell configuration.
/// Used by all shell tool implementations.
#[derive(Clone)]
pub struct ShellContext {
    /// Security configuration
    pub config: Arc<ShellConfig>,
}

impl ShellContext {
    /// Create a new context
    pub fn new(config: ShellConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Check if shell execution is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Validate command against security configuration
    pub fn validate_command(&self, command: &str) -> Result<()> {
        self.config.validate_command(command)
    }

    /// Validate working directory against security configuration
    pub fn validate_directory(&self, dir: &str) -> Result<()> {
        self.config.validate_directory(dir)
    }

    /// Get timeout duration in seconds
    pub fn timeout_seconds(&self) -> u64 {
        self.config.timeout_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_disabled() {
        let config = ShellConfig::default();
        assert!(!config.enabled);
        assert!(!config.is_command_allowed("echo hello"));
    }

    #[test]
    fn test_config_with_allowed_commands() {
        let config = ShellConfig::with_allowed_commands(vec![
            "echo".to_string(),
            "ls".to_string(),
        ]);

        assert!(config.enabled);
        assert!(config.is_command_allowed("echo hello"));
        assert!(config.is_command_allowed("ls -la"));
        assert!(!config.is_command_allowed("rm file.txt"));
    }

    #[test]
    fn test_config_blocked_commands() {
        let mut config = ShellConfig::with_allowed_commands(vec!["*".to_string()]);
        config.add_blocked_command("dangerous");

        assert!(config.is_command_allowed("echo hello"));
        assert!(!config.is_command_allowed("dangerous command"));
    }

    #[test]
    fn test_config_default_blocked() {
        let config = ShellConfig::with_allowed_commands(vec!["rm".to_string()]);

        // These dangerous commands should be blocked by default
        assert!(!config.is_command_allowed("rm -rf /"));
        assert!(!config.is_command_allowed("rm -rf /*"));
    }

    #[test]
    fn test_config_allow_all() {
        let config = ShellConfig::allow_all();

        assert!(config.enabled);
        assert!(config.is_command_allowed("any_command"));
        assert!(config.is_command_allowed("echo hello"));
        // But dangerous commands are still blocked
        assert!(!config.is_command_allowed("rm -rf /"));
    }

    #[test]
    fn test_validate_command_disabled() {
        let config = ShellConfig::default();
        let result = config.validate_command("echo hello");

        assert!(result.is_err());
        match result {
            Err(AetherError::PermissionDenied { message, .. }) => {
                assert!(message.contains("disabled"));
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_validate_command_not_allowed() {
        let config = ShellConfig::with_allowed_commands(vec!["echo".to_string()]);
        let result = config.validate_command("rm file.txt");

        assert!(result.is_err());
        match result {
            Err(AetherError::PermissionDenied { message, .. }) => {
                assert!(message.contains("not allowed"));
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_directory_allowed() {
        let mut config = ShellConfig::with_allowed_commands(vec!["*".to_string()]);
        config.allowed_directories = vec!["/home/user".to_string()];

        assert!(config.is_directory_allowed("/home/user/projects"));
        assert!(!config.is_directory_allowed("/etc"));
    }

    #[test]
    fn test_directory_no_restriction() {
        let config = ShellConfig::with_allowed_commands(vec!["*".to_string()]);

        // No directory restrictions
        assert!(config.is_directory_allowed("/anywhere"));
        assert!(config.is_directory_allowed("/etc"));
    }

    #[test]
    fn test_context_timeout() {
        let config = ShellConfig::default().with_timeout(60);
        let ctx = ShellContext::new(config);

        assert_eq!(ctx.timeout_seconds(), 60);
    }
}
