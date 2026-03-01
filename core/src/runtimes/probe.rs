//! Probe module — detects installed tools by probing the system.
//!
//! The Prober checks for capabilities in two locations (in priority order):
//! 1. Aleph-managed paths under `~/.aleph/runtimes/`
//! 2. System PATH via `which`
//!
//! It never installs anything — only detects what is already available.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::sync_primitives::Mutex;
use tracing::{debug, trace, warn};

use crate::runtimes::ledger::CapabilitySource;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Result of probing for a capability.
#[derive(Debug)]
pub struct ProbeResult {
    /// Whether the capability was found.
    pub found: bool,
    /// Absolute path to the binary, if found.
    pub bin_path: Option<PathBuf>,
    /// Detected version string, if parseable.
    pub version: Option<String>,
    /// Where the binary was found.
    pub source: CapabilitySource,
    /// Warning if the detected version is below the minimum.
    pub version_warning: Option<String>,
}

impl ProbeResult {
    /// Construct a "not found" result.
    fn not_found() -> Self {
        Self {
            found: false,
            bin_path: None,
            version: None,
            source: CapabilitySource::System,
            version_warning: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Probe specification table
// ---------------------------------------------------------------------------

/// Specification for how to probe a single capability.
struct ProbeSpec {
    /// Capability name (e.g. "python", "node").
    capability: &'static str,
    /// Binary names to search for (tried in order).
    binaries: &'static [&'static str],
    /// Flag passed to the binary to get its version (e.g. "--version").
    version_flag: &'static str,
    /// Regex to extract the version from the binary's output.
    version_regex: &'static str,
    /// Minimum acceptable version (major.minor). `None` means any version is OK.
    min_version: Option<&'static str>,
    /// Paths relative to `~/.aleph/runtimes/` where Aleph may have installed the binary.
    aleph_paths: &'static [&'static str],
}

const PROBE_SPECS: &[ProbeSpec] = &[
    ProbeSpec {
        capability: "python",
        binaries: &["python3", "python"],
        version_flag: "--version",
        version_regex: r"Python (\d+\.\d+\.\d+)",
        min_version: Some("3.10"),
        aleph_paths: &[
            "python/default/bin/python3",
            "uv/envs/default/bin/python3",
        ],
    },
    ProbeSpec {
        capability: "node",
        binaries: &["node"],
        version_flag: "--version",
        version_regex: r"v(\d+\.\d+\.\d+)",
        min_version: Some("18.0"),
        aleph_paths: &["fnm/versions/default/bin/node"],
    },
    ProbeSpec {
        capability: "ffmpeg",
        binaries: &["ffmpeg"],
        version_flag: "-version",
        version_regex: r"ffmpeg version (\S+)",
        min_version: None,
        aleph_paths: &["ffmpeg/ffmpeg"],
    },
    ProbeSpec {
        capability: "uv",
        binaries: &["uv"],
        version_flag: "--version",
        version_regex: r"uv (\d+\.\d+\.\d+)",
        min_version: None,
        aleph_paths: &["uv/uv"],
    },
    ProbeSpec {
        capability: "yt-dlp",
        binaries: &["yt-dlp"],
        version_flag: "--version",
        version_regex: r"(\d{4}\.\d+\.\d+)",
        min_version: None,
        aleph_paths: &["yt-dlp"],
    },
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe the system for a named capability.
///
/// Checks Aleph-managed paths first, then falls back to system PATH.
/// Returns a [`ProbeResult`] describing what was found.
pub fn probe(name: &str) -> ProbeResult {
    let spec = match PROBE_SPECS.iter().find(|s| s.capability == name) {
        Some(s) => s,
        None => {
            debug!("No probe spec for capability '{}'", name);
            return ProbeResult::not_found();
        }
    };

    // Priority 1: Aleph-managed paths
    if let Some(result) = probe_aleph_managed(spec) {
        debug!(
            "Found '{}' at Aleph-managed path: {:?}",
            name,
            result.bin_path.as_deref().unwrap_or(Path::new("?"))
        );
        return result;
    }

    // Priority 2: System PATH
    if let Some(result) = probe_system_path(spec) {
        debug!(
            "Found '{}' on system PATH: {:?}",
            name,
            result.bin_path.as_deref().unwrap_or(Path::new("?"))
        );
        return result;
    }

    debug!("Capability '{}' not found anywhere", name);
    ProbeResult::not_found()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check Aleph-managed paths under `~/.aleph/runtimes/`.
fn probe_aleph_managed(spec: &ProbeSpec) -> Option<ProbeResult> {
    let runtimes_dir = match crate::utils::paths::get_runtimes_dir() {
        Ok(dir) => dir,
        Err(e) => {
            warn!("Cannot determine runtimes dir: {}", e);
            return None;
        }
    };

    for rel_path in spec.aleph_paths {
        let candidate = runtimes_dir.join(rel_path);
        trace!("Checking Aleph path: {:?}", candidate);

        if candidate.is_file() {
            let version = get_version(&candidate, spec.version_flag, spec.version_regex);
            let version_warning = check_version_warning(spec, version.as_deref());

            return Some(ProbeResult {
                found: true,
                bin_path: Some(candidate),
                version,
                source: CapabilitySource::AlephManaged,
                version_warning,
            });
        }
    }

    None
}

/// Probe system PATH for the binary using `which`.
fn probe_system_path(spec: &ProbeSpec) -> Option<ProbeResult> {
    for bin_name in spec.binaries {
        trace!("Looking for '{}' on system PATH", bin_name);

        let cmd = if cfg!(target_os = "windows") { "where" } else { "which" };
        let output = Command::new(cmd).arg(bin_name).output().ok()?;

        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path_str.is_empty() {
                continue;
            }

            let bin_path = PathBuf::from(&path_str);
            let version = get_version(&bin_path, spec.version_flag, spec.version_regex);
            let version_warning = check_version_warning(spec, version.as_deref());

            return Some(ProbeResult {
                found: true,
                bin_path: Some(bin_path),
                version,
                source: CapabilitySource::System,
                version_warning,
            });
        }
    }

    None
}

/// Thread-safe cache for compiled regexes, keyed by pattern string.
static REGEX_CACHE: Lazy<Mutex<HashMap<&'static str, Regex>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Get a compiled regex from the cache, or compile and cache it.
fn get_compiled_regex(pattern: &'static str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().ok()?;
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            cache.insert(pattern, re.clone());
            Some(re)
        }
        Err(e) => {
            warn!("Invalid version regex '{}': {}", pattern, e);
            None
        }
    }
}

