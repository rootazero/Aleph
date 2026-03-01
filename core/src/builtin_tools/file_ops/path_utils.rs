//! Path validation and resolution utilities

use std::path::{Path, PathBuf};
use tracing::info;

use crate::builtin_tools::error::ToolError;
use super::state::get_working_dir;

/// Denied paths for security
pub fn get_denied_paths() -> Vec<String> {
    let mut denied_paths = vec![
        // Unix sensitive directories
        "~/.ssh".to_string(),
        "~/.gnupg".to_string(),
        "~/.aws".to_string(),
    ];

    // Add specific Aleph config files (not the entire directory)
    // We allow the output directory but deny sensitive config files
    if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
        info!(config_dir = %config_dir.display(), "FileOpsTool: config_dir for denied_paths");
        // Deny config files but NOT the output directory
        denied_paths.push(format!("{}/config.toml", config_dir.display()));
        denied_paths.push(format!("{}/memory.db", config_dir.display()));
        denied_paths.push(format!("{}/conversations.db", config_dir.display()));
        denied_paths.push(format!("{}/skills", config_dir.display()));
        denied_paths.push(format!("{}/plugins", config_dir.display()));
        denied_paths.push(format!("{}/mcp", config_dir.display()));
        // Note: output directory is intentionally NOT denied
    }

    // Add Unix-specific paths
    #[cfg(unix)]
    {
        denied_paths.extend(["/etc/passwd".to_string(), "/etc/shadow".to_string()]);
    }

    // Add Windows-specific sensitive paths
    #[cfg(target_os = "windows")]
    {
        denied_paths.extend([
            "%APPDATA%\\Microsoft\\Credentials".to_string(),
            "%LOCALAPPDATA%\\Microsoft\\Credentials".to_string(),
            "C:\\Windows\\System32\\config".to_string(),
        ]);
    }

    denied_paths
}

/// Check if path is allowed and resolve it
///
/// Path resolution rules:
/// 1. Environment variables ($HOME, $USER, etc.) - expanded first
/// 2. Absolute paths (starting with `/`) - used as-is
/// 3. Home paths (starting with `~`) - expanded to home directory
/// 4. Relative paths - resolved relative to output directory (~/.aleph/output/)
pub fn check_and_resolve_path(path: &Path, denied_paths: &[String]) -> Result<PathBuf, ToolError> {
    info!(path = %path.display(), "check_path: input path");

    // First, expand environment variables in the path string
    let path_str = path.to_string_lossy();
    let expanded_str = if path_str.contains('$') {
        let mut result = path_str.to_string();

        // Expand $HOME
        if let Some(home) = dirs::home_dir() {
            result = result.replace("$HOME", &home.to_string_lossy());
        }

        // Expand $USER
        if let Ok(user) = std::env::var("USER") {
            result = result.replace("$USER", &user);
        }

        // Expand other common environment variables.
        // Sort by key length (longest first) to prevent shorter names
        // matching as prefixes of longer ones (e.g., $HOME before $HOMEDIR).
        let mut env_vars: Vec<(String, String)> = std::env::vars().collect();
        env_vars.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (key, value) in env_vars {
            let pattern = format!("${}", key);
            if result.contains(&pattern) {
                result = result.replace(&pattern, &value);
            }
        }

        info!(original = %path_str, expanded = %result, "check_path: expanded environment variables");
        PathBuf::from(result)
    } else {
        path.to_path_buf()
    };

    // Expand ~ to home directory
    let expanded = if expanded_str.starts_with("~") {
        let home = dirs::home_dir().ok_or_else(|| {
            ToolError::InvalidArgs("Cannot determine home directory".to_string())
        })?;
        home.join(expanded_str.strip_prefix("~").unwrap())
    } else if expanded_str.is_relative() {
        // Relative paths are resolved to:
        // 1. Current working directory (if set by session/topic)
        // 2. Default output directory (~/.aleph/output/)
        let base_dir = if let Some(wd) = get_working_dir() {
            info!(working_dir = %wd.display(), "check_path: using session working directory");
            wd
        } else {
            let output_dir = crate::utils::paths::get_output_dir().map_err(|e| {
                ToolError::Execution(format!("Failed to get output directory: {}", e))
            })?;
            info!(output_dir = %output_dir.display(), "check_path: using default output directory");
            output_dir
        };
        base_dir.join(expanded_str)
    } else {
        expanded_str
    };

    info!(expanded = %expanded.display(), exists = expanded.exists(), "check_path: expanded path");

    // Canonicalize if exists; for non-existent files, manually normalize to resolve ".."
    // components. This prevents path traversal bypasses (e.g., "/allowed/../secret/file").
    let canonical = if expanded.exists() {
        expanded
            .canonicalize()
            .map_err(|e| ToolError::Execution(format!("Failed to resolve path: {}", e)))?
    } else {
        // Normalize path components without filesystem access
        use std::path::Component;
        let mut normalized = PathBuf::new();
        for component in expanded.components() {
            match component {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => {}
                _ => {
                    normalized.push(component);
                }
            }
        }
        normalized
    };

    info!(canonical = %canonical.display(), "check_path: canonical path");

    // Check against denied paths
    let path_str = canonical.to_string_lossy();
    for denied in denied_paths {
        let denied_expanded = if denied.starts_with("~") {
            if let Some(home) = dirs::home_dir() {
                home.join(denied.strip_prefix("~/").unwrap_or(denied))
                    .to_string_lossy()
                    .to_string()
            } else {
                denied.clone()
            }
        } else {
            denied.clone()
        };

        if path_str.starts_with(&denied_expanded) {
            info!(
                path_str = %path_str,
                denied = %denied,
                denied_expanded = %denied_expanded,
                "check_path: ACCESS DENIED - path matches denied pattern"
            );
            return Err(ToolError::InvalidArgs(format!(
                "Access denied: {} is in a protected location",
                path.display()
            )));
        }
    }

    info!(canonical = %canonical.display(), "check_path: path allowed");
    Ok(canonical)
}
