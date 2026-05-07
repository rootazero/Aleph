//! Bundled content extractor — extracts embedded skills/plugins on startup.
//!
//! Extraction occurs when:
//! - manifest.json doesn't exist (first install or upgrade from old version)
//! - bundled_version differs from manifest's bundled_version

use super::manifest::{SkillEntry, SkillManifest, SkillOrigin};
use super::{BUNDLED_PLUGINS, BUNDLED_SKILLS, BUNDLED_VERSION};
use include_dir::Dir;
use std::path::Path;
use tracing::{debug, info, warn};

/// Main entry point — called during server startup.
///
/// Extracts bundled skills to `~/.aleph/skills/` and plugins to
/// `~/.aleph/plugins/cache/aleph-official/` if version has changed.
pub fn extract_bundled_content(aleph_home: &Path) {
    let skills_dir = aleph_home.join("skills");
    let plugins_cache = aleph_home
        .join("plugins")
        .join("cache")
        .join("aleph-official");

    // Ensure directories exist
    if let Err(e) = std::fs::create_dir_all(&skills_dir) {
        warn!(error = %e, path = %skills_dir.display(), "Failed to create skills directory");
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&plugins_cache) {
        warn!(error = %e, path = %plugins_cache.display(), "Failed to create plugins cache directory");
        return;
    }

    // Load or create manifest
    let mut manifest = match SkillManifest::load(&skills_dir) {
        Some(m) => m,
        None => {
            // First run or upgrade from old version — reconcile existing skills first
            info!("No skills manifest found, performing initial reconcile");
            let mut m = SkillManifest::new("");
            if let Err(e) = m.reconcile(&skills_dir) {
                warn!(error = %e, "Failed to reconcile skills directory, will retry on next startup");
            }
            m
        }
    };

    // Check if extraction is needed
    if manifest.bundled_version == BUNDLED_VERSION {
        debug!(version = BUNDLED_VERSION, "Bundled content is up to date");
        // Still reconcile to catch manually added skills
        if let Err(e) = manifest.reconcile(&skills_dir) {
            warn!(error = %e, "Failed to reconcile skills directory");
        }
        if let Err(e) = manifest.save(&skills_dir) {
            warn!(error = %e, "Failed to save manifest after reconcile");
        }
        return;
    }

    info!(
        from = %manifest.bundled_version,
        to = BUNDLED_VERSION,
        "Extracting bundled content"
    );

    // Extract skills
    let skills_ok = extract_skills(&BUNDLED_SKILLS, &skills_dir, &mut manifest);

    // Extract plugins (marketplace cache)
    let plugins_ok = extract_plugins(&BUNDLED_PLUGINS, &plugins_cache);

    // Only update bundled_version if ALL extractions succeeded
    if skills_ok && plugins_ok {
        manifest.bundled_version = BUNDLED_VERSION.to_string();
        info!(
            version = BUNDLED_VERSION,
            "Bundled content extraction complete"
        );
    } else {
        warn!("Partial extraction failure — will retry on next startup");
    }

    // Reconcile and save
    if let Err(e) = manifest.reconcile(&skills_dir) {
        warn!(error = %e, "Failed to reconcile skills directory, manifest may be stale");
    }
    if let Err(e) = manifest.save(&skills_dir) {
        warn!(error = %e, "Failed to save manifest");
    }

    // Clean up legacy skills-official directory
    cleanup_legacy_dir(aleph_home);
}

/// Extract bundled skills to the skills directory.
/// Returns true if all extractions succeeded.
fn extract_skills(bundled: &Dir, skills_dir: &Path, manifest: &mut SkillManifest) -> bool {
    let mut all_ok = true;

    for dir in bundled.dirs() {
        // Use file_name() to get the base directory name, matching reconcile()
        let Some(name_os) = dir.path().file_name() else {
            warn!(path = ?dir.path(), "Bundled skill dir has no file_name, skipping");
            continue;
        };
        let name = name_os.to_string_lossy().to_string();

        // Defensive: reject empty or path-traversal directory names
        if name.is_empty() || name == "." || name == ".." {
            warn!(skill = %name, "Skipping bundled skill with invalid name");
            continue;
        }

        // Skip if user has a non-official skill with the same name
        if let Some(entry) = manifest.skills.get(&name) {
            if entry.source != SkillOrigin::Official {
                debug!(skill = %name, source = ?entry.source, "Skipping user skill");
                continue;
            }
        }

        // Extract this skill
        let target = skills_dir.join(&name);
        match extract_dir_recursive(dir, &target) {
            Ok(()) => {
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Official,
                        version: Some(BUNDLED_VERSION.to_string()),
                        url: None,
                        installed_at: None,
                    },
                );
                debug!(skill = %name, "Extracted bundled skill");
            }
            Err(e) => {
                warn!(skill = %name, error = %e, "Failed to extract skill");
                all_ok = false;
            }
        }
    }

    all_ok
}

