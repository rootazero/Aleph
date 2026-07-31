//! Cross-platform Chromium browser discovery.
//!
//! Searches for a usable Chromium-based browser binary in three stages:
//! 1. `ALEPH_CHROME_PATH` environment variable override
//! 2. Platform-specific well-known installation paths
//! 3. `PATH` lookup via `which` for common binary names
//!
//! [`find_chromium_preferred`] runs the same stages but tries the configured
//! engine's candidates first, falling back to the remaining ones in the
//! standard order — a profile that asks for Brave should not silently launch
//! Chrome when Brave is installed.

use std::path::PathBuf;

use super::profile::BrowserType;
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

/// Stage 1 of discovery, shared by both entry points: the explicit
/// `ALEPH_CHROME_PATH` override. Returns `None` when unset or pointing at a
/// non-existent file (with a warning, so a typo doesn't silently fall through).
fn env_override() -> Option<PathBuf> {
    let env_path = std::env::var("ALEPH_CHROME_PATH").ok()?;
    let p = PathBuf::from(&env_path);
    if p.is_file() {
        tracing::debug!("Chromium found via ALEPH_CHROME_PATH: {}", p.display());
        Some(p)
    } else {
        tracing::warn!(
            "ALEPH_CHROME_PATH set to '{}' but file does not exist, continuing search",
            env_path
        );
        None
    }
}

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
    if let Some(p) = env_override() {
        return Ok(p);
    }

    // Stage 2: Platform-specific well-known paths
    for path in platform_paths() {
        if path.is_file() {
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

/// Engine-specific discovery hints: path substrings identifying the engine's
/// install locations, plus its PATH binary names. Substrings are matched
/// case-sensitively against the platform paths; each engine lists the casings
/// its real install paths use (macOS "Google Chrome" / "Brave Browser" /
/// "Microsoft Edge" vs linux `/usr/bin/…` lowercase names).
fn engine_hints(browser: &BrowserType) -> (&'static [&'static str], &'static [&'static str]) {
    match browser {
        BrowserType::Chromium => (&["Chromium", "chromium"], &["chromium-browser", "chromium"]),
        BrowserType::Chrome => (
            &["Google Chrome", "Chrome", "chrome"],
            &["google-chrome-stable", "google-chrome"],
        ),
        BrowserType::Brave => (&["Brave", "brave"], &["brave-browser"]),
        BrowserType::Edge => (
            &["Edge", "msedge"],
            &["microsoft-edge-stable", "microsoft-edge"],
        ),
    }
}

/// Partition `paths` into preferred-first order: entries whose string form
/// contains any of `substrings` lead, the rest follow in their original order.
/// Pure so the candidate ordering is testable without browsers on disk.
fn prefer_paths(paths: Vec<PathBuf>, substrings: &[&str]) -> Vec<PathBuf> {
    let (preferred, rest): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| {
        let s = p.to_string_lossy();
        substrings.iter().any(|sub| s.contains(sub))
    });
    preferred.into_iter().chain(rest).collect()
}

