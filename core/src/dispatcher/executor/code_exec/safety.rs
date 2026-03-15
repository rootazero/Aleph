//! Safety mechanisms for code execution
//!
//! Command blocking and sandbox configuration.

use std::path::PathBuf;

use regex::Regex;
use tracing::warn;

use crate::dispatcher::{DEFAULT_ALLOW_NETWORK, DEFAULT_SANDBOX_ENABLED};

/// Command checker for blocking dangerous commands
pub struct CommandChecker {
    /// Blocked command patterns
    patterns: Vec<Regex>,
}

impl Default for CommandChecker {
    fn default() -> Self {
        Self::new(vec![])
    }
}

impl CommandChecker {
    /// Default blocked patterns
    const DEFAULT_BLOCKED: &'static [&'static str] = &[
        r"rm\s+-rf\s+/\s*$",             // rm -rf /
        r"rm\s+-rf\s+/\*",               // rm -rf /*
        r"rm\s+-rf\s+~\s*$",             // rm -rf ~
        r"sudo\s+",                      // any sudo command
        r"chmod\s+777\s+/",              // chmod 777 /
        r":\(\)\s*\{\s*:\|:&\s*\}\s*;:", // fork bomb
        r">\s*/dev/sd[a-z]",             // overwrite disk
        r"mkfs\.",                       // format filesystem
        r"dd\s+if=.*of=/dev/",           // dd to device
    ];

    /// Create a new command checker with additional blocked patterns
    pub fn new(additional_blocked: Vec<String>) -> Self {
        let mut patterns = Vec::new();

        // Add default patterns
        for pattern in Self::DEFAULT_BLOCKED {
            if let Ok(regex) = Regex::new(pattern) {
                patterns.push(regex);
            }
        }

        // Add user-defined patterns
        for pattern in additional_blocked {
            match Regex::new(&pattern) {
                Ok(regex) => patterns.push(regex),
                Err(e) => warn!("Invalid blocked pattern '{}': {}", pattern, e),
            }
        }

        Self { patterns }
    }

    /// Check if a command is dangerous
    pub fn is_blocked(&self, command: &str) -> Option<String> {
        for pattern in &self.patterns {
            if pattern.is_match(command) {
                return Some(format!("Matches blocked pattern: {}", pattern.as_str()));
            }
        }
        None
    }
}

/// Sandbox configuration for macOS
///
/// Note: Fields are currently unused as sandbox integration is pending.
/// They will be used when sandbox-exec integration is completed.
#[allow(dead_code)] // Architecture reserve: sandbox-exec integration pending
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Whether sandbox is enabled
    pub enabled: bool,

    /// Allowed read paths
    pub read_paths: Vec<PathBuf>,

    /// Allowed write paths
    pub write_paths: Vec<PathBuf>,

    /// Allow network access
    pub allow_network: bool,

    /// Allow process execution
    pub allow_exec: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_SANDBOX_ENABLED,
            read_paths: vec![],
            write_paths: vec![],
            allow_network: DEFAULT_ALLOW_NETWORK,
            allow_exec: true,
        }
    }
}

impl SandboxConfig {
    /// Generate macOS sandbox-exec profile
    ///
    /// Note: Currently unused, will be integrated when sandbox execution is enabled.
    #[allow(dead_code)] // Architecture reserve: sandbox-exec integration pending
    #[cfg(target_os = "macos")]
    pub fn generate_profile(&self) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");

        // Allow basic process operations
        profile.push_str("(allow process-fork)\n");

        if self.allow_exec {
            profile.push_str("(allow process-exec)\n");
        }

        // Allow read paths
        for path in &self.read_paths {
            let path_str = path.to_string_lossy();
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", path_str));
        }

        // Allow write paths
        for path in &self.write_paths {
            let path_str = path.to_string_lossy();
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", path_str));
        }

        // Network access
        if self.allow_network {
            profile.push_str("(allow network*)\n");
        }

        // Allow reading system libraries and frameworks
        profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
        profile.push_str("(allow file-read* (subpath \"/System\"))\n");
        profile.push_str("(allow file-read* (subpath \"/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/private/var\"))\n");

        // Allow reading home directory basics
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}/Library\"))\n",
                home_str
            ));
        }

        profile
    }

    #[allow(dead_code)] // Architecture reserve: sandbox-exec integration pending
    #[cfg(not(target_os = "macos"))]
    pub fn generate_profile(&self) -> String {
        // Non-macOS platforms: return empty (sandbox not supported)
        String::new()
    }
}
