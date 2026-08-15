//! Path validation and resolution utilities

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{info, warn};

use crate::builtin_tools::error::ToolError;

/// Denied paths for security.
///
/// Adding entries here is backwards-compatible (strictly tighter) — these are
/// well-known credential stores an agent should never read or overwrite.
/// Matched by [`check_and_resolve_path`] via symlink-canonicalizing prefix
/// comparison, so a directory entry (e.g. `~/.ssh`) covers everything beneath
/// it and a leaf file (e.g. `~/.netrc`) covers exactly that file.
///
/// The credential breadth here mirrors `OpenSquilla`'s `sensitive_paths.py`
/// (SSH/cloud/registry/secret stores) layered onto Aleph's stronger checker,
/// and — like hermes-agent's `get_read_block_error` — extends the deny set to
/// Aleph's *own* credential surface (the encrypted `secrets.vault` and the
/// `data/` auth/device-pairing databases), which an agent must never read or
/// clobber through its file tools.
///
/// The returned list carries TWO kinds of entry, told apart by
/// [`looks_like_glob`] and compiled by [`compile_denied_entry`]:
/// 1. the fixed credential locations above, matched by canonicalizing prefix;
/// 2. the operator's `[sandbox] deny_read_globs`
///    ([`configured_deny_read_globs`]), matched by the same anchored regex the
///    OS sandbox floor uses.
///
/// (2) exists because a file has **two faces that can read it** and the
/// setting used to bind only one: `deny_read_globs` reached the OS drivers
/// (macOS seatbelt `(deny file-read* …)` / Windows deny-read ACEs) and stopped
/// there, so `deny_read_globs = ["**/.env"]` kernel-blocked `bash` while
/// `file_read`, `file_ops search` and `file_ops stats` read the same file in
/// plain text — with nothing anywhere telling the operator the floor was
/// half-applied.
pub fn get_denied_paths() -> Vec<String> {
    let mut denied_paths = vec![
        // SSH / PGP / AWS — the original Unix credential directories.
        "~/.ssh".to_string(),
        "~/.gnupg".to_string(),
        "~/.aws".to_string(),
        // Cloud-provider credential stores.
        "~/.config/gcloud".to_string(),
        "~/.kube".to_string(),
        "~/.azure".to_string(),
        // Container-registry + package-registry credentials.
        "~/.docker/config.json".to_string(),
        "~/.npmrc".to_string(),
        "~/.pypirc".to_string(),
        // Generic secret stores and credential leaf files.
        "~/.password-store".to_string(),
        "~/.netrc".to_string(),
        "~/.git-credentials".to_string(),
    ];

    // Add specific Aleph config files (not the entire directory)
    // We allow the output directory but deny sensitive config files
    if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
        info!(config_dir = %config_dir.display(), "FileOpsTool: config_dir for denied_paths");
        // Deny config files but NOT the output directory
        denied_paths.push(format!("{}/config.toml", config_dir.display()));
        denied_paths.push(format!("{}/memory.db", config_dir.display()));
        denied_paths.push(format!("{}/conversations.db", config_dir.display()));
        denied_paths.push(format!("{}/skills", config_dir.display()));
        denied_paths.push(format!("{}/plugins", config_dir.display()));
        denied_paths.push(format!("{}/mcp", config_dir.display()));
        // Aleph's own credential / auth state — the crown jewels. `secrets.vault`
        // is the encrypted credential store (`VaultStore::default_path()` =
        // `<config_dir>/secrets.vault`); `data/` holds the device-pairing,
        // session, security and devices databases plus the singleton
        // `aleph.lock`. Denying the directory covers every current and future
        // leaf beneath it via the canonicalizing prefix match. Without this the
        // agent's own `file_read`/`file_write` could exfiltrate or corrupt the
        // vault — a hole the OS `deny_globs` does not close because it only
        // applies to commands run inside the sandbox, not to the file tools.
        // The reverse leg of that asymmetry is closed at the bottom of this
        // function: the operator's `deny_read_globs` are appended here so they
        // bind the file tools too.
        denied_paths.push(format!("{}/secrets.vault", config_dir.display()));
        denied_paths.push(format!("{}/secrets.vault.lock", config_dir.display()));
        denied_paths.push(format!("{}/data", config_dir.display()));
        // Note: output directory is intentionally NOT denied
    }

    // Add Unix-specific paths. Beyond the classic credential files, deny the
    // privilege-escalation / persistence surfaces an agent's file tools must
    // never read or clobber — writing any of these is a host-takeover vector
    // (sudoers, cron, PAM, the dynamic-linker preload hook), and reading the
    // SSH host-key dir or root's home leaks credentials. Mirrors hermes-agent's
    // `_SENSITIVE_PATH_PREFIXES`; each is a directory or leaf covered by the
    // canonicalizing prefix match below.
    #[cfg(unix)]
    {
        denied_paths.extend([
            "/etc/passwd".to_string(),
            "/etc/shadow".to_string(),
            "/etc/sudoers".to_string(),
            "/etc/sudoers.d".to_string(),
            "/etc/ssh".to_string(),
            "/etc/pam.d".to_string(),
            "/etc/crontab".to_string(),
            "/etc/cron.d".to_string(),
            "/etc/ld.so.preload".to_string(),
            "/root/.ssh".to_string(),
        ]);
    }

    // Add Windows-specific sensitive paths. The `%APPDATA%` / `%LOCALAPPDATA%`
    // tokens are expanded at match time by [`path_is_denied`] — without that
    // two of these three rules never fire (a canonical path never literally
    // contains `%APPDATA%`).
    #[cfg(target_os = "windows")]
    {
        denied_paths.extend([
            "%APPDATA%\\Microsoft\\Credentials".to_string(),
            "%LOCALAPPDATA%\\Microsoft\\Credentials".to_string(),
            "C:\\Windows\\System32\\config".to_string(),
        ]);
    }

    // The operator's `[sandbox] deny_read_globs` floor. Appended last so a
    // reader of this function sees the fixed credential set first and the
    // configured patterns as an explicit extension of it.
    denied_paths.extend_from_slice(configured_deny_read_globs());

    denied_paths
}

/// The operator's `[sandbox] deny_read_globs`, read once per process.
///
/// **Why a snapshot and not a live read.** `[sandbox]` is a restart-scoped
/// section (`ReloadImpact::classify("sandbox") == Restart`), and the OS drivers
/// that consume the same setting latch it when the sandbox is constructed. A
/// process-lifetime snapshot is therefore the honest reading, and it matches
/// how the rest of this module already behaves — [`get_denied_paths`] is called
/// at tool construction and [`denied_entry_normalized`] memoises each entry for
/// the process lifetime.
///
/// **Why the raw file and not `Config::load()`.** `Config::load()` *writes* a
/// default config file when none exists; a deny check running inside a tool
/// call must not create what it measures. This reads the effective config path
/// (a pure lookup) and parses nothing but the one array it needs, so an
/// unrelated malformed section cannot take the credential denylist down with
/// it.
fn configured_deny_read_globs() -> &'static [String] {
    static GLOBS: OnceLock<Vec<String>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let path = crate::config::Config::effective_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            // No config file (first boot, or a test/CI home): no floor to apply.
            return Vec::new();
        };
        let globs = parse_deny_read_globs(&text);
        if !globs.is_empty() {
            info!(
                count = globs.len(),
                config = %path.display(),
                "file_ops: [sandbox] deny_read_globs floor applied to the file tools"
            );
        }
        globs
    })
}

