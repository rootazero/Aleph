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
    let plugins_cache = aleph_home.join("plugins").join("cache").join("aleph-official");

    // Ensure directories exist
    let _ = std::fs::create_dir_all(&skills_dir);
    let _ = std::fs::create_dir_all(&plugins_cache);

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
        info!(version = BUNDLED_VERSION, "Bundled content extraction complete");
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
        let name = dir.path().to_string_lossy().to_string();

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

/// Recursively extract an include_dir Dir to a filesystem path.
fn extract_dir_recursive(dir: &Dir, target: &Path) -> std::io::Result<()> {
    // Remove existing and recreate (for clean update)
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    std::fs::create_dir_all(target)?;

    extract_dir_contents(dir, target)
}

/// Extract contents of a Dir (files + subdirs) into target path.
fn extract_dir_contents(dir: &Dir, target: &Path) -> std::io::Result<()> {
    for file in dir.files() {
        let file_path = target.join(file.path().file_name().unwrap_or_default());
        std::fs::write(&file_path, file.contents())?;
    }

    for subdir in dir.dirs() {
        let subdir_name = subdir.path().file_name().unwrap_or_default();
        let subdir_target = target.join(subdir_name);
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
