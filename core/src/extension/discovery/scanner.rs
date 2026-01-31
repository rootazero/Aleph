//! Directory scanning for plugin discovery

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::{parse_manifest_from_dir_sync, PluginManifest};
use crate::extension::types::{PluginKind, PluginOrigin};

/// A discovered plugin candidate
#[derive(Debug, Clone)]
pub struct PluginCandidate {
    /// Plugin ID
    pub id: String,
    /// Entry file path
    pub source: PathBuf,
    /// Plugin root directory
    pub root_dir: PathBuf,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Plugin kind
    pub kind: PluginKind,
    /// Parsed manifest
    pub manifest: PluginManifest,
}

/// Scan a directory for plugins
pub fn scan_directory(
    dir: &Path,
    origin: PluginOrigin,
) -> Vec<Result<PluginCandidate, ExtensionError>> {
    let mut results = Vec::new();

    if !dir.exists() {
        debug!("Plugin directory does not exist: {:?}", dir);
        return results;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to read plugin directory {:?}: {}", dir, e);
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip hidden files/directories
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }

        if path.is_dir() {
            // Try to parse as plugin directory
            match scan_plugin_dir(&path, origin) {
                Ok(Some(candidate)) => results.push(Ok(candidate)),
                Ok(None) => {} // Not a plugin directory
                Err(e) => results.push(Err(e)),
            }
        } else if path.is_file() {
            // Check for standalone files (WASM, MD)
            if let Some(candidate) = scan_standalone_file(&path, origin) {
                results.push(Ok(candidate));
            }
        }
    }

    results
}

/// Scan a single directory as a potential plugin
fn scan_plugin_dir(
    dir: &Path,
    origin: PluginOrigin,
) -> Result<Option<PluginCandidate>, ExtensionError> {
    // Try manifest-based plugins first
    if let Ok(manifest) = parse_manifest_from_dir_sync(dir) {
        return Ok(Some(PluginCandidate {
            id: manifest.id.clone(),
            source: manifest.entry_path(),
            root_dir: dir.to_path_buf(),
            origin,
            kind: manifest.kind,
            manifest,
        }));
    }

    // Check for static plugins (SKILL.md, COMMAND.md, AGENT.md)
    for filename in ["SKILL.md", "COMMAND.md", "AGENT.md"] {
        let md_path = dir.join(filename);
        if md_path.exists() {
            let id = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            return Ok(Some(PluginCandidate {
                id: id.clone(),
                source: md_path.clone(),
                root_dir: dir.to_path_buf(),
                origin,
                kind: PluginKind::Static,
                manifest: PluginManifest::new(
                    id.clone(),
                    id,
                    PluginKind::Static,
                    md_path.file_name().unwrap().into(),
                )
                .with_root_dir(dir.to_path_buf()),
            }));
        }
    }

    Ok(None)
}

/// Scan a standalone file as a potential plugin
fn scan_standalone_file(path: &Path, origin: PluginOrigin) -> Option<PluginCandidate> {
    let kind = PluginKind::detect_from_path(path)?;

    // Only process WASM and MD files as standalone
    if !matches!(kind, PluginKind::Wasm | PluginKind::Static) {
        return None;
    }

    let id = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let root_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();

    Some(PluginCandidate {
        id: id.clone(),
        source: path.to_path_buf(),
        root_dir: root_dir.clone(),
        origin,
        kind,
        manifest: PluginManifest::new(id.clone(), id, kind, path.file_name().unwrap().into())
            .with_root_dir(root_dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_scan_directory_with_wasm_plugin() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("aether.plugin.json"),
            r#"{
            "id": "my-plugin",
            "name": "My Plugin",
            "entry": "plugin.wasm"
        }"#,
        )
        .unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "my-plugin");
        assert_eq!(candidate.kind, PluginKind::Wasm);
    }

    #[test]
    fn test_scan_directory_with_static_skill() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# My Skill\n\nContent").unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "my-skill");
        assert_eq!(candidate.kind, PluginKind::Static);
    }

    #[test]
    fn test_scan_directory_skips_hidden() {
        let dir = tempdir().unwrap();

        // Create hidden directory
        let hidden_dir = dir.path().join(".hidden-plugin");
        fs::create_dir(&hidden_dir).unwrap();
        fs::write(hidden_dir.join("SKILL.md"), "# Hidden").unwrap();

        // Create visible directory
        let visible_dir = dir.path().join("visible-plugin");
        fs::create_dir(&visible_dir).unwrap();
        fs::write(visible_dir.join("SKILL.md"), "# Visible").unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "visible-plugin");
    }

    #[test]
    fn test_scan_directory_empty() {
        let dir = tempdir().unwrap();
        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_directory_nonexistent() {
        let results = scan_directory(Path::new("/nonexistent/path"), PluginOrigin::Global);
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_standalone_wasm_file() {
        let dir = tempdir().unwrap();
        let wasm_file = dir.path().join("standalone.wasm");
        fs::write(&wasm_file, b"fake wasm content").unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "standalone");
        assert_eq!(candidate.kind, PluginKind::Wasm);
    }
}
