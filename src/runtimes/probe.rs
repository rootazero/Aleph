//! Probe module — detects installed runtimes by checking PATH.
//!
//! Reads spec data from `super::specs::SPECS`. Does NOT install anything —
//! only reports whether a binary is present and its version.

use crate::sync_primitives::Mutex;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, trace, warn};

use crate::runtimes::ledger::CapabilitySource;
use crate::runtimes::specs::{find_spec, RuntimeSpec};

/// Result of probing for a capability.
#[derive(Debug)]
pub struct ProbeResult {
    pub found: bool,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: CapabilitySource,
    pub version_warning: Option<String>,
}

impl ProbeResult {
    const fn not_found() -> Self {
        Self {
            found: false,
            bin_path: None,
            version: None,
            source: CapabilitySource::System,
            version_warning: None,
        }
    }
}

/// Probe for a named capability. Returns a `ProbeResult` describing what
/// was found on the system PATH (or nothing).
pub fn probe(name: &str) -> ProbeResult {
    let spec = match find_spec(name) {
        Some(s) => s,
        None => {
            debug!("no spec for capability '{}'", name);
            return ProbeResult::not_found();
        }
    };

    if let Some(result) = probe_system_path(spec) {
        debug!(
            "found '{}' on system PATH: {:?}",
            name,
            result.bin_path.as_deref().unwrap_or(Path::new("?"))
        );
        return result;
    }

    debug!("capability '{}' not found", name);
    ProbeResult::not_found()
}

fn probe_system_path(spec: &RuntimeSpec) -> Option<ProbeResult> {
    let bin_name = spec.binaries.iter().next()?;
    let bin_path = find_on_path(bin_name)?;
    let version = get_version(&bin_path, spec.version_flag, spec.version_regex);
    let version_warning = check_version_warning(spec, version.as_deref());
    Some(ProbeResult {
        found: true,
        bin_path: Some(bin_path),
        version,
        source: CapabilitySource::System,
        version_warning,
    })
}

/// Locate a binary on the system PATH, enriched with well-known runtime
/// install directories.
///
/// A GUI-launched daemon (macOS `.app` / launchd) inherits a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`) that excludes Homebrew (`/opt/homebrew/bin`),
/// cargo (`~/.cargo/bin`), and the fnm-managed node dir — so probing the raw
/// inherited PATH reports installed runtimes as missing. We therefore search the
/// inherited PATH *plus* those known install dirs, without mutating the global
/// process env (thread-safe; each probe gets a private enriched PATH).
///
/// First tries the platform-native locator (`which` on Unix, `where` on Windows).
/// Falls back to a manual PATH walk for minimal environments (e.g. containers
/// without the `which` binary).
fn find_on_path(bin_name: &str) -> Option<PathBuf> {
    let search_path = extend_path(
        &std::env::var_os("PATH").unwrap_or_default(),
        &install_dir_candidates(),
    );

    // 1. Try the native locator first (more reliable, handles aliases, etc.)
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    trace!("looking for '{}' via {}", bin_name, locator);
    if let Ok(output) = Command::new(locator)
        .arg(bin_name)
        .env("PATH", &search_path)
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let path_str = stdout.lines().next().unwrap_or("").trim();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }

    // 2. Fallback: manual traversal of the enriched PATH.
    trace!("falling back to manual PATH search for '{}'", bin_name);
    find_in_dirs(bin_name, &search_path)
}

/// Manually walk `search_path` (a joined PATH string) looking for `bin_name`.
fn find_in_dirs(bin_name: &str, search_path: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(search_path) {
        let candidates: Vec<PathBuf> = if cfg!(target_os = "windows") {
            vec![
                dir.join(format!("{bin_name}.exe")),
                dir.join(format!("{bin_name}.bat")),
                dir.join(format!("{bin_name}.cmd")),
                dir.join(bin_name),
            ]
        } else {
            vec![dir.join(bin_name)]
        };
        for candidate in candidates {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Prepend known runtime install dirs (that exist and aren't already present)
/// to `base`, returning a PATH string for `Command::env("PATH", ..)`. Pure and
/// side-effect free — does NOT touch the global process env.
fn extend_path(base: &OsStr, candidates: &[PathBuf]) -> OsString {
    let mut seen: HashSet<PathBuf> = std::env::split_paths(base).collect();
    let mut prepended: Vec<PathBuf> = Vec::new();
    for cand in candidates {
        if cand.is_dir() && seen.insert(cand.clone()) {
            prepended.push(cand.clone());
        }
    }
    if prepended.is_empty() {
        return base.to_os_string();
    }
    prepended.extend(std::env::split_paths(base));
    std::env::join_paths(&prepended).unwrap_or_else(|_| base.to_os_string())
}

/// Directories where runtimes commonly install but a GUI-launched daemon's
/// minimal PATH won't include: Homebrew, cargo/rustup, fnm-managed node, Xcode
/// CLT, winget shims. Mirrors `bootstrap::enrich_path_for_reprobe`'s candidate
/// set, plus the fnm node bin dir (node + npm-global CLIs like playwright-cli).
fn install_dir_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(h);
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join(".local").join("bin"));
    }
    if let Some(d) = fnm_node_bin_dir() {
        dirs.push(d);
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/Library/Developer/CommandLineTools/usr/bin"));
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(h) = std::env::var_os("USERPROFILE") {
            dirs.push(
                PathBuf::from(h)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
        }
        dirs.push(PathBuf::from(r"C:\Program Files\Git\cmd"));
    }
    dirs
}

/// Resolve the fnm-managed node bin dir *without invoking fnm* (fnm itself may
/// be off PATH). fnm keeps the active node under
/// `$FNM_DIR/aliases/{default,lts}/bin` (Unix) — default `$FNM_DIR` is
/// `~/.local/share/fnm`. On Windows `node.exe` sits directly in the alias dir.
fn fnm_node_bin_dir() -> Option<PathBuf> {
    let fnm_dir = std::env::var_os("FNM_DIR").map(PathBuf::from).or_else(|| {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".local").join("share").join("fnm"))
    })?;
    for alias in ["default", "lts"] {
        let alias_dir = fnm_dir.join("aliases").join(alias);
        // Unix: node lives under <alias>/bin; Windows: directly in <alias>.
        let bin = alias_dir.join("bin");
        if bin.is_dir() {
            return Some(bin);
        }
        if alias_dir.is_dir() {
            return Some(alias_dir);
        }
    }
    None
}

