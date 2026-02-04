//! Common download utilities for runtime installation
//!
//! Provides functions for downloading binaries from GitHub releases,
//! extracting archives, and setting executable permissions.
//!
//! Cross-platform support:
//! - Uses Rust native libraries for archive extraction (no external tools)
//! - Works on macOS, Linux, and Windows

use crate::error::{AlephError, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tracing::{debug, info};

/// Download a file from URL to the specified path
///
/// Uses reqwest for cross-platform HTTP downloads without external dependencies.
/// Handles redirects automatically.
pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    info!(url = %url, dest = ?dest, "Downloading file");

    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AlephError::runtime(
                "download",
                format!("Failed to create directory {:?}: {}", parent, e),
            )
        })?;
    }

    // Use reqwest for cross-platform download
    let client = reqwest::Client::builder()
        .user_agent("Aleph/1.0")
        .build()
        .map_err(|e| {
            AlephError::runtime("download", format!("Failed to create HTTP client: {}", e))
        })?;

    let response =
        client.get(url).send().await.map_err(|e| {
            AlephError::runtime("download", format!("Download request failed: {}", e))
        })?;

    if !response.status().is_success() {
        return Err(AlephError::runtime(
            "download",
            format!("Download failed with status: {}", response.status()),
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AlephError::runtime("download", format!("Failed to read response: {}", e)))?;

    std::fs::write(dest, &bytes).map_err(|e| {
        AlephError::runtime(
            "download",
            format!("Failed to write file {:?}: {}", dest, e),
        )
    })?;

    debug!(dest = ?dest, size = bytes.len(), "Download completed");
    Ok(())
}

/// Extract a tar.gz archive to the specified directory
///
/// This uses Rust native libraries (flate2 + tar) for cross-platform support.
/// Does not require external tools like `tar`.
///
/// # Arguments
/// * `archive_path` - Path to the .tar.gz file
/// * `dest_dir` - Directory to extract files into
/// * `strip_components` - Number of leading path components to strip (like tar --strip-components)
pub fn extract_tar_gz(archive_path: &Path, dest_dir: &Path, strip_components: usize) -> Result<()> {
    info!(archive = ?archive_path, dest = ?dest_dir, "Extracting tar.gz archive");

    let file = File::open(archive_path)
        .map_err(|e| AlephError::runtime("extract", format!("Failed to open archive: {}", e)))?;

    let buf_reader = BufReader::new(file);
    let gz_decoder = flate2::read::GzDecoder::new(buf_reader);
    let mut archive = tar::Archive::new(gz_decoder);

    // Create destination directory if it doesn't exist
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        AlephError::runtime("extract", format!("Failed to create directory: {}", e))
    })?;

    // Extract each entry, stripping path components as needed
    for entry in archive.entries().map_err(|e| {
        AlephError::runtime("extract", format!("Failed to read archive entries: {}", e))
    })? {
        let mut entry = entry
            .map_err(|e| AlephError::runtime("extract", format!("Failed to read entry: {}", e)))?;

        let entry_path = entry.path().map_err(|e| {
            AlephError::runtime("extract", format!("Failed to get entry path: {}", e))
        })?;

        // Strip leading components from path
        let stripped_path: std::path::PathBuf =
            entry_path.components().skip(strip_components).collect();

        // Skip if path is empty after stripping
        if stripped_path.as_os_str().is_empty() {
            continue;
        }

        let dest_path = dest_dir.join(&stripped_path);

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create parent dir: {}", e))
            })?;
        }

        // Extract based on entry type
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            std::fs::create_dir_all(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create directory: {}", e))
            })?;
        } else if entry_type.is_file() || entry_type.is_symlink() {
            entry.unpack(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to extract file: {}", e))
            })?;
        }
    }

    debug!(dest = ?dest_dir, "tar.gz extraction completed");
    Ok(())
}

