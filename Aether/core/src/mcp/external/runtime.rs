//! Runtime Detection for External MCP Servers
//!
//! Checks availability of Node.js, Python, Bun, etc. before starting servers.

use std::process::Command;

/// Runtime types that external MCP servers may require
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    /// Node.js runtime
    Node,
    /// Python runtime (python3)
    Python,
    /// Bun runtime
    Bun,
    /// Deno runtime
    Deno,
    /// No runtime required (native binary)
    None,
}

impl RuntimeKind {
    /// Parse runtime kind from string
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "node" | "nodejs" => Self::Node,
            "python" | "python3" => Self::Python,
            "bun" => Self::Bun,
            "deno" => Self::Deno,
            "none" | "" => Self::None,
            _ => Self::None,
        }
    }

    /// Get the command to check for this runtime
    pub fn check_command(&self) -> Option<&'static str> {
        match self {
            Self::Node => Some("node"),
            Self::Python => Some("python3"),
            Self::Bun => Some("bun"),
            Self::Deno => Some("deno"),
            Self::None => None,
        }
    }

    /// Get human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Node => "Node.js",
            Self::Python => "Python 3",
            Self::Bun => "Bun",
            Self::Deno => "Deno",
            Self::None => "None",
        }
    }
}

impl std::fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Result of runtime check
#[derive(Debug, Clone)]
pub struct RuntimeCheckResult {
    /// Runtime kind that was checked
    pub kind: RuntimeKind,
    /// Whether the runtime is available
    pub available: bool,
    /// Version string if available
    pub version: Option<String>,
    /// Path to the runtime binary if found
    pub path: Option<String>,
}

/// Check if a runtime is available on the system
///
/// # Arguments
/// * `kind` - The runtime to check for
///
/// # Returns
/// RuntimeCheckResult with availability and version info
pub fn check_runtime(kind: RuntimeKind) -> RuntimeCheckResult {
    let cmd = match kind.check_command() {
        Some(cmd) => cmd,
        None => {
            return RuntimeCheckResult {
                kind,
                available: true,
                version: None,
                path: None,
            };
        }
    };

    // Check version
    let version_output = Command::new(cmd)
        .arg("--version")
        .output();

    match version_output {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();

            // Try to get the path using 'which' or 'where'
            let path = get_runtime_path(cmd);

            tracing::debug!(
                runtime = %kind,
                version = %version,
                path = ?path,
                "Runtime found"
            );

            RuntimeCheckResult {
                kind,
                available: true,
                version: Some(version),
                path,
            }
        }
        Ok(_) | Err(_) => {
            tracing::debug!(
                runtime = %kind,
                "Runtime not found"
            );

            RuntimeCheckResult {
                kind,
                available: false,
                version: None,
                path: None,
            }
        }
    }
}

/// Get the full path to a runtime binary
fn get_runtime_path(cmd: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    let which_cmd = "where";
    #[cfg(not(target_os = "windows"))]
    let which_cmd = "which";

    Command::new(which_cmd)
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Check multiple runtimes at once
#[allow(dead_code)]
pub fn check_all_runtimes() -> Vec<RuntimeCheckResult> {
    vec![
        check_runtime(RuntimeKind::Node),
        check_runtime(RuntimeKind::Python),
        check_runtime(RuntimeKind::Bun),
        check_runtime(RuntimeKind::Deno),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_kind_from_str() {
        assert_eq!(RuntimeKind::from_str("node"), RuntimeKind::Node);
        assert_eq!(RuntimeKind::from_str("nodejs"), RuntimeKind::Node);
        assert_eq!(RuntimeKind::from_str("Node"), RuntimeKind::Node);
        assert_eq!(RuntimeKind::from_str("python"), RuntimeKind::Python);
        assert_eq!(RuntimeKind::from_str("python3"), RuntimeKind::Python);
        assert_eq!(RuntimeKind::from_str("bun"), RuntimeKind::Bun);
        assert_eq!(RuntimeKind::from_str("deno"), RuntimeKind::Deno);
        assert_eq!(RuntimeKind::from_str("none"), RuntimeKind::None);
        assert_eq!(RuntimeKind::from_str(""), RuntimeKind::None);
        assert_eq!(RuntimeKind::from_str("unknown"), RuntimeKind::None);
    }

    #[test]
    fn test_runtime_check_command() {
        assert_eq!(RuntimeKind::Node.check_command(), Some("node"));
        assert_eq!(RuntimeKind::Python.check_command(), Some("python3"));
        assert_eq!(RuntimeKind::Bun.check_command(), Some("bun"));
        assert_eq!(RuntimeKind::Deno.check_command(), Some("deno"));
        assert_eq!(RuntimeKind::None.check_command(), None);
    }

    #[test]
    fn test_check_runtime_none() {
        let result = check_runtime(RuntimeKind::None);
        assert!(result.available);
    }

    // Note: These tests depend on the actual system environment
    // They're marked to run but may pass or fail based on what's installed

    #[test]
    fn test_check_runtime_result_structure() {
        // Just check that the function returns a valid structure
        let result = check_runtime(RuntimeKind::Node);
        assert_eq!(result.kind, RuntimeKind::Node);
        // available could be true or false depending on system
    }

    #[test]
    fn test_check_all_runtimes() {
        let results = check_all_runtimes();
        assert_eq!(results.len(), 4);

        // Check that all expected runtimes are checked
        let kinds: Vec<RuntimeKind> = results.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&RuntimeKind::Node));
        assert!(kinds.contains(&RuntimeKind::Python));
        assert!(kinds.contains(&RuntimeKind::Bun));
        assert!(kinds.contains(&RuntimeKind::Deno));
    }
}
