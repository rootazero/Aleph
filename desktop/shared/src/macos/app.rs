//! Application lifecycle on macOS — the single source of truth.
//!
//! There were two of these. `action::app_launch` backed the `desktop` tool's
//! `launch_app` / `quit_app` / `restart_app`, and `desktop-macos`'s
//! `system::workspace` backed the `system` tool's identically named actions.
//! They did not agree on the one question that matters at the call site:
//!
//! | | `action::app_launch` (old) | `system::workspace` (old) |
//! |---|---|---|
//! | launch accepts | name **or** bundle id | name **or** bundle id |
//! | quit accepts | bundle id **only** | name **or** bundle id |
//! | quit verifies | no — `terminate()`'s answer discarded | partly — checked the return |
//!
//! So `desktop(action="launch_app", bundle_id="Calculator")` worked and
//! `desktop(action="quit_app", bundle_id="Calculator")` did not, while the
//! `system` tool did both. A model that launched an app by name could not close
//! it again through the same tool, and the error it got back — "no running
//! application found with identifier 'Calculator'" — pointed at the app rather
//! than at the spelling.
//!
//! This module answers it once. It lives in `aleph-desktop` rather than in the
//! macOS limb because the dependency runs `desktop-macos → desktop-shared`: the
//! truth has to sit on the side that is depended upon, which is the same reason
//! the Linux session/clipboard/launcher logic lives in [`crate::linux`].

use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

use crate::error::{DesktopError, Result};
use crate::system_types::InstalledApp;

/// How long [`quit`] waits for a terminate request to actually take effect.
///
/// `NSRunningApplication.terminate` is a *request*: it returns `true` once the
/// Apple Event is delivered, not once the app is gone, and an app with unsaved
/// changes answers it by putting up a save sheet and staying exactly where it
/// was. Reporting success at that point tells the model the app is closed while
/// the user is looking at a modal dialog.
///
/// Three seconds because the two ways to be wrong here are not symmetric. Too
/// short turns a large app that is merely slow to flush state into a reported
/// failure, and the model then retries or works around something that was
/// already happening. Too long only costs the turn a few seconds. Ordinary apps
/// are gone well inside half a second; this is sized for the slow tail, not for
/// the median.
const TERMINATE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Poll interval while waiting for termination.
const TERMINATE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Resolve `query` to a launchable application URL.
///
/// A query containing a dot is treated as a bundle identifier first, because
/// that is what a dot means here in practice (`com.apple.Safari`), and falls
/// back to a name lookup so that an app whose *name* contains a dot
/// (`Google Chrome Beta.app`, `IINA.app`) is still reachable. The old code only
/// tried one branch or the other, so a dotted name resolved to nothing.
fn application_url(query: &str) -> Option<objc2::rc::Retained<NSURL>> {
    let ws = NSWorkspace::sharedWorkspace();
    let ns_query = NSString::from_str(query);

    #[allow(deprecated)] // `fullPathForApplication` is the only name-based lookup.
    let by_name = || {
        ws.fullPathForApplication(&ns_query)
            .map(|path| NSURL::fileURLWithPath(&path))
    };

    if query.contains('.') {
        if let Some(url) = ws.URLForApplicationWithBundleIdentifier(&ns_query) {
            return Some(url);
        }
    }
    by_name()
}

/// Whether `app` is the one `query` names.
///
/// Matches the bundle identifier or the localized name, case-insensitively —
/// the two handles a caller can actually have. Split out as its own function
/// because both `quit` and the running-app lookup have to agree on it, and
/// because "which spellings of an app name are accepted" is the thing the two
/// old implementations disagreed about.
///
/// Normalises `query` itself rather than trusting the caller to have done it:
/// the version that took a pre-lowercased argument silently answered `false` for
/// a caller that forgot, which is the failure mode this function exists to end.
fn app_matches(app: &NSRunningApplication, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    app.bundleIdentifier()
        .is_some_and(|b| b.to_string().to_lowercase() == query)
        || app
            .localizedName()
            .is_some_and(|n| n.to_string().to_lowercase() == query)
}

