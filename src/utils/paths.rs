//! Path utilities for Aleph configuration and data files
//!
//! This module provides helper functions for getting paths to various
//! Aleph configuration and data directories.
//!
//! Cross-platform support:
//! - All platforms: Uses ~/.aleph/ (unified path)
//!
//! Note: This was changed from ~/.config/aleph/ to ~/.aleph/ for better
//! Windows compatibility (avoids nested .config directory).
//!
//! Fallback for home directory:
//! - Unix: Uses $HOME environment variable
//! - Windows: Uses $USERPROFILE or $HOMEDRIVE+$HOMEPATH

use crate::error::{AlephError, Result};
use std::path::{Path, PathBuf};

/// Process-global environment guard for tests that mutate `ALEPH_HOME`.
/// Acquiring this mutex serialises tests so they don't observe each other's
/// temporary directories or leave stale values behind.
///
/// **This is the single source of ALEPH_HOME mutual exclusion.** Any test that
/// sets/removes `ALEPH_HOME` MUST hold this guard for its whole body:
/// `let _g = ALEPH_HOME_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());`.
/// Do NOT introduce a separate `#[serial(...)]` group for this — two regimes do
/// not exclude each other, and a mutex-guarded test will then observe the
/// serial-group test's dropped tempdir mid-save (config paths resolve off
/// `ALEPH_HOME`).
#[cfg(test)]
pub(crate) static ALEPH_HOME_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct AlephHomeEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl AlephHomeEnvGuard {
    pub(crate) fn acquire_and_set(value: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("ALEPH_HOME");
        std::env::set_var("ALEPH_HOME", value);
        Self {
            _lock: lock,
            previous,
        }
    }
}

#[cfg(test)]
impl Drop for AlephHomeEnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("ALEPH_HOME", value),
            None => std::env::remove_var("ALEPH_HOME"),
        }
    }
}

/// Get the user's home directory in a cross-platform way
///
/// Tries in order:
/// 1. HOME environment variable (Unix standard, also works on Git Bash for Windows)
/// 2. USERPROFILE environment variable (Windows standard)
/// 3. HOMEDRIVE + HOMEPATH (older Windows fallback)
///
/// # Returns
/// * `Result<PathBuf>` - Path to home directory
///
/// # Errors
/// Returns error if no home directory can be determined
pub fn get_home_dir() -> Result<PathBuf> {
    // Try HOME first (Unix standard, also set in Git Bash/MSYS2 on Windows)
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }

    // Try USERPROFILE (Windows standard)
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }

    // Try HOMEDRIVE + HOMEPATH (older Windows)
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        return Ok(PathBuf::from(format!("{drive}{path}")));
    }

    Err(AlephError::config(
        "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
    ))
}

/// Get the Aleph configuration directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.aleph/
///
/// This ensures that configuration, memory database, skills, and other
/// data are stored in a consistent location regardless of the operating system.
///
/// # Returns
/// * `Result<PathBuf>` - Path to config directory (~/.aleph/)
///
/// # Errors
/// Returns error if home directory cannot be determined
pub fn get_config_dir() -> Result<PathBuf> {
    // Explicit override: `ALEPH_HOME` points directly at the `.aleph` data
    // directory (same convention as canvas_io / cron carryover). This is the
    // single authoritative knob for relocating *all* Aleph state — honoured
    // here so config, data, vault and lock resolution stay consistent (e.g.
    // test harnesses can fully isolate from the real ~/.aleph). When unset,
    // behaviour is byte-identical to before.
    if let Some(dir) = std::env::var_os("ALEPH_HOME") {
        return Ok(PathBuf::from(dir));
    }

    // Use unified path ~/.aleph/ across all platforms
    let home_dir = get_home_dir()?;
    Ok(home_dir.join(".aleph"))
}

/// Get the path for the config.toml file
///
/// Returns: `<config_dir>/config.toml`
pub fn get_config_file_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("config.toml"))
}

/// Get the cache directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.aleph/cache/
///
/// This keeps all Aleph data under the same root directory.
pub fn get_cache_dir() -> Result<PathBuf> {
    // Use unified path ~/.aleph/cache/ across all platforms
    Ok(get_config_dir()?.join("cache"))
}