/// Extract `[sandbox] deny_read_globs` from a config TOML document.
///
/// Fail-soft by necessity — this runs on the file-tool path, where hard-failing
/// on an unrelated config problem would take every file operation down — but
/// never silently: an unparseable document or a non-string entry is logged as
/// a warning that names what is *not* being enforced.
fn parse_deny_read_globs(toml_text: &str) -> Vec<String> {
    let doc = match toml_text.parse::<toml::Value>() {
        Ok(doc) => doc,
        Err(e) => {
            warn!(
                error = %e,
                "file_ops: config file is not valid TOML; [sandbox] deny_read_globs NOT applied to the file tools"
            );
            return Vec::new();
        }
    };
    let Some(entries) = doc
        .get("sandbox")
        .and_then(|s| s.get("deny_read_globs"))
        .and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };
    let mut globs = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry.as_str() {
            Some(pattern) if !pattern.is_empty() => globs.push(pattern.to_string()),
            _ => warn!(
                entry = ?entry,
                "file_ops: ignoring empty/non-string deny_read_globs entry; it denies NOTHING to the file tools"
            ),
        }
    }
    globs
}

/// Expand a **literal** denylist entry's leading `~` (home) and Windows
/// environment tokens (`%APPDATA%` / `%LOCALAPPDATA%` / `%USERPROFILE%`) to
/// concrete paths so the prefix comparison below sees the same shape a
/// canonical path has. Unix entries carry no `%…%` tokens, so the Windows
/// expansion is a no-op there.
///
/// Pattern entries deliberately do NOT come through here — see
/// [`compile_denied_entry`].
fn expand_denied_entry(denied: &str) -> String {
    // `mut` is only exercised on Windows (the env-token expansion below); on
    // other targets the binding is written once.
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut out = if denied.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            home.join(denied.strip_prefix("~/").unwrap_or(denied))
                .to_string_lossy()
                .to_string()
        } else {
            denied.to_string()
        }
    } else {
        denied.to_string()
    };
    #[cfg(target_os = "windows")]
    {
        for (token, var) in [
            ("%APPDATA%", "APPDATA"),
            ("%LOCALAPPDATA%", "LOCALAPPDATA"),
            ("%USERPROFILE%", "USERPROFILE"),
        ] {
            if out.contains(token) {
                if let Ok(val) = std::env::var(var) {
                    out = out.replace(token, &val);
                }
            }
        }
    }
    out
}

/// One denylist entry in the form the matchers consume.
enum DeniedEntry {
    /// A concrete location: expanded ([`expand_denied_entry`]) and normalized
    /// ([`safe_normalize`]) the same way an input path is, then matched by
    /// path-component prefix so the entry covers its whole subtree.
    Literal(PathBuf),
    /// A git-style pattern from `[sandbox] deny_read_globs`, translated by the
    /// SAME function the OS floor uses —
    /// [`crate::sandbox::deny_globs::glob_to_anchored_regex`], which feeds the
    /// macOS seatbelt `(deny file-read* (regex …))` rules and the Windows
    /// deny-read ACE resolver. A second translator here would be the exact
    /// mistake `src/sandbox/platforms/common.rs` documents deleting: a
    /// semantically weaker twin that passes its own tests while producing a
    /// quieter deny floor than the one the operator configured.
    Glob(regex::Regex),
    /// A `deny_read_globs` entry that did not translate or did not compile.
    /// Matches nothing — the same outcome the OS floor reaches (it drops
    /// uncompilable patterns with a warning). Kept as an explicit third state,
    /// rather than dropped at parse time, so the memo stays a total function of
    /// the entry string and the warning fires exactly once per process.
    InertGlob,
}

/// Whether a raw denylist entry is a glob pattern rather than a concrete path.
///
/// Provenance would be the better discriminator — "did this come from
/// `deny_read_globs`" — but the denylist reaches its two matchers as a plain
/// `&[String]` from seven call sites (`file_ops::{read,write,edit,apply_patch,
/// tool}`, `builtin_tools::node_file`, `cluster::node_file_cmd`), so shape is
/// the only channel available without changing a type all seven own. None of
/// the fixed credential entries contains one of these three characters; a
/// *user* path that does (a home directory literally named `a[1]`) is misread
/// as a pattern — `*`/`?` widen the deny, a character class can shift it — and
/// that is why this stays a documented shape test rather than a silent
/// heuristic.
fn looks_like_glob(entry: &str) -> bool {
    glob_shape_subject(entry).contains(['*', '?', '['])
}