/// Extract bundled plugins to the marketplace cache directory.
///
/// Uses an atomic swap: extracts to a temporary directory first, then renames
/// it into place. This ensures the cache is never in a partially-extracted
/// state if the process crashes mid-extraction.
fn extract_plugins(bundled: &Dir, cache_dir: &Path) -> bool {
    let tmp_dir = cache_dir.with_extension("tmp");

    // Clean up any leftover temp directory from a previous crash
    if tmp_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&tmp_dir) {
            warn!(error = %e, "Failed to remove old plugin cache temp directory");
            return false;
        }
    }
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        warn!(error = %e, "Failed to create plugin cache temp directory");
        return false;
    }

    // Extract all files and directories into the temp directory
    match extract_dir_contents(bundled, &tmp_dir) {
        Ok(()) => {
            // Atomically swap the old cache for the new one
            if let Err(e) = std::fs::rename(&tmp_dir, cache_dir) {
                warn!(error = %e, "Failed to atomically swap plugin cache");
                return false;
            }
            info!("Extracted bundled plugins to marketplace cache");
            true
        }
        Err(e) => {
            warn!(error = %e, "Failed to extract bundled plugins");
            false
        }
    }
}

fn extract_dir_recursive(dir: &Dir, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    extract_dir_contents(dir, target)?;
    prune_stale_entries(dir, target)?;
    Ok(())
}

fn prune_stale_entries(dir: &Dir, target: &Path) -> std::io::Result<()> {
    use std::collections::HashSet;

    let mut bundle_names: HashSet<std::ffi::OsString> = HashSet::new();
    for file in dir.files() {
        if let Some(name) = file.path().file_name() {
            bundle_names.insert(name.to_os_string());
        }
    }
    for subdir in dir.dirs() {
        if let Some(name) = subdir.path().file_name() {
            bundle_names.insert(name.to_os_string());
        }
    }

    let entries = std::fs::read_dir(target)?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        if !bundle_names.contains(&name) {
            let path = entry.path();
            // Use file_type (no follow) so we don't traverse symlinks — prevents
            // accidental deletion outside the target directory.
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to stat entry, skipping");
                    continue;
                }
            };
            if ft.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    warn!(path = %path.display(), error = %e, "Failed to remove stale directory");
                }
            } else if ft.is_file() || ft.is_symlink() {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!(path = %path.display(), error = %e, "Failed to remove stale file");
                }
            } else {
                warn!(path = %path.display(), "Skipping unknown file type during pruning");
            }
        }
    }

    Ok(())
}

/// Extract contents of a Dir (files + subdirs) into target path.
///
/// Uses atomic writes (temp file + rename) to avoid partial files
/// if the process crashes during extraction.
fn extract_dir_contents(dir: &Dir, target: &Path) -> std::io::Result<()> {
    for file in dir.files() {
        let Some(name) = file.path().file_name() else {
            warn!(path = ?file.path(), "Bundled file has no file_name, skipping");
            continue;
        };
        let dest = target.join(name);
        // Atomic write: write to a uniquely-named temp file, then rename.
        // The temp name includes a nanosecond timestamp so concurrent extractors
        // (e.g. rapid server restart) cannot collide on the same temp file.
        let tmp_name = format!(
            ".{}.tmp.{}",
            name.to_string_lossy(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let tmp = target.join(&tmp_name);
        std::fs::write(&tmp, file.contents())?;
        std::fs::rename(&tmp, &dest)?;
    }

    for subdir in dir.dirs() {
        let Some(name) = subdir.path().file_name() else {
            warn!(path = ?subdir.path(), "Bundled dir has no file_name, skipping");
            continue;
        };
        let subdir_target = target.join(name);
        std::fs::create_dir_all(&subdir_target)?;
        extract_dir_contents(subdir, &subdir_target)?;
    }

    Ok(())
}

/// Remove legacy `~/.aleph/skills-official/` directory if it exists.
fn cleanup_legacy_dir(aleph_home: &Path) {
    let legacy = aleph_home.join("skills-official");
    // Use symlink_metadata so we don't follow symlinks.
    let meta = match legacy.symlink_metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(error = %e, "Failed to stat legacy skills-official path");
            return;
        }
    };
    if meta.is_dir() {
        info!("Removing legacy skills-official directory");
        if let Err(e) = std::fs::remove_dir_all(&legacy) {
            warn!(error = %e, "Failed to remove legacy skills-official directory");
        }
    } else {
        warn!(
            path = %legacy.display(),
            "Legacy skills-official exists but is not a directory, skipping removal"
        );
    }
}