/// Get the path for the memory database directory (`SQLite`)
///
/// Returns: `<config_dir>/memory/`
///
/// Creates the directory if it doesn't exist.
pub fn get_memory_db_path() -> Result<PathBuf> {
    let memory_dir = get_config_dir()?.join("memory");
    std::fs::create_dir_all(&memory_dir)
        .map_err(|e| AlephError::config(format!("Failed to create memory directory: {e}")))?;
    Ok(memory_dir)
}

/// Get skills directory path
///
/// Returns: `<config_dir>/skills`
pub fn get_skills_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("skills"))
}

/// Get skills directory path as String (for `UniFFI` export)
pub fn get_skills_dir_string() -> Result<String> {
    Ok(get_skills_dir()?.to_string_lossy().to_string())
}

/// Get runtimes directory path
///
/// Returns: `<config_dir>/runtimes`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("runtimes"))
}

/// Get the data directory for operational databases
///
/// Returns: `<config_dir>/data/`
///
/// Creates the directory if it doesn't exist.
pub fn get_data_dir() -> Result<PathBuf> {
    let data_dir = get_config_dir()?.join("data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| AlephError::config(format!("Failed to create data directory: {e}")))?;
    Ok(data_dir)
}

/// Get the note memory directory for compiled knowledge notes.
///
/// Returns: `<config_dir>/memory/note/`
///
/// Creates the directory if it doesn't exist.
pub fn get_note_memory_dir() -> Result<PathBuf> {
    let dir = get_config_dir()?.join("memory").join("note");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AlephError::config(format!("Failed to create note memory directory: {e}")))?;
    Ok(dir)
}

/// Get the raw memory directory for original sources.
///
/// Returns: `<config_dir>/memory/raw/`
///
/// Creates the directory if it doesn't exist.
pub fn get_raw_memory_dir() -> Result<PathBuf> {
    let dir = get_config_dir()?.join("memory").join("raw");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AlephError::config(format!("Failed to create raw memory directory: {e}")))?;
    Ok(dir)
}

/// Get the path for the devices database
///
/// Returns: `<data_dir>/devices.db`
pub fn get_devices_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("devices.db"))
}

/// Get the path for the security database
///
/// Returns: `<data_dir>/security.db`
pub fn get_security_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("security.db"))
}

/// Get the path for the pairing database
///
/// Returns: `<data_dir>/pairing.db`
pub fn get_pairing_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("pairing.db"))
}

/// Get the path for the sessions database
///
/// Returns: `<data_dir>/sessions.db`
pub fn get_sessions_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("sessions.db"))
}

/// Get the path for the scratchpad session→plan binding store.
///
/// Returns: `<data_dir>/scratchpad_bindings.json`
///
/// Mirrors the goal-loop hook's session→active-plan pointers so that an
/// in-flight multi-step task keeps its continuation across a daemon restart
/// (see [`crate::builtin_tools::scratchpad_registry`]).
pub fn get_scratchpad_bindings_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("scratchpad_bindings.json"))
}

// ============================================================================
// Multi-location Skills Discovery (OpenCode Compatible)
// ============================================================================

/// Find the git repository root by traversing up from the start directory
///
/// Looks for a .git directory (or file for worktrees) starting from `start`
/// and traversing up to the filesystem root.
///
/// # Arguments
///
/// * `start` - The directory to start searching from
///
/// # Returns
///
/// * `Option<PathBuf>` - The git root directory, or None if not found
#[must_use]
pub fn find_git_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let git_path = current.join(".git");
        if git_path.exists() {
            return Some(current);
        }

        // Move up one directory
        if !current.pop() {
            // Reached filesystem root
            return None;
        }
    }
}