/// The part of a denylist entry the glob shape test is allowed to read.
///
/// Windows verbatim paths open with `\\?\` — a literal `?` that is not a
/// wildcard, and that `std::fs::canonicalize` puts in front of *every* path it
/// returns on that platform. [`looks_like_glob`] reading it classified every
/// canonicalized entry as a pattern and sent it to the regex translator, which
/// then produced an `InertGlob` that denies nothing: a deny that silently
/// evaporated, on Windows only, for any caller that handed the list an
/// already-canonical path. `fs_scope_rebase_cannot_bypass_deny` is the test
/// that caught it — a rebased worktree target went from refused to `Ok`.
///
/// The strip is for the *classification* question alone. The entry stored for
/// the component-wise `starts_with` in [`path_is_denied`] keeps its full
/// spelling on purpose: a canonical input carries the prefix too, so removing
/// it from one side of that comparison is the shape that has flipped
/// `starts_with` from allow to deny elsewhere in this repo (see
/// `utils::paths::display_string`, whose own conversion is deliberately
/// *partial* and therefore not reusable here — it keeps the prefix for UNC and
/// past-MAX_PATH paths, i.e. exactly the entries that would stay misclassified).
///
/// Unconditional rather than `#[cfg(windows)]`: `\\?\` prefixes no legitimate
/// Unix path either, and keeping it cross-platform is what makes the test below
/// run on the machine you are reading this on.
fn glob_shape_subject(entry: &str) -> &str {
    entry.strip_prefix(r"\\?\").unwrap_or(entry)
}

/// Compile one raw denylist entry into the form the matchers use.
fn compile_denied_entry(denied: &str) -> DeniedEntry {
    if looks_like_glob(denied) {
        // Deliberately NO `~` / `%APPDATA%` expansion for patterns: the OS
        // floor does not expand either, and a pattern that meant two different
        // things to the two faces is the very asymmetry this wiring closes.
        let Some(pattern) = crate::sandbox::deny_globs::glob_to_anchored_regex(denied) else {
            warn!(entry = %denied, "file_ops: empty deny_read_globs entry ignored");
            return DeniedEntry::InertGlob;
        };
        return match regex::Regex::new(&pattern) {
            Ok(re) => DeniedEntry::Glob(re),
            Err(e) => {
                warn!(
                    entry = %denied,
                    regex = %pattern,
                    error = %e,
                    "file_ops: deny_read_globs pattern failed to compile; it denies NOTHING to the file tools"
                );
                DeniedEntry::InertGlob
            }
        };
    }
    let expanded = expand_denied_entry(denied);
    DeniedEntry::Literal(
        safe_normalize(Path::new(&expanded)).unwrap_or_else(|_| PathBuf::from(&expanded)),
    )
}

/// Memo of the compiled form of each raw denylist entry.
static DENIED_NORM_CACHE: OnceLock<RwLock<HashMap<String, Arc<DeniedEntry>>>> = OnceLock::new();

/// The compiled ([`compile_denied_entry`]) form of one raw denylist entry —
/// computed once per process.
///
/// [`path_is_denied`] runs once per glob match inside the `search` / `stats`
/// walks, and normalizing every entry on every call meant a `canonicalize()`
/// syscall per entry per match: `stats` over a few thousand files issued tens of
/// thousands of blocking syscalls on a tokio worker before returning four
/// numbers — minutes of round-trips on a network mount. The entries are derived
/// from the user's home / config dir at process start and do not change for the
/// process lifetime, which is what makes compiling each one exactly once sound.
/// The same argument covers pattern entries: regex compilation is far more
/// expensive than a `canonicalize()`, and `[sandbox]` is restart-scoped.
///
/// This is also the reason the two directions cannot disagree: both
/// [`path_is_denied`] and [`contains_denied_descendant`] read an entry's
/// meaning from here and nowhere else.
fn denied_entry_normalized(denied: &str) -> Arc<DeniedEntry> {
    let cache = DENIED_NORM_CACHE.get_or_init(Default::default);
    if let Some(hit) = cache
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(denied)
        .cloned()
    {
        return hit;
    }
    let compiled = Arc::new(compile_denied_entry(denied));
    cache
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(denied.to_string(), Arc::clone(&compiled));
    compiled
}

/// Whether an already-canonical path falls under any denylist entry.
///
/// The single source of truth for the deny check, shared by
/// [`check_and_resolve_path`] and by the per-entry re-checks that enumeration /
/// relocation operations (`stats`, `organize`, recursive `copy`) run on paths
/// they discover *after* the initial gate — a symlink or glob match can point
/// at a denied target the top-level path never named.
///
/// Literal entries are expanded ([`expand_denied_entry`]) and normalized the
/// SAME way as the input (resolving symlinks in existing ancestors) before the
/// component-wise prefix compare, so a symlinked ancestor (`/etc` →
/// `/private/etc` on macOS) cannot defeat it. Pattern entries are matched
/// against the `/`-normalised string form of the same canonical path — the
/// identical normalisation
/// [`crate::sandbox::deny_globs::resolve_deny_read_paths_under`] applies before
/// handing paths to the Windows ACE stamper, so a Windows `\` path and a Unix
/// `/` path are judged by one rule.
pub fn path_is_denied(canonical: &Path, denied_paths: &[String]) -> bool {
    // Computed at most once per call, and only if a pattern entry is present.
    let mut slash_form: Option<String> = None;
    for denied in denied_paths {
        match &*denied_entry_normalized(denied) {
            DeniedEntry::Literal(location) => {
                if canonical.starts_with(location) {
                    return true;
                }
            }
            DeniedEntry::Glob(re) => {
                let subject = slash_form
                    .get_or_insert_with(|| canonical.to_string_lossy().replace('\\', "/"));
                if re.is_match(subject) {
                    return true;
                }
            }
            DeniedEntry::InertGlob => {}
        }
    }
    false
}

/// The denylist entry living *beneath* `candidate`, if any.
///
/// [`path_is_denied`] only answers the downward question — "is this path under a
/// protected entry" — so an operation on a PARENT sailed past it: nothing on the
/// denylist names `<config_dir>` itself, yet `remove_dir_all` on it wipes the
/// `secrets.vault` and `data/` auth databases that deleting either directly is
/// correctly refused, and `rename` relocates that whole protected tree out to an
/// undenied location in a single syscall. Shares
/// [`denied_entry_normalized`] with the downward check so the two directions can
/// never disagree about what an entry means.
///
/// Returns the protected location so the refusal can name it. Equality is not a
/// hit: a candidate that *is* a denied entry is already refused by the downward
/// check.
///
/// # Pattern entries answer only the downward question
///
/// `deny_read_globs` entries ([`DeniedEntry::Glob`]) are skipped here, and that
/// is a deliberate, disclosed gap rather than an oversight:
///
/// * A glob is a *predicate over paths*, not a location. There is no "the
///   protected entry beneath `candidate`" to return without walking the
///   candidate's subtree, and a walk has to be bounded (the OS-side walk
///   [`crate::sandbox::deny_globs::resolve_deny_read_paths_under`] caps at
///   50 000 entries). A capped walk answers "I found nothing" when it means "I
///   stopped looking" — a fail-soft skip read as evidence of absence, on a
///   security gate. That is worse than a documented gap.
/// * The OS floor draws the same line. Seatbelt emits per-access
///   `(deny file-read* …)` / `(deny file-write-unlink …)` rules; it refuses to
///   *read or unlink a matching path*, and equally does not refuse renaming an
///   ancestor directory that happens to contain one.
///
/// Consequence, stated plainly: with `deny_read_globs = ["**/.env"]`, a
/// `file_ops delete` or `move` aimed at a *parent directory* still takes the
/// matching file with it, whereas naming the file directly is refused (the
/// downward check in [`path_is_denied`] covers that) and a recursive `copy`
/// skips it and says so. The fixed credential entries keep full two-direction
/// coverage, which is why the match below is on the entry kind and not a bare
/// `if` — the two directions still read one compiled entry from
/// [`denied_entry_normalized`] and cannot disagree about what an entry *means*.
pub fn contains_denied_descendant(candidate: &Path, denied_paths: &[String]) -> Option<PathBuf> {
    denied_paths
        .iter()
        .find_map(|denied| match &*denied_entry_normalized(denied) {
            DeniedEntry::Literal(location) => {
                (location != candidate && location.starts_with(candidate)).then(|| location.clone())
            }
            DeniedEntry::Glob(_) | DeniedEntry::InertGlob => None,
        })
}

/// Whether `canonical` is a Linux `/proc/<pid>/…` pseudo-file that leaks another
/// process's secrets (environment, memory, mappings). These are not covered by
/// the credential denylist and are not regular files an agent has any business
/// reading — `/proc/<pid>/environ` alone exposes every exported secret of a
/// running process. Defense-in-depth mirroring hermes-agent's
/// `_is_blocked_device_path`; a no-op on non-Linux where `/proc` is absent.
pub fn is_blocked_proc_path(canonical: &Path) -> bool {
    use std::path::Component;
    let mut comps = canonical.components();
    // Must be rooted at `/proc/<something>/…`.
    if comps.next() != Some(Component::RootDir) {
        return false;
    }
    if comps.next() != Some(Component::Normal(std::ffi::OsStr::new("proc"))) {
        return false;
    }
    // `<pid>` (or `self` / `thread-self`) — any single component.
    if comps.next().is_none() {
        return false;
    }
    // Block the secret-bearing leaves anywhere below the pid dir.
    const BLOCKED_LEAVES: &[&str] = &[
        "environ",
        "cmdline",
        "mem",
        "maps",
        "smaps",
        "smaps_rollup",
        "numa_maps",
        "auxv",
        "pagemap",
        "stack",
        "syscall",
    ];
    canonical
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|leaf| BLOCKED_LEAVES.contains(&leaf))
}

