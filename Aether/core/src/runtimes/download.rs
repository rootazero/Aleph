//! Common download utilities for runtime installation
//!
//! Provides functions for downloading binaries from GitHub releases
//! and setting executable permissions.

use crate::error::{AetherError, Result};
use std::path::Path;
use tracing::{debug, info};

/// Download a file from URL to the specified path
///
/// Uses curl for compatibility with macOS (built-in) and handles redirects.
pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    info!(url = %url, dest = ?dest, "Downloading file");

    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AetherError::runtime(
                "download",
                format!("Failed to create directory {:?}: {}", parent, e),
            )
        })?;
    }

    // Use curl for download (available on macOS by default)
    let output = std::process::Command::new("curl")
        .args([
            "-L",                              // Follow redirects
            "-f",                              // Fail on HTTP errors
            "--progress-bar",                  // Show progress
            "-o",
            dest.to_str().unwrap_or(""),
            url,
        ])
        .output()
        .map_err(|e| AetherError::runtime("download", format!("Failed to run curl: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AetherError::runtime(
            "download",
            format!("Download failed: {}", stderr),
        ));
    }

    debug!(dest = ?dest, "Download completed");
    Ok(())
}

/// Set executable permissions on a file (Unix)
#[cfg(unix)]
pub fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path)
        .map_err(|e| AetherError::runtime("permissions", format!("Failed to get metadata: {}", e)))?
        .permissions();

    // Add execute permission for owner, group, and others
    perms.set_mode(perms.mode() | 0o111);

    std::fs::set_permissions(path, perms).map_err(|e| {
        AetherError::runtime("permissions", format!("Failed to set permissions: {}", e))
    })?;

    debug!(path = ?path, "Set executable permissions");
    Ok(())
}

/// Set executable permissions (no-op on non-Unix)
#[cfg(not(unix))]
pub fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Get the current system architecture string for download URLs
///
/// Returns architecture identifiers commonly used in GitHub releases:
/// - "x86_64" or "aarch64" for macOS
/// - "x86_64" or "aarch64" for Linux
pub fn get_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "unknown"
    }
}

/// Get the current OS string for download URLs
///
/// Returns OS identifiers commonly used in GitHub releases:
/// - "apple-darwin" for macOS
/// - "unknown-linux-gnu" for Linux
pub fn get_os() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "apple-darwin"
    }
    #[cfg(target_os = "linux")]
    {
        "unknown-linux-gnu"
    }
    #[cfg(target_os = "windows")]
    {
        "pc-windows-msvc"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unknown"
    }
}

/// Get platform string for download URLs (e.g., "aarch64-apple-darwin")
pub fn get_platform() -> String {
    format!("{}-{}", get_arch(), get_os())
}

/// Fetch the latest release version from GitHub API
///
/// Returns the tag name (e.g., "v0.5.14" or "2024.12.23")
pub async fn get_github_latest_version(owner: &str, repo: &str) -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        owner, repo
    );

    let client = reqwest::Client::builder()
        .user_agent("Aether/1.0")
        .build()
        .map_err(|e| AetherError::runtime("github", format!("Failed to create client: {}", e)))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AetherError::runtime("github", format!("Failed to fetch release: {}", e)))?;

    if !response.status().is_success() {
        return Err(AetherError::runtime(
            "github",
            format!("GitHub API returned {}", response.status()),
        ));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AetherError::runtime("github", format!("Failed to parse response: {}", e)))?;

    json.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AetherError::runtime("github", "No tag_name in release response"))
}

/// Extract version number from a tag (removes 'v' prefix if present)
pub fn normalize_version(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_platform() {
        let platform = get_platform();
        assert!(!platform.is_empty());
        assert!(platform.contains('-'));
    }

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("v0.5.14"), "0.5.14");
        assert_eq!(normalize_version("2024.12.23"), "2024.12.23");
        assert_eq!(normalize_version("v1.0.0-beta"), "1.0.0-beta");
    }
}
