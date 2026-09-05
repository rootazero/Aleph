//! Which Chromium the managed driver launches, and where it came from.
//!
//! Order (spec §6.1): the operator's pin > a system Chromium-family browser >
//! Playwright's own. A system browser wins by default because Windows almost
//! always has Edge and macOS usually has Chrome — the ~150 MB download is for
//! a clean Linux host. The Chrome spike ran system Chrome 152 against
//! playwright-core 1.60, so mixing versions across that boundary is measured.
//!
//! # Why the Playwright path asks the CLI instead of globbing a cache
//!
//! The cache root is `~/Library/Caches/ms-playwright` on macOS, `~/.cache/…`
//! on Linux, `%LOCALAPPDATA%\ms-playwright` on Windows; the revision in the
//! directory name changes with every playwright-core release; and the
//! executable inside is `Google Chrome for Testing.app/Contents/MacOS/Google
//! Chrome for Testing` on macOS, not `Chromium`. Hard-coding that is three
//! platform tables and a revision guess — four facts that rot independently of
//! the installer that produces them. `playwright-cli install-browser <b>
//! --dry-run` prints the install location for the exact build THIS CLI would
//! use, so the same binary that installs it is the one that says where it is
//! (判据 §1: one derivation).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use crate::utils::no_window::NoWindow;

use super::error::BrowserError;
use super::profile::{BrowserRuntimeConfig, BrowserType};

/// How long the `--dry-run` probe may take.
///
/// It performs **no download** — it prints a table and exits (measured: it
/// answered instantly on the machine this plan was written on). Six seconds is
/// therefore generous, and the ceiling matters upward as well as downward: the
/// doctor check that calls this (`diagnostics::checks::chromium_missing`)
/// bounds the whole resolution at 8 s so that ITS own "could not verify" answer
/// fires, and that 8 s sits under the engine's `DEFAULT_CHECK_TIMEOUT` of 20 s
/// (`src/diagnostics/check.rs:27`), past which the engine abandons the check
/// and emits a `Warning` of its own. Three budgets, strictly nested: 6 < 8 < 20,
/// and the nesting is asserted by
/// `diagnostics::checks::chromium_missing::the_check_answers_before_the_engine_abandons_it`
/// rather than restated as prose — which is why this is `pub(crate)`.
pub(crate) const DRY_RUN_TIMEOUT: Duration = Duration::from_secs(6);

/// The header anchor of the browser block in `--dry-run` output.
///
/// The trailing `v` matters: `chromium-headless-shell` starts with `chromium`,
/// and its block names a directory with no browser in it.
const CHROMIUM_BLOCK: &str = "(playwright chromium v";

/// Executable leaf names, best first.
///
/// macOS is verified on this machine; the Linux and Windows leaves come from
/// Playwright's published layout and are **not** verified here. An unknown
/// layout therefore yields `None` and a fail-closed error naming the install
/// command — never a wrong file.
///
/// **`chrome-headless-shell` is deliberately NOT here.** It is a real Chromium
/// binary and spec §6.1 names it as the no-root Linux degrade, but taking it
/// would be a silent capability cut: it cannot run headed, so a
/// `headless = false` profile that resolved to it would launch a browser that
/// can never show a window, and nothing in this plan forces `headless` off the
/// resolution. Wiring that degrade properly (a source variant the launch reads,
/// plus an install path for the shell) is a separate piece of work; listing the
/// binary without it would be a route that reports success and delivers less.
const EXECUTABLE_LEAVES: &[&str] = &[
    "Google Chrome for Testing",
    "Chromium",
    "chrome",
    "chrome.exe",
];

/// How deep the install directory is walked looking for the executable.
///
/// macOS is the deepest known layout and needs five levels below the install
/// dir (`chrome-mac-arm64` / `X.app` / `Contents` / `MacOS` / `X`); Linux and
/// Windows need two. ⚠️ The walk enumerates the whole `.app` bundle, which is
/// thousands of entries — acceptable because it runs only on the
/// Playwright-managed route, i.e. only when no pin and no system browser
/// answered, and the result is cached by the caller.
const WALK_MAX_DEPTH: usize = 5;