/// `preferred` names first, then the remaining `all` names in their original
/// order. Pure twin of [`prefer_paths`] for the PATH-lookup stage.
fn prefer_names<'a>(all: &[&'a str], preferred: &[&'a str]) -> Vec<&'a str> {
    preferred
        .iter()
        .copied()
        .chain(all.iter().copied().filter(|n| !preferred.contains(n)))
        .collect()
}

/// Like [`find_chromium`], but the candidates for `browser` are tried first
/// within each stage; the remaining candidates follow in the standard order,
/// so a missing preferred engine degrades to the usual fallback chain instead
/// of failing.
pub fn find_chromium_preferred(browser: &BrowserType) -> Result<PathBuf, BrowserError> {
    // Stage 1: Environment variable override — an explicit path always wins.
    if let Some(p) = env_override() {
        return Ok(p);
    }

    let (path_substrings, names) = engine_hints(browser);

    // Stage 2: platform paths, preferred engine's install locations first.
    for path in prefer_paths(platform_paths(), path_substrings) {
        if path.is_file() {
            tracing::debug!("Chromium found at platform path: {}", path.display());
            return Ok(path);
        }
    }

    // Stage 3: PATH lookup, preferred engine's binary names first.
    for name in prefer_names(CHROMIUM_NAMES, names) {
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
pub(crate) fn platform_paths() -> Vec<PathBuf> {
    platform_paths_impl()
}

#[cfg(target_os = "macos")]
fn platform_paths_impl() -> Vec<PathBuf> {
    vec![
        // Google Chrome
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        // Microsoft Edge
        PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        // Brave
        PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        // Chromium
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        // Chrome Canary
        PathBuf::from("/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"),
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
        paths.push(
            base.join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe"),
        );
        // Microsoft Edge
        paths.push(
            base.join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe"),
        );
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
                assert!(
                    path.exists(),
                    "Returned path should exist: {}",
                    path.display()
                );
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
    #[cfg(unix)] // POSIX-only: uses /bin/sh as the sentinel executable
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
            assert!(
                p.is_absolute(),
                "Expected absolute path, got: {}",
                p.display()
            );
        }
    }

    #[test]
    fn prefer_paths_puts_matching_engine_first_and_keeps_fallback() {
        let paths = vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ];
        let (subs, _) = engine_hints(&BrowserType::Brave);
        let ordered = prefer_paths(paths, subs);
        // Preferred engine leads…
        assert!(ordered[0].to_string_lossy().contains("Brave"));
        // …and every other candidate survives as fallback, order preserved.
        assert_eq!(ordered.len(), 3);
        assert!(ordered[1].to_string_lossy().contains("Google Chrome"));
        assert!(ordered[2].to_string_lossy().contains("Chromium"));
    }

    #[test]
    fn prefer_paths_no_match_keeps_standard_order() {
        let paths = vec![PathBuf::from("/a/chrome"), PathBuf::from("/b/chromium")];
        let ordered = prefer_paths(paths, &["NoSuchEngine"]);
        assert_eq!(ordered[0], PathBuf::from("/a/chrome"));
        assert_eq!(ordered[1], PathBuf::from("/b/chromium"));
    }

    #[test]
    fn prefer_names_puts_engine_names_first_without_duplicates() {
        let (_, names) = engine_hints(&BrowserType::Edge);
        let ordered = prefer_names(CHROMIUM_NAMES, names);
        assert_eq!(ordered[0], "microsoft-edge-stable");
        assert_eq!(ordered[1], "microsoft-edge");
        // Full list preserved, no duplicates: preferred names appear exactly once.
        assert_eq!(ordered.len(), CHROMIUM_NAMES.len());
        let mut sorted = ordered.clone();
        sorted.sort_unstable();
        let mut baseline = CHROMIUM_NAMES.to_vec();
        baseline.sort_unstable();
        assert_eq!(sorted, baseline);
    }

    #[test]
    fn engine_hints_cover_every_browser_type() {
        // Each engine yields non-empty hints and its names are a subset of the
        // PATH lookup list (otherwise the preferred stage would be a no-op).
        for bt in [
            BrowserType::Chromium,
            BrowserType::Chrome,
            BrowserType::Brave,
            BrowserType::Edge,
        ] {
            let (subs, names) = engine_hints(&bt);
            assert!(!subs.is_empty() && !names.is_empty(), "{bt:?} hints empty");
            for n in names {
                assert!(
                    CHROMIUM_NAMES.contains(n),
                    "{bt:?} name '{n}' missing from CHROMIUM_NAMES"
                );
            }
        }
    }

    #[test]
    fn engine_hints_match_real_platform_paths() {
        // The substrings must actually discriminate the current platform's
        // candidate list — a hint matching nothing would silently degrade to
        // the standard order. Only checked when the platform list has >1 entry.
        let paths = platform_paths();
        if paths.len() > 1 {
            let (subs, _) = engine_hints(&BrowserType::Chrome);
            let ordered = prefer_paths(paths, subs);
            assert!(
                ordered[0]
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("chrome"),
                "Chrome hint should lead with a Chrome path, got {}",
                ordered[0].display()
            );
        }
    }
}
