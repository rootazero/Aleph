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
    // 0. Reject a destination name that could escape the install directory.
    //    `plugin_name` originates from marketplace manifests / user input, so a
    //    crafted value like `../../etc` would let `install_dir.join(...)` write
    //    outside the managed plugins directory.
    super::names::reject_unsafe_segment("plugin name", plugin_name)?;

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

    // 3. Stage into a temp dir, then atomically rename into place. A direct
    //    recursive copy that fails partway (disk full, permission error, or a
    //    concurrent install) would leave a half-populated directory at `dest`
    //    that both looks installed and blocks reinstall.
    let staging = stage_plugin_copy(source_path, install_dir, plugin_name)?;
    if let Err(e) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "Failed to finalize install of '{plugin_name}' into '{}': {e}",
            dest.display()
        ));
    }

    Ok(dest)
}

/// Update an already-installed plugin in place by atomically swapping in a fresh
/// copy from `source_path` (a marketplace cache directory).
///
/// Unlike [`install_plugin_from_cache`], the destination is *expected* to exist.
/// The swap is crash-safe: the new copy is staged in a temp dir, the existing
/// install is moved aside to a backup, the new copy is renamed into place, and
/// only then is the backup removed. If any step fails the previous install is
/// restored, so a failed update never destroys a working plugin.
///
/// The plugin's persistent data directory lives outside the install tree
/// (`~/.aleph/plugins/data/<id>/`), so it is untouched by the swap.
///
/// # Errors
///
/// * invalid `plugin_name` (path separators / `..`)
/// * `source_path` does not exist
/// * any I/O failure during staging or the swap (previous install restored)
///
/// # Returns
///
/// The absolute path of the updated plugin directory.
pub fn update_plugin_from_cache(
    source_path: &Path,
    install_dir: &Path,
    plugin_name: &str,
) -> Result<PathBuf, String> {
    super::names::reject_unsafe_segment("plugin name", plugin_name)?;

    if !source_path.exists() {
        return Err(format!(
            "Plugin source not found at '{}'. Try running a marketplace update first.",
            source_path.display()
        ));
    }

    let dest = install_dir.join(plugin_name);

    // Stage the new copy first; if this fails the existing install is untouched.
    let staging = stage_plugin_copy(source_path, install_dir, plugin_name)?;

    // Move the current install aside so we can roll back on failure.
    let backup = install_dir.join(format!(".bak-{plugin_name}"));
    if backup.exists() {
        let _ = std::fs::remove_dir_all(&backup);
    }
    let had_existing = dest.exists();
    if had_existing {
        if let Err(e) = std::fs::rename(&dest, &backup) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "Failed to back up existing plugin '{plugin_name}' before update: {e}"
            ));
        }
    }

    // Swap the staged copy into place. On failure, restore the backup.
    if let Err(e) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        if had_existing {
            let _ = std::fs::rename(&backup, &dest);
        }
        return Err(format!(
            "Failed to finalize update of '{plugin_name}' into '{}': {e}",
            dest.display()
        ));
    }

    // Success — drop the backup.
    if had_existing {
        let _ = std::fs::remove_dir_all(&backup);
    }

    Ok(dest)
}

/// Copy `source_path` into a staging directory under `install_dir`, returning
/// the staging path. Shared by [`install_plugin_from_cache`] and
/// [`update_plugin_from_cache`] so the staging/cleanup logic lives in one place.
fn stage_plugin_copy(
    source_path: &Path,
    install_dir: &Path,
    plugin_name: &str,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(install_dir).map_err(|e| {
        format!(
            "Failed to create install directory '{}': {e}",
            install_dir.display()
        )
    })?;

    let staging = install_dir.join(format!(".tmp-install-{plugin_name}"));
    if let Ok(metadata) = std::fs::symlink_metadata(&staging) {
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Staging path must not be a symlink: '{}'",
                staging.display()
            ));
        }
        std::fs::remove_dir_all(&staging).map_err(|e| {
            format!(
                "Failed to remove stale staging directory '{}': {e}",
                staging.display()
            )
        })?;
    }
    if let Err(e) = copy_dir_recursive(source_path, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(staging)
}