/// Where the resolved binary came from. Carried so the log line and the doctor
/// finding can say which of the three answers won, instead of only that one did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChromiumSource {
    Pinned,
    System,
    PlaywrightManaged,
}

impl ChromiumSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Pinned => "pinned by [browser.runtime] binary_path",
            Self::System => "a system Chromium-family browser",
            Self::PlaywrightManaged => "Playwright's managed Chromium",
        }
    }
}

/// What a resolution answered: the file, which route found it, and **which
/// engine it turned out to be**.
///
/// The third field is the replacement for the boot warning this round deletes
/// (`manager::unhonored_managed_fields`). `find_chromium_preferred` degrades
/// silently when the requested engine is absent, so without this the
/// substitution "asked for Brave, got Chrome" would be reported nowhere.
/// `None` means the path matched no engine hint — unidentifiable, which is
/// **not** evidence that the request was honoured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedChromium {
    pub path: PathBuf,
    pub source: ChromiumSource,
    pub engine: Option<BrowserType>,
}

/// The browser block's `Install location:` out of `--dry-run` output.
pub(crate) fn parse_install_location(dry_run_stdout: &str) -> Option<PathBuf> {
    let mut in_block = false;
    for line in dry_run_stdout.lines() {
        if line.contains(CHROMIUM_BLOCK) {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("Install location:") {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
        // A blank line ends the block; anything else non-indented starts the
        // next product. Either way, this block had no location.
        if !line.starts_with(' ') {
            in_block = false;
        }
    }
    None
}

/// The best executable among a directory listing, by [`EXECUTABLE_LEAVES`] order.
pub(crate) fn executable_among(files: &[PathBuf]) -> Option<PathBuf> {
    EXECUTABLE_LEAVES.iter().find_map(|leaf| {
        files
            .iter()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n == std::ffi::OsStr::new(leaf))
            })
            .cloned()
    })
}

/// Every file under `dir`, to a bounded depth. Bounded because this walks a
/// browser distribution: hundreds of files, and an unbounded walk over a
/// symlink loop would not return.
fn files_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(files_under(&path, depth - 1)),
            Ok(_) => out.push(path),
            Err(_) => {}
        }
    }
    out
}

/// Resolve the binary the managed driver should launch for `browser`.
///
/// # Errors
///
/// [`BrowserError::ChromiumUnavailable`] when no route produced a file — its
/// message names the install command and the pin, because a closed gate that
/// does not say how to open it is fail-dead (判据 §14). A pin that does not
/// exist is that error too, and deliberately not a fallback: launching a
/// different browser than the one the operator named would be a silent
/// substitution of exactly the thing they pinned to prevent.
///
/// **This function never installs anything.** Spec §6.1's "try once more at
/// first use" is deliberately not done here: the download is ~150 MB and this
/// runs on the first browser tool call, whose hard budget is 180 s. Installing
/// has three explicit entrances instead — the ledger's post-install, the
/// doctor's fix hint, and `runtime_manage{install}`.
pub(crate) async fn resolve_binary(
    runtime: &BrowserRuntimeConfig,
    browser: &BrowserType,
    cli_binary: &Path,
) -> Result<ResolvedChromium, BrowserError> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(pin) = runtime.pinned_binary() {
        let path = PathBuf::from(pin);
        if path.is_file() {
            let engine = super::discovery::engine_of(&path);
            return Ok(ResolvedChromium {
                path,
                source: ChromiumSource::Pinned,
                engine,
            });
        }
        return Err(BrowserError::ChromiumUnavailable {
            tried: format!("[browser.runtime] binary_path = {pin:?} does not exist"),
        });
    }

    if runtime.prefer_system_browser {
        match super::discovery::find_chromium_preferred(browser) {
            Ok(path) => {
                let engine = super::discovery::engine_of(&path);
                return Ok(ResolvedChromium {
                    path,
                    source: ChromiumSource::System,
                    engine,
                });
            }
            Err(e) => tried.push(format!("no system browser ({e})")),
        }
    } else {
        tried.push("system browsers skipped (prefer_system_browser = false)".to_string());
    }

    match playwright_managed(cli_binary).await {
        Ok(path) => {
            if *browser != BrowserType::Chromium {
                // Naming the substitution rather than performing it silently:
                // Playwright manages Chromium and nothing else, so a profile
                // asking for Brave gets Chromium here. The caller's own
                // requested-vs-resolved check covers the OTHER silent
                // substitution (`find_chromium_preferred` degrading to whatever
                // is installed); this arm covers the one that route cannot see.
                tracing::warn!(
                    requested = ?browser,
                    "no system browser for the requested engine; falling back to \
                     Playwright's managed Chromium"
                );
            }
            let engine = super::discovery::engine_of(&path);
            Ok(ResolvedChromium {
                path,
                source: ChromiumSource::PlaywrightManaged,
                engine,
            })
        }
        Err(why) => {
            tried.push(why);
            Err(BrowserError::ChromiumUnavailable {
                tried: tried.join("; "),
            })
        }
    }
}

