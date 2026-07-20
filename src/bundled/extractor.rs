//! Bundled content extractor — extracts embedded skills/plugins on startup.
//!
//! Extraction occurs when:
//! - manifest.json doesn't exist (first install or upgrade from old version)
//! - `bundled_version` differs from manifest's `bundled_version`

use super::manifest::{InstallRegistry, SkillEntry, SkillOrigin};
use super::{BUNDLED_PLUGINS, BUNDLED_SKILLS, BUNDLED_VERSION};
use include_dir::Dir;
use std::path::Path;
use tracing::{debug, info, warn};

/// Which official content to refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Skills,
    Plugins,
    All,
}

/// Result of an explicit sync.
#[derive(Debug, Clone, Copy)]
pub struct SyncReport {
    pub skills: bool,
    pub plugins: bool,
}

/// Explicit refresh (the `bundled.sync` RPC / CLI) against the official repos.
pub fn sync_official_now(aleph_home: &Path, kind: SyncKind) -> Result<SyncReport, String> {
    sync_official_with_urls(
        aleph_home,
        kind,
        crate::bundled::OFFICIAL_SKILLS_REPO,
        crate::bundled::OFFICIAL_PLUGINS_REPO,
    )
}

/// URL-injectable core of the explicit refresh. Clones each requested repo's
/// latest `main` into an isolated checkout under `~/.aleph/cache/`, then
/// re-extracts via the fs-source path. Returns `Err` if every requested clone
/// failed (nothing was refreshed). Injectable URLs keep this deterministically
/// testable with local fixture repos (no network).
pub(crate) fn sync_official_with_urls(
    aleph_home: &Path,
    kind: SyncKind,
    skills_url: &str,
    plugins_url: &str,
) -> Result<SyncReport, String> {
    let skills_dir = aleph_home.join("skills");
    let plugins_cache = aleph_home
        .join("plugins")
        .join("cache")
        .join("aleph-official");
    let cache = aleph_home.join("cache");
    if let Err(e) = std::fs::create_dir_all(&skills_dir) {
        return Err(format!(
            "failed to create skills directory {}: {e}",
            skills_dir.display()
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&plugins_cache) {
        return Err(format!(
            "failed to create plugins cache directory {}: {e}",
            plugins_cache.display()
        ));
    }

    let mut report = SyncReport {
        skills: false,
        plugins: false,
    };
    let mut last_err: Option<String> = None;

    if matches!(kind, SyncKind::Skills | SyncKind::All) {
        let checkout = cache.join("aleph-skills-checkout");
        match crate::bundled::clone_or_update(skills_url, &checkout) {
            Ok(()) if !checkout_has_content(&checkout) => {
                // A clone whose remote HEAD is unborn yields an empty working
                // tree. Treat that as a failed refresh so the caller falls back
                // to the embedded snapshot instead of recording a hollow success.
                last_err = Some(format!(
                    "cloned skills checkout is empty: {}",
                    checkout.display()
                ));
            }
            Ok(()) => {
                let mut manifest =
                    InstallRegistry::load(&skills_dir).unwrap_or_else(|| InstallRegistry::new(""));
                let _ = manifest.reconcile(&skills_dir);
                report.skills = extract_skill_tree_from_dir(&checkout, &skills_dir, &mut manifest);
                let _ = manifest.reconcile(&skills_dir);
                let _ = manifest.save(&skills_dir);
            }
            Err(e) => last_err = Some(e),
        }
    }
    if matches!(kind, SyncKind::Plugins | SyncKind::All) {
        let checkout = cache.join("aleph-plugins-checkout");
        match crate::bundled::clone_or_update(plugins_url, &checkout) {
            Ok(()) if !checkout_has_content(&checkout) => {
                last_err = Some(format!(
                    "cloned plugins checkout is empty: {}",
                    checkout.display()
                ));
            }
            Ok(()) => report.plugins = extract_plugins_from_dir(&checkout, &plugins_cache),
            Err(e) => last_err = Some(e),
        }
    }

    if !report.skills && !report.plugins {
        return Err(last_err.unwrap_or_else(|| "nothing synced".into()));
    }
    Ok(report)
}

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

    // First-run bootstrap: try to pull the latest official content from the
    // external repos; on any failure fall back to the embedded snapshot below.
    let first_run = manifest.skills.is_empty() && manifest.bundled_version.is_empty();
    if first_run {
        match sync_official_now(aleph_home, SyncKind::All) {
            Ok(r) => {
                info!(
                    skills = r.skills,
                    plugins = r.plugins,
                    "Bootstrapped official content from remote"
                );
                // sync_official_now already wrote the manifest to disk with the
                // cloned skills stamped Official. Adopt it unconditionally — even
                // on a partial success (e.g. skills cloned, plugins failed) — so
                // the embedded fall-through below does not reconcile the freshly
                // cloned Official skills back into Local (which would make a later
                // official sync skip them).
                if let Some(m) = InstallRegistry::load(&skills_dir) {
                    manifest = m;
                }
                if r.skills && r.plugins {
                    manifest.bundled_version = BUNDLED_VERSION.to_string();
                    let _ = manifest.reconcile(&skills_dir);
                    let _ = manifest.save(&skills_dir);
                    cleanup_legacy_dir(aleph_home);
                    return;
                }
            }
            Err(e) => {
                warn!(error = %e, "Remote bootstrap failed, falling back to embedded snapshot")
            }
        }
    }

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
                // Record the skill as Official even though extraction failed.
                // Otherwise the partially-written directory is left untracked,
                // and the next reconcile() reclassifies it as a user-owned
                // Local skill — which extract_skills then permanently skips,
                // leaving a half-extracted skill that never self-heals and is
                // mislabeled as a user skill. version=None marks it incomplete;
                // since bundled_version is not bumped on failure, the next
                // startup re-extracts and repairs it.
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Official,
                        version: None,
                        url: None,
                        installed_at: None,
                    },
                );
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