/// Execute the binary with its version flag and parse the version string.
fn get_version(bin_path: &Path, version_flag: &str, version_regex: &'static str) -> Option<String> {
    let output = Command::new(bin_path)
        .arg(version_flag)
        .output()
        .ok()?;

    // Some tools print version to stdout, others to stderr
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

/// Generate a warning string if the detected version is below the minimum.
fn check_version_warning(spec: &ProbeSpec, version: Option<&str>) -> Option<String> {
    let min = spec.min_version?;
    let actual = version?;

    if version_lt(actual, min) {
        Some(format!(
            "{} version {} is below minimum {} — some features may not work",
            spec.capability, actual, min
        ))
    } else {
        None
    }
}

/// Simple semver comparison on major.minor (ignores patch).
///
/// Returns `true` if `actual < minimum`.
///
/// # Examples
/// ```ignore
/// assert!(version_lt("3.9", "3.10"));
/// assert!(!version_lt("3.12", "3.10"));
/// ```
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

    let (a_major, a_minor) = parse(actual);
    let (m_major, m_minor) = parse(minimum);

    (a_major, a_minor) < (m_major, m_minor)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_lt() {
        assert!(version_lt("3.9", "3.10"));
        assert!(!version_lt("3.12", "3.10"));
        assert!(!version_lt("3.10", "3.10"));
        assert!(version_lt("2.0", "3.0"));
    }

    #[test]
    fn test_version_lt_with_patch() {
        // Patch component is ignored; only major.minor compared
        assert!(version_lt("3.9.7", "3.10"));
        assert!(!version_lt("3.12.1", "3.10"));
    }

    #[test]
    fn test_check_version_warning() {
        let spec = &PROBE_SPECS[0]; // python, min 3.10
        assert!(check_version_warning(spec, Some("3.9.0")).is_some());
        assert!(check_version_warning(spec, Some("3.12.0")).is_none());
        assert!(check_version_warning(spec, None).is_none());
    }

    #[test]
    fn test_check_version_warning_no_min() {
        let spec = &PROBE_SPECS[2]; // ffmpeg, no min_version
        assert!(check_version_warning(spec, Some("6.1")).is_none());
        assert!(check_version_warning(spec, None).is_none());
    }

    #[test]
    fn test_probe_unknown_capability() {
        let result = probe("nonexistent_tool");
        assert!(!result.found);
    }

    #[test]
    fn test_probe_python_finds_system() {
        let result = probe("python");
        // Python may or may not be installed in CI, so we just
        // check consistency: if found, bin_path must be present.
        if result.found {
            assert!(result.bin_path.is_some());
        }
    }

    #[test]
    fn test_probe_result_not_found_defaults() {
        let r = ProbeResult::not_found();
        assert!(!r.found);
        assert!(r.bin_path.is_none());
        assert!(r.version.is_none());
        assert!(r.version_warning.is_none());
        assert_eq!(r.source, CapabilitySource::System);
    }
}
