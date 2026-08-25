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
    // directory (same convention as json_canvas_io / cron carryover). This is the
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

/// Get skills directory path
///
/// Returns: `<config_dir>/skills`
pub fn get_skills_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("skills"))
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

/// Get the path for the extension tool-usage sidecar.
///
/// Returns: `<data_dir>/tool_usage.json` — **without creating anything**.
///
/// Deliberately a pure lookup (`aleph_protocol::paths::data_dir` + join)
/// rather than `get_data_dir()?.join(..)` like its neighbours above. Its
/// readers are a diagnostic (`diagnostics::checks::IdleExtensionsCheck`) and a
/// read-only tool, and a sensor must not create what it measures (§5.9): a
/// `doctor` run on a machine that has never written the sidecar would
/// otherwise materialise `~/.aleph/data/` as a side effect of *asking* whether
/// it exists. The one writer ([`crate::tools::usage::ToolUsageStore`]) creates
/// the parent itself.
///
/// Returns `None` only when no home directory can be determined — the same
/// condition every other path helper reports as an error.
#[must_use]
pub fn tool_usage_path() -> Option<PathBuf> {
    Some(aleph_protocol::paths::data_dir()?.join("tool_usage.json"))
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

/// Get the directory for cross-process background `bash` job records.
///
/// Returns: `<data_dir>/background_processes/`
///
/// One function so the boot-time reconciliation and every later read/write of
/// a job's journal row resolve the same directory — the same discipline as the
/// background sub-agent sidecar above (see
/// [`crate::builtin_tools::process_journal`]).
pub fn get_background_processes_dir() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("background_processes"))
}

/// Directory for the busy-input wait lane's crash-durability journal
/// (`busy_queue::durable`) — one entry per queued message, tombstoned on
/// admission/stop/timeout, reinjected at boot.
pub fn get_busy_queue_dir() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("busy_queue"))
}

// ============================================================================
// Private scratch root (shared OS temp dir)
// ============================================================================

/// The owner-only root for everything Aleph drops in the *shared* system temp
/// dir: `<temp_dir>/aleph-<uid>` on unix, `<temp_dir>/aleph` elsewhere.
///
/// Two consumers today — inbound channel media ([`crate::media::cache`]) and
/// the `VirtualFs` skill sandbox
/// ([`crate::tools::markdown_skill::executor`]). Both used to write straight
/// under `temp_dir()` with the process umask, which on the headless Linux
/// server (the documented shared-host product form) means `/tmp` at mode 1777
/// and attachment bytes at 0644: readable by every local account. A fixed name
/// is worse than readable — it is *pre-creatable*, so another user can own the
/// tree the model later reads from and substitute what it sees.
///
/// The uid in the name only keeps two accounts on one host from colliding; it
/// is not a secret and does not stop planting. **The ownership check on every
/// resolution is what makes planting useless**, which is why this refuses
/// rather than warns: the caller's next act is to write bytes into what it
/// returns.
///
/// The root stays *under* `temp_dir()` on purpose.
/// `MediaCache::safe_local_media_path` gates outbound `media_send` on exactly
/// that prefix and must keep accepting both this tree and the native
/// camera/audio captures that land in the bare temp root — see
/// `private_temp_root_stays_inside_the_os_temp_dir`.
///
/// # Errors
///
/// [`std::io::ErrorKind::PermissionDenied`] when the name is taken by anything
/// other than a directory this process exclusively owns; otherwise whatever
/// `mkdir`/`lstat` reported.
pub(crate) fn private_temp_root() -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    let name = {
        // SAFETY: geteuid() always succeeds and returns the effective user ID
        // of the calling process. It is async-signal-safe.
        let euid = unsafe { libc::geteuid() };
        format!("aleph-{euid}")
    };
    // Windows `%TEMP%` is already per-user and has no mode bits to set, so the
    // name keeps its pre-existing spelling and the checks below are a no-op —
    // the same shape as the vault's unix-gated 0o600 chmod.
    #[cfg(not(unix))]
    let name = "aleph".to_string();

    ensure_private_dir(std::env::temp_dir().join(name))
}

