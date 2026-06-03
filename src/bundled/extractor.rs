//! Bundled content extractor — extracts embedded skills/plugins on startup.
//!
//! Extraction occurs when:
//! - manifest.json doesn't exist (first install or upgrade from old version)
//! - bundled_version differs from manifest's bundled_version

use super::manifest::{InstallRegistry, SkillEntry, SkillOrigin};
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
    let mut manifest = match InstallRegistry::load(&skills_dir) {
        Some(m) => m,
        None => {
            // First run or upgrade from old version — reconcile existing skills first
            info!("No skills manifest found, performing initial reconcile");
            let mut m = InstallRegistry::new("");
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

    // Reconcile before extracting so that user skills added since the last run
    // (manually placed, not yet tracked in the manifest) are marked Local and
    // therefore skipped by extract_skills. Without this, a bundled skill with
    // the same name would overwrite and prune the user's files on upgrade.
    if let Err(e) = manifest.reconcile(&skills_dir) {
        warn!(error = %e, "Failed to reconcile before extraction; user skills may be at risk");
    }

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
fn extract_skills(bundled: &Dir, skills_dir: &Path, manifest: &mut InstallRegistry) -> bool {
    let mut all_ok = true;

    for dir in bundled.dirs() {
        // Use file_name() to get the base directory name, matching reconcile()
        let Some(name_os) = dir.path().file_name() else {
            warn!(path = ?dir.path(), "Bundled skill dir has no file_name, skipping");
            continue;
        };
        let name = name_os.to_string_lossy().to_string();

        // Defensive: reject empty or path-traversal directory names
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.len() > 255
        {
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

    // Clean up any leftover temp directory from a previous crash.
    // Use symlink_metadata to avoid following symlinks — prevents deletion
    // outside the cache_dir if tmp_dir is a malicious symlink.
    if let Ok(meta) = tmp_dir.symlink_metadata() {
        if meta.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&tmp_dir) {
                warn!(error = %e, "Failed to remove old plugin cache temp directory");
                return false;
            }
        } else {
            warn!(path = %tmp_dir.display(), "Plugin cache temp path exists but is not a directory, skipping removal");
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
            if swap_dir_into_place(&tmp_dir, cache_dir) {
                info!("Extracted bundled plugins to marketplace cache");
                true
            } else {
                false
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to extract bundled plugins");
            false
        }
    }
}

/// Atomically move `staged` into `dest`, replacing any existing `dest`.
///
/// `dest` is expected to already exist (the caller creates it with
/// `create_dir_all`), so on every run after the first the rename must replace a
/// *populated* directory. Renaming onto an existing directory fails differently
/// per platform:
/// - Unix: a non-empty target yields `ENOTEMPTY` → `ErrorKind::DirectoryNotEmpty`.
/// - Windows: rename cannot overwrite a directory at all → `ErrorKind::AlreadyExists`.
///
/// In both cases the destination already exists, so we remove it and retry.
/// Returns true on success. (Previously only `AlreadyExists` was handled, so
/// the swap failed on every Unix upgrade once a cache was present.)
fn swap_dir_into_place(staged: &Path, dest: &Path) -> bool {
    if let Err(e) = std::fs::rename(staged, dest) {
        let dest_exists = matches!(
            e.kind(),
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
        );
        if !dest_exists {
            warn!(error = %e, "Failed to atomically swap plugin cache");
            return false;
        }
        if let Err(e) = std::fs::remove_dir_all(dest) {
            warn!(error = %e, "Failed to remove old plugin cache before swap");
            return false;
        }
        if let Err(e) = std::fs::rename(staged, dest) {
            warn!(error = %e, "Failed to atomically swap plugin cache after removing old");
            return false;
        }
    }
    true
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
            ".{}.tmp.{}.{}",
            name.to_string_lossy(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let tmp = target.join(&tmp_name);
        if let Err(e) = std::fs::write(&tmp, file.contents()) {
            // Clean up the (possibly partial) temp file so we don't leak it.
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                let _ = std::fs::remove_file(&dest);
                if let Err(e) = std::fs::rename(&tmp, &dest) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
            } else {
                // Clean up the temp file so we don't leak partial extractions.
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }

    for subdir in dir.dirs() {
        let Some(name) = subdir.path().file_name() else {
            warn!(path = ?subdir.path(), "Bundled dir has no file_name, skipping");
            continue;
        };
        let subdir_target = target.join(name);
        extract_dir_recursive(subdir, &subdir_target)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: an upgrade swaps the staged cache over a *non-empty* existing
    /// cache. On Unix this rename fails with ENOTEMPTY; the swap must fall back
    /// to remove-then-rename rather than giving up.
    #[test]
    fn swap_dir_replaces_nonempty_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged");
        let dest = tmp.path().join("dest");

        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("new.txt"), b"new").unwrap();

        // Pre-existing, populated destination — the upgrade scenario.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), b"old").unwrap();

        assert!(swap_dir_into_place(&staged, &dest));
        assert!(dest.join("new.txt").exists(), "new content present");
        assert!(!dest.join("old.txt").exists(), "old content replaced");
        assert!(!staged.exists(), "staged dir consumed by rename");
    }

    /// First install: destination exists but is empty (created via
    /// create_dir_all). The direct rename should succeed.
    #[test]
    fn swap_dir_into_empty_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged");
        let dest = tmp.path().join("dest");

        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("f.txt"), b"x").unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        assert!(swap_dir_into_place(&staged, &dest));
        assert!(dest.join("f.txt").exists());
    }

    /// Nested content must survive the swap intact.
    #[test]
    fn swap_dir_preserves_nested_content() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged");
        let dest = tmp.path().join("dest");

        std::fs::create_dir_all(staged.join("sub")).unwrap();
        std::fs::write(staged.join("sub").join("deep.txt"), b"deep").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("stale.txt"), b"stale").unwrap();

        assert!(swap_dir_into_place(&staged, &dest));
        assert_eq!(
            std::fs::read_to_string(dest.join("sub").join("deep.txt")).unwrap(),
            "deep"
        );
        assert!(!dest.join("stale.txt").exists());
    }
}