/// Get all skills directories in priority order
///
/// Implements multi-location skill discovery following `OpenCode`'s pattern,
/// in descending precedence:
///
/// 0. **Agent level** (highest precedence, only when a run is active):
///    - `~/.aleph/agents/<id>/skills` - the currently-active agent's private skills
///
/// 1. **Project level** (traverse up from current directory to git root):
///    - `.aleph/skills/` - Aleph native
///    - `.claude/skills/` - Claude Code compatibility
///
/// 2. **User level** (global):
///    - `~/.aleph/skills` - Aleph native
///    - `~/.claude/skills` - Claude Code compatibility
///
/// # Arguments
///
/// * `project_dir` - Optional project directory to start from. If None, uses current directory.
///
/// # Returns
///
/// * `Vec<PathBuf>` - List of skill directories that exist, in priority order
///
/// # Example
///
/// ```rust,ignore
/// let dirs = get_all_skills_dirs(Some("/path/to/project"))?;
/// for dir in dirs {
///     // Scan for SKILL.md files
/// }
/// ```
fn collect_project_skills_dirs(
    start_dir: &std::path::Path,
    stop_at: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    use tracing::info;

    let mut dirs = Vec::new();
    let mut current = start_dir.to_path_buf();
    loop {
        // Check .aleph/skills/
        let aleph_skills = current.join(".aleph").join("skills");
        if aleph_skills.is_dir() && !dirs.contains(&aleph_skills) {
            info!(path = %aleph_skills.display(), "Found project-level .aleph/skills");
            dirs.push(aleph_skills);
        }

        // Check .claude/skills/ (Claude Code compatibility)
        let claude_skills = current.join(".claude").join("skills");
        if claude_skills.is_dir() && !dirs.contains(&claude_skills) {
            info!(path = %claude_skills.display(), "Found project-level .claude/skills");
            dirs.push(claude_skills);
        }

        // Stop at git root or if we've reached filesystem root
        if stop_at.is_some_and(|stop| current == stop) {
            break;
        }

        if !current.pop() {
            break;
        }
    }
    dirs
}

/// Process-global set of installed-plugin skill base directories
/// (`<plugin_root>/skills`), published once by the extension manager after it
/// parses every plugin (`ExtensionManager::load_all`).
///
/// `get_all_skills_dirs` — the single source both the `skill_read` tool and the
/// per-run skill discovery consult — appends these so a plugin's bundled skills
/// are `skill_read`-able by their bare directory name, exactly like a native
/// `~/.aleph/skills` skill. The extension manager owns the authoritative plugin
/// locations (it just parsed them) but runs long after `get_all_skills_dirs`'s
/// callers are constructed, so a process-global publish is used rather than
/// threading the list through every call site (mirrors the `CHANNEL_CONFIG_
/// SNAPSHOT` pattern). Empty until the first `load_all`, which is fine: plugins
/// aren't loaded before then and `skill_read` only runs per-request afterwards.
static PLUGIN_SKILL_DIRS: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());

/// Publish the installed plugins' skill base directories for skill discovery.
/// Called by the extension manager after every (re)load so the set stays in
/// sync with what is actually installed. Replaces the previous set wholesale.
pub fn publish_plugin_skill_dirs(dirs: Vec<PathBuf>) {
    let mut guard = PLUGIN_SKILL_DIRS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = dirs;
}

/// Snapshot the currently-published plugin skill base directories.
#[must_use]
pub fn plugin_skill_dirs() -> Vec<PathBuf> {
    PLUGIN_SKILL_DIRS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// True when `agent_id` is a single safe path component (non-empty, no path
/// separators, no parent refs, no NUL). Mirrors [`get_agent_config_dir`]'s
/// guard so skill discovery can build `~/.aleph/agents/<id>/skills` without
/// risking traversal outside the agents root.
fn is_safe_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && !agent_id.contains('/')
        && !agent_id.contains('\\')
        && !agent_id.contains("..")
        && !agent_id.contains('\0')
}

/// Append each plugin's `skills` directory found under `plugins_root` to `dirs`
/// (deduplicated). A plugin ships its skills at `<plugin>/skills/<skill>/SKILL.md`.
///
/// Mirrors the discovery scanner's plugin-layout support
/// ([`crate::discovery`]'s `scan_plugin_parent`): besides the direct
/// `<entry>/skills`, it descends **one** level for monorepo layouts where a
/// cloned repo holds several plugins (`<entry>/<plugin>/skills`) — only when
/// `<entry>` has no direct `skills` dir, matching the scanner's else-branch.
/// Without the monorepo descent, a monorepo-shipped plugin skill was *indexed*
/// (the loader walks the registry's real `root_dir`) yet `skill_read` /
/// `skill_list` returned `NotFound` — re-opening the `cat` fallback for that
/// install shape (the two enumerations had drifted).
fn collect_plugin_skills_from_root(plugins_root: &Path, dirs: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(plugins_root) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let direct = entry_path.join("skills");
        if direct.is_dir() {
            // Direct layout: <plugins_root>/<plugin>/skills
            if !dirs.contains(&direct) {
                dirs.push(direct);
            }
        } else if let Ok(sub_entries) = std::fs::read_dir(&entry_path) {
            // Monorepo layout: <plugins_root>/<repo>/<plugin>/skills. Only
            // descended when there is no direct `skills` dir, mirroring
            // `scan_plugin_parent`'s "no direct manifest → scan subdirs" branch.
            for sub in sub_entries.flatten() {
                let skills = sub.path().join("skills");
                if skills.is_dir() && !dirs.contains(&skills) {
                    dirs.push(skills);
                }
            }
        }
    }
}