/// Verify the SHA-256 hash of a directory by hashing all files recursively.
/// Returns Ok(()) if hash matches or if `expected_hash` is None (no verification).
pub fn verify_plugin_integrity(
    source_path: &Path,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected_hash else {
        return Ok(()); // No hash to verify
    };
    let actual = directory_digest(source_path)?;
    if actual != expected {
        return Err(format!(
            "Plugin integrity check failed: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

/// SHA-256 over a directory tree: every file's repo-relative path and bytes, in
/// sorted order, `.git` excluded. Symlinks are neither hashed nor copied (see
/// `copy_dir_recursive`), so the digest covers exactly what an install writes.
///
/// Path separators are normalized to `/` so the digest a publisher computes on
/// one OS matches what a client computes on another — without that, every
/// hash-pinned install fails on Windows and only on Windows.
pub fn directory_digest(source_path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();

    // Collect and sort files for deterministic ordering. A walk error
    // (unreadable directory/entry) must FAIL the check, not be silently
    // skipped — a dropped file would let a tampered archive pass verification.
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(source_path) {
        let entry = entry.map_err(|e| format!("Failed to walk plugin source: {e}"))?;
        if entry.file_type().is_file()
            && !entry.path().components().any(|c| c.as_os_str() == ".git")
        {
            files.push(entry);
        }
    }
    files.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in files {
        // Hash the path relative to the source root so the digest is
        // reproducible across machines. A strip_prefix failure means the
        // walk escaped `source_path` — fail rather than fold an absolute
        // (host-specific) path into the hash.
        let relative = entry.path().strip_prefix(source_path).map_err(|e| {
            format!(
                "Failed to compute relative path for {}: {e}",
                entry.path().display()
            )
        })?;
        let portable = relative.to_string_lossy().replace('\\', "/");
        hasher.update(portable.as_bytes());
        let content = std::fs::read(entry.path())
            .map_err(|e| format!("Failed to read {}: {}", entry.path().display(), e))?;
        hasher.update(&content);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Recursively copy `src` directory into `dst`, skipping any `.git` directories.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let source_meta = std::fs::symlink_metadata(src)
        .map_err(|e| format!("Failed to inspect source '{}': {e}", src.display()))?;
    if source_meta.file_type().is_symlink() || !source_meta.is_dir() {
        return Err(format!(
            "Plugin source must be a real directory: '{}'",
            src.display()
        ));
    }

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

        // Never follow symlinks from an untrusted plugin source: `fs::copy`
        // dereferences them and would copy arbitrary host files (SSH keys,
        // /etc/passwd, ...) into the installed plugin directory.
        if file_type.is_symlink() {
            tracing::warn!(
                "Skipping symlink in plugin source: '{}'",
                src_path.display()
            );
            continue;
        }

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

    /// A nested path must fold into the digest with `/` separators on every OS,
    /// or a hash computed by the publisher can never match on Windows.
    #[test]
    fn directory_digest_uses_portable_path_separators() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub").join("f.txt"), "x").unwrap();

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"sub/f.txt");
        hasher.update(b"x");
        let expected = format!("{:x}", hasher.finalize());

        assert_eq!(directory_digest(tmp.path()).unwrap(), expected);
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
    fn test_update_plugin_replaces_existing() {
        let cache = TempDir::new().unwrap();
        let install_root = TempDir::new().unwrap();

        // Install v1.
        let source = make_plugin_dir(cache.path());
        install_plugin_from_cache(&source, install_root.path(), "my-plugin").unwrap();

        // Build a v2 source with changed content + a new file.
        let v2 = cache.path().join("my-plugin-v2");
        fs::create_dir_all(&v2).unwrap();
        fs::write(
            v2.join("manifest.toml"),
            "[plugin]\nname = \"my-plugin\"\nversion=\"2\"",
        )
        .unwrap();
        fs::write(v2.join("NEW.txt"), "added in v2").unwrap();

        let dest = update_plugin_from_cache(&v2, install_root.path(), "my-plugin").unwrap();
        assert!(dest.join("NEW.txt").exists(), "new file should be present");
        // Old-only file (src/main.py) should be gone after a full swap.
        assert!(
            !dest.join("src/main.py").exists(),
            "stale file should be removed"
        );
        // No backup or staging residue.
        assert!(!install_root.path().join(".bak-my-plugin").exists());
        assert!(!install_root.path().join(".tmp-install-my-plugin").exists());
    }

    #[test]
    fn test_update_plugin_preserves_install_on_bad_source() {
        let install_root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();

        let source = make_plugin_dir(cache.path());
        install_plugin_from_cache(&source, install_root.path(), "my-plugin").unwrap();

        // Update from a nonexistent source must fail without harming the install.
        let result = update_plugin_from_cache(
            Path::new("/nonexistent/source"),
            install_root.path(),
            "my-plugin",
        );
        assert!(result.is_err());
        // Original install still intact.
        assert!(install_root.path().join("my-plugin/manifest.toml").exists());
    }

    #[test]
    fn test_update_plugin_on_fresh_install() {
        // Updating something not yet installed should just install it.
        let cache = TempDir::new().unwrap();
        let install_root = TempDir::new().unwrap();
        let source = make_plugin_dir(cache.path());

        let dest = update_plugin_from_cache(&source, install_root.path(), "my-plugin").unwrap();
        assert!(dest.join("manifest.toml").exists());
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