/// Reject glob patterns that would escape the (already deny-checked) base
/// directory: absolute patterns replace the base via `Path::join`, and any
/// `..` component climbs out of it. Relative, non-climbing patterns are safe
/// because every match still lands under `canonical`.
///
/// Uses `has_root()` instead of `is_absolute()` so that root-anchored-but-
/// drive-relative patterns (e.g. `/etc/*` on Windows, which has a root but no
/// drive prefix) are also rejected — they still escape the base via `join`.
///
/// Additionally rejects any pattern containing a drive or UNC prefix
/// (`Component::Prefix`) — e.g. `C:foo` on Windows. Such patterns are not
/// root-anchored (`has_root()` returns false) yet `Path::join(base, "C:foo")`
/// discards the base entirely and resolves relative to drive C's current
/// directory, bypassing the deny-checked base. On Unix `Component::Prefix`
/// never occurs, so this check is a safe no-op there.
pub(crate) fn reject_unsafe_glob_pattern(pattern: &str) -> Result<(), ToolError> {
    let p = std::path::Path::new(pattern);
    if p.has_root() {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must be relative to the search directory: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::Prefix(_)))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain a drive/UNC prefix: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain `..`: {pattern}"
        )));
    }
    Ok(())
}

/// Expand `$HOME`/`$USER`, a leading `~`, and a relative base into a concrete
/// path **without canonicalizing** — so a final-component symlink is preserved
/// (canonicalization would resolve it to its target). Shared by
/// [`check_and_resolve_path`] and [`resolve_for_removal`].
fn expand_input_path(
    path: &Path,
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    // First, expand environment variables in the path string
    let path_str = path.to_string_lossy();
    let expanded_str = if path_str.contains('$') {
        let mut result = path_str.to_string();
        // Expand $HOME
        if let Some(home) = dirs::home_dir() {
            result = result.replace("$HOME", &home.to_string_lossy());
        }
        // Expand $USER
        if let Ok(user) = std::env::var("USER") {
            result = result.replace("$USER", &user);
        }
        // Only expand $HOME and $USER for security — arbitrary env var expansion
        // could allow path injection via attacker-controlled environment variables.
        PathBuf::from(result)
    } else {
        path.to_path_buf()
    };

    // Expand ~ to home directory
    if expanded_str.starts_with("~/") || expanded_str.as_os_str() == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| ToolError::InvalidArgs("Cannot determine home directory".to_string()))?;
        Ok(home.join(
            expanded_str
                .strip_prefix("~")
                .unwrap_or_else(|_| std::path::Path::new("")),
        ))
    } else if expanded_str.is_relative() {
        // Relative paths are resolved to:
        // 1. Per-run FsScope base (task-local — worktree root for isolated
        //    agents, workspace artifact dir for normal runs)
        // 2. ToolContext output_dir override (workspace-scoped, set by ExecutionEngine)
        // 3. Error if neither is available — callers must provide a base directory
        let base_dir = if let Some(scope) = crate::tools::fs_scope::current() {
            info!(fs_scope = %scope.base.display(), "check_path: using per-run FsScope base");
            scope.base
        } else if let Some(override_dir) = output_dir_override {
            info!(output_dir = %override_dir.display(), "check_path: using ToolContext output_dir override");
            override_dir.to_path_buf()
        } else {
            return Err(ToolError::InvalidArgs(
                "Relative path requires an active run scope or an output directory override; \
                 provide an absolute path instead"
                    .to_string(),
            ));
        };
        Ok(base_dir.join(expanded_str))
    } else {
        Ok(expanded_str)
    }
}

/// Resolve a path for a **removal or rename** whose final component must NOT be
/// followed when it is a symlink.
///
/// `check_and_resolve_path` canonicalizes a final-component symlink to its
/// *target*; a `delete`/`move` acting on that target would destroy the tree the
/// link points at and leave the link dangling (or move the target out from
/// under it). Filesystem `remove_file` / `rename` never follow a final symlink,
/// so operating on the link path is both correct and what the user meant.
///
/// The full deny check still runs against the resolved target (via
/// [`check_and_resolve_path`]), and the link's own location is deny-checked too,
/// so neither the link nor its target can name a protected location. Returns the
/// path to operate on: the un-followed link when the final component is a
/// symlink, otherwise the canonical target (identical to
/// `check_and_resolve_path`).
pub fn resolve_for_removal(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    // Deny-check the resolved target first (conservative: a link whose target is
    // protected cannot be used as a handle to it).
    let canonical_target = check_and_resolve_path(path, denied_paths, output_dir_override)?;

    let expanded = expand_input_path(path, output_dir_override)?;
    let is_symlink = std::fs::symlink_metadata(&expanded)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return Ok(canonical_target);
    }

    // The final component is a symlink: operate on the LINK, not its target.
    // Canonicalize only the PARENT (resolving any intermediate symlinks + the
    // FsScope rebase) and re-attach the un-followed final component.
    let Some(file_name) = expanded.file_name() else {
        return Ok(canonical_target);
    };
    let parent = expanded.parent().unwrap_or_else(|| Path::new("/"));
    let canon_parent = safe_normalize(parent)
        .map_err(|e| ToolError::Execution(format!("Failed to resolve parent: {e}")))?;
    let canon_parent =
        match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canon_parent)) {
            Some(rebased) => safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased parent: {e}"))
            })?,
            None => canon_parent,
        };
    let link_path = canon_parent.join(file_name);
    if path_is_denied(&link_path, denied_paths) {
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }
    Ok(link_path)
}

