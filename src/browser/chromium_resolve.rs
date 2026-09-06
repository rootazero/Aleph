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
//!
//! # The dry-run answer names what the CLI would install NEXT, not what IS installed
//!
//! `--dry-run` reports the revision this `playwright-cli` release would
//! fetch today — not necessarily the revision sitting on disk. Playwright
//! bumps that number with nearly every release, and a browser installed a
//! few releases back stays perfectly usable. Measured on the machine this
//! module was written on: the dry-run named `chromium-1219`, while
//! `chromium-1208` and `chromium-1228` were the ones actually present, each
//! with a working executable — so treating the dry-run answer as "the
//! answer" would fail next to two usable browsers. [`resolve_binary`]'s
//! Playwright route therefore treats the dry-run's directory as a POINTER
//! INTO the cache, not as the answer: if nothing usable sits there,
//! [`scan_sibling_installs`] looks at its PARENT for other
//! `chromium-<revision>` directories and picks the newest one that looks
//! like a complete install.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use crate::utils::no_window::NoWindow;

use super::error::BrowserError;
use super::profile::{BrowserRuntimeConfig, BrowserType};

/// How long the `--dry-run` probe may take.
///
/// It performs **no download** — it prints a table and exits (measured: it
/// answered instantly on the machine this plan was written on). Six seconds
/// is therefore generous.
///
/// This must stay under whatever deadline the doctor check that will call it
/// sets for the WHOLE resolution, which must in turn stay under the
/// diagnostics engine's own `DEFAULT_CHECK_TIMEOUT` of 20 s
/// (`src/diagnostics/check.rs:27`) — past which the engine abandons the check
/// and emits a `Warning` of its own. **That doctor check does not exist yet**
/// (`diagnostics::checks::chromium_missing` is Task 7's work); asserting a
/// specific bound for it here would state a relationship no code enforces.
/// Task 7 owns choosing that deadline and asserting the nesting for real —
/// this constant is `pub(crate)` so Task 7's test can read it rather than
/// restate `6`.
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

/// Marker Playwright writes into a completed install directory — already
/// present as unrecognised filler in this file's own fixture
/// (`an_unrecognised_layout_answers_none_rather_than_a_wrong_file`). Reused
/// here as a positive signal: a directory with an executable but no marker
/// is a half-finished install, and [`scan_sibling_installs`] ranks it below
/// one that has both.
const INSTALLATION_COMPLETE_MARKER: &str = "INSTALLATION_COMPLETE";

/// Maximum bytes of a failing subprocess's stderr kept in an error message —
/// enough to show a real diagnostic without letting a runaway stream flood a
/// log line.
const STDERR_LOG_CAP: usize = 400;

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
            Self::Pinned => "pinned by [general.browser.runtime] binary_path",
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

/// Truncate `s` to at most `max_bytes` bytes, cutting only at a char
/// boundary. `s` came from `String::from_utf8_lossy` over a subprocess's
/// stderr — arbitrary bytes — so a blind `&s[..max_bytes]` could land inside
/// a multibyte character and panic (P7: never slice a string by raw byte
/// count; use `char_indices()`/`is_char_boundary`).
fn capped(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Installed `chromium-<revision>` siblings of the dry-run's own answer,
/// found by scanning its PARENT directory — see the module doc for why the
/// dry-run's own directory is not necessarily what is on disk.
///
/// Returns every revision whose directory name matched the `chromium-`
/// prefix (so a caller can report "these are the revisions we saw" even when
/// none of them panned out) alongside the best candidate's executable, if
/// any.
///
/// Ranking, highest first: (has [`INSTALLATION_COMPLETE_MARKER`] AND an
/// executable) before (has an executable but no marker), and within either
/// tier, the numerically highest revision wins — a finished install beats a
/// newer half-finished one, and the newest finished install beats an older
/// one.
///
/// `chromium_headless_shell-<rev>` siblings use an underscore where this
/// scan requires a hyphen, so `strip_prefix("chromium-")` excludes them
/// without extra logic — never hand one of those to a `headless = false`
/// profile, which is exactly what [`EXECUTABLE_LEAVES`] already refuses to do.
fn scan_sibling_installs(dry_run_location: &Path) -> (Vec<u64>, Option<PathBuf>) {
    let Some(parent) = dry_run_location.parent() else {
        return (Vec::new(), None);
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return (Vec::new(), None);
    };

    let mut seen_revisions = Vec::new();
    let mut candidates: Vec<(bool, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(revision) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("chromium-"))
            .and_then(|rev| rev.parse::<u64>().ok())
        else {
            continue;
        };
        seen_revisions.push(revision);
        if let Some(exe) = executable_among(&files_under(&path, WALK_MAX_DEPTH)) {
            let has_marker = path.join(INSTALLATION_COMPLETE_MARKER).is_file();
            candidates.push((has_marker, revision, exe));
        }
    }
    seen_revisions.sort_unstable();
    let best = candidates
        .into_iter()
        .max_by_key(|(has_marker, revision, _)| (*has_marker, *revision))
        .map(|(_, _, exe)| exe);
    (seen_revisions, best)
}

