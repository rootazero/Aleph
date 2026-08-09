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

/// Returns true when two paths refer to the same filesystem entry by
/// canonicalizing both (symlink-resistant). Falls back to byte-wise
/// comparison when canonicalization fails for either path.
#[must_use]
pub fn equivalent(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Legacy `MAX_PATH`. Past this length the `\\?\` prefix is load-bearing on
/// Windows, so [`display_string`] leaves it alone.
const LEGACY_MAX_PATH: usize = 260;

/// Render an already-canonical path for a human.
///
/// `std::fs::canonicalize` on Windows returns the extended-length form
/// (`\\?\C:\Users\zou\proj`). That prefix is correct at the API layer and wrong
/// everywhere a person reads it: it has surfaced in the Panel's project chip,
/// the directory browser's rows, and inside server-side refusal messages that
/// embed a path.
///
/// **Display boundary only — never on a value that is stored or compared.**
/// The transform is deliberately partial: a path past [`LEGACY_MAX_PATH`], or a
/// UNC path (`\\?\UNC\server\share`, whose un-prefixed form is not a path at
/// all), keeps its prefix. So simplifying one side of a `starts_with` and not
/// the other silently flips an allow into a deny — which is exactly what an
/// allowed-roots scope check does. Convert once, on the way out.
///
/// The rule runs on every platform rather than under `#[cfg(windows)]`: a Unix
/// path can never carry this prefix, so the arm is a no-op there, and keeping
/// it unconditional means the tests below actually run on the machine you are
/// reading this on.
#[must_use]
pub fn display_string(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let Some(rest) = raw.strip_prefix(r"\\?\") else {
        return raw.into_owned();
    };
    let bytes = rest.as_bytes();
    let is_plain_drive = bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(1) == Some(&b':')
        && bytes.get(2) == Some(&b'\\');
    if is_plain_drive && rest.len() < LEGACY_MAX_PATH {
        return rest.to_string();
    }
    raw.into_owned()
}

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
///
/// **A test that needs `$HOME` as well must go through
/// [`crate::runtimes::post_install::HomeEnvGuards`]** — never take that lock and
/// this one by hand. They are two separate mutexes over two separate env vars,
/// so acquiring them by hand admits two orders, and two orders is an ABBA
/// deadlock: one test taking them in the reverse order to its siblings hung the
/// whole `--lib` suite forever (a hang, not a failure) with every other
/// `ALEPH_HOME` test queued behind it. `HomeEnvGuards` fixes the order in one
/// place, and `nothing_acquires_the_two_env_locks_separately` fails if a new
/// site goes around it.
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

/// A throwaway `ALEPH_HOME` for a test that never names one.
///
/// Aleph state resolves off `ALEPH_HOME`, falling back to the *real* `~/.aleph`
/// when it is unset. So a test that merely reads config or opens a store still
/// lands in the developer's home unless it says otherwise — and there it both
/// mutates real state and races every other unisolated test for it. Two ways
/// that bites, one loud and one quiet:
///
/// * `Config::load()` persists a default config when the file is missing, so a
///   read reaches a write and trips `config::save`'s real-home assertion. It
///   only fires where `~/.aleph/config.toml` does *not* already exist, which is
///   every CI runner and no developer machine — hence green locally, red in CI.
/// * Stores under `~/.aleph/data` are shared, so two tests opening the same
///   fresh SQLite file collide with "database is locked".
///
/// Hold one of these for the whole test body when the test has no opinion about
/// where Aleph's home is — only that it must not be the real one. Reach for
/// [`AlephHomeEnvGuard`] directly instead when the test needs to *populate* the
/// directory before pointing at it. The two share one mutex and neither is
/// reentrant, so never nest them.
#[cfg(test)]
pub(crate) struct IsolatedAlephHome {
    // Declaration order is drop order: restore the env var, then delete the
    // directory it pointed at.
    _guard: AlephHomeEnvGuard,
    _dir: tempfile::TempDir,
}

#[cfg(test)]
impl IsolatedAlephHome {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir for isolated ALEPH_HOME");
        let guard = AlephHomeEnvGuard::acquire_and_set(dir.path());
        Self {
            _guard: guard,
            _dir: dir,
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
    // The ladder itself lives in `aleph_protocol::paths` — see `get_config_dir`
    // for why. This wrapper exists only to turn "not found" into this crate's
    // error type with its actionable message.
    aleph_protocol::paths::home_dir().ok_or_else(|| {
        AlephError::config(
            "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
        )
    })
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
    // test harnesses can fully isolate from the real ~/.aleph).
    //
    // The rule is `aleph_protocol::paths::aleph_home`, not a copy of it. It was
    // moved down there when the CLI needed to find the server's self-signed
    // certificate: `aleph-cli` and `aleph-client` are forbidden to depend on
    // `alephcore`, so the alternative was a second spelling of this rule — and
    // a second spelling of THIS rule in particular is undetectable on any
    // machine where `ALEPH_HOME` is unset, which is every developer's.
    aleph_protocol::paths::aleph_home().ok_or_else(|| {
        AlephError::config(
            "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
        )
    })
}

/// Resolve where a `~/.aleph`-rooted path lived *before* `ALEPH_HOME` existed.
///
/// Several subsystems used to expand their `"~/.aleph/…"` config strings with
/// `dirs::home_dir()`, which ignores `ALEPH_HOME`: relocating the Aleph home
/// silently left their state behind in the real home with no error anywhere.
/// A boot-time migration has to look at exactly that old location, so the
/// legacy spelling is resolved *here* — in the module that owns path
/// resolution and is the one place exempt from the hand-rolled-path guard —
/// instead of every caller re-deriving it and re-seeding the bug.
///
/// `relative` is the part after `~/.aleph/` (e.g. `"data/tasks.db"`).
/// Returns `None` when no home directory can be determined.
#[must_use]
pub fn legacy_home_aleph_path(relative: &str) -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".aleph").join(relative))
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
/// Creates the directory if it doesn't exist — so this is NOT the function a
/// diagnostic or an audit should call (see the module note on
/// [`get_config_dir`] being a pure lookup and this one not being). The layout
/// itself is [`aleph_protocol::paths::data_dir`], because a client that cannot
/// depend on this crate still has to find the same directory.
pub fn get_data_dir() -> Result<PathBuf> {
    let data_dir = aleph_protocol::paths::data_dir().ok_or_else(|| {
        AlephError::config(
            "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
        )
    })?;
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

/// Get the directory for cross-process background sub-agent records.
///
/// Returns: `<data_dir>/background_subagents/`
///
/// One function so the boot-time reconciliation and every later read/write of
/// a sub-agent sidecar resolve the same directory — the same discipline as the
/// scratchpad bindings above (see
/// [`crate::agents::background_persistence`]).
pub fn get_background_subagents_dir() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("background_subagents"))
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
    // Cap depth to prevent unbounded traversal in pathological filesystems
    // (e.g. circular symlink chains, bind mounts). 100 covers any sane repo
    // depth and is the same limit `discovery::paths::find_git_root` used
    // before this consolidation.
    const MAX_DEPTH: usize = 100;

    // Resolve symlinks up front so `current.join(".git").exists()` does not
    // follow a `.git` symlink to an arbitrary directory (which used to mis-
    // report any ancestor dir as a git root). If canonicalize fails (path
    // does not exist, permission denied), fall back to the un-resolved start.
    let mut current = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut depth = 0;

    loop {
        if depth >= MAX_DEPTH {
            return None;
        }
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
                depth += 1;
            }
            None => return None,
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