/// Check if path is allowed and resolve it — the **file layer's** sole path
/// resolver.
///
/// # There are two path resolvers in this repo, on purpose
///
/// The other one is `sandbox::workspace::path::normalize_path` in
/// `src/sandbox/workspace/path.rs`, and the two answer *different questions*.
/// Unifying them would silently delete one of the two answers, so
/// `path_utils::tests::the_two_path_resolvers_stay_split` fails by name if
/// either stops being the sole resolver for its own layer, or if a third
/// appears.
///
/// | | this function (file layer) | `sandbox::workspace::path::normalize_path` (exec layer) |
/// |---|---|---|
/// | question | "may the model's file tools touch this path, and where does it really land?" | "does this path stay inside the session's workspace jail?" |
/// | `~` / `$HOME` / `$USER` | expanded | not expanded |
/// | relative base | task-local [`FsScope`](crate::tools::fs_scope::FsScope), else the `ToolContext` output dir | the workspace root, always |
/// | symlinks | canonicalized (existing ancestors resolved) | never resolved — `..` is popped *lexically*, before any syscall |
/// | denylist | yes: credential entries + `[sandbox] deny_read_globs` + `/proc` secrets | none |
/// | root containment | none — an absolute path is used as-is (see the tool `DESCRIPTION`) | hard jail enforced by the caller |
///
/// Net: the exec layer is an **allowlist jail with no denylist**; the file layer
/// is a **denylist with no jail**. Each is unsound as the other's gate.
///
/// Path resolution rules:
/// 1. Environment variables ($HOME, $USER, etc.) - expanded first
/// 2. Absolute paths (starting with `/`) - used as-is, then rebased through
///    the active [`FsScope`](crate::tools::fs_scope::FsScope) remap when the
///    run is worktree-isolated (parent-repo paths land inside the worktree,
///    mirroring what `WorktreeSandbox` already does for command execution)
/// 3. Home paths (starting with `~`) - expanded to home directory
/// 4. Relative paths - resolved relative to:
///    a. the per-run `FsScope` task-local base — per-run truth, immune to a
///    concurrent run rewriting the shared `ToolContextHandle` mid-run
///    b. `output_dir_override` if provided (workspace-scoped output dir from `ToolContext`)
///    c. Error if neither is available — no global fallback
///
/// The deny check always runs on the FINAL path (post-rebase), so a remap can
/// never smuggle a denied location past the gate.
pub fn check_and_resolve_path(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    info!(path = %path.display(), "check_path: input path");

    // Env-var / `~` / relative-base expansion (NO canonicalization — a final
    // symlink is preserved). Shared with `resolve_for_removal` so the two
    // resolvers cannot drift on how a spelled path becomes a filesystem path.
    let expanded = expand_input_path(path, output_dir_override)?;

    info!(expanded = %expanded.display(), exists = expanded.exists(), "check_path: expanded path");

    // Canonicalize if exists; for non-existent files, manually normalize to resolve ".."
    // components. This prevents path traversal bypasses (e.g., "/allowed/../secret/file").
    let canonical = if expanded.exists() {
        expanded
            .canonicalize()
            .map_err(|e| ToolError::Execution(format!("Failed to resolve path: {e}")))?
    } else {
        // For non-existent paths, canonicalize the longest existing ancestor
        // then append remaining components. This prevents symlink-based traversal
        // that pure component normalization would miss.
        safe_normalize(&expanded).map_err(|e| {
            ToolError::Execution(format!("Failed to normalize non-existent path: {e}"))
        })?
    };

    info!(canonical = %canonical.display(), "check_path: canonical path");

    // Worktree-isolation remap: when the active FsScope declares a rebase,
    // canonical paths under the parent repo are redirected into the isolated
    // worktree BEFORE the deny check below — the gate therefore evaluates the
    // path that will actually be touched.
    let canonical = match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canonical))
    {
        Some(rebased) => {
            info!(
                from = %canonical.display(),
                to = %rebased.display(),
                "check_path: FsScope rebase into isolated worktree"
            );
            // Re-normalize so the result stays canonical (the worktree side
            // may sit behind a symlinked tmpdir) — keeps `path_locks` keys
            // consistent across spellings of the same file.
            safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased path: {e}"))
            })?
        }
        None => canonical,
    };

    // Check against denied paths. Uses Path-component prefix matching (not
    // string starts_with, which would falsely match "/foo-bar" against "/foo")
    // via the shared `path_is_denied` helper, which canonicalizes each denied
    // entry the same way as the input so a symlinked ancestor (macOS
    // `/etc` -> `/private/etc`) cannot defeat it.
    if path_is_denied(&canonical, denied_paths) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - path matches denied pattern"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }

    // Defense-in-depth: block `/proc/<pid>/{environ,maps,mem,…}` secret-bearing
    // pseudo-files regardless of the credential denylist.
    if is_blocked_proc_path(&canonical) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - /proc secret pseudo-file"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} exposes another process's secrets",
            path.display()
        )));
    }

    info!(canonical = %canonical.display(), "check_path: path allowed");
    Ok(canonical)
}