/// Return each installed / project plugin's `skills` subdirectory.
///
/// Plugin skills live at `<plugins_root>/<plugin>/skills`. Surfacing these to the
/// `skill_read` / `skill_list` tools (which resolve skills by directory name)
/// lets the model read a plugin-shipped skill through the skill mechanism instead
/// of falling back to a raw `cat` on the plugin's files. Ordered global-then-project
/// so callers can treat them as the lowest-precedence tier (a user/project skill of
/// the same id shadows the plugin one by first occurrence in `get_all_skills_dirs`;
/// `guess_source` separately classes these paths `SkillSource::Plugin` for the
/// prompt index).
#[must_use]
pub fn get_plugin_skills_dirs(project_dir: Option<&std::path::Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Global: ~/.aleph/plugins/<plugin>/skills
    if let Ok(config_dir) = get_config_dir() {
        collect_plugin_skills_from_root(&config_dir.join("plugins"), &mut dirs);
    }

    // Project: <project|cwd>/.aleph/plugins/<plugin>/skills
    let start = project_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    if let Some(start) = start {
        collect_plugin_skills_from_root(&start.join(".aleph").join("plugins"), &mut dirs);
    }

    dirs
}

pub fn get_all_skills_dirs(project_dir: Option<&std::path::Path>) -> Result<Vec<PathBuf>> {
    use tracing::info;

    let mut dirs = Vec::new();

    // 0. Agent level (highest precedence): the currently-active agent's private
    //    skills at `~/.aleph/agents/<id>/skills`, so an agent can ship/override
    //    skills for its own runs. The id comes from the per-run task-local
    //    (`with_agent_id`, set by the gateway run loop); absent outside a run /
    //    in tests, in which case this is skipped. The directory is only READ —
    //    never created — and the id is validated to stay inside the agents root.
    if let Some(agent_id) = crate::agents::current_agent_id() {
        if is_safe_agent_id(&agent_id) {
            if let Ok(config_dir) = get_config_dir() {
                let agent_skills = config_dir.join("agents").join(&agent_id).join("skills");
                if agent_skills.is_dir() && !dirs.contains(&agent_skills) {
                    info!(
                        path = %agent_skills.display(),
                        agent_id = %agent_id,
                        "Found agent-level ~/.aleph/agents/<id>/skills"
                    );
                    dirs.push(agent_skills);
                }
            }
        }
    }

    // Determine start directory
    let start_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| AlephError::config(format!("Failed to get current directory: {e}")))?,
    };

    info!(
        start_dir = %start_dir.display(),
        "get_all_skills_dirs: Starting discovery"
    );

    // Find git root to limit traversal
    let git_root = find_git_root(&start_dir);
    let stop_at = git_root.as_deref();

    // 1. Project level: traverse up from start to git root
    dirs.extend(collect_project_skills_dirs(&start_dir, stop_at));

    // 2. User level: global directories
    let global_aleph = get_skills_dir()?;
    if global_aleph.is_dir() && !dirs.contains(&global_aleph) {
        info!(path = %global_aleph.display(), "Found global ~/.aleph/skills");
        dirs.push(global_aleph);
    }

    if let Ok(home) = get_home_dir() {
        info!(home = %home.display(), "Checking global directories");

        // ~/.claude/skills (Claude Code compatibility)
        let global_claude = home.join(".claude").join("skills");
        if global_claude.is_dir() && !dirs.contains(&global_claude) {
            info!(path = %global_claude.display(), "Found global ~/.claude/skills");
            dirs.push(global_claude);
        } else {
            info!(
                path = %global_claude.display(),
                exists = global_claude.exists(),
                is_dir = global_claude.is_dir(),
                "~/.claude/skills not found or not a directory"
            );
        }
    }

    // 3. Plugin-shipped skills (lowest precedence). Two sources, unioned and
    //    deduped, appended last so a same-id user/project skill (scanned
    //    earlier) shadows a plugin's — `skill_read`/`skill_list` win by first
    //    occurrence. Without this, `skill_read(<plugin skill>)` returned
    //    NotFound and the model fell back to `cat` on the raw plugin files.
    //    a) Static scan of the well-known roots: ~/.aleph/plugins/<p>/skills
    //       and project .aleph/plugins/<p>/skills — works before the extension
    //       manager has loaded anything.
    for d in get_plugin_skills_dirs(project_dir) {
        if !dirs.contains(&d) {
            info!(path = %d.display(), "Found plugin skills dir");
            dirs.push(d);
        }
    }
    //    b) Base dirs published by the extension manager (`<plugin_root>/skills`)
    //       via `publish_plugin_skill_dirs` — covers plugin roots outside the
    //       well-known locations (e.g. `plugins/cache/<market>/<id>/skills`).
    //       Empty until the first `ExtensionManager::load_all`.
    for plugin_dir in plugin_skill_dirs() {
        if plugin_dir.is_dir() && !dirs.contains(&plugin_dir) {
            info!(path = %plugin_dir.display(), "Found plugin skills dir");
            dirs.push(plugin_dir);
        }
    }

    info!(
        total_dirs = dirs.len(),
        dirs = ?dirs,
        "get_all_skills_dirs: Discovery complete"
    );

    Ok(dirs)
}