/// Extract a tar.xz archive to the specified directory
///
/// This uses Rust native libraries (xz2 + tar) for cross-platform support.
/// Does not require external tools like `tar` or `xz`.
///
/// # Arguments
/// * `archive_path` - Path to the .tar.xz file
/// * `dest_dir` - Directory to extract files into
/// * `strip_components` - Number of leading path components to strip (like tar --strip-components)
#[allow(dead_code)] // Used by ffmpeg on Linux, may appear unused on other platforms
pub fn extract_tar_xz(archive_path: &Path, dest_dir: &Path, strip_components: usize) -> Result<()> {
    info!(archive = ?archive_path, dest = ?dest_dir, "Extracting tar.xz archive");

    let file = File::open(archive_path)
        .map_err(|e| AlephError::runtime("extract", format!("Failed to open archive: {}", e)))?;

    let buf_reader = BufReader::new(file);
    let xz_decoder = xz2::read::XzDecoder::new(buf_reader);
    let mut archive = tar::Archive::new(xz_decoder);

    // Create destination directory if it doesn't exist
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        AlephError::runtime("extract", format!("Failed to create directory: {}", e))
    })?;

    // Extract each entry, stripping path components as needed
    for entry in archive.entries().map_err(|e| {
        AlephError::runtime("extract", format!("Failed to read archive entries: {}", e))
    })? {
        let mut entry = entry
            .map_err(|e| AlephError::runtime("extract", format!("Failed to read entry: {}", e)))?;

        let entry_path = entry.path().map_err(|e| {
            AlephError::runtime("extract", format!("Failed to get entry path: {}", e))
        })?;

        // Strip leading components from path
        let stripped_path: std::path::PathBuf =
            entry_path.components().skip(strip_components).collect();

        // Skip if path is empty after stripping
        if stripped_path.as_os_str().is_empty() {
            continue;
        }

        let dest_path = dest_dir.join(&stripped_path);

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create parent dir: {}", e))
            })?;
        }

        // Extract based on entry type
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            std::fs::create_dir_all(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create directory: {}", e))
            })?;
        } else if entry_type.is_file() || entry_type.is_symlink() {
            entry.unpack(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to extract file: {}", e))
            })?;
        }
    }

    debug!(dest = ?dest_dir, "tar.xz extraction completed");
    Ok(())
}

/// Extract a ZIP archive to the specified directory
///
/// This uses Rust native library (zip) for cross-platform support.
/// Does not require external tools like `unzip`.
///
/// # Arguments
/// * `archive_path` - Path to the .zip file
/// * `dest_dir` - Directory to extract files into
pub fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    info!(archive = ?archive_path, dest = ?dest_dir, "Extracting ZIP archive");

    let file = File::open(archive_path)
        .map_err(|e| AlephError::runtime("extract", format!("Failed to open archive: {}", e)))?;

    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        AlephError::runtime("extract", format!("Failed to read ZIP archive: {}", e))
    })?;

    // Create destination directory if it doesn't exist
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        AlephError::runtime("extract", format!("Failed to create directory: {}", e))
    })?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            AlephError::runtime("extract", format!("Failed to read ZIP entry: {}", e))
        })?;

        // Get the entry name (handling both forward and back slashes)
        let entry_name = match entry.enclosed_name() {
            Some(name) => name.to_path_buf(),
            None => continue, // Skip invalid entries
        };

        let dest_path = dest_dir.join(&entry_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create directory: {}", e))
            })?;
        } else {
            // Create parent directories
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AlephError::runtime("extract", format!("Failed to create parent dir: {}", e))
                })?;
            }

            // Extract file
            let mut outfile = File::create(&dest_path).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to create file: {}", e))
            })?;

            std::io::copy(&mut entry, &mut outfile).map_err(|e| {
                AlephError::runtime("extract", format!("Failed to write file: {}", e))
            })?;

            // Preserve executable permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    let permissions = std::fs::Permissions::from_mode(mode);
                    std::fs::set_permissions(&dest_path, permissions).ok();
                }
            }
        }
    }

    debug!(dest = ?dest_dir, "ZIP extraction completed");
    Ok(())
}

/// Set executable permissions on a file (Unix)
#[cfg(unix)]
pub fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path)
        .map_err(|e| AlephError::runtime("permissions", format!("Failed to get metadata: {}", e)))?
        .permissions();

    // Add execute permission for owner, group, and others
    perms.set_mode(perms.mode() | 0o111);

    std::fs::set_permissions(path, perms).map_err(|e| {
        AlephError::runtime("permissions", format!("Failed to set permissions: {}", e))
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
        .user_agent("Aleph/1.0")
        .build()
        .map_err(|e| AlephError::runtime("github", format!("Failed to create client: {}", e)))?;

    let response =
        client.get(&url).send().await.map_err(|e| {
            AlephError::runtime("github", format!("Failed to fetch release: {}", e))
        })?;

    if !response.status().is_success() {
        return Err(AlephError::runtime(
            "github",
            format!("GitHub API returned {}", response.status()),
        ));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AlephError::runtime("github", format!("Failed to parse response: {}", e)))?;

    json.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AlephError::runtime("github", "No tag_name in release response"))
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