/// Normalize a non-existent path by canonicalizing the longest existing ancestor,
/// then appending the remaining components. This prevents symlink-based path traversal
/// that pure component-level normalization would miss.
///
/// Returns an error if the longest existing ancestor cannot be canonicalized
/// (e.g., due to permission issues), ensuring we never return an uncanonicalized
/// path that could bypass security checks.
fn safe_normalize(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();
    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_owned());
            existing.pop();
        } else {
            break;
        }
    }
    let mut result = existing.canonicalize().map_err(|e| {
        format!(
            "Failed to canonicalize ancestor '{}': {}",
            existing.display(),
            e
        )
    })?;
    for component in remaining.into_iter().rev() {
        if component == ".." {
            result.pop();
        } else if component != "." {
            result.push(component);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // --- reject_unsafe_glob_pattern ---

    #[test]
    fn glob_guard_allows_relative_patterns() {
        assert!(
            reject_unsafe_glob_pattern("*.txt").is_ok(),
            "bare wildcard must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("images/photo.jpg").is_ok(),
            "relative sub-path must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("**/foo").is_ok(),
            "recursive glob must be accepted"
        );
    }

    #[test]
    fn glob_guard_rejects_root_anchored() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("/etc/*"),
                Err(ToolError::InvalidArgs(_))
            ),
            "/etc/* is root-anchored and must be rejected"
        );
    }

    #[test]
    fn glob_guard_rejects_parent_dir() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("../secrets"),
                Err(ToolError::InvalidArgs(_))
            ),
            "../secrets contains `..` and must be rejected"
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_guard_rejects_drive_relative_prefix() {
        // On Windows, `C:foo` has a Prefix component but no root — Path::join
        // with any base replaces the base entirely, so it must be rejected.
        assert!(
            matches!(
                reject_unsafe_glob_pattern("C:foo"),
                Err(ToolError::InvalidArgs(_))
            ),
            "C:foo is a drive-relative pattern and must be rejected on Windows"
        );
    }

    /// The denylist must include Aleph's own encrypted vault and the `data/`
    /// auth directory. Asserted by path *suffix* so the test stays hermetic and
    /// independent of where `get_config_dir()` resolves in the test environment
    /// (no `ALEPH_HOME`/`$HOME` mutation, hence no cross-test env leak).
    #[test]
    fn denied_paths_cover_aleph_credential_stores() {
        let denied = get_denied_paths();
        assert!(
            denied.iter().any(|p| p.ends_with("/secrets.vault")),
            "secrets.vault missing from denylist: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p.ends_with("/data")),
            "data/ auth dir missing from denylist: {denied:?}"
        );
    }

    /// End-to-end enforcement: the vault leaf file is rejected, a file *inside*
    /// the denied `data/` directory is rejected via the canonicalizing prefix
    /// match, and an unrelated sibling under the same root is still allowed.
    #[test]
    fn check_path_blocks_vault_and_data_allows_sibling() {
        let root = tempdir().unwrap();
        let vault = root.path().join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let data = root.path().join("data");
        fs::create_dir(&data).unwrap();
        let pairing = data.join("pairing.db");
        fs::write(&pairing, b"db").unwrap();
        let allowed = root.path().join("output.txt");
        fs::write(&allowed, b"ok").unwrap();

        let denied = vec![
            vault.to_string_lossy().to_string(),
            data.to_string_lossy().to_string(),
        ];

        // Vault leaf file is denied.
        assert!(
            check_and_resolve_path(&vault, &denied, None).is_err(),
            "vault read should be denied"
        );
        // A file inside the denied data/ dir is denied (directory-prefix match).
        assert!(
            check_and_resolve_path(&pairing, &denied, None).is_err(),
            "data/pairing.db read should be denied"
        );
        // An unrelated sibling under the same root is allowed.
        assert!(
            check_and_resolve_path(&allowed, &denied, None).is_ok(),
            "unrelated sibling should be allowed"
        );
    }

    /// Relative paths anchor at the per-run `FsScope` base when one is
    /// published — and the scope wins over the (potentially stale, shared)
    /// `output_dir_override`.
    #[tokio::test]
    async fn fs_scope_base_anchors_relative_paths() {
        let scope_root = tempdir().unwrap();
        let other_root = tempdir().unwrap();
        let scope = crate::tools::fs_scope::FsScope::workspace(scope_root.path().to_path_buf());
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async {
            check_and_resolve_path(Path::new("sub/file.txt"), &[], Some(other_root.path()))
        })
        .await
        .expect("relative path must resolve inside the scope base");
        let canonical_scope = scope_root.path().canonicalize().unwrap();
        assert_eq!(resolved, canonical_scope.join("sub/file.txt"));
    }

    /// Worktree isolation: an absolute path under the parent repo is rebased
    /// into the worktree checkout before any filesystem access.
    #[tokio::test]
    async fn fs_scope_rebase_redirects_parent_repo_paths() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::create_dir_all(repo.path().join("src")).unwrap();
        fs::write(repo.path().join("src/a.rs"), b"fn main() {}").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c.clone(), repo_c.clone());
        let input = repo_c.join("src/a.rs");
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &[], None)
        })
        .await
        .expect("rebase must succeed");
        assert_eq!(resolved, wt_c.join("src/a.rs"));
    }

    #[test]
    fn path_is_denied_matches_directory_prefix_not_string_prefix() {
        let root = tempdir().unwrap();
        let secret_dir = root.path().join("secret");
        fs::create_dir(&secret_dir).unwrap();
        let sibling = root.path().join("secret-sibling");
        fs::create_dir(&sibling).unwrap();
        let denied = vec![secret_dir.to_string_lossy().to_string()];
        // `path_is_denied` expects an already-canonical input (its contract);
        // canonicalize the dirs so a symlinked tempdir root (macOS
        // `/var` → `/private/var`) does not defeat the prefix compare.
        let secret_c = secret_dir.canonicalize().unwrap();
        let sibling_c = sibling.canonicalize().unwrap();

        assert!(path_is_denied(&secret_c.join("k.pem"), &denied));
        // A string-prefix sibling ("secret-sibling") must NOT match.
        assert!(!path_is_denied(&sibling_c.join("ok.txt"), &denied));
    }

    /// The upward check sees a protected entry the downward check cannot: a
    /// parent is not "under" the denylist, but destroying or relocating it takes
    /// the protected entry with it. Directions must stay disjoint — a candidate
    /// that IS the entry is the downward check's case.
    #[test]
    fn contains_denied_descendant_finds_protected_child_only() {
        let root = tempdir().unwrap();
        let config = root.path().join("aleph");
        fs::create_dir(&config).unwrap();
        let vault = config.join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let sibling = root.path().join("other");
        fs::create_dir(&sibling).unwrap();
        let denied = vec![vault.to_string_lossy().to_string()];

        let config_c = config.canonicalize().unwrap();
        let vault_c = vault.canonicalize().unwrap();
        assert_eq!(
            contains_denied_descendant(&config_c, &denied),
            Some(vault_c.clone()),
            "the parent must report the protected entry it holds"
        );
        assert_eq!(
            contains_denied_descendant(&vault_c, &denied),
            None,
            "the entry itself is the downward check's case, not a descendant"
        );
        assert_eq!(
            contains_denied_descendant(&sibling.canonicalize().unwrap(), &denied),
            None,
            "an unrelated directory holds nothing protected"
        );
    }

    /// Each raw denylist entry is expanded + normalized ONCE per process: the
    /// walk operations call `path_is_denied` per glob match, and re-running
    /// `canonicalize()` for every entry on every call is what turned a `stats`
    /// over a few thousand files into tens of thousands of blocking syscalls.
    ///
    /// Observable via a swap the memo must not notice: the entry first resolves
    /// through a symlink, then the symlink is replaced by a real directory.
    #[cfg(unix)]
    #[test]
    fn denied_entry_is_normalized_once_per_process() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = root.path().join("link");
        symlink(&real, &link).unwrap();
        let real_c = real.canonicalize().unwrap();

        let denied = vec![link.to_string_lossy().to_string()];
        assert!(
            path_is_denied(&real_c.join("k.pem"), &denied),
            "the entry resolves through the symlink to `real`"
        );

        // Replace the symlink with a real directory: a re-normalization would
        // now resolve the entry to `link` itself and stop covering `real`.
        fs::remove_file(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert!(
            path_is_denied(&real_c.join("k.pem"), &denied),
            "the entry must not be re-normalized on the second call"
        );
    }

    #[cfg(unix)]
    #[test]
    fn is_blocked_proc_path_flags_secret_leaves_only() {
        use std::path::Path;
        assert!(is_blocked_proc_path(Path::new("/proc/1234/environ")));
        assert!(is_blocked_proc_path(Path::new("/proc/self/maps")));
        assert!(is_blocked_proc_path(Path::new("/proc/1/mem")));
        // A benign /proc leaf and non-/proc paths are allowed.
        assert!(!is_blocked_proc_path(Path::new("/proc/1234/status")));
        assert!(!is_blocked_proc_path(Path::new("/proc/cpuinfo")));
        assert!(!is_blocked_proc_path(Path::new("/home/u/environ")));
    }

    #[cfg(unix)]
    #[test]
    fn denied_paths_cover_privilege_escalation_surfaces() {
        let denied = get_denied_paths();
        for p in ["/etc/sudoers", "/etc/cron.d", "/etc/pam.d", "/root/.ssh"] {
            assert!(
                denied.iter().any(|d| d == p),
                "{p} missing from denylist: {denied:?}"
            );
        }
    }

    // --- `[sandbox] deny_read_globs` (the OS floor's second face) ---

    /// RED before the fix: `deny_read_globs = ["**/.env"]` kernel-blocked
    /// `bash` while `file_read` / `file_ops` read the same file in plain text,
    /// because the file layer only ever understood literal path entries.
    #[test]
    fn deny_read_glob_entry_refuses_the_matching_path() {
        let root = tempdir().unwrap();
        let secret = root.path().join("app/.env");
        fs::create_dir_all(secret.parent().unwrap()).unwrap();
        fs::write(&secret, b"TOKEN=1").unwrap();
        let benign = root.path().join("app/config.toml");
        fs::write(&benign, b"k=1").unwrap();

        // Exactly the string an operator puts in `[sandbox] deny_read_globs`.
        let denied = vec!["**/.env".to_string()];

        let err = check_and_resolve_path(&secret, &denied, None)
            .expect_err("a deny_read_globs match must be refused by the file tools too");
        assert!(
            err.to_string().contains("protected location"),
            "refusal must name the reason, got: {err}"
        );
        assert!(
            check_and_resolve_path(&benign, &denied, None).is_ok(),
            "a non-matching sibling must still be readable"
        );
    }

    /// The pattern is translated by the OS floor's own translator, so the two
    /// faces cannot drift: component-scoped `*`, `**/` spanning directories,
    /// and a metacharacter-free entry covering its whole subtree.
    #[test]
    fn deny_read_glob_semantics_match_the_os_floor() {
        let root = tempdir().unwrap();
        let deep = root.path().join("a/b");
        fs::create_dir_all(&deep).unwrap();
        let nested_pem = deep.join("key.pem");
        fs::write(&nested_pem, b"-----BEGIN-----").unwrap();
        let sub = root.path().join("a/keys");
        fs::create_dir_all(&sub).unwrap();
        let under_dir = sub.join("id_rsa");
        fs::write(&under_dir, b"priv").unwrap();
        let root_c = root.path().canonicalize().unwrap();

        // `**/*.pem` crosses directories; the same regex the seatbelt driver
        // would emit.
        assert!(path_is_denied(
            &nested_pem.canonicalize().unwrap(),
            &["**/*.pem".to_string()]
        ));
        // `*` stays inside one component, so it must NOT reach a nested file.
        assert!(!path_is_denied(
            &nested_pem.canonicalize().unwrap(),
            &[format!("{}/*.pem", root_c.display())]
        ));
        // A metacharacter-free entry is a literal location covering its subtree
        // (both the glob translator and the literal prefix match agree here).
        assert!(path_is_denied(
            &under_dir.canonicalize().unwrap(),
            &[sub.to_string_lossy().to_string()]
        ));
    }

    /// A Windows verbatim path is a LOCATION, not a pattern.
    ///
    /// `std::fs::canonicalize` returns `\\?\C:\...` on Windows, so a caller that
    /// hands the denylist an already-canonical path — which
    /// `fs_scope_rebase_cannot_bypass_deny` does, and which is a perfectly
    /// reasonable thing to do — used to have that entry read as a glob (the
    /// prefix's literal `?`) and compiled to an `InertGlob` that denies
    /// nothing. The deny evaporated in silence, on Windows only.
    ///
    /// Runs everywhere: the classification is a pure string test, so this pins
    /// the behaviour on the machine you are reading it on rather than waiting
    /// for a Windows runner to disagree.
    #[test]
    fn a_windows_verbatim_entry_is_a_literal_not_a_pattern() {
        const CANONICAL: &str = r"\\?\C:\Users\me\creds\id_rsa";

        assert!(
            !looks_like_glob(CANONICAL),
            "the `?` in the verbatim prefix is not a wildcard"
        );
        assert!(
            matches!(compile_denied_entry(CANONICAL), DeniedEntry::Literal(_)),
            "a canonical Windows entry must compile to a literal location, \
             or the deny it encodes matches nothing"
        );

        // Stripping the prefix must not disarm the shape test for an entry
        // that carries a real wildcard behind it.
        assert!(
            looks_like_glob(r"\\?\C:\Users\me\**\.env"),
            "a genuine wildcard after the prefix is still a pattern"
        );

        // Unprefixed entries are judged exactly as before.
        assert!(looks_like_glob("**/.env"));
        assert!(!looks_like_glob("/home/me/.ssh"));
    }

    /// A pattern entry answers only the DOWNWARD question. The upward twin
    /// returns `None` for it by design (see `contains_denied_descendant`), and
    /// this pins that so the gap stays a decision rather than a regression —
    /// while a literal entry keeps full two-direction coverage.
    #[test]
    fn glob_entries_are_downward_only_literals_are_two_directional() {
        let root = tempdir().unwrap();
        let proj = root.path().join("proj");
        fs::create_dir(&proj).unwrap();
        let env = proj.join(".env");
        fs::write(&env, b"TOKEN=1").unwrap();
        let proj_c = proj.canonicalize().unwrap();
        let env_c = env.canonicalize().unwrap();

        // Downward: the pattern denies the file itself.
        assert!(path_is_denied(&env_c, &["**/.env".to_string()]));
        // Upward: the pattern cannot name a protected location under `proj`.
        assert_eq!(
            contains_denied_descendant(&proj_c, &["**/.env".to_string()]),
            None,
            "a glob is a predicate, not a location — see the doc comment"
        );
        // A literal entry naming the same file still answers upward.
        assert_eq!(
            contains_denied_descendant(&proj_c, &[env.to_string_lossy().to_string()]),
            Some(env_c),
            "literal entries must keep two-direction coverage"
        );
    }

    /// An uncompilable pattern denies nothing (matching the OS floor, which
    /// drops patterns whose regex will not compile) and must not poison the
    /// literal entries sitting beside it in the same list.
    #[test]
    fn inert_glob_entry_denies_nothing_and_does_not_break_the_list() {
        let root = tempdir().unwrap();
        let vault = root.path().join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let plain = root.path().join("notes.txt");
        fs::write(&plain, b"hi").unwrap();

        // `[z-a]` translates to a syntactically valid glob class but an
        // invalid regex range.
        let denied = vec![
            "[z-a]".to_string(),
            "**/.env".to_string(),
            vault.to_string_lossy().to_string(),
        ];
        assert!(
            matches!(&*denied_entry_normalized("[z-a]"), DeniedEntry::InertGlob),
            "an uncompilable pattern must land in the inert state, not silently \
             become a literal"
        );
        assert!(check_and_resolve_path(&vault, &denied, None).is_err());
        assert!(check_and_resolve_path(&plain, &denied, None).is_ok());
    }

    /// The config reader is a narrow, hermetic parse of one array — no
    /// `Config::load()` (which writes a default file) and no dependency on the
    /// developer's real `~/.aleph/config.toml`.
    #[test]
    fn parse_deny_read_globs_reads_the_sandbox_array_only() {
        let toml = r#"
[gateway]
host = "127.0.0.1"

[sandbox]
enabled = true
deny_read_globs = ["**/.env", "**/*.pem"]
"#;
        assert_eq!(
            parse_deny_read_globs(toml),
            vec!["**/.env".to_string(), "**/*.pem".to_string()]
        );
        // Absent section / absent key / wrong shape → empty, never a panic.
        assert!(parse_deny_read_globs("[gateway]\nhost = \"x\"\n").is_empty());
        assert!(parse_deny_read_globs("[sandbox]\nenabled = true\n").is_empty());
        assert!(parse_deny_read_globs("this is not toml {{{").is_empty());
        // Non-string / empty entries are dropped, the rest survive.
        assert_eq!(
            parse_deny_read_globs("[sandbox]\ndeny_read_globs = [\"**/.env\", 7, \"\"]\n"),
            vec!["**/.env".to_string()]
        );
    }

    /// The shape test that tells a pattern entry from a concrete location.
    #[test]
    fn looks_like_glob_separates_patterns_from_paths() {
        assert!(looks_like_glob("**/.env"));
        assert!(looks_like_glob("/tmp/file?.txt"));
        assert!(looks_like_glob("/tmp/[abc].txt"));
        for literal in [
            "~/.ssh",
            "/etc/passwd",
            // Spelled without the real config-dir name on purpose:
            // `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist`
            // is a FILE-level guard, and this module legitimately calls
            // `dirs::home_dir()` — naming that directory here would make the
            // pair look like a hand-rolled home resolution.
            "/Users/x/config-dir/secrets.vault",
            "%APPDATA%\\Microsoft\\Credentials",
        ] {
            assert!(
                !looks_like_glob(literal),
                "{literal} must stay a literal entry"
            );
        }
    }

    /// V4 guard: this repo has TWO path resolvers and the split is deliberate.
    ///
    /// `file_ops::path_utils::check_and_resolve_path` is a denylist with no
    /// jail (tilde/HOME expansion, `FsScope` anchoring, canonicalization,
    /// credential + glob denylist, `/proc` block, and — per the tool
    /// DESCRIPTION — absolute paths used as-is).
    /// `sandbox::workspace::path::normalize_path` is a jail with no denylist
    /// (lexical `..` popping *before* any syscall, no expansion, no denylist,
    /// hard containment enforced by its caller).
    ///
    /// Unifying them silently deletes one of the two answers, so this fails by
    /// name if either stops being the sole resolver for its own layer, or if a
    /// third resolver appears in either file.
    #[test]
    fn the_two_path_resolvers_stay_split() {
        // CRLF-safe: strip carriage returns FIRST, then split on an unanchored
        // needle (the bare attribute, no surrounding newlines) so a CRLF
        // checkout does not turn the "production prefix" into the whole file.
        fn production_prefix(src: &str) -> String {
            src.replace('\r', "")
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .to_string()
        }
        /// Every `fn` name in `src` whose name mentions resolving or
        /// normalizing — i.e. every candidate path resolver. Matches on a line
        /// that *starts* a definition (after visibility / `const` / `async` /
        /// `unsafe`), so prose mentioning a function name is not counted.
        fn resolver_fn_names(src: &str) -> Vec<String> {
            src.lines()
                .filter_map(|line| {
                    let mut rest = line.trim_start();
                    for prefix in [
                        "pub(crate) ",
                        "pub(super) ",
                        "pub ",
                        "const ",
                        "async ",
                        "unsafe ",
                    ] {
                        if let Some(stripped) = rest.strip_prefix(prefix) {
                            rest = stripped;
                        }
                    }
                    let name = rest.strip_prefix("fn ")?;
                    let name = name.split(['(', '<', ' ']).next().unwrap_or_default();
                    (name.contains("resolve") || name.contains("normaliz"))
                        .then(|| name.to_string())
                })
                .collect()
        }

        let file_layer_src = include_str!("path_utils.rs");
        let exec_layer_src = include_str!("../../sandbox/workspace/path.rs");
        let file_layer = production_prefix(file_layer_src);
        let exec_layer = production_prefix(exec_layer_src);

        // Non-vacuity: the split really removed this file's test module, and
        // both halves really are the files we think they are.
        assert!(
            file_layer.len() < file_layer_src.replace('\r', "").len(),
            "the cfg(test) split cut nothing off path_utils.rs — the needle drifted"
        );
        assert!(
            !file_layer.contains("fn the_two_path_resolvers_stay_split"),
            "this very test leaked into the production prefix"
        );
        assert!(
            exec_layer.contains("workspace sandbox"),
            "exec-layer source not found where expected"
        );

        // 1. Each layer has exactly the resolvers it is supposed to have. A new
        //    one — or a moved one — fails here, by name.
        let mut file_resolvers = resolver_fn_names(&file_layer);
        file_resolvers.sort();
        assert_eq!(
            file_resolvers,
            vec![
                "check_and_resolve_path",
                "denied_entry_normalized",
                "resolve_for_removal",
                "safe_normalize",
            ],
            "file-layer resolvers changed; if this is a new resolver, say which \
             of the two questions it answers before adding it"
        );
        let mut exec_resolvers = resolver_fn_names(&exec_layer);
        exec_resolvers.sort();
        assert_eq!(
            exec_resolvers,
            vec!["normalize_path"],
            "exec-layer resolvers changed; `normalize_path` must stay the only one"
        );

        // 2. The properties that make them different must survive. The exec
        //    layer must never grow a denylist (its caller's jail is the gate),
        //    and it must not start canonicalizing (its `..` popping is
        //    deliberately lexical and pre-syscall).
        for banned in ["path_is_denied", "denied_paths", "canonicalize("] {
            assert!(
                !exec_layer.contains(banned),
                "`{banned}` appeared in the exec-layer resolver: the jail does not \
                 get a denylist — that is the file layer's question"
            );
        }
        // And the file layer must not start jailing through the exec resolver.
        assert!(
            !file_layer.contains("normalize_path("),
            "the file layer must not call the exec-layer resolver; it has no \
             workspace root to jail against"
        );
        assert!(
            file_layer.contains("fn path_is_denied") && file_layer.contains("fs_scope"),
            "the file layer lost its denylist or its FsScope anchoring"
        );
    }

    /// The deny gate evaluates the FINAL (post-rebase) path — a rebase can
    /// never launder a denied target.
    #[tokio::test]
    async fn fs_scope_rebase_cannot_bypass_deny() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::write(repo.path().join("secret.txt"), b"s").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        // Deny the REBASED location only.
        let denied = vec![wt_c.join("secret.txt").to_string_lossy().to_string()];
        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c, repo_c.clone());
        let input = repo_c.join("secret.txt");
        let result = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &denied, None)
        })
        .await;
        assert!(
            result.is_err(),
            "deny must apply to the post-rebase target, got {result:?}"
        );
    }
}
