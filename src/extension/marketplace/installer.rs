//! Plugin installer — copy a plugin from the marketplace cache into the
//! installed-plugins directory.

use std::path::{Path, PathBuf};

// =============================================================================
// Public API
// =============================================================================

/// Install a plugin by copying it from `source_path` (inside a marketplace
/// cache) into `<install_dir>/<plugin_name>`.
///
/// # Errors
///
/// * `source_path` does not exist → "plugin not found, try marketplace update"
/// * destination already exists   → "already installed, uninstall first"
/// * any I/O failure during copy
///
/// # Returns
///
/// The absolute path of the newly installed plugin directory.
pub fn install_plugin_from_cache(
    source_path: &Path,
    install_dir: &Path,
    plugin_name: &str,
) -> Result<PathBuf, String> {
    // 1. Validate source exists.
    if !source_path.exists() {
        return Err(format!(
            "Plugin source not found at '{}'. Try running a marketplace update first.",
            source_path.display()
        ));
    }

    let dest = install_dir.join(plugin_name);

    // 2. Validate destination does not already exist.
    if dest.exists() {
        return Err(format!(
            "Plugin '{plugin_name}' is already installed at '{}'. Uninstall it first.",
            dest.display()
        ));
    }

    // 3. Ensure the install directory exists.
    std::fs::create_dir_all(install_dir).map_err(|e| {
        format!(
            "Failed to create install directory '{}': {e}",
            install_dir.display()
        )
    })?;

    // 4. Copy the plugin directory recursively.
    copy_dir_recursive(source_path, &dest)?;

    Ok(dest)
}

/// Verify the SHA-256 hash of a directory by hashing all files recursively.
/// Returns Ok(()) if hash matches or if expected_hash is None (no verification).
pub fn verify_plugin_integrity(
    source_path: &Path,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected_hash else {
        return Ok(()); // No hash to verify
    };

    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();

    // Collect and sort files for deterministic ordering
    let mut files: Vec<_> = walkdir::WalkDir::new(source_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().components().any(|c| c.as_os_str() == ".git"))
        .collect();
    files.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in files {
        let relative = entry
            .path()
            .strip_prefix(source_path)
            .unwrap_or(entry.path());
        hasher.update(relative.to_string_lossy().as_bytes());
        let content = std::fs::read(entry.path())
            .map_err(|e| format!("Failed to read {}: {}", entry.path().display(), e))?;
        hasher.update(&content);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "Plugin integrity check failed: expected {}, got {}",
            expected, actual
        ));
    }

    Ok(())
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Recursively copy `src` directory into `dst`, skipping any `.git` directories.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create directory '{}': {e}", dst.display()))?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("Failed to read directory '{}': {e}", src.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // Skip .git directories.
        if name == ".git" {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(&file_name);

        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to get file type for '{}': {e}", src_path.display()))?;

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
                format!(
                    "Failed to copy '{}' → '{}': {e}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_plugin_dir(base: &Path) -> PathBuf {
        let plugin = base.join("my-plugin");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(
            plugin.join("manifest.toml"),
            "[plugin]\nname = \"my-plugin\"",
        )
        .unwrap();
        // Nested subdir.
        let sub = plugin.join("src");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("main.py"), "print('hello')").unwrap();
        // .git dir — should be excluded.
        let git = plugin.join(".git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("config"), "[core]").unwrap();
        plugin
    }

    #[test]
    fn test_install_plugin_success() {
        let cache = TempDir::new().unwrap();
        let install_root = TempDir::new().unwrap();

        let source = make_plugin_dir(cache.path());
        let result = install_plugin_from_cache(&source, install_root.path(), "my-plugin");

        assert!(result.is_ok(), "{:?}", result.err());
        let dest = result.unwrap();
        assert!(dest.exists());
        assert!(dest.join("manifest.toml").exists());
        assert!(dest.join("src/main.py").exists());
        // .git should not have been copied.
        assert!(!dest.join(".git").exists());
    }

    #[test]
    fn test_install_plugin_source_missing() {
        let install_root = TempDir::new().unwrap();
        let result = install_plugin_from_cache(
            Path::new("/nonexistent/path/to/plugin"),
            install_root.path(),
            "ghost-plugin",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("marketplace update"), "got: {msg}");
    }

    #[test]
    fn test_install_plugin_already_installed() {
        let cache = TempDir::new().unwrap();
        let install_root = TempDir::new().unwrap();

        let source = make_plugin_dir(cache.path());

        // First install.
        install_plugin_from_cache(&source, install_root.path(), "my-plugin").unwrap();

        // Second install should fail.
        let result = install_plugin_from_cache(&source, install_root.path(), "my-plugin");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("already installed"), "got: {msg}");
    }

    #[test]
    fn test_verify_plugin_integrity_no_hash() {
        let tmp = TempDir::new().unwrap();
        // No expected hash → always passes
        assert!(verify_plugin_integrity(tmp.path(), None).is_ok());
    }

    #[test]
    fn test_verify_plugin_integrity_match() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), "world").unwrap();

        // Compute the expected hash by running the same logic
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"hello.txt");
        hasher.update(b"world");
        let expected = format!("{:x}", hasher.finalize());

        assert!(verify_plugin_integrity(tmp.path(), Some(&expected)).is_ok());
    }

    #[test]
    fn test_verify_plugin_integrity_mismatch() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), "world").unwrap();

        let result = verify_plugin_integrity(tmp.path(), Some("bad_hash"));
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("integrity check failed"), "got: {msg}");
    }

    #[test]
    fn test_copy_dir_recursive_skips_git() {
        let src_tmp = TempDir::new().unwrap();
        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("copy");

        let src = src_tmp.path();
        fs::write(src.join("file.txt"), "data").unwrap();
        let git_dir = src.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main").unwrap();

        copy_dir_recursive(src, &dst).unwrap();

        assert!(dst.join("file.txt").exists());
        assert!(!dst.join(".git").exists());
    }
}