/// Ask the CLI where its own Chromium lives, then find the executable there.
///
/// The `Err` is a sentence for [`BrowserError::ChromiumUnavailable`]'s `tried`
/// field, not an error to propagate: "the CLI would not answer" and "the CLI
/// answered a directory that is not there yet" are both just "this route did
/// not produce a browser".
async fn playwright_managed(cli_binary: &Path) -> Result<PathBuf, String> {
    let mut cmd = Command::new(cli_binary);
    cmd.args(["install-browser", "chromium", "--dry-run"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(DRY_RUN_TIMEOUT, cmd.no_window().output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("playwright-cli install-browser --dry-run: {e}")),
        Err(_) => {
            return Err(format!(
                "playwright-cli install-browser --dry-run did not answer in {}s",
                DRY_RUN_TIMEOUT.as_secs()
            ))
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(dir) = parse_install_location(&stdout) else {
        return Err("playwright-cli did not report an install location for chromium".to_string());
    };
    executable_among(&files_under(&dir, WALK_MAX_DEPTH))
        .ok_or_else(|| format!("no chromium executable under {}", dir.display()))
}

/// Whether the resolved binary is a substitution worth warning about.
///
/// A bare `resolved != Some(requested)` would fire on essentially every
/// launch: [`BrowserType::default`] is `Chromium`, and the resolved engine is
/// Chrome on macOS, Edge on Windows, or "Google Chrome for Testing" read off
/// the Playwright route — none of which equal `Chromium` even though nothing
/// went wrong. An always-firing warning is not a warning (判据 §2).
///
/// This reproduces exactly the scope of the boot warning this plan deletes
/// (`manager::unhonored_managed_fields`), which fired only for a managed Brave
/// profile: true only when the profile asked for a **non-default** engine and
/// did not get it.
pub(crate) fn engine_mismatch(requested: &BrowserType, resolved: Option<&BrowserType>) -> bool {
    *requested != BrowserType::default() && resolved != Some(requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real transcript, verbatim. Three `Install location:` lines appear;
    /// only the FIRST section is the browser — the other two are ffmpeg and the
    /// headless shell. A parser that took "the first Install location" would be
    /// right today by luck; this one anchors on the section header, which is
    /// the thing that says which product the block is about.
    const DRY_RUN: &str = "\
Chrome for Testing 147.0.7727.49 (playwright chromium v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium-1219
  Download url:        https://cdn.playwright.dev/builds/cft/147.0.7727.49/mac-arm64/chrome-mac-arm64.zip

FFmpeg (playwright ffmpeg v1011)
  Install location:    /Users/x/Library/Caches/ms-playwright/ffmpeg-1011
  Download url:        https://cdn.playwright.dev/dbazure/download/playwright/builds/ffmpeg/1011/ffmpeg-mac-arm64.zip

Chrome Headless Shell 147.0.7727.49 (playwright chromium-headless-shell v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1219
  Download url:        https://cdn.playwright.dev/builds/cft/147.0.7727.49/mac-arm64/chrome-headless-shell-mac-arm64.zip
";

    #[test]
    fn the_install_location_comes_from_the_chromium_block_not_the_first_line() {
        assert_eq!(
            parse_install_location(DRY_RUN),
            Some(PathBuf::from(
                "/Users/x/Library/Caches/ms-playwright/chromium-1219"
            ))
        );
    }

    /// `chromium-headless-shell` starts with `chromium` — a substring match on
    /// the product name would pick the shell's directory, which has no browser
    /// in it. The anchor carries the closing `v` on purpose.
    #[test]
    fn the_headless_shell_block_is_not_mistaken_for_the_browser() {
        let shell_only = "\
Chrome Headless Shell 147.0.7727.49 (playwright chromium-headless-shell v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1219
";
        assert_eq!(parse_install_location(shell_only), None);
    }

    #[test]
    fn unparseable_output_answers_i_do_not_know_rather_than_a_path() {
        for bad in [
            "",
            "playwright-cli: unknown option --dry-run",
            "Chrome for Testing 147 (playwright chromium v1219)\n", // header, no location
            "  Install location:    /tmp/x\n",                      // location, no header
        ] {
            assert_eq!(parse_install_location(bad), None, "accepted {bad:?}");
        }
    }

    /// The macOS layout is the one this machine actually has; the Linux and
    /// Windows leaves are documented-but-unverified, so they are listed as
    /// candidates and nothing more. Whatever the layout, the answer must be a
    /// FILE inside the directory the CLI named — never a guess assembled from
    /// a platform constant.
    ///
    /// ⚠️ Every path here uses `/`. A backslash Windows path (`C:\…\chrome.exe`)
    /// written as a literal would make this test RED on macOS and Linux, where
    /// `\` is not a separator and `Path::file_name` therefore returns the whole
    /// string — and the RED→GREEN loop would push the executor to "fix" a
    /// correct implementation. `file_name()` handles forward slashes on every
    /// target, and Windows accepts them too.
    #[test]
    fn the_executable_is_found_in_each_known_layout() {
        let mac = vec![
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/Info.plist"),
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"),
        ];
        assert_eq!(
            executable_among(&mac),
            Some(PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"))
        );

        let linux = vec![
            PathBuf::from("/c/chromium-1219/chrome-linux/icudtl.dat"),
            PathBuf::from("/c/chromium-1219/chrome-linux/chrome"),
        ];
        assert_eq!(
            executable_among(&linux),
            Some(PathBuf::from("/c/chromium-1219/chrome-linux/chrome"))
        );

        let windows = vec![
            PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.dll"),
            PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.exe"),
        ];
        assert_eq!(
            executable_among(&windows),
            Some(PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.exe"))
        );
    }

    /// An install directory that exists but holds no browser (a half-extracted
    /// download, a layout this list does not know) must answer `None`, so the
    /// caller fails closed with the install command rather than handing
    /// `Command::new` a directory.
    #[test]
    fn an_unrecognised_layout_answers_none_rather_than_a_wrong_file() {
        let files = vec![
            PathBuf::from("/c/chromium-1219/INSTALLATION_COMPLETE"),
            PathBuf::from("/c/chromium-1219/DEPENDENCIES_VALIDATED"),
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/Info.plist"),
        ];
        assert_eq!(executable_among(&files), None);
    }

    /// `chrome-headless-shell` is a real Chromium binary and spec §6.1 names it
    /// as the no-root Linux degrade — and it is deliberately NOT a candidate
    /// here. Taking it silently would give a `headless = false` profile a
    /// browser that can never show a window. A route that reports success and
    /// delivers less is worse than one that refuses, so the shell must not be
    /// picked even when it is the ONLY thing in the directory.
    #[test]
    fn the_headless_shell_is_never_picked_even_when_it_is_the_only_binary() {
        let both = vec![
            PathBuf::from("/c/x/chrome-headless-shell-linux/chrome-headless-shell"),
            PathBuf::from("/c/x/chrome-linux/chrome"),
        ];
        assert_eq!(
            executable_among(&both),
            Some(PathBuf::from("/c/x/chrome-linux/chrome"))
        );
        let shell_only = vec![PathBuf::from(
            "/c/x/chrome-headless-shell-linux/chrome-headless-shell",
        )];
        assert_eq!(executable_among(&shell_only), None);
    }

    /// The engine identifier the substitution warning is derived from. It must
    /// answer from the SAME table `find_chromium_preferred` orders by, or the
    /// warning would be about a different notion of "which browser is this".
    #[test]
    fn the_resolved_engine_is_read_off_the_path_by_the_discovery_table() {
        use crate::browser::profile::BrowserType;
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new(
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
            )),
            Some(BrowserType::Brave)
        );
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new("/usr/bin/google-chrome")),
            Some(BrowserType::Chrome)
        );
        // Playwright's own build is Chromium, whatever its file is called.
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new(
                "/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
            )),
            Some(BrowserType::Chrome),
            "'Google Chrome for Testing' matches the Chrome hints; the caller \
             compares against what it ASKED for, and the Playwright route \
             already logs its own substitution"
        );
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new("/opt/weird/browser")),
            None,
            "unidentifiable must be None, not a guess — an unknown engine is \
             not evidence that the requested one was honoured"
        );
    }

    /// Four cases, matching ruling 3 verbatim: the warning must be silent on
    /// the default-engine path (which is every launch that never named an
    /// engine) and must fire only for the one case the deleted boot warning
    /// covered — a non-default request that was not honoured.
    #[test]
    fn default_request_resolved_to_chrome_is_not_a_mismatch() {
        assert!(!engine_mismatch(
            &BrowserType::default(),
            Some(&BrowserType::Chrome)
        ));
    }

    #[test]
    fn default_request_unidentified_engine_is_not_a_mismatch() {
        assert!(!engine_mismatch(&BrowserType::default(), None));
    }

    #[test]
    fn brave_requested_but_chrome_resolved_is_a_mismatch() {
        assert!(engine_mismatch(
            &BrowserType::Brave,
            Some(&BrowserType::Chrome)
        ));
    }

    #[test]
    fn brave_requested_and_brave_resolved_is_not_a_mismatch() {
        assert!(!engine_mismatch(
            &BrowserType::Brave,
            Some(&BrowserType::Brave)
        ));
    }

    /// The only one of `resolve_binary`'s three routes reachable without a
    /// real browser or a real `playwright-cli`: a pin pointing at a file that
    /// exists. `cli_binary` is a nonsense path — the pin route must win before
    /// either of the other two routes is even consulted.
    #[tokio::test]
    async fn a_valid_pin_resolves_without_touching_the_other_two_routes() {
        let dir = std::env::temp_dir().join(format!(
            "chromium_resolve_pin_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pinned = dir.join("Google Chrome for Testing");
        std::fs::write(&pinned, b"not a real binary, just needs to exist").unwrap();

        let runtime = BrowserRuntimeConfig {
            binary_path: Some(pinned.to_string_lossy().into_owned()),
            prefer_system_browser: true,
            download_host: None,
        };
        let result = resolve_binary(
            &runtime,
            &BrowserType::Brave,
            Path::new("/nonexistent/playwright-cli-should-never-be-invoked"),
        )
        .await
        .expect("a pin pointing at an existing file must resolve");

        assert_eq!(result.path, pinned);
        assert_eq!(result.source, ChromiumSource::Pinned);
        // The file name matches the Chrome hints — the pin route reports
        // whatever engine_of says, it does not echo back the request.
        assert_eq!(result.engine, Some(BrowserType::Chrome));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pin that does not exist is `ChromiumUnavailable`, not a fallback to
    /// the other two routes — launching a different browser than the one
    /// named would be exactly the silent substitution the pin exists to
    /// prevent.
    #[tokio::test]
    async fn a_pin_that_does_not_exist_is_unavailable_not_a_fallback() {
        let runtime = BrowserRuntimeConfig {
            binary_path: Some("/nonexistent/pinned-chrome".to_string()),
            prefer_system_browser: true,
            download_host: None,
        };
        let err = resolve_binary(
            &runtime,
            &BrowserType::Chromium,
            Path::new("/nonexistent/playwright-cli"),
        )
        .await
        .expect_err("a missing pin must not fall back to system/Playwright routes");

        match err {
            BrowserError::ChromiumUnavailable { tried } => {
                assert!(tried.contains("/nonexistent/pinned-chrome"));
            }
            other => panic!("expected ChromiumUnavailable, got {other:?}"),
        }
    }
}