/// Create `dir` owner-only if absent, then verify we exclusively own it.
///
/// Split out from [`private_temp_root`] so the refusal arms are testable: a
/// test cannot chmod the real root without breaking every sibling test that
/// writes under it.
fn ensure_private_dir(dir: PathBuf) -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // `create`, not `create_dir_all`: the parent is temp_dir() and already
        // exists, and a non-recursive mkdir is the atomic "claim this name at
        // 0700" the check below then confirms.
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }

        // `symlink_metadata`, never `metadata`: a symlink planted at this name
        // would otherwise report its *target's* owner and mode, which is the
        // one stat an attacker can choose.
        let meta = std::fs::symlink_metadata(&dir)?;
        // SAFETY: geteuid() always succeeds and returns the effective user ID
        // of the calling process. It is async-signal-safe.
        let euid = unsafe { libc::geteuid() };
        if let Some(defect) = private_root_defect(&meta, euid) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to use private scratch root {}: {defect}",
                    dir.display()
                ),
            ));
        }
        Ok(dir)
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// Why a pre-existing scratch root must not be used, if it must not be.
///
/// Takes the already-`lstat`ed metadata and the caller's euid rather than a
/// path so the foreign-owner arm is reachable from a test — a test process
/// cannot `chown` a directory to someone else.
#[cfg(unix)]
fn private_root_defect(meta: &std::fs::Metadata, euid: u32) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    if !meta.file_type().is_dir() {
        return Some(format!(
            "the name is taken by {} instead",
            if meta.file_type().is_symlink() {
                "a symlink"
            } else {
                "a file"
            }
        ));
    }
    let uid = meta.uid();
    if uid != euid {
        return Some(format!("owned by uid {uid}, not by this process ({euid})"));
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Some(format!("mode {mode:04o} grants group or other access"));
    }
    None
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

/// Why `agent_id` is not a usable single path component, or `None` when it is.
///
/// Returns the reason rather than a bare bool because the caller that REPORTS a
/// rejection has to explain it, and one fused sentence listing every rule is
/// wrong for whichever rule actually fired: a NUL byte used to be answered with
/// a lecture about path traversal and Windows device names, which names neither
/// what the operator did nor what to change. A boolean is enough to block a
/// call; it is not enough to explain one.
///
/// Order is the order an operator would want to hear about: the cheapest,
/// most specific fault first.
fn unsafe_agent_id_reason(agent_id: &str) -> Option<&'static str> {
    if agent_id.is_empty() {
        return Some("must not be empty");
    }
    if agent_id.contains('\0') {
        return Some("must not contain null bytes");
    }
    if agent_id.contains('/') || agent_id.contains('\\') {
        return Some("must be a single path component, with no '/' or '\\' separator");
    }
    if agent_id.contains("..") {
        return Some("must not contain '..' (path traversal)");
    }
    if is_windows_reserved_name(agent_id) {
        return Some(
            "must not match a Windows reserved device name \
             (CON, PRN, AUX, NUL, COM1-9, LPT1-9)",
        );
    }
    None
}

/// True when `agent_id` is a single safe path component (non-empty, no path
/// separators, no parent refs, no NUL, not a Windows reserved device name).
/// Mirrors [`get_agent_config_dir`]'s guard so skill discovery can build
/// `~/.aleph/agents/<id>/skills` without risking traversal outside the agents
/// root.
///
/// Derived from [`unsafe_agent_id_reason`] so the predicate and the explanation
/// can never disagree about what is safe.
fn is_safe_agent_id(agent_id: &str) -> bool {
    unsafe_agent_id_reason(agent_id).is_none()
}