/// Append the active agent's skills directory (if any) to `dirs`.
fn agent_skills_dir(dirs: &mut Vec<PathBuf>) {
    use tracing::info;
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
}

/// Append global user-level skills directories to `dirs`.
fn user_skills_dirs(dirs: &mut Vec<PathBuf>) -> Result<()> {
    use tracing::info;

    let global_aleph = get_skills_dir()?;
    if global_aleph.is_dir() && !dirs.contains(&global_aleph) {
        info!(path = %global_aleph.display(), "Found global ~/.aleph/skills");
        dirs.push(global_aleph);
    }

    if let Ok(home) = get_home_dir() {
        info!(home = %home.display(), "Checking global directories");
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

    Ok(())
}

/// Push `dir` into `dirs` if it's a directory that isn't already present.
fn push_if_new_skills_dir(dirs: &mut Vec<PathBuf>, dir: &std::path::Path, label: &str) {
    use tracing::info;
    if dir.is_dir() && !dirs.iter().any(|d| d.as_path() == dir) {
        info!(path = %dir.display(), "Found {label}");
        dirs.push(dir.to_path_buf());
    }
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
    agent_skills_dir(&mut dirs);

    let start_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| AlephError::config(format!("Failed to get current directory: {e}")))?,
    };

    info!(
        start_dir = %start_dir.display(),
        "get_all_skills_dirs: Starting discovery"
    );

    let git_root = find_git_root(&start_dir);
    let stop_at = git_root.as_deref();

    // 1. Project level: traverse up from start to git root
    dirs.extend(collect_project_skills_dirs(&start_dir, stop_at));

    // 2. User level: global directories
    user_skills_dirs(&mut dirs)?;

    // 3. Plugin-shipped skills (lowest precedence). Two sources, unioned and
    //    deduped, appended last so a same-id user/project skill (scanned
    //    earlier) shadows a plugin's — `skill_read`/`skill_list` win by first
    //    occurrence. Without this, `skill_read(<plugin skill>)` returned
    //    NotFound and the model fell back to `cat` on the raw plugin files.
    //    a) Static scan of the well-known roots: ~/.aleph/plugins/<p>/skills
    //       and project .aleph/plugins/<p>/skills — works before the extension
    //       manager has loaded anything.
    for d in get_plugin_skills_dirs(project_dir) {
        push_if_new_skills_dir(&mut dirs, &d, "plugin skills dir");
    }
    //    b) Base dirs published by the extension manager (`<plugin_root>/skills`)
    //       via `publish_plugin_skill_dirs` — covers plugin roots outside the
    //       well-known locations (e.g. `plugins/cache/<market>/<id>/skills`).
    //       Empty until the first `ExtensionManager::load_all`.
    for plugin_dir in plugin_skill_dirs() {
        push_if_new_skills_dir(&mut dirs, &plugin_dir, "plugin skills dir");
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
    fn display_string_strips_the_verbatim_prefix_only_where_it_is_reversible() {
        // The case the Panel kept showing.
        assert_eq!(
            display_string(Path::new(r"\\?\C:\Users\zou\proj")),
            r"C:\Users\zou\proj"
        );
        // No prefix ⇒ untouched, on every platform.
        assert_eq!(
            display_string(Path::new("/home/zou/proj")),
            "/home/zou/proj"
        );
        assert_eq!(
            display_string(Path::new(r"C:\Users\zou\proj")),
            r"C:\Users\zou\proj"
        );
        // A UNC share un-prefixed is not a path — leave it whole.
        assert_eq!(
            display_string(Path::new(r"\\?\UNC\server\share\proj")),
            r"\\?\UNC\server\share\proj"
        );
        // Past MAX_PATH the prefix is what makes the path openable, so a
        // rendered-then-pasted value must keep it.
        let long = format!(r"\\?\C:\{}", "x".repeat(LEGACY_MAX_PATH));
        assert_eq!(display_string(Path::new(&long)), long);
    }

    /// Files allowed to hand-roll a `.aleph` path off `dirs::home_dir()`,
    /// each with the reason it is not the bug this guard hunts.
    ///
    /// Everything else must go through [`get_config_dir`] — see the guard's
    /// own doc for why.
    const HOME_JOIN_ALLOWLIST: &[(&str, &str)] = &[
        (
            "src/utils/paths.rs",
            "this module IS the resolver; the allowlist strings live here too",
        ),
        (
            "src/extension/watcher.rs",
            "resolves `.claude` (Claude Code's dir, real home by definition) \
             next to an ALEPH_HOME-aware `.aleph`",
        ),
        (
            "src/extension/marketplace/types.rs",
            "prefers discovery::aleph_home_dir(); home_dir is only the error fallback",
        ),
        (
            "src/extension/mod.rs",
            "prefers discovery::aleph_plugins_dir(); home_dir is only the error fallback",
        ),
        (
            "src/sandbox/proxy/netns_bridge.rs",
            "unix-socket parent chosen for sun_path's 108-byte limit, not for state \
             location; a long ALEPH_HOME would bind-fail after a successful mkdir",
        ),
        (
            "src/bin/aleph-server/daemon.rs",
            "expands a user-written `~/` prefix, which means the real home",
        ),
        (
            "src/config/agent_resolver/mod.rs",
            "resolve_user_path expands a user-written `~` prefix (real home); the \
             file's only `.aleph` goes through get_config_dir()",
        ),
        (
            "src/thinker/project_instructions.rs",
            "home_dir is the upward-walk boundary (real home); its `.aleph/…` \
             strings are *project*-relative directory names, not home-rooted",
        ),
    ];

    /// Files the file-level guard catches today and that are **not** exempt —
    /// each really does hand-roll a home-rooted `.aleph` path and really does
    /// ignore `ALEPH_HOME`. They are grandfathered so tightening the guard from
    /// line-level to file-level could land without a repo-wide edit.
    ///
    /// **This list may only shrink.** Adding an entry means shipping the bug;
    /// fix the file instead. A second assertion below fails when an entry stops
    /// offending, so a fix cannot silently leave a stale exemption behind.
    const HOME_JOIN_PENDING_FIX: &[(&str, &str)] = &[
        ("src/acp/manager/persistence.rs", "acp_sessions.json"),
        ("src/approval/config.rs", "approval-policy.json"),
        (
            "src/bin/aleph-server/commands/service/mod.rs",
            "~/.aleph/logs (the LaunchAgents path beside it is correctly real-home)",
        ),
        ("src/builtin_tools/pdf_generate/mod.rs", "output directory"),
        ("src/builtin_tools/skill_manage.rs", "skills directory"),
        (
            "src/builtin_tools/team/create.rs",
            "agents + workspaces roots",
        ),
        (
            "src/executor/builtin_registry/builder/constructor/collab_session_tools.rs",
            "note memory directory",
        ),
        (
            "src/executor/builtin_registry/builder/constructor/mod.rs",
            "note memory directory (two sites)",
        ),
        ("src/gateway/agent_env/mod.rs", "agent_envs.db"),
        (
            "src/gateway/agent_instance.rs",
            "default workspace + agent dir",
        ),
        ("src/gateway/config.rs", "default agent dir"),
        ("src/gateway/handlers/daemon_control.rs", "log directory"),
        ("src/gateway/handlers/hooks_admin.rs", "hooks.json"),
        (
            "src/gateway/handlers/markdown_skills.rs",
            "skills directory",
        ),
        ("src/gateway/interfaces/wechat/config.rs", "state root"),
        ("src/sandbox/config.rs", "workspaces root"),
        ("src/tools/context.rs", "default workspace"),
    ];

    /// Split a source file into the lines that are real code — the guard's
    /// two markers are both discussed by name in comments and doc comments
    /// throughout the repo, and only code can actually resolve a path.
    fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
        text.lines().enumerate().filter_map(|(i, line)| {
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with('*') {
                None
            } else {
                Some((i + 1, line))
            }
        })
    }

    /// Every `.rs` file under `src/`, as repo-relative slash-separated paths
    /// paired with their contents.
    fn all_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");

        files
            .into_iter()
            .filter_map(|file| {
                let rel = file
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                std::fs::read_to_string(&file).ok().map(|text| (rel, text))
            })
            .collect()
    }

    /// Does this file both reach for the real home *and* name `.aleph`?
    fn hand_rolls_aleph_home(text: &str) -> bool {
        let mut home = false;
        let mut aleph = false;
        for (_, line) in code_lines(text) {
            home |= line.contains("dirs::home_dir()");
            aleph |= line.contains(".aleph");
        }
        home && aleph
    }

    /// Guard against the single most repeated wiring bug in this repo: a path
    /// under `~/.aleph` resolved by hand instead of through [`get_config_dir`].
    ///
    /// Its whole failure mode is invisibility. With `ALEPH_HOME` unset the two
    /// resolutions are byte-identical, so a developer machine, CI, and every
    /// unit test agree — and the divergence only appears on a relocated home,
    /// where the writer writes one place and the reader reads another and
    /// *nothing errors*. The 2026-08-05 round found eight live instances at
    /// once (identity files the prompt could never see, a guides directory
    /// nobody wrote, a silently empty user-hooks layer, a second answer for
    /// the agents root).
    ///
    /// Source-level on purpose: at runtime the two spellings produce the same
    /// value under the test environment, so only the text can tell them apart.
    ///
    /// **File-level, not line-level.** The line-level version only fired when
    /// one physical line held both markers, so every multi-line spelling walked
    /// straight past it — `dirs::home_dir()` on one line, `.join(".aleph")` on
    /// the next — and so did cron's, where the `"~/.aleph/data/tasks.db"`
    /// default and the `dirs::home_dir()` that expanded its `~` sat fifty lines
    /// apart in the same file. Widening the window to the file is what makes
    /// the guard see the shape it was written for; the cost is a handful of
    /// false positives, and those get an allowlist entry stating why.
    #[test]
    fn no_hand_rolled_aleph_home_outside_the_allowlist() {
        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in all_sources() {
            if HOME_JOIN_ALLOWLIST.iter().any(|(f, _)| *f == rel)
                || HOME_JOIN_PENDING_FIX.iter().any(|(f, _)| *f == rel)
            {
                continue;
            }
            if !hand_rolls_aleph_home(&text) {
                continue;
            }
            let sites: Vec<String> = code_lines(&text)
                .filter(|(_, line)| line.contains("dirs::home_dir()") || line.contains(".aleph"))
                .map(|(n, line)| format!("{rel}:{n}: {}", line.trim()))
                .collect();
            offenders.push(sites.join("\n    "));
        }

        assert!(
            offenders.is_empty(),
            "these resolve an Aleph path by hand instead of through \
             utils::paths::get_config_dir(), so they ignore ALEPH_HOME and will read/write \
             a different directory than the rest of the process — with no error:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The guard's predicate itself: it must see the spellings the line-level
    /// version walked past, which is the whole reason for the widening. The
    /// second case is cron's, where the two halves sat in different functions.
    #[test]
    fn guard_predicate_sees_multi_line_spellings() {
        let single_line = "let p = dirs::home_dir().unwrap().join(\".aleph\");";
        let two_lines = "let p = dirs::home_dir()\n    .unwrap()\n    .join(\".aleph\");";
        let far_apart = "fn default() -> String { \"~/.aleph/data/tasks.db\".into() }\n\
                         // fifty lines of unrelated code\n\
                         fn expand(p: &str) -> PathBuf { dirs::home_dir().unwrap().join(p) }";
        for source in [single_line, two_lines, far_apart] {
            assert!(
                hand_rolls_aleph_home(source),
                "guard missed a hand-rolled path:\n{source}"
            );
        }

        // Still text-blind to comments — the repo discusses this bug by name.
        assert!(!hand_rolls_aleph_home(
            "// dirs::home_dir().join(\".aleph\") is what NOT to do"
        ));
        // And a file that only does one half is not an offender.
        assert!(!hand_rolls_aleph_home("let h = dirs::home_dir();"));
        assert!(!hand_rolls_aleph_home("let p = root.join(\".aleph\");"));
    }

    /// A grandfathered exemption that no longer offends is a lie the next
    /// reader has to disprove by hand. Fail so the fix deletes its own entry.
    #[test]
    fn pending_fix_list_only_shrinks() {
        let sources = all_sources();
        let mut stale: Vec<&str> = Vec::new();
        for (file, _) in HOME_JOIN_PENDING_FIX {
            match sources.iter().find(|(rel, _)| rel == file) {
                Some((_, text)) if hand_rolls_aleph_home(text) => {}
                _ => stale.push(file),
            }
        }
        assert!(
            stale.is_empty(),
            "these no longer hand-roll a home-rooted `.aleph` path (fixed, moved, or \
             deleted) — remove them from HOME_JOIN_PENDING_FIX so the list keeps \
             meaning what it says:\n  {}",
            stale.join("\n  ")
        );
    }

    #[test]
    fn test_find_git_root() {
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().canonicalize().unwrap();
        let project = temp_path.join("project");
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
        let root = find_git_root(&temp_path);
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
            let _env =
                crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&aleph_home, &home);
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
            !dirs
                .iter()
                .any(|d| d == &direct.join("do-thing").join("skills")),
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
