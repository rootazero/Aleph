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
            m.reconcile(&skills_dir);
            m
        }
    };

    // Check if extraction is needed
    if manifest.bundled_version == BUNDLED_VERSION {
        debug!(version = BUNDLED_VERSION, "Bundled content is up to date");
        // Still reconcile to catch manually added skills
        manifest.reconcile(&skills_dir);
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
    manifest.reconcile(&skills_dir);
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
/// Overwrites the entire cache directory.
fn extract_plugins(bundled: &Dir, cache_dir: &Path) -> bool {
    // Remove existing cache and recreate
    if cache_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(cache_dir) {
            warn!(error = %e, "Failed to remove old plugin cache");
            return false;
        }
    }
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        warn!(error = %e, "Failed to create plugin cache directory");
        return false;
    }

    // Extract all files and directories
    match extract_dir_contents(bundled, cache_dir) {
        Ok(()) => {
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

    let mut read_err = None;
    if let Ok(entries) = std::fs::read_dir(target) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            if !bundle_names.contains(&name) {
                let path = entry.path();
                if path.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(&path) {
                        warn!(path = %path.display(), error = %e, "Failed to remove stale directory");
                    }
                } else if let Err(e) = std::fs::remove_file(&path) {
                    warn!(path = %path.display(), error = %e, "Failed to remove stale file");
                }
            }
        }
    } else {
        read_err = Some(std::io::Error::other(
            "Failed to read target directory for pruning",
        ));
    }

    if let Some(e) = read_err {
        return Err(e);
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
        // Atomic write: write to temp file, then rename
        let tmp = target.join(format!(".{}.tmp", name.to_string_lossy()));
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
    if legacy.exists() {
        info!("Removing legacy skills-official directory");
        if let Err(e) = std::fs::remove_dir_all(&legacy) {
            warn!(error = %e, "Failed to remove legacy skills-official directory");
        }
    }
}
