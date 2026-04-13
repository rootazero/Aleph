//! Probe module — detects installed runtimes by checking PATH.
//!
//! Reads spec data from `super::specs::SPECS`. Does NOT install anything —
//! only reports whether a binary is present and its version.

use crate::sync_primitives::Mutex;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
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
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    for bin_name in spec.binaries {
        trace!("looking for '{}' via {}", bin_name, locator);
        let output = Command::new(locator).arg(bin_name).output().ok()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // On Windows, `where` can return multiple lines; take the first.
            let path_str = stdout.lines().next().unwrap_or("").trim().to_string();
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

/// Simple semver comparison on major.minor only.
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
}