static REGEX_CACHE: Lazy<Mutex<HashMap<&'static str, Regex>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn get_compiled_regex(pattern: &'static str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            cache.insert(pattern, re.clone());
            Some(re)
        }
        Err(e) => {
            warn!("invalid version regex '{}': {}", pattern, e);
            None
        }
    }
}

fn get_version(bin_path: &Path, version_flag: &str, version_regex: &'static str) -> Option<String> {
    let output = Command::new(bin_path).arg(version_flag).output().ok()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let re = get_compiled_regex(version_regex)?;
    re.captures(&combined)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn check_version_warning(spec: &RuntimeSpec, version: Option<&str>) -> Option<String> {
    let min = spec.min_version?;
    let actual = version?;
    if version_lt(actual, min) {
        Some(format!(
            "{} version {} is below minimum {} — some features may not work",
            spec.name, actual, min
        ))
    } else {
        None
    }
}

/// Compare two version strings, checking only major.minor.
///
/// Patch and pre-release components are ignored — this is intentional
/// because runtime specs declare `min_version` at the minor level
/// (e.g. "18.0" for Node). If finer-grained comparison is needed in
/// the future, this should be replaced by a proper semver parser.
fn version_lt(actual: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> (u64, u64) {
        let mut parts = s.split('.');
        let major = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        let minor = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        (major, minor)
    };
    parse(actual) < parse(minimum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_lt_basics() {
        assert!(version_lt("3.9", "3.10"));
        assert!(!version_lt("3.12", "3.10"));
        assert!(!version_lt("3.10", "3.10"));
    }

    #[test]
    fn test_version_lt_ignores_patch() {
        assert!(version_lt("3.9.7", "3.10"));
        assert!(!version_lt("3.12.1", "3.10"));
    }

    #[test]
    fn test_probe_unknown_returns_not_found() {
        let r = probe("nonexistent_capability_xyz");
        assert!(!r.found);
        assert!(r.bin_path.is_none());
    }

    #[test]
    fn test_probe_known_spec_consistency() {
        // fnm may or may not be on the test machine; just assert the contract.
        let r = probe("fnm");
        if r.found {
            assert!(r.bin_path.is_some());
        } else {
            assert!(r.bin_path.is_none());
        }
    }

    #[test]
    fn test_probe_result_not_found_defaults() {
        let r = ProbeResult::not_found();
        assert!(!r.found);
        assert_eq!(r.source, CapabilitySource::System);
    }

    #[test]
    fn test_extend_path_prepends_existing_candidate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cand = tmp.path().to_path_buf();
        let base = OsString::from("/usr/bin:/bin");
        let out = extend_path(&base, std::slice::from_ref(&cand));
        let dirs: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(
            dirs.first(),
            Some(&cand),
            "existing candidate must be prepended ahead of the base PATH"
        );
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
    }

    #[test]
    fn test_extend_path_skips_nonexistent() {
        let base = OsString::from("/usr/bin");
        let ghost = PathBuf::from("/no/such/dir/xyz123");
        let out = extend_path(&base, std::slice::from_ref(&ghost));
        assert_eq!(out, base, "non-existent candidate must not be added");
    }

    #[test]
    fn test_extend_path_skips_duplicate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cand = tmp.path().to_path_buf();
        let base = std::env::join_paths([cand.clone(), PathBuf::from("/bin")]).unwrap();
        let out = extend_path(&base, std::slice::from_ref(&cand));
        let count = std::env::split_paths(&out).filter(|d| d == &cand).count();
        assert_eq!(count, 1, "candidate already on PATH must not be duplicated");
    }

    /// Reproduces the GUI-minimal-PATH root cause: a binary that lives in a dir
    /// absent from the inherited PATH is invisible until that dir is folded into
    /// the enriched search path.
    #[test]
    fn test_find_in_dirs_finds_binary_only_after_enrichment() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("mytool");
        std::fs::write(&bin, b"#!/bin/sh\ntrue\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&bin).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&bin, p).unwrap();
        }

        // Minimal base PATH does not include tmp → not found (the bug).
        let minimal = OsString::from("/usr/bin:/bin");
        assert!(
            find_in_dirs("mytool", &minimal).is_none(),
            "precondition: tool must be invisible on the minimal PATH"
        );

        // Enriched PATH includes tmp → found (the fix).
        let enriched = extend_path(&minimal, std::slice::from_ref(&tmp.path().to_path_buf()));
        assert_eq!(
            find_in_dirs("mytool", &enriched).as_deref(),
            Some(bin.as_path()),
            "tool must be found once its dir is part of the enriched search path"
        );
    }
}