/// Launch an application by name or bundle identifier.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] when the application cannot be found or the
/// open request is refused.
pub fn launch(query: &str) -> Result<()> {
    let url = application_url(query).ok_or_else(|| {
        DesktopError::InputFailed(format!(
            "launch_app: no application matches '{query}'. Pass its name as shown in Finder \
             (\"Safari\") or its bundle id (\"com.apple.Safari\")."
        ))
    })?;

    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        return Err(DesktopError::InputFailed(format!(
            "launch_app: macOS refused to open '{query}'"
        )));
    }
    tracing::info!(app = query, "App launched (macOS)");
    Ok(())
}

/// Ask an application to quit, by name or bundle identifier, and confirm it did.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] when nothing matches `query`, when macOS
/// refuses the request, or when the application is still running afterwards —
/// which is the honest report for the save-sheet case, and the one that lets a
/// model decide whether to deal with the dialog or leave the app alone.
pub fn quit(query: &str) -> Result<()> {
    let ws = NSWorkspace::sharedWorkspace();
    let matches: Vec<objc2::rc::Retained<NSRunningApplication>> = ws
        .runningApplications()
        .iter()
        .filter(|app| app_matches(app, query))
        .collect();

    if matches.is_empty() {
        return Err(DesktopError::InputFailed(format!(
            "quit_app: no running application matches '{query}'. Use the `system` tool's \
             list_running_apps to see what is running, by name and bundle id."
        )));
    }

    // Every match, not just the first: an app can legitimately have more than
    // one instance, and quitting one of them while reporting the app closed is
    // the same lie in a smaller size.
    for app in &matches {
        if !app.terminate() {
            return Err(DesktopError::InputFailed(format!(
                "quit_app: macOS refused the terminate request for '{query}'"
            )));
        }
    }

    // Verify rather than assume. `terminate` returning true only means the
    // request was accepted.
    let deadline = std::time::Instant::now() + TERMINATE_WAIT;
    loop {
        if matches.iter().all(|app| app.isTerminated()) {
            tracing::info!(app = query, count = matches.len(), "App quit (macOS)");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(TERMINATE_POLL);
    }

    Err(DesktopError::InputFailed(format!(
        "quit_app: '{query}' accepted the quit request but is still running after {} ms. The \
         usual cause is a save or confirmation dialog waiting for an answer — take a screenshot \
         to see it. Nothing was forced.",
        TERMINATE_WAIT.as_millis()
    )))
}

// ── Installed-app catalogue ──────────────────────────────────────────────────

/// Directories macOS installs application bundles into.
///
/// `~/Applications` is appended at runtime. Deliberately a fixed list rather
/// than a Spotlight query: Spotlight can be disabled, excluded per-volume, or
/// still indexing, and an empty catalogue on such a machine would read as "you
/// have no apps" — a confident wrong answer where a directory listing gives a
/// plain right one.
const APP_DIRECTORIES: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];

/// How deep below each root a `.app` is still found.
///
/// One level covers the way people actually organise `/Applications` (an
/// "Adobe" or "Microsoft" folder holding the bundles). Deeper would start
/// walking *into* application bundles, which contain helper `.app`s of their own
/// that are not separately launchable.
const APP_SCAN_DEPTH: usize = 1;

/// Every application bundle installed on this machine, sorted by name.
///
/// Deduplicated by bundle identifier, first sighting winning, so a user copy in
/// `/Applications` shadows a system one rather than appearing twice.
///
/// # Errors
///
/// Never fails as a whole: a directory that cannot be read is skipped, because
/// a permission-denied on one folder is not a reason to withhold the rest.
pub fn list_installed() -> Result<Vec<InstalledApp>> {
    let mut roots: Vec<std::path::PathBuf> = APP_DIRECTORIES
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }

    let mut seen_bundles = std::collections::HashSet::new();
    let mut seen_paths = std::collections::HashSet::new();
    let mut out: Vec<InstalledApp> = Vec::new();

    for root in &roots {
        collect_apps(
            root,
            APP_SCAN_DEPTH,
            &mut out,
            &mut seen_bundles,
            &mut seen_paths,
        );
    }

    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