/// Bare names Windows reserves at the filesystem layer regardless of
/// extension: the four classic devices plus COM1-9 and LPT1-9. Matching is
/// case-insensitive and applies to the STEM, so `CON.txt` is reserved too.
///
/// That last clause is the whole point and this doc used to deny it, claiming
/// "`CON.txt` is a normal file". It is not: Win32 strips the extension when it
/// resolves a device name, so `CON.txt`, `CON.log` and `C:\anywhere\CON.txt`
/// all open the console device. A file "created" under such a name is written
/// to a device instead — silently, for a caller that only sees a returned
/// string. The stem check has always been correct; only the sentence
/// describing it was wrong, and it was copied into a test that then pinned the
/// misconception.
///
/// Lives here (not in `utils::filename`) so the agent-id validator and the
/// filename sanitizer share one source of truth. `pub(crate)` for the latter.
pub(crate) fn is_windows_reserved_name(name: &str) -> bool {
    // The reserved set is exactly 23 names; the base lookup is O(23).
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.rsplit_once('.').map_or(name, |(b, _)| b);
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
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

/// The root every agent's state directory hangs off: `<config_dir>/agents`.
///
/// Pure lookup — nothing is created. [`get_agent_config_dir`] is the per-agent
/// form and *does* create.
///
/// This exists because "where do agents live" had five answers at once
/// (`agent_resolver::default_agents_root`, this module's per-agent form,
/// `team::create`, `AgentInstanceConfig::default`, `GatewayConfig`), three of
/// them spelled `dirs::home_dir().join(".aleph/agents")` and therefore blind
/// to `ALEPH_HOME`. One derivation, delegated to, is the only shape that
/// cannot drift — see the guard in this module's tests for why the drift is
/// invisible on the machine that writes it.
pub fn get_agents_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("agents"))
}

/// The root every agent workspace hangs off: `<config_dir>/workspaces`.
///
/// Pure lookup — nothing is created.
///
/// Workspaces hold runtime data (tool output, project files); identity files
/// (SOUL.md / AGENTS.md / MEMORY.md) live under [`get_agents_dir`]. The two
/// are siblings and are routinely confused, which is why they are stated once
/// here rather than re-derived per subsystem — the sandbox jails into this
/// root, so a second answer for it is a containment divergence, not a
/// cosmetic one.
pub fn get_workspaces_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("workspaces"))
}

/// The root every whiteboard canvas document hangs off: `<data_dir>/canvas`.
///
/// Creates the directory — this is a write-path helper for the canvas store
/// (`src/canvas/`), NOT for diagnostics: a sensor must not create what it
/// measures, so read-only surfaces need a pure lookup (the [`tool_usage_path`]
/// shape) instead of this.
///
/// One function so the store's writes and every later read resolve the same
/// directory — the same discipline as [`get_background_processes_dir`].
pub fn get_canvas_root() -> Result<PathBuf> {
    let dir = get_data_dir()?.join("canvas");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AlephError::config(format!("Failed to create canvas root directory: {e}")))?;
    Ok(dir)
}

