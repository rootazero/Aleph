//! Cross-platform Chromium browser discovery.
//!
//! Searches for a usable Chromium-based browser binary in three stages:
//! 1. `ALEPH_CHROME_PATH` environment variable override
//! 2. Platform-specific well-known installation paths
//! 3. `PATH` lookup via `which` for common binary names

use std::path::PathBuf;

use super::BrowserError;

/// Well-known binary names for PATH lookup (cross-platform).
const CHROMIUM_NAMES: &[&str] = &[
    "google-chrome-stable",
    "google-chrome",
    "chromium-browser",
    "chromium",
    "microsoft-edge-stable",
    "microsoft-edge",
    "brave-browser",
];

/// Discover a Chromium-based browser binary on the current system.
///
/// Search order:
/// 1. `ALEPH_CHROME_PATH` env var — explicit user override
/// 2. Platform-specific default installation paths (see [`platform_paths`])
/// 3. `PATH` lookup via `which` for common names
///
/// Returns the first existing path found, or [`BrowserError::ChromiumNotFound`].
pub fn find_chromium() -> Result<PathBuf, BrowserError> {
    // Stage 1: Environment variable override
    if let Ok(env_path) = std::env::var("ALEPH_CHROME_PATH") {
        let p = PathBuf::from(&env_path);
        if p.exists() {
            tracing::debug!("Chromium found via ALEPH_CHROME_PATH: {}", p.display());
            return Ok(p);
        }
        tracing::warn!(
            "ALEPH_CHROME_PATH set to '{}' but file does not exist, continuing search",
            env_path
        );
    }

    // Stage 2: Platform-specific well-known paths
    for path in platform_paths() {
        if path.exists() {
            tracing::debug!("Chromium found at platform path: {}", path.display());
            return Ok(path);
        }
    }

    // Stage 3: PATH lookup
    for name in CHROMIUM_NAMES {
        if let Ok(path) = which::which(name) {
            tracing::debug!("Chromium found via PATH as '{}': {}", name, path.display());
            return Ok(path);
        }
    }

    Err(BrowserError::ChromiumNotFound)
}

/// Return platform-specific default Chromium installation paths.
///
/// Uses conditional compilation to provide the correct paths for each OS.
pub fn platform_paths() -> Vec<PathBuf> {
    platform_paths_impl()
}

#[cfg(target_os = "macos")]
fn platform_paths_impl() -> Vec<PathBuf> {
    vec![
        // Google Chrome
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        // Microsoft Edge
        PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ),
        // Brave
        PathBuf::from(
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ),
        // Chromium
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        // Chrome Canary
        PathBuf::from(
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ),
    ]
}

#[cfg(target_os = "windows")]
fn platform_paths_impl() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Collect base directories: ProgramFiles, ProgramFiles(x86), LOCALAPPDATA
    let base_dirs: Vec<PathBuf> = [
        std::env::var("ProgramFiles").ok(),
        std::env::var("ProgramFiles(x86)").ok(),
        std::env::var("LOCALAPPDATA").ok(),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from)
    .collect();

    for base in &base_dirs {
        // Google Chrome
        paths.push(base.join("Google").join("Chrome").join("Application").join("chrome.exe"));
        // Microsoft Edge
        paths.push(base.join("Microsoft").join("Edge").join("Application").join("msedge.exe"));
        // Brave
        paths.push(
            base.join("BraveSoftware")
                .join("Brave-Browser")
                .join("Application")
                .join("brave.exe"),
        );
    }

    paths
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_paths_impl() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/microsoft-edge-stable"),
        PathBuf::from("/usr/bin/microsoft-edge"),
        PathBuf::from("/usr/bin/brave-browser"),
        PathBuf::from("/snap/bin/chromium"),
        PathBuf::from("/usr/lib/chromium/chromium"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_chromium_returns_existing_path() {
        match find_chromium() {
            Ok(path) => {
                assert!(path.exists(), "Returned path should exist: {}", path.display());
            }
            Err(BrowserError::ChromiumNotFound) => {
                // Acceptable in CI / environments without a browser installed
                eprintln!("No Chromium found — acceptable in CI");
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }
    }

    #[serial_test::serial]
    #[test]
    fn test_env_override() {
        // /bin/sh exists on all Unix systems and serves as a reliable test target
        let sentinel = "/bin/sh";
        std::env::set_var("ALEPH_CHROME_PATH", sentinel);
        let result = find_chromium();
        std::env::remove_var("ALEPH_CHROME_PATH");

        let path = result.expect("ALEPH_CHROME_PATH pointing to /bin/sh should succeed");
        assert_eq!(path, PathBuf::from(sentinel));
    }

    #[test]
    fn test_platform_paths_not_empty() {
        let paths = platform_paths();
        assert!(
            !paths.is_empty(),
            "platform_paths() must return at least one candidate"
        );
        // Every entry should be an absolute path
        for p in &paths {
            assert!(p.is_absolute(), "Expected absolute path, got: {}", p.display());
        }
    }
}
