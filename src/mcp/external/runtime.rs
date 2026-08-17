//! Runtime Detection for External MCP Servers
//!
//! Checks availability of Node.js, Python, Bun, etc. before starting servers.

use crate::utils::no_window::NoWindow;
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
    /// Parse runtime kind from string with fallback to None
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::None)
    }

    /// All command names to probe for this runtime, in priority order.
    ///
    /// Most runtimes expose a single canonical binary across every platform
    /// (`node`, `bun`, `deno`). Python is the exception: Unix/macOS canonically
    /// ship `python3`, but the Windows python.org installer and the `py`
    /// launcher provide `python` (`python.exe`), and minimal Linux images often
    /// only have `python`. Probing both prevents Python MCP servers from being
    /// false-skipped on hosts where only `python` is on PATH.
    #[must_use]
    pub const fn check_commands(&self) -> &'static [&'static str] {
        match self {
            Self::Node => &["node"],
            Self::Python => &["python3", "python"],
            Self::Bun => &["bun"],
            Self::Deno => &["deno"],
            Self::None => &[],
        }
    }

    /// Get human-readable name
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
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

impl std::str::FromStr for RuntimeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "node" | "nodejs" => Ok(Self::Node),
            "python" | "python3" => Ok(Self::Python),
            "bun" => Ok(Self::Bun),
            "deno" => Ok(Self::Deno),
            "none" | "" => Ok(Self::None),
            _ => Err(format!("Unknown runtime kind: {s}")),
        }
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
/// `RuntimeCheckResult` with availability and version info
pub fn check_runtime(kind: RuntimeKind) -> RuntimeCheckResult {
    let candidates = kind.check_commands();
    if candidates.is_empty() {
        // No runtime required (e.g. native binary) — always available.
        return RuntimeCheckResult {
            kind,
            available: true,
            version: None,
            path: None,
        };
    }

    // Probe each candidate command in priority order; the first one that
    // reports a version wins. This lets Python fall back from `python3` to
    // `python` on Windows / minimal Linux hosts.
    for cmd in candidates {
        let version_output = Command::new(cmd).arg("--version").no_window().output();

        if let Ok(output) = version_output {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();

                // Try to get the path using 'which' or 'where'
                let path = get_runtime_path(cmd);

                tracing::debug!(
                    runtime = %kind,
                    command = %cmd,
                    version = %version,
                    path = ?path,
                    "Runtime found"
                );

                return RuntimeCheckResult {
                    kind,
                    available: true,
                    version: Some(version),
                    path,
                };
            }
        }
    }

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

/// Get the full path to a runtime binary
fn get_runtime_path(cmd: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    let which_cmd = "where";
    #[cfg(not(target_os = "windows"))]
    let which_cmd = "which";

    Command::new(which_cmd)
        .arg(cmd)
        .no_window()
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            // Take only the first line — `where` on Windows may return multiple paths
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(|line| line.trim().to_string())
        })
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_kind_from_str() {
        assert_eq!(RuntimeKind::from_str_or_default("node"), RuntimeKind::Node);
        assert_eq!(
            RuntimeKind::from_str_or_default("nodejs"),
            RuntimeKind::Node
        );
        assert_eq!(RuntimeKind::from_str_or_default("Node"), RuntimeKind::Node);
        assert_eq!(
            RuntimeKind::from_str_or_default("python"),
            RuntimeKind::Python
        );
        assert_eq!(
            RuntimeKind::from_str_or_default("python3"),
            RuntimeKind::Python
        );
        assert_eq!(RuntimeKind::from_str_or_default("bun"), RuntimeKind::Bun);
        assert_eq!(RuntimeKind::from_str_or_default("deno"), RuntimeKind::Deno);
        assert_eq!(RuntimeKind::from_str_or_default("none"), RuntimeKind::None);
        assert_eq!(RuntimeKind::from_str_or_default(""), RuntimeKind::None);
        assert_eq!(
            RuntimeKind::from_str_or_default("unknown"),
            RuntimeKind::None
        );
    }

    #[test]
    fn test_python_probes_python3_then_python() {
        // Regression: Python must fall back to `python` so that Windows
        // (python.org installer / `py` launcher) and minimal Linux hosts —
        // where only `python` exists — are not false-skipped.
        assert_eq!(RuntimeKind::Python.check_commands(), &["python3", "python"]);
    }

    #[test]
    fn test_single_binary_runtimes_have_one_candidate() {
        assert_eq!(RuntimeKind::Node.check_commands(), &["node"]);
        assert_eq!(RuntimeKind::Bun.check_commands(), &["bun"]);
        assert_eq!(RuntimeKind::Deno.check_commands(), &["deno"]);
        assert!(RuntimeKind::None.check_commands().is_empty());
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

    }