/// Filesystem-source analogue of `extract_skills`: walks a cloned checkout
/// (each top-level dir is one skill) and applies the SAME manifest gating —
/// official-only overwrite, skip names owned by non-official skills, prune
/// stale files within each official skill dir. Returns true if all extractions
/// succeeded.
pub(crate) fn extract_skill_tree_from_dir(
    src_root: &Path,
    skills_dir: &Path,
    manifest: &mut InstallRegistry,
) -> bool {
    let mut all_ok = true;
    let entries = match std::fs::read_dir(src_root) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, src = %src_root.display(), "Failed to read cloned skills checkout");
            return false;
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        // Use symlink_metadata so a top-level symlink-to-dir is NOT classified
        // as a directory — copying through such a link would pull arbitrary
        // external content under skills_dir. Match the convention already used
        // by `copy_dir_into` / `copy_tree_with_prune`'s keep-side.
        let is_dir = std::fs::symlink_metadata(entry.path())
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue; // skip top-level files (.gitignore, README.md, .git)
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.contains('/') || name.contains('\\') || name.len() > 255 {
            continue;
        }
        // Skip if a non-official skill already owns this name (user/3rd-party wins).
        if let Some(e) = manifest.skills.get(&name) {
            if e.source != SkillOrigin::Official {
                debug!(skill = %name, source = ?e.source, "Skipping user-owned skill name");
                continue;
            }
        }
        let target = skills_dir.join(&name);
        match copy_tree_with_prune(&entry.path(), &target) {
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
            }
            Err(e) => {
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Official,
                        version: None,
                        url: None,
                        installed_at: None,
                    },
                );
                warn!(skill = %name, error = %e, "Failed to extract skill from checkout");
                all_ok = false;
            }
        }
    }
    all_ok
}

/// Filesystem-source analogue of `extract_plugins`: atomically swap the cloned
/// plugins checkout into the official marketplace cache dir.
pub(crate) fn extract_plugins_from_dir(src_root: &Path, cache_dir: &Path) -> bool {
    let tmp_dir = cache_dir.with_extension("tmp");

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
        warn!(error = %e, "Failed to create plugin cache temp dir");
        return false;
    }
    if let Err(e) = copy_dir_into(src_root, &tmp_dir) {
        warn!(error = %e, "Failed to copy plugins from checkout");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return false;
    }
    swap_dir_into_place(&tmp_dir, cache_dir)
}

/// Public alias for installing a single skill leaf from a filesystem source
/// (used by the hub GitDir→skill installer). Copies + prunes like an official sync.
pub(crate) fn copy_skill_leaf(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    copy_tree_with_prune(src, dst)
}