/// Get the identity/config directory for a specific agent
///
/// Agent capabilities (skills, plugins) and identity files are stored here.
///
/// Returns: `<config_dir>/agents/<agent_id>/`
///
/// The directory is created if it doesn't exist.
pub fn get_agent_config_dir(agent_id: &str) -> Result<PathBuf> {
    if let Some(reason) = unsafe_agent_id_reason(agent_id) {
        return Err(AlephError::config(format!(
            "Invalid agent ID '{agent_id}': {reason}"
        )));
    }

    let agent_dir = get_agents_dir()?.join(agent_id);

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
             roots delegate to get_agents_dir()/get_workspaces_dir(), and the \
             remaining `.aleph` is the /tmp stand-in for the no-home case",
        ),
        (
            "src/bin/aleph-server/commands/service/mod.rs",
            "launchd/systemd install paths are real-home by necessity: the \
             daemon started at login inherits no ALEPH_HOME, so the directory \
             created here and the one baked into the plist both have to be the \
             one that daemon will actually resolve. The other `.aleph` matches \
             are the `ai.aleph.server` service label, not a path",
        ),
        (
            "src/thinker/project_instructions.rs",
            "home_dir is the upward-walk boundary (real home); its `.aleph/…` \
             strings are *project*-relative directory names, not home-rooted",
        ),
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

    /// Names of consts whose value *is* the Aleph home directory, derived from
    /// the sources rather than listed here.
    ///
    /// The predicate below is conjunctive — a file has to reach for the real
    /// home *and* name `.aleph`. Its blind spot is a file that does the second
    /// half through an identifier: `home.join(ALEPH_HOME_DIR)` names no
    /// `.aleph` anywhere in the file, so the guard walks past it. Deriving the
    /// alias set keeps that from being closed with a hand-written list, which
    /// would be the second source of truth this whole module exists to remove:
    /// a const that stops holding `.aleph` stops being an alias on its own.
    ///
    /// Only the bare directory name and the `~/.aleph` prefix qualify. A
    /// const like `ENVELOPE_SUFFIX = ".aleph-sig.json"` names a file suffix,
    /// not a root, and joining it to a home is not this bug.
    fn aleph_home_aliases(sources: &[(String, String)]) -> Vec<String> {
        let mut names = Vec::new();
        for (_, text) in sources {
            for (_, line) in code_lines(text) {
                let Some((decl, value)) = line.split_once('=') else {
                    continue;
                };
                if !decl.contains("const ") && !decl.contains("static ") {
                    continue;
                }
                let value = value.trim();
                if !(value.starts_with("\".aleph\"") || value.starts_with("\"~/.aleph")) {
                    continue;
                }
                let Some(name) = decl
                    .split(':')
                    .next()
                    .and_then(|lhs| lhs.split_whitespace().last())
                else {
                    continue;
                };
                names.push(name.to_string());
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// Does this file both reach for the real home *and* name `.aleph` —
    /// literally, or through one of the aliases that holds that literal?
    fn hand_rolls_aleph_home(text: &str, aliases: &[String]) -> bool {
        let mut home = false;
        let mut aleph = false;
        for (_, line) in code_lines(text) {
            home |= line.contains("dirs::home_dir()");
            aleph |= line.contains(".aleph") || aliases.iter().any(|a| line.contains(a.as_str()));
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
    ///
    /// **Zero exemptions for the bug itself.** The widening originally landed
    /// with a 16-file `HOME_JOIN_PENDING_FIX` list of real offenders,
    /// grandfathered so the tightening could land without a repo-wide edit.
    /// That list is drained: `hooks.json` was being *written* to the real home
    /// while `load_user_hooks` read `ALEPH_HOME` (a silently empty layer, the
    /// exact failure that reader's own comment names); `skill_manage` and
    /// `markdown_skills` were two more answers for the directory
    /// `get_skills_dir()` owns; `team::create` provisioned members where
    /// `agent_resolver` would not look for them; and `sandbox::config` was
    /// handing the jail a root no other subsystem knew about. A grandfathered
    /// list is a licence with a deadline — it needs draining, not renewing, so
    /// the only list left is the one whose entries say why they are *not* the
    /// bug.
    #[test]
    fn no_hand_rolled_aleph_home_outside_the_allowlist() {
        let sources = all_sources();
        let aliases = aleph_home_aliases(&sources);
        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in sources {
            if HOME_JOIN_ALLOWLIST.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            if !hand_rolls_aleph_home(&text, &aliases) {
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

    /// Every `build.rs` in the workspace, paired with its contents. Found by
    /// walking, never listed: a fifth build script added tomorrow inherits the
    /// guard below without anyone remembering to tell it.
    fn all_build_scripts() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<PathBuf>) {
            if depth > 3 {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    // target/ holds cargo's own copies of nothing we author.
                    if name.starts_with('.') || name == "target" || name == "node_modules" {
                        continue;
                    }
                    walk(&path, depth + 1, out);
                } else if name == "build.rs" {
                    out.push(path);
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        walk(root, 0, &mut files);
        // A scan that stopped finding build scripts and a repo with no
        // offenders read exactly alike in the report; say which one this is.
        assert!(
            files.len() >= 4,
            "walk found {} build scripts, and this workspace has at least 4 — \
             the scan is no longer looking where they live, so its green means \
             nothing",
            files.len()
        );

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

    /// The statement beginning at `lines[i]` — through the first line holding
    /// a `;`. The window ends at the unit's own syntactic end rather than
    /// after a fixed number of lines or characters, because a window sized by
    /// anything else eventually reads into whatever happens to sit beside it
    /// and then reports on its neighbour.
    fn statement_at(lines: &[&str], i: usize) -> String {
        let mut stmt = String::new();
        for line in &lines[i..] {
            stmt.push_str(line);
            stmt.push('\n');
            if line.contains(';') {
                break;
            }
        }
        stmt
    }

    fn is_ident_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    /// Whether `text` names `ident` as a whole identifier — `plist` must not
    /// match inside `plist_dir`.
    fn mentions_ident(text: &str, ident: &str) -> bool {
        let bytes = text.as_bytes();
        let mut from = 0;
        while let Some(pos) = text[from..].find(ident) {
            let start = from + pos;
            let end = start + ident.len();
            let clean_before = start == 0 || !is_ident_byte(bytes[start - 1]);
            let clean_after = end == bytes.len() || !is_ident_byte(bytes[end]);
            if clean_before && clean_after {
                return true;
            }
            from = end;
        }
        false
    }

    /// The name a `let` binding introduces, if this line is one.
    fn let_binding(line: &str) -> Option<&str> {
        let rest = line.trim_start().strip_prefix("let ")?;
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        let name = rest
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .next()?;
        (!name.is_empty()).then_some(name)
    }

    /// Names carrying a value derived from `CARGO_MANIFEST_DIR`, followed
    /// transitively through `let` bindings in the same file.
    ///
    /// Seeded from the source rather than chased back from the sink: the
    /// invariant is "this value must not reach a link-arg", and a taint set is
    /// the shape of that sentence.
    fn manifest_dir_tainted(text: &str) -> std::collections::BTreeSet<String> {
        let lines: Vec<&str> = text.lines().collect();
        let mut tainted = std::collections::BTreeSet::new();
        for _ in 0..8 {
            let before = tainted.len();
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with('*') {
                    continue;
                }
                let Some(name) = let_binding(line) else {
                    continue;
                };
                if tainted.contains(name) {
                    continue;
                }
                let stmt = statement_at(&lines, i);
                let carries = stmt.contains("CARGO_MANIFEST_DIR")
                    || tainted.iter().any(|t: &String| mentions_ident(&stmt, t));
                if carries {
                    tainted.insert(name.to_string());
                }
            }
            if tainted.len() == before {
                break;
            }
        }
        tainted
    }

    /// Link-arg emissions in `text` whose value comes from the source tree.
    fn link_args_naming_the_source_tree(text: &str) -> Vec<String> {
        let lines: Vec<&str> = text.lines().collect();
        let tainted = manifest_dir_tainted(text);
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//")
                || trimmed.starts_with('*')
                || !line.contains("rustc-link-arg")
            {
                continue;
            }
            let stmt = statement_at(&lines, i);
            if stmt.contains("CARGO_MANIFEST_DIR")
                || tainted.iter().any(|t: &String| mentions_ident(&stmt, t))
            {
                out.push(format!("L{}: {}", i + 1, stmt.trim().replace('\n', " ")));
            }
        }
        out
    }

    /// A `cargo:rustc-link-arg` is **cached**: cargo records the line in the
    /// build script's `output` file and replays it on every later build whose
    /// fingerprint is fresh — that is, on every build where the script itself
    /// does not run. A path it names therefore must not be able to outlive
    /// what it points at, and no `exists()` check written inside the build
    /// script can enforce that: the failing case is by definition the one
    /// where none of that code executes.
    ///
    /// `build.rs` emitted `{CARGO_MANIFEST_DIR}/src/bin/aleph-server/Info.plist`
    /// for the macOS `__info_plist` section. The cache lives in `target/`, the
    /// plist lived in the source tree, and those two lifetimes are unrelated:
    /// removing the checkout — a `git worktree remove` sharing this target
    /// dir, a moved or renamed clone — left the replayed link-arg naming a
    /// path that was gone, so `aleph-server` failed to **link** with an ld
    /// error mentioning no `.rs` file at all, on a tree the reader had not
    /// touched. `cargo test --lib` links no binary and stayed green, so it
    /// surfaced on `--test '*'` as a red belonging to nobody's change.
    ///
    /// The fix stages the file into `OUT_DIR`, which shares the cache's own
    /// lifetime. The regression is one perfectly reasonable-looking `format!`
    /// away and has no runtime signal, which is why it is guarded here rather
    /// than described in a comment.
    ///
    /// Boundary, stated: the taint follows `let` bindings inside one file. A
    /// manifest-rooted path that reaches a link-arg through a function call is
    /// still invisible to it.
    #[test]
    fn no_link_arg_names_a_path_in_the_source_tree() {
        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in all_build_scripts() {
            for site in link_args_naming_the_source_tree(&text) {
                offenders.push(format!("{rel}:{site}"));
            }
        }

        assert!(
            offenders.is_empty(),
            "these bake a source-tree path into a link-arg that cargo caches and \
             replays without re-running the build script, so deleting or moving the \
             checkout makes the binary fail to LINK with an error naming no source \
             file. Stage the file into OUT_DIR and link that instead:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The guard, falsified: it has to go red on the shape that shipped, and
    /// stay green on the one that replaced it. A guard nobody has broken on
    /// purpose is only a guard nobody has broken on purpose.
    #[test]
    fn link_arg_guard_sees_the_shape_that_shipped() {
        let shipped = r#"
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            let plist = format!("{manifest_dir}/src/bin/aleph-server/Info.plist");
            println!(
                "cargo:rustc-link-arg-bin=aleph-server=-Wl,-sectcreate,__TEXT,__info_plist,{plist}"
            );
        "#;
        let staged = r#"
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            let out_dir = std::env::var("OUT_DIR").unwrap();
            let source = std::path::Path::new(&manifest_dir).join("src/bin/x/Info.plist");
            let staged = std::path::Path::new(&out_dir).join("x-Info.plist");
            std::fs::copy(&source, &staged).unwrap();
            println!(
                "cargo:rustc-link-arg-bin=aleph-server=-Wl,-sectcreate,__TEXT,__info_plist,{}",
                staged.display()
            );
        "#;
        // The one-liner the widening exists for: no intermediate binding at all.
        let inline = r#"
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            println!("cargo:rustc-link-arg=-Wl,-force_load,{manifest_dir}/lib/libx.a");
        "#;

        assert!(
            !link_args_naming_the_source_tree(shipped).is_empty(),
            "the guard walked past the exact emission that broke the build"
        );
        assert!(
            !link_args_naming_the_source_tree(inline).is_empty(),
            "the guard only sees the value when it passes through a named binding"
        );
        assert!(
            link_args_naming_the_source_tree(staged).is_empty(),
            "the guard reds the OUT_DIR-staged form it is supposed to be asking for: {:?}",
            link_args_naming_the_source_tree(staged)
        );
        // Text-blind to comments — this defect is discussed by name, in this
        // very file and in build.rs, using the spelling it hunts.
        assert!(link_args_naming_the_source_tree(
            "// println!(\"cargo:rustc-link-arg={CARGO_MANIFEST_DIR}/x\");"
        )
        .is_empty());
        // And a link-arg carrying no path at all is not an offender.
        assert!(link_args_naming_the_source_tree(
            "let manifest_dir = env!(\"CARGO_MANIFEST_DIR\");\n\
             println!(\"cargo:rustc-link-arg=-lz\");"
        )
        .is_empty());
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
        // The fourth spells `.aleph` nowhere: it reaches the home directory and
        // then names it through the const that holds the literal. Before the
        // alias derivation this file read as clean.
        let via_alias = "use crate::discovery::paths::ALEPH_HOME_DIR;\n\
                         fn root() -> PathBuf { dirs::home_dir().unwrap().join(ALEPH_HOME_DIR) }";
        let aliases = aleph_home_aliases(&all_sources());
        assert!(
            aliases.iter().any(|a| a == "ALEPH_HOME_DIR"),
            "the alias derivation stopped finding the const that *is* `.aleph`, so the \
             identifier half of the predicate now matches nothing: {aliases:?}"
        );
        for source in [single_line, two_lines, far_apart] {
            assert!(
                hand_rolls_aleph_home(source, &[]),
                "guard missed a hand-rolled path:\n{source}"
            );
        }
        assert!(
            !hand_rolls_aleph_home(via_alias, &[]),
            "this case is only interesting if the literal-only predicate walks past it"
        );
        assert!(
            hand_rolls_aleph_home(via_alias, &aliases),
            "guard missed a home path composed through an alias:\n{via_alias}"
        );

        // Still text-blind to comments — the repo discusses this bug by name.
        assert!(!hand_rolls_aleph_home(
            "// dirs::home_dir().join(\".aleph\") is what NOT to do",
            &[]
        ));
        // And a file that only does one half is not an offender.
        assert!(!hand_rolls_aleph_home("let h = dirs::home_dir();", &[]));
        assert!(!hand_rolls_aleph_home(
            "let p = root.join(\".aleph\");",
            &[]
        ));
    }

    /// An exemption that no longer offends is a lie the next reader has to
    /// disprove by hand — and worse, it is a standing licence for the next
    /// person to reintroduce the bug in that file without the guard saying a
    /// word. Fail so a fix deletes its own entry.
    ///
    /// This assertion was written for the (now drained) `HOME_JOIN_PENDING_FIX`
    /// list; it applies to the exemptions that remain for exactly the same
    /// reason.
    #[test]
    fn every_exemption_still_offends() {
        let sources = all_sources();
        let aliases = aleph_home_aliases(&sources);
        let mut stale: Vec<&str> = Vec::new();
        for (file, _) in HOME_JOIN_ALLOWLIST {
            match sources.iter().find(|(rel, _)| rel == file) {
                Some((_, text)) if hand_rolls_aleph_home(text, &aliases) => {}
                _ => stale.push(file),
            }
        }
        assert!(
            stale.is_empty(),
            "these no longer hand-roll a home-rooted `.aleph` path (fixed, moved, or \
             deleted), so their HOME_JOIN_ALLOWLIST entry now exempts a file that \
             does not need exempting — delete the entry:\n  {}",
            stale.join("\n  ")
        );
    }

    /// The source-level guard proves a *spelling* changed; this proves the
    /// behaviour it stands for.
    ///
    /// Relocating `ALEPH_HOME` has to move every root together or it moves
    /// none of them usefully: a subsystem left behind writes into a directory
    /// the rest of the process never reads, and nothing errors. These are the
    /// roots that were each resolving their own way until the exemption list
    /// was drained — asserted here rather than in six modules so "they agree"
    /// has one home.
    ///
    /// Deliberately `starts_with(get_config_dir())` and not an equality against
    /// a literal: the claim is containment under the relocated home, and an
    /// equality test would just restate each implementation back to itself.
    #[test]
    fn relocating_aleph_home_moves_every_state_root_with_it() {
        let _home = IsolatedAlephHome::new();
        let root = get_config_dir().expect("isolated config dir resolves");

        let sandbox_root = crate::sandbox::config::SandboxConfig::default().workspace_root;
        let agent_default = crate::gateway::agent_instance::AgentInstanceConfig::default();
        let env_db = crate::gateway::agent_env::AgentEnvStoreConfig::default().db_path;

        let roots: Vec<(&str, PathBuf)> = vec![
            (
                "paths::get_agents_dir",
                get_agents_dir().expect("agents dir"),
            ),
            (
                "paths::get_workspaces_dir",
                get_workspaces_dir().expect("workspaces dir"),
            ),
            (
                "paths::get_skills_dir",
                get_skills_dir().expect("skills dir"),
            ),
            (
                "agent_resolver::default_agents_root",
                crate::config::agent_resolver::default_agents_root(),
            ),
            (
                "agent_resolver::default_workspace_root",
                crate::config::agent_resolver::default_workspace_root(),
            ),
            ("SandboxConfig::default().workspace_root", sandbox_root),
            (
                "AgentInstanceConfig::default().workspace",
                agent_default.workspace,
            ),
            (
                "AgentInstanceConfig::default().agent_dir",
                agent_default.agent_dir,
            ),
            ("AgentEnvStoreConfig::default().db_path", env_db),
        ];

        let strays: Vec<String> = roots
            .into_iter()
            .filter(|(_, p)| !p.starts_with(&root))
            .map(|(name, p)| format!("{name} -> {}", p.display()))
            .collect();

        assert!(
            strays.is_empty(),
            "ALEPH_HOME is {}, but these resolved outside it — they are reading or \
             writing state the rest of the process cannot see:\n  {}",
            root.display(),
            strays.join("\n  ")
        );
    }

    /// The whiteboard store's root: one derivation, under the data dir, and
    /// created on resolve — it is a write-path helper, so the store may rely
    /// on the directory existing (diagnostics must not call it, §5.9).
    #[test]
    fn get_canvas_root_lives_under_the_data_dir_and_creates_it() {
        let _home = IsolatedAlephHome::new();
        let root = get_canvas_root().expect("canvas root resolves");
        assert_eq!(
            root,
            get_data_dir().expect("data dir").join("canvas"),
            "canvas root must be <data_dir>/canvas"
        );
        assert!(
            root.is_dir(),
            "get_canvas_root is a write-path helper and must create the directory"
        );
    }

    /// The private root must stay under `temp_dir()`.
    ///
    /// Not a style rule: `MediaCache::safe_local_media_path` accepts an
    /// outbound `media_send` path only if it canonicalizes inside
    /// `std::env::temp_dir()`, and the cache now writes its attachments here.
    /// Move this root anywhere else and every cached attachment starts being
    /// refused on the way out — silently, since the gate's answer is "do not
    /// attach", not an error.
    #[test]
    fn private_temp_root_stays_inside_the_os_temp_dir() {
        let root = private_temp_root().expect("private scratch root resolves");
        assert!(
            root.starts_with(std::env::temp_dir()),
            "{} escaped {}",
            root.display(),
            std::env::temp_dir().display()
        );
        assert!(
            root.is_dir(),
            "{} must exist as a directory",
            root.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_creates_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let root = ensure_private_dir(tmp.path().join("aleph-0")).expect("fresh root is created");
        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            mode, 0o700,
            "created root must be owner-only, got {mode:04o}"
        );
        // Idempotent: the second resolution finds its own directory and keeps it.
        assert_eq!(
            ensure_private_dir(root.clone()).expect("re-resolution succeeds"),
            root
        );
    }

    /// A pre-existing root anyone else can reach is refused, not adopted — the
    /// name is public, so "it was already there" says nothing about who made
    /// it. The cases cover group-only and other-only reachability separately: a
    /// `0o770` root is exactly as readable as a `0o701` one to whoever is on
    /// the other side of it, and only one of the two looks obviously wrong.
    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_refuses_a_root_others_can_reach() {
        use std::os::unix::fs::PermissionsExt;

        for mode in [0o755, 0o770, 0o701] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().join("aleph-0");
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();

            let err = ensure_private_dir(root)
                .expect_err("a root others can reach must be refused, not adopted");
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(
                err.to_string().contains("grants group or other access"),
                "refusal must name the defect, got: {err}"
            );
        }
    }

    /// The stat must be an `lstat`. A symlink planted at the name is the one
    /// case where following it reports an owner and mode the attacker chose —
    /// their own 0700 directory passes every other check.
    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_refuses_a_symlink_planted_at_the_name() {
        let tmp = TempDir::new().unwrap();
        let elsewhere = tmp.path().join("attacker-owned");
        std::fs::create_dir(&elsewhere).unwrap();
        let root = tmp.path().join("aleph-0");
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();

        let err = ensure_private_dir(root).expect_err("a symlink at the name must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            err.to_string().contains("symlink"),
            "refusal must name the defect, got: {err}"
        );
    }

    /// The foreign-owner arm, which no test can reach through the filesystem:
    /// a process cannot `chown` a directory to somebody else. Inject the euid
    /// instead — that is why the check takes one.
    #[cfg(unix)]
    #[test]
    fn private_root_defect_refuses_a_foreign_owner() {
        let tmp = TempDir::new().unwrap();
        let root = ensure_private_dir(tmp.path().join("aleph-0")).unwrap();
        let meta = std::fs::symlink_metadata(&root).unwrap();

        // SAFETY: geteuid() always succeeds and is async-signal-safe.
        let euid = unsafe { libc::geteuid() };
        assert!(
            private_root_defect(&meta, euid).is_none(),
            "our own 0700 directory must be accepted"
        );
        let defect = private_root_defect(&meta, euid.wrapping_add(1))
            .expect("a root owned by another uid must be refused");
        assert!(
            defect.contains("owned by uid"),
            "refusal must name the defect, got: {defect}"
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
        assert!(get_agent_config_dir("CON").is_err());
        assert!(get_agent_config_dir("con").is_err());
        assert!(get_agent_config_dir("COM1").is_err());
        assert!(get_agent_config_dir("lpt9").is_err());
    }

    #[test]
    fn test_is_safe_agent_id_rejects_reserved_names() {
        assert!(!is_safe_agent_id("CON"));
        assert!(!is_safe_agent_id("con"));
        assert!(!is_safe_agent_id("COM1"));
        assert!(!is_safe_agent_id("LPT9"));
        // Plain agents stay allowed.
        assert!(is_safe_agent_id("researcher"));
        assert!(is_safe_agent_id("my-agent_01"));
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