/// Get the identity/config directory for a specific agent
///
/// Agent capabilities (skills, plugins) and identity files are stored here.
///
/// Returns: `<config_dir>/agents/<agent_id>/`
///
/// The directory is created if it doesn't exist.
pub fn get_agent_config_dir(agent_id: &str) -> Result<PathBuf> {
    if agent_id.contains('/')
        || agent_id.contains('\\')
        || agent_id.contains("..")
        || agent_id.is_empty()
        || agent_id.contains('\0')
    {
        return Err(AlephError::config(format!(
            "Invalid agent ID '{agent_id}': must not contain path separators, '..', or null bytes"
        )));
    }

    let agent_dir = get_config_dir()?.join("agents").join(agent_id);

    std::fs::create_dir_all(&agent_dir)
        .map_err(|e| AlephError::config(format!("Failed to create agent config directory: {e}")))?;

    Ok(agent_dir)
}

/// Migrate legacy flat database files from ~/.aleph/*.db to ~/.aleph/data/*.db
///
/// This handles the transition from the old flat layout where databases were
/// stored directly in ~/.aleph/ to the new organized layout under ~/.aleph/data/.
/// Only moves files that exist at the old location and don't exist at the new location.
pub fn migrate_legacy_db_files() {
    let Ok(config_dir) = get_config_dir() else {
        return;
    };
    let Ok(data_dir) = get_data_dir() else { return };

    for name in &["devices.db", "security.db", "pairing.db", "sessions.db"] {
        let old = config_dir.join(name);
        let new = data_dir.join(name);
        if old.exists() && !new.exists() {
            if let Err(e) = std::fs::rename(&old, &new) {
                tracing::warn!("Failed to migrate {}: {}", name, e);
            } else {
                tracing::info!("Migrated {} to {}", old.display(), new.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_git_root() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let subdir = project.join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        // Create .git directory
        std::fs::create_dir(project.join(".git")).unwrap();

        // Should find git root from subdirectory
        let root = find_git_root(&subdir);
        assert!(root.is_some());
        assert_eq!(root.unwrap(), project);

        // Should find git root from project root
        let root = find_git_root(&project);
        assert!(root.is_some());
        assert_eq!(root.unwrap(), project);

        // Should not find git root from temp dir (above project)
        let root = find_git_root(temp_dir.path());
        assert!(root.is_none());
    }

    #[test]
    fn test_get_all_skills_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        // Create .git
        std::fs::create_dir(project.join(".git")).unwrap();

        // Create project-level skills
        let aleph_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aleph_skills).unwrap();

        let claude_skills = project.join(".claude").join("skills");
        std::fs::create_dir_all(&claude_skills).unwrap();

        // Get all skills dirs
        let dirs = get_all_skills_dirs(Some(&project)).unwrap();

        // Should find both project-level directories
        assert!(dirs.iter().any(|d| d == &aleph_skills));
        assert!(dirs.iter().any(|d| d == &claude_skills));

        // .aleph should come before .claude (priority order)
        let aleph_idx = dirs.iter().position(|d| d == &aleph_skills);
        let claude_idx = dirs.iter().position(|d| d == &claude_skills);
        assert!(aleph_idx < claude_idx);
    }

    #[test]
    fn aleph_home_is_authoritative_for_skill_resolver() {
        let temp_dir = TempDir::new().unwrap();
        let home = temp_dir.path().join("home");
        let aleph_home = temp_dir.path().join("aleph-home");
        let project = temp_dir.path().join("project");
        let global_aleph = aleph_home.join("skills");
        let legacy_aleph = home.join(".aleph").join("skills");
        let global_claude = home.join(".claude").join("skills");
        std::fs::create_dir_all(&global_aleph).unwrap();
        std::fs::create_dir_all(&legacy_aleph).unwrap();
        std::fs::create_dir_all(&global_claude).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();

        let dirs = {
            let _aleph_home = AlephHomeEnvGuard::acquire_and_set(&aleph_home);
            let _home = crate::runtimes::post_install::HomeEnvGuard::acquire_and_set(&home);
            get_all_skills_dirs(Some(&project)).unwrap()
        };

        assert!(dirs.contains(&global_aleph));
        assert!(dirs.contains(&global_claude));
        assert!(!dirs.contains(&legacy_aleph));
    }

    #[test]
    fn test_get_agent_config_dir_rejects_null_bytes() {
        let result = get_agent_config_dir("agent\0name");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("null bytes"),
            "error should mention null bytes: {}",
            err
        );
    }

    #[test]
    fn test_get_agent_config_dir_rejects_invalid_chars() {
        assert!(get_agent_config_dir("").is_err());
        assert!(get_agent_config_dir("a/b").is_err());
        assert!(get_agent_config_dir("a\\b").is_err());
        assert!(get_agent_config_dir("a..b").is_err());
        assert!(get_agent_config_dir("a\0b").is_err());
    }

    #[test]
    fn get_all_skills_dirs_includes_plugin_skills_last() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();

        // A regular project skills dir and a plugin skills dir.
        let aleph_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aleph_skills).unwrap();
        let plugin_skills = project
            .join(".aleph")
            .join("plugins")
            .join("p")
            .join("skills");
        std::fs::create_dir_all(&plugin_skills).unwrap();

        let dirs = get_all_skills_dirs(Some(&project)).unwrap();

        let aleph_idx = dirs.iter().position(|d| d == &aleph_skills);
        let plugin_idx = dirs.iter().position(|d| d == &plugin_skills);
        assert!(plugin_idx.is_some(), "plugin skills dir must be included");
        assert!(
            aleph_idx < plugin_idx,
            "plugin skills must be lowest precedence (appended last): {dirs:?}"
        );
    }

    #[test]
    fn test_get_all_skills_dirs_subdir() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let subdir = project.join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        // Create .git at project root
        std::fs::create_dir(project.join(".git")).unwrap();

        // Create skills dir at project root
        let aleph_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aleph_skills).unwrap();

        // Search from subdirectory
        let dirs = get_all_skills_dirs(Some(&subdir)).unwrap();

        // Should find the skills dir at project root
        assert!(dirs.iter().any(|d| d == &aleph_skills));
    }

    #[test]
    fn test_get_plugin_skills_dirs_finds_project_plugin_skills() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        // Plugin `alpha` ships skills; `beta` has no skills subdir.
        let alpha_skills = project
            .join(".aleph")
            .join("plugins")
            .join("alpha")
            .join("skills");
        std::fs::create_dir_all(&alpha_skills).unwrap();
        let beta = project.join(".aleph").join("plugins").join("beta");
        std::fs::create_dir_all(&beta).unwrap();

        let dirs = get_plugin_skills_dirs(Some(&project));
        assert!(
            dirs.iter().any(|d| d == &alpha_skills),
            "a plugin's skills dir must be discovered"
        );
        // A plugin without a skills subdir contributes nothing.
        assert!(
            !dirs.iter().any(|d| d.starts_with(&beta)),
            "a plugin without a skills subdir must be absent"
        );
    }

    #[test]
    fn test_get_plugin_skills_dirs_finds_monorepo_plugin_skills() {
        // Monorepo layout: `<plugins>/repo/<plugin>/skills`, where `repo` itself
        // has NO direct `skills` dir. The index side (`scan_plugin_parent`)
        // descends one level here; the read side must match or `skill_read`
        // NotFounds a skill that appears in the index (the enumeration drift).
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let nested_skills = project
            .join(".aleph")
            .join("plugins")
            .join("repo")
            .join("inner-plugin")
            .join("skills");
        std::fs::create_dir_all(&nested_skills).unwrap();

        let dirs = get_plugin_skills_dirs(Some(&project));
        assert!(
            dirs.iter().any(|d| d == &nested_skills),
            "a monorepo-nested plugin's skills dir must be discovered (one-level descent)"
        );
    }

    #[test]
    fn test_get_plugin_skills_dirs_direct_wins_no_double_descent() {
        // A plugin with a DIRECT skills dir must not also trigger the monorepo
        // descent (which would probe `<plugin>/skills/<x>/skills`). Assert the
        // direct dir is present and no spurious deeper dir is added.
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let direct = project
            .join(".aleph")
            .join("plugins")
            .join("alpha")
            .join("skills");
        // A skill under the direct dir — its own (non-existent) `skills` subdir
        // must not be picked up.
        std::fs::create_dir_all(direct.join("do-thing")).unwrap();

        let dirs = get_plugin_skills_dirs(Some(&project));
        assert!(dirs.iter().any(|d| d == &direct));
        assert!(
            !dirs.iter().any(|d| d == &direct.join("do-thing").join("skills")),
            "the direct branch must not descend into the skills dir's children"
        );
    }

    #[test]
    fn test_get_plugin_skills_dirs_missing_root_no_panic() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("empty");
        std::fs::create_dir_all(&project).unwrap();
        // No `.aleph/plugins` under the project → the project tier contributes
        // nothing. (The global tier may exist on the host, so assert the project
        // path specifically is absent rather than the whole vec being empty.)
        let dirs = get_plugin_skills_dirs(Some(&project));
        let project_plugins = project.join(".aleph").join("plugins");
        assert!(!dirs.iter().any(|d| d.starts_with(&project_plugins)));
    }

    #[test]
    fn test_get_all_skills_dirs_appends_plugin_skills_last() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        let aleph_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aleph_skills).unwrap();
        let plugin_skills = project
            .join(".aleph")
            .join("plugins")
            .join("alpha")
            .join("skills");
        std::fs::create_dir_all(&plugin_skills).unwrap();

        let dirs = get_all_skills_dirs(Some(&project)).unwrap();
        assert!(
            dirs.iter().any(|d| d == &plugin_skills),
            "plugin skills dir must be discovered by get_all_skills_dirs"
        );
        // Plugin skills are lowest precedence — appended after project skills, so
        // a same-id user/project skill (scanned earlier) shadows a plugin's.
        let skills_idx = dirs.iter().position(|d| d == &aleph_skills).unwrap();
        let plugin_idx = dirs.iter().position(|d| d == &plugin_skills).unwrap();
        assert!(
            skills_idx < plugin_idx,
            "plugin skills must be appended last (lowest precedence)"
        );
    }

    #[test]
    fn is_safe_agent_id_accepts_plain_ids() {
        assert!(is_safe_agent_id("researcher"));
        assert!(is_safe_agent_id("my-agent_01"));
    }

    #[test]
    fn is_safe_agent_id_rejects_traversal_and_separators() {
        assert!(!is_safe_agent_id(""));
        assert!(!is_safe_agent_id("a/b"));
        assert!(!is_safe_agent_id("a\\b"));
        assert!(!is_safe_agent_id(".."));
        assert!(!is_safe_agent_id("a..b"));
        assert!(!is_safe_agent_id("a\0b"));
    }

    #[test]
    fn agent_skills_dir_absent_without_active_agent_scope() {
        // Outside any `with_agent_id` scope, discovery must not inject an
        // agent-level dir (byte-identical to the pre-wiring behaviour).
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();

        let dirs = get_all_skills_dirs(Some(&project)).unwrap();
        assert!(
            !dirs
                .iter()
                .any(|d| d.to_string_lossy().contains("/agents/")),
            "no agent dir without an active agent scope, got {dirs:?}"
        );
    }
}