/// Recursively copy `src` → `dst`, skipping VCS metadata, then prune any
/// entries in `dst` no longer present in `src` (mirrors `prune_stale_entries`).
fn copy_tree_with_prune(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    copy_dir_into(src, dst)?;
    use std::collections::HashSet;
    let mut keep: HashSet<std::ffi::OsString> = HashSet::new();
    for e in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
        if is_vcs_meta(&e.file_name()) {
            continue;
        }
        // Use symlink_metadata so symlinks are not preserved; copying skips them,
        // so leaving them in `keep` would leave stale/dangling symlinks behind.
        let ft = std::fs::symlink_metadata(e.path())?.file_type();
        if !ft.is_symlink() {
            keep.insert(e.file_name());
        }
    }
    for e in std::fs::read_dir(dst)?.filter_map(|e| e.ok()) {
        if !keep.contains(&e.file_name()) {
            let p = e.path();
            // Use symlink_metadata so a symlink-to-dir here is treated as a
            // symlink (and removed via remove_file), not recursed into via
            // remove_dir_all — matches the keep-side already using
            // symlink_metadata a few lines above.
            let ft = std::fs::symlink_metadata(&p)?.file_type();
            if ft.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(())
}

/// Recursively copy directory contents (no prune), skipping `.git`/`.gitignore`.
fn copy_dir_into(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        if is_vcs_meta(&name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        // Use symlink_metadata so we don't follow symlinks — prevents copying
        // files from outside the source tree or recursing into symlinked dirs.
        let ft = std::fs::symlink_metadata(entry.path())?.file_type();
        if ft.is_dir() {
            copy_dir_into(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        } else {
            // Skip symlinks and other special files.
            continue;
        }
    }
    Ok(())
}

fn is_vcs_meta(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_string_lossy().as_ref(),
        ".git" | ".gitignore" | ".gitmodules"
    )
}

/// True if `dir` holds at least one non-VCS-metadata entry. An empty working
/// tree (e.g. a clone whose remote HEAD is unborn) contains only `.git`, so this
/// distinguishes a real checkout from a broken/empty clone.
fn checkout_has_content(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| !is_vcs_meta(&e.file_name()))
        })
        .unwrap_or(false)
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

    #[test]
    fn extract_skill_tree_from_dir_copies_official_and_skips_user_owned() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(src.join("api-design")).unwrap();
        std::fs::write(src.join("api-design").join("SKILL.md"), b"# api").unwrap();
        std::fs::create_dir_all(src.join("search")).unwrap();
        std::fs::write(src.join("search").join("SKILL.md"), b"# official search").unwrap();
        std::fs::create_dir_all(&skills).unwrap();

        // User already owns a skill named "search" → official sync must skip it.
        let mut manifest = InstallRegistry::new("");
        manifest.skills.insert(
            "search".to_string(),
            SkillEntry {
                source: SkillOrigin::Local,
                version: None,
                url: None,
                installed_at: None,
            },
        );

        let ok = extract_skill_tree_from_dir(&src, &skills, &mut manifest);
        assert!(ok);
        assert!(
            skills.join("api-design").join("SKILL.md").exists(),
            "official extracted"
        );
        assert!(!skills.join("search").exists(), "user-owned name skipped");
        assert_eq!(
            manifest.skills.get("api-design").unwrap().source,
            SkillOrigin::Official
        );
        assert_eq!(
            manifest.skills.get("search").unwrap().source,
            SkillOrigin::Local
        );
    }

    /// Build a local git repo containing a single skill leaf `<leaf>/SKILL.md`.
    fn make_skill_repo(dir: &std::path::Path, leaf: &str, body: &str) -> String {
        std::fs::create_dir_all(dir.join(leaf)).unwrap();
        std::fs::write(dir.join(leaf).join("SKILL.md"), body).unwrap();
        let repo = git2::Repository::init(dir).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // Point HEAD at main so the clone checks out our commit (libgit2 inits
        // HEAD to an unborn `master`; without this the clone's working tree is
        // empty). Mirrors `sync::tests::make_source_repo`.
        repo.set_head("refs/heads/main").unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn sync_official_with_urls_extracts_skills_from_local_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_skill_repo(&src, "api-design", "# api");
        let home = tmp.path().join("home");

        let report = sync_official_with_urls(&home, SyncKind::Skills, &url, "").expect("ok");
        assert!(report.skills);
        assert!(home
            .join("skills")
            .join("api-design")
            .join("SKILL.md")
            .exists());
    }

    #[test]
    fn sync_official_with_urls_reports_err_on_bad_url() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let report = sync_official_with_urls(&home, SyncKind::Skills, "/nonexistent/repo/path", "");
        assert!(report.is_err());
    }

    /// A clone whose remote HEAD is unborn (commit on `main`, HEAD left on the
    /// default `master`) yields an EMPTY working tree. The sync must report that
    /// as a failure so the startup bootstrap falls back to the embedded snapshot
    /// instead of recording a hollow success that suppresses the fallback.
    #[test]
    fn sync_official_with_urls_treats_empty_checkout_as_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("api-design")).unwrap();
        std::fs::write(src.join("api-design").join("SKILL.md"), b"# api").unwrap();
        let repo = git2::Repository::init(&src).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // Deliberately do NOT set HEAD to main → clone checks out unborn `master`.
        let url = src.to_string_lossy().to_string();
        let home = tmp.path().join("home");

        let report = sync_official_with_urls(&home, SyncKind::Skills, &url, "");
        assert!(report.is_err(), "empty checkout must fail, got {report:?}");
        assert!(!home
            .join("skills")
            .join("api-design")
            .join("SKILL.md")
            .exists());
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