fn collect_apps(
    dir: &std::path::Path,
    depth: usize,
    out: &mut Vec<InstalledApp>,
    seen_bundles: &mut std::collections::HashSet<String>,
    seen_paths: &mut std::collections::HashSet<std::path::PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "app") {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let bundle_id = bundle_identifier(&path).unwrap_or_default();
            // A bundle with no identifier cannot collide with anything, so it is
            // never deduplicated away — only identified ones are.
            if !bundle_id.is_empty() && !seen_bundles.insert(bundle_id.clone()) {
                continue;
            }
            out.push(InstalledApp {
                name,
                bundle_id,
                path: path.to_string_lossy().into_owned(),
            });
        } else if depth > 0 && path.is_dir() {
            collect_apps(&path, depth - 1, out, seen_bundles, seen_paths);
        }
    }
}

/// Read a bundle's `CFBundleIdentifier`, or `None` when it declares none.
fn bundle_identifier(path: &std::path::Path) -> Option<String> {
    use objc2_foundation::{NSBundle, NSString, NSURL};

    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let bundle = NSBundle::bundleWithURL(&url)?;
    bundle.bundleIdentifier().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finder is always running on a macOS desktop and is addressable by both
    /// spellings — which is the property that used to hold for one tool and not
    /// the other.
    #[test]
    fn a_running_app_matches_by_name_and_by_bundle_id() {
        let ws = NSWorkspace::sharedWorkspace();
        let apps = ws.runningApplications();
        let finder = apps
            .iter()
            .find(|a| {
                a.bundleIdentifier()
                    .is_some_and(|b| b.to_string() == "com.apple.finder")
            })
            .expect("Finder is running on any macOS desktop session");

        assert!(app_matches(&finder, "com.apple.finder"));
        assert!(app_matches(&finder, "com.apple.FINDER"));
        let name = finder
            .localizedName()
            .expect("Finder has a localized name")
            .to_string()
            .to_lowercase();
        assert!(app_matches(&finder, &name));
    }

    #[test]
    fn an_unrelated_query_matches_nothing() {
        let ws = NSWorkspace::sharedWorkspace();
        let apps = ws.runningApplications();
        assert!(!apps
            .iter()
            .any(|a| app_matches(&a, "aleph-definitely-not-an-app")));
    }

    /// A bundle id resolves to an application URL; so does the plain name. Both
    /// spellings reaching `launch` is the whole point of the merge.
    #[test]
    fn both_spellings_resolve_to_an_application_url() {
        assert!(application_url("com.apple.finder").is_some());
        assert!(application_url("Finder").is_some());
    }

    #[test]
    fn quitting_something_that_is_not_running_names_the_way_out() {
        let err = quit("aleph-definitely-not-an-app").expect_err("must not succeed");
        let msg = err.to_string();
        assert!(msg.contains("list_running_apps"), "{msg}");
    }

    /// The catalogue has to contain something every Mac has, or it is not
    /// answering the question it exists for.
    #[test]
    fn the_installed_catalogue_finds_system_apps() {
        let apps = list_installed().unwrap();
        assert!(!apps.is_empty(), "no applications found at all");
        assert!(
            apps.iter().any(|a| a.bundle_id == "com.apple.Safari"),
            "Safari ships with macOS and must be in the catalogue"
        );
    }

    /// Every entry must be launchable by what it reports, which is the whole
    /// point: the catalogue exists so `launch_app` has something to be given.
    #[test]
    fn every_listed_app_resolves_back_to_a_launchable_url() {
        let apps = list_installed().unwrap();
        // A handful is enough to prove the shape; resolving all ~200 is a
        // LaunchServices round trip each.
        for app in apps.iter().filter(|a| !a.bundle_id.is_empty()).take(10) {
            assert!(
                application_url(&app.bundle_id).is_some(),
                "listed bundle id '{}' does not resolve",
                app.bundle_id
            );
        }
    }

    /// One bundle id, one entry — a system app shadowed by a user copy would
    /// otherwise show up twice and make `launch_app` look ambiguous.
    #[test]
    fn the_catalogue_has_no_duplicate_bundle_ids() {
        let apps = list_installed().unwrap();
        let mut seen = std::collections::HashSet::new();
        for app in apps.iter().filter(|a| !a.bundle_id.is_empty()) {
            assert!(
                seen.insert(app.bundle_id.clone()),
                "duplicate bundle id: {}",
                app.bundle_id
            );
        }
    }
}