/// Whether `path` is executable, as far as this platform can tell.
///
/// Unix: exists AND carries at least one exec bit. `is_file()` alone
/// establishes existence, not launchability — a pin that lost its `+x`, a
/// downloaded artifact, or a plain (non-executable) file inside a `.app`
/// bundle all pass it and then die at `spawn()` with `Permission denied`,
/// after both doctor and `runtime_manage{list}` have already reported the
/// browser healthy (Final Review I2). Checked as `mode & 0o111 != 0` —
/// ANY exec bit — rather than picking the one that applies to this
/// process (owner/group/other): the OS already resolves that at `exec()`
/// time, and guessing which one applies here would just be a different
/// wrong answer dressed as precision.
///
/// Windows has no equivalent bit: executability there is decided by file
/// association and extension, which this resolver already establishes a
/// different way (the candidate is only ever a path this module itself
/// found or an operator's own `.exe` pin). So there is nothing to check on
/// that platform, and this always answers `true` — deliberately, not by
/// omission.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
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
            if !is_executable(&path) {
                // Final Review I2: `is_file()` establishes existence, not
                // launchability. A pin that lost its `+x`, a downloaded
                // artifact, or a plain file inside a `.app` bundle all pass
                // `is_file()` and then die at `spawn()` with `Permission
                // denied` — well after doctor and `runtime_manage{list}`
                // have both already reported the browser healthy. A third,
                // distinguishable answer, not "does not exist" (a different
                // fact) and not silently `Ok` (the bug this closes).
                return Err(BrowserError::ChromiumUnavailable {
                    tried: format!(
                        "[general.browser.runtime] binary_path = {pin:?} exists but is not executable"
                    ),
                });
            }
            let engine = super::discovery::engine_of(&path);
            return Ok(ResolvedChromium {
                path,
                source: ChromiumSource::Pinned,
                engine,
            });
        }
        return Err(BrowserError::ChromiumUnavailable {
            tried: format!("[general.browser.runtime] binary_path = {pin:?} does not exist"),
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

/// Ask the CLI where its own Chromium lives, then find the executable there
/// — or, if the dry-run's own directory holds nothing usable, the newest
/// sibling install that does (see [`scan_sibling_installs`] and the module
/// doc).
///
/// The `Err` is a sentence for [`BrowserError::ChromiumUnavailable`]'s `tried`
/// field, not an error to propagate: "the CLI would not answer", "the CLI
/// exited non-zero", and "neither the named directory nor any sibling had a
/// usable executable" are all just "this route did not produce a browser".
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
    if !output.status.success() {
        // A non-zero exit is "I could not ask", never "it is not there" — the
        // only evidence (stderr) is captured here instead of thrown away, so
        // this cannot collapse into the same message as "nothing installed".
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "playwright-cli install-browser chromium --dry-run exited with {}: {}",
            output.status,
            capped(stderr.trim(), STDERR_LOG_CAP)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(dir) = parse_install_location(&stdout) else {
        return Err("playwright-cli did not report an install location for chromium".to_string());
    };
    if let Some(exe) = executable_among(&files_under(&dir, WALK_MAX_DEPTH)) {
        return Ok(exe);
    }

    // The dry-run names the revision the CLI would install NEXT, which may
    // not be on disk even when an older, perfectly usable revision is.
    let asked_revision = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unparsed)");
    let (seen_revisions, sibling_exe) = scan_sibling_installs(&dir);
    if let Some(exe) = sibling_exe {
        return Ok(exe);
    }
    Err(format!(
        "no chromium executable under {} (dry-run asked for {asked_revision}; scanned sibling \
         installs {seen_revisions:?} and found none with a usable executable)",
        dir.display()
    ))
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

    /// The fifth case: a non-default request whose resolved engine is
    /// unidentifiable is still a mismatch. Reachable since the sibling scan
    /// (F1) can hand back an executable `engine_of` cannot name — an
    /// unidentified engine must not be read as "the request was honoured".
    #[test]
    fn brave_requested_but_engine_unidentified_is_a_mismatch() {
        assert!(engine_mismatch(&BrowserType::Brave, None));
    }

    /// The only one of `resolve_binary`'s three routes reachable without a
    /// real browser or a real `playwright-cli`: a pin pointing at a file that
    /// exists. `cli_binary` is a nonsense path — the pin route must win before
    /// either of the other two routes is even consulted.
    ///
    /// Uses `tempfile::tempdir()` rather than hand-rolling `env::temp_dir()`
    /// plus manual cleanup: cleanup then runs on drop, including on a panic
    /// from a failing assert, instead of leaking a directory (`src/utils/
    /// scratch.rs`'s own doc records 4,987 leaked entries / 3.8 GB from
    /// exactly this class of hand-rolled cleanup).
    #[tokio::test]
    async fn a_valid_pin_resolves_without_touching_the_other_two_routes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pinned = dir.path().join("Google Chrome for Testing");
        std::fs::write(&pinned, b"not a real binary, just needs to exist").unwrap();
        // A "valid" pin means launchable, not merely present (Final Review
        // I2) — `std::fs::write` leaves the default, non-executable mode, so
        // without this the fixture would no longer be testing what its own
        // name claims.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&pinned, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

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

    /// Final Review I2: `is_file()` establishes existence, not launchability.
    /// A pin at a non-executable file (lost `+x`, a downloaded artifact, a
    /// plain file inside a bundle) must be `ChromiumUnavailable` naming THAT
    /// reason — not "does not exist" (a different fact) and not a silent
    /// `Ok` (the bug: doctor and `runtime_manage{list}` both reported this
    /// exact shape healthy, and only the next browser call discovered
    /// `Permission denied`).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_pin_that_exists_but_is_not_executable_is_unavailable_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pinned = dir.path().join("chrome-no-exec-bit");
        std::fs::write(&pinned, b"exists, but nobody chmod +x'd it").unwrap();
        // `std::fs::write`'s default mode already has no exec bit on every
        // platform this runs on, but state the precondition rather than
        // relying on it: the whole test is dead if this ever stops holding.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&pinned).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o111,
            0,
            "fixture precondition: must start non-executable"
        );

        let runtime = BrowserRuntimeConfig {
            binary_path: Some(pinned.to_string_lossy().into_owned()),
            prefer_system_browser: true,
            download_host: None,
        };
        let err = resolve_binary(
            &runtime,
            &BrowserType::Chromium,
            Path::new("/nonexistent/playwright-cli-should-never-be-invoked"),
        )
        .await
        .expect_err("a non-executable pin must not resolve as healthy");

        match err {
            BrowserError::ChromiumUnavailable { tried } => {
                assert!(
                    tried.contains("exists but is not executable"),
                    "must distinguish this from \"does not exist\": {tried}"
                );
            }
            other => panic!("expected ChromiumUnavailable, got {other:?}"),
        }
    }

    /// The System route is the DEFAULT production path
    /// (`prefer_system_browser` defaults to `true`), and it is fully
    /// testable without a real browser: `find_chromium_preferred` starts
    /// with `ALEPH_CHROME_PATH` (`discovery::env_override`), an explicit
    /// override that always wins. Serialized because it mutates
    /// process-global environment — same discipline as
    /// `discovery::tests::test_env_override`.
    #[serial_test::serial]
    #[tokio::test]
    async fn the_system_route_resolves_via_aleph_chrome_path_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("sentinel-chrome");
        std::fs::write(&sentinel, b"stand-in binary, never executed").unwrap();

        std::env::set_var("ALEPH_CHROME_PATH", &sentinel);
        let runtime = BrowserRuntimeConfig::default();
        let result = resolve_binary(
            &runtime,
            &BrowserType::Chromium,
            Path::new("/nonexistent/playwright-cli-should-never-be-invoked"),
        )
        .await;
        std::env::remove_var("ALEPH_CHROME_PATH");

        let resolved =
            result.expect("ALEPH_CHROME_PATH override must resolve via the System route");
        assert_eq!(resolved.path, sentinel);
        assert_eq!(resolved.source, ChromiumSource::System);
    }

    /// Build a fake `chromium-<revision>` install directory under `parent`,
    /// laid out exactly like the real macOS cache (five levels deep,
    /// matching `EXECUTABLE_LEAVES[0]` — see [`WALK_MAX_DEPTH`]'s doc),
    /// optionally dropping the `INSTALLATION_COMPLETE` marker at its root.
    /// Returns the install root.
    fn fake_install(parent: &Path, revision: u64, marker: bool) -> PathBuf {
        let root = parent.join(format!("chromium-{revision}"));
        let exe_dir = root
            .join("chrome-mac-arm64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("Google Chrome for Testing"), b"fake").unwrap();
        if marker {
            std::fs::write(root.join(INSTALLATION_COMPLETE_MARKER), b"").unwrap();
        }
        root
    }

    /// Build a fake `chromium_headless_shell-<revision>` sibling — same
    /// depth, same recognised leaf name as [`fake_install`], so a scan that
    /// matched on `starts_with("chromium")` instead of the `chromium-`
    /// prefix would have a real candidate to wrongly prefer.
    fn fake_headless_shell(parent: &Path, revision: u64) -> PathBuf {
        let root = parent.join(format!("chromium_headless_shell-{revision}"));
        let exe_dir = root
            .join("chrome-mac-arm64")
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("Google Chrome for Testing"), b"fake").unwrap();
        root
    }

    /// The base fixture the ruling measured: revisions 1208 and 1228 exist
    /// (both with a usable executable, neither with the marker), a
    /// headless-shell sibling at 1228 must never be picked, and the dry-run
    /// itself named 1219, which does not exist on disk.
    #[test]
    fn the_newest_installed_sibling_wins_when_the_dry_run_directory_is_absent() {
        let cache = tempfile::tempdir().expect("tempdir");
        fake_install(cache.path(), 1208, false);
        let newest = fake_install(cache.path(), 1228, false);
        fake_headless_shell(cache.path(), 1228);

        let dry_run_location = cache.path().join("chromium-1219"); // never created
        let (seen, best) = scan_sibling_installs(&dry_run_location);

        assert_eq!(seen, vec![1208, 1228]);
        assert_eq!(
            best,
            Some(
                newest
                    .join("chrome-mac-arm64")
                    .join("Google Chrome for Testing.app")
                    .join("Contents")
                    .join("MacOS")
                    .join("Google Chrome for Testing")
            )
        );
    }

    /// The marker matters more than the revision number: a half-finished
    /// install at a HIGHER revision must not beat a finished one at a lower
    /// revision — "prefer a sibling that has BOTH the marker and an
    /// executable".
    #[test]
    fn a_finished_install_beats_a_newer_half_finished_one() {
        let cache = tempfile::tempdir().expect("tempdir");
        let finished = fake_install(cache.path(), 1208, true); // has the marker
        fake_install(cache.path(), 1228, false); // newer, but no marker

        let dry_run_location = cache.path().join("chromium-1219");
        let (_, best) = scan_sibling_installs(&dry_run_location);

        assert_eq!(
            best,
            Some(
                finished
                    .join("chrome-mac-arm64")
                    .join("Google Chrome for Testing.app")
                    .join("Contents")
                    .join("MacOS")
                    .join("Google Chrome for Testing")
            )
        );
    }

    /// When nothing on disk has a usable executable, the caller still learns
    /// WHICH revisions were seen — distinguishing "nothing installed" from
    /// "installed, but not the revision the CLI wants" depends on this.
    #[test]
    fn scan_reports_revisions_seen_even_when_none_have_a_usable_executable() {
        let cache = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(cache.path().join("chromium-1050")).unwrap(); // empty, no exe

        let dry_run_location = cache.path().join("chromium-1219");
        let (seen, best) = scan_sibling_installs(&dry_run_location);

        assert_eq!(seen, vec![1050]);
        assert_eq!(best, None);
    }

    #[test]
    fn capped_returns_the_whole_string_when_it_is_already_short() {
        assert_eq!(capped("short", 400), "short");
    }

    /// 401 lands mid-character for a string made entirely of 2-byte chars
    /// (valid boundaries are only at even byte offsets) — `capped` must back
    /// off to the last valid boundary (400) rather than slicing mid-char
    /// (which would panic) or returning something longer than requested.
    #[test]
    fn capped_cuts_at_a_char_boundary_not_mid_character() {
        let long = "é".repeat(300); // 600 bytes, boundaries only at even offsets
        let out = capped(&long, 401);
        assert_eq!(out.len(), 400);
        assert_eq!(out, "é".repeat(200));
    }

    /// A fake `playwright-cli` that exits non-zero and writes to stderr,
    /// proving the exit status is checked and the stderr text reaches the
    /// error message rather than being silently dropped. Before this fix, a
    /// non-zero exit collapsed into the same message as "nothing installed"
    /// — "I could not ask" read as "it is not there".
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_cli_reports_its_exit_status_and_stderr_not_a_generic_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("fake-playwright-cli");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'update required: run npm install -g playwright' >&2\nexit 7\n",
        )
        .expect("write fake cli");
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let err = playwright_managed(&script)
            .await
            .expect_err("a non-zero exit must not read as \"not installed\"");
        assert!(err.contains("exited with"), "missing exit status: {err}");
        assert!(
            err.contains("update required"),
            "stderr text was dropped: {err}"
        );
    }
}
