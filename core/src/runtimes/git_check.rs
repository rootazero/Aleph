//! Git availability checker
//!
//! Lightweight utility to check if Git is available on the system.
//! Does not manage Git installation - only checks and provides install hints.

use crate::error::{AlephError, Result};
use std::process::Command;

/// Check if Git is available, return version if found
///
/// # Returns
/// - Ok(version) if Git is available
/// - Err with install hint if Git is not found
///
/// # Example
/// ```no_run
/// use alephcore::runtimes::git_check;
///
/// match git_check::ensure_git_available() {
///     Ok(version) => println!("Git found: {}", version),
///     Err(e) => eprintln!("Git not available: {}", e),
/// }
/// ```
pub fn ensure_git_available() -> Result<String> {
    if let Ok(output) = Command::new("git").arg("--version").output() {
        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            tracing::debug!(version = %version, "Git found");
            return Ok(version);
        }
    }

    tracing::warn!("Git not found in PATH");
    Err(AlephError::other(get_install_hint()))
}

/// Get platform-specific installation hint
fn get_install_hint() -> String {
    #[cfg(target_os = "macos")]
    {
        "Git not found. Please install:\n  • xcode-select --install\n  • or: brew install git"
            .to_string()
    }

    #[cfg(target_os = "linux")]
    {
        "Git not found. Please install:\n  • apt install git (Debian/Ubuntu)\n  • or: dnf install git (Fedora/RHEL)"
            .to_string()
    }

    #[cfg(target_os = "windows")]
    {
        "Git not found. Please install from: https://git-scm.com/download/win".to_string()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "Git not found. Please install Git for your platform.".to_string()
    }
}

/// Get the full path to git executable if available
///
/// # Returns
/// Some(path) if git is found, None otherwise
pub fn get_git_path() -> Option<String> {
    #[cfg(target_os = "windows")]
    let which_cmd = "where";
    #[cfg(not(target_os = "windows"))]
    let which_cmd = "which";

    Command::new(which_cmd)
        .arg("git")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Check if Git is available (simple boolean check)
pub fn is_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_install_hint() {
        let hint = get_install_hint();
        assert!(!hint.is_empty());
        assert!(hint.contains("Git not found"));
    }

    #[test]
    fn test_is_git_available() {
        // This test may pass or fail depending on the system
        let available = is_git_available();
        // Just check that it doesn't panic
        let _ = available;
    }

    #[test]
    fn test_get_git_path() {
        // This test may return Some or None depending on the system
        let path = get_git_path();
        if let Some(p) = path {
            assert!(!p.is_empty());
        }
    }
}
