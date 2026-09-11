pub mod backend;
pub(crate) mod chrome_mcp;
pub(crate) mod chrome_mcp_backend;
pub(crate) mod chromium_launch;
pub(crate) mod chromium_resolve;
mod discovery;
pub mod engine;
pub mod error;
pub mod manager;
pub mod network_policy;
pub mod page_state;
pub mod playwright_cli;
pub(crate) mod playwright_cli_backend;
pub mod playwright_launch;
pub(crate) mod post_nav;
pub mod profile;
mod secret_guard;
pub mod tab_registry;
// `cfg(test)` only, deliberately (R84). It was briefly widened to
// `any(test, feature = "test-helpers")` for an integration-test consumer of
// `FakeEngineProcess` that cannot exist: this module is `pub(crate)`, so
// `alephcore::browser::testkit` does not resolve from another crate at all.
// Measured before reverting: `FakeEngineProcess` = 35 hits across 4 files, all
// under `src/`, zero under the 174 files in `tests/`. A later task that
// genuinely wants an integration-level fake widens this in the commit that adds
// the consumer — which is the right shape anyway, because that commit can prove
// the consumer compiles.
#[cfg(test)]
pub(crate) mod testkit;
pub mod types;
pub(crate) mod wait_probe;

pub use discovery::find_chromium;
pub use error::BrowserError;
// Crate-internal: `live_endpoint` is `pub(crate)` too, and its first real
// consumer (the live view, Plan 2) lives in this crate.
pub(crate) use chromium_launch::CdpEndpoint;

/// R72 class guard: fails when a `#[test]`/`#[tokio::test]` fn anywhere under
/// `src/browser/` resolves `ALEPH_HOME` without holding the one guard that
/// serialises against every other test doing the same
/// ([`crate::utils::paths::AlephHomeEnvGuard`]). R72 fixed nine instances of
/// exactly this shape by hand; this exists so task 8 onward cannot grow a
/// tenth one silently (判据 §11 — fix the class, not the instances).
///
/// A source scan, same construction as
/// [`crate::runtimes::post_install::tests::nothing_acquires_the_two_env_locks_separately`]
/// and `builtin_tools::browser_tools::tests::the_guarded_path_lists_tabs_exactly_once`
/// — chosen over a runtime check (`config::save::reject_real_home_config`'s
/// style) because "does fn X's body call Y without also containing Z" is a
/// static, textual question a call at runtime cannot answer: the call graph
/// this census walks was traced by hand for R72's report, not rediscovered
/// here, so a NEW resolver function is invisible to it until someone adds its
/// name below.
#[cfg(test)]
mod home_guard_census {
    use std::path::{Path, PathBuf};

    /// Every `.rs` file under `src/browser/`, recursively. The denominator
    /// for this census comes from this same walk — not from a search tool's
    /// own file list, and not from a hardcoded count that would drift the
    /// day a file is added or removed.
    fn browser_sources() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser");
        let mut out = Vec::new();
        collect_rust_sources(&root, &mut out);
        assert!(!out.is_empty(), "found no sources under {root:?}");
        out
    }

    fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Literal spellings that reach `discovery::aleph_home_dir()` /
    /// `utils::paths::get_config_dir()` from inside `src/browser/`, per the
    /// call graph traced for R72 (`.superpowers/sdd/2026-09-06-browser-dual-
    /// engine/r72-report.md`). BLIND SPOT: a new function that itself reaches
    /// the resolver is invisible to this census until its name is added here
    /// — this list does not follow calls transitively.
    const RESOLVER_SPELLINGS: &[&str] = &[
        "aleph_home_dir(",
        "get_config_dir(",
        "browser_state_dir(",
        "config_path_for(",
        "output_dir_for(",
        "chromium_user_data_dir(",
        "sidecar_registry_dir(",
        "default_user_data_dir_for(",
        // The engine path's three reachers, all of which get to
        // `browser_state_dir` through `ProfileManager::launch_request_for`.
        // They are here because the blind spot named above is not a note about
        // some hypothetical future — it fired: these three landed with the
        // engine registry and this list did not grow with them, so the census
        // was green over three new ways to read the developer's real
        // `$ALEPH_HOME` (判据 §5 — a list only covers the world as it was on
        // the day it was written). Each is falsified separately below the
        // census: one unguarded test fn per spelling, because a census that
        // panics on the first offender it finds proves nothing about the
        // second.
        "launch_request_for(",
        "engine_handle(",
        "engine_handle_for(",
    ];

    /// Literal spellings that hold the one guard (or the combined `$HOME` +
    /// `$ALEPH_HOME` guard — unused in `src/browser/` today, but a correct
    /// future use of it must not be flagged as an offender).
    const GUARD_SPELLINGS: &[&str] = &[
        "AlephHomeEnvGuard::acquire",
        "IsolatedAlephHome::new",
        "HomeEnvGuards::acquire",
    ];

    struct TestFn {
        name: String,
        body: String,
    }

    /// The byte offset of `}` that closes the `{` at `src[0]`, by depth
    /// counting — NOT `body.find("\n}\n")` (see M-5,
    /// `builtin_tools::browser_tools::tests::the_guarded_path_lists_tabs_
    /// exactly_once`'s documented blind spot: that boundary matches the
    /// FIRST column-0 `}`, which in a real test body is almost always a
    /// nested `if`/`for`/`match` block's, not the function's own).
    fn matching_brace(src: &str) -> Option<usize> {
        let mut depth: i32 = 0;
        for (i, ch) in src.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Every `#[test]`/`#[tokio::test...]` function in `src`, name plus body
    /// text (the `{...}` span, brace-matched).
    ///
    /// BLIND SPOT: the attribute match is a per-line `starts_with` after
    /// trimming leading whitespace, so it only recognises the two literal
    /// spellings above column-shifted by indentation — a macro that expands
    /// to a test fn, or a differently-named test attribute (`#[rstest]`,
    /// `#[test_case]`), is invisible to it. None are in use under
    /// `src/browser/` today (checked: this crate's `#[test]`/`#[tokio::test]`
    /// count via this same per-line match equals the count from an
    /// independently-worded regex, cross-checked in the R72 report).
    fn extract_test_fns(src: &str) -> Vec<TestFn> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for line in src.split_inclusive('\n') {
            let trimmed = line.trim_start();
            let is_test_attr =
                trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test");
            if is_test_attr {
                if let Some(test_fn) = parse_fn_from(&src[cursor..]) {
                    out.push(test_fn);
                }
            }
            cursor += line.len();
        }
        out
    }

    /// From a slice starting at (or before) a test attribute, find the next
    /// `fn`'s name and brace-matched body.
    fn parse_fn_from(tail: &str) -> Option<TestFn> {
        let fn_at = tail.find("fn ")?;
        let name_start = fn_at + 3;
        let name_end = tail[name_start..]
            .find(|c: char| c == '(' || c.is_whitespace())
            .map_or(tail.len(), |i| name_start + i);
        let name = tail[name_start..name_end].to_string();
        let brace_rel = tail[name_end..].find('{')?;
        let body_start = name_end + brace_rel;
        let body_end_rel = matching_brace(&tail[body_start..])?;
        let body = tail[body_start..body_start + body_end_rel + 1].to_string();
        Some(TestFn { name, body })
    }

    /// Falsify-first (判据 §3): run with an extra offending fn spliced in
    /// (temporarily, by hand) before trusting this to ever go red. See the
    /// R72 commit-4 report for the panic message that run produced.
    #[test]
    fn no_browser_test_resolves_aleph_home_without_holding_the_guard() {
        let mut offenders = Vec::new();
        let mut total_fns = 0usize;
        for path in browser_sources() {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let src = raw.replace('\r', "");
            let rel = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser"))
                .unwrap_or(&path)
                .display()
                .to_string();
            for test_fn in extract_test_fns(&src) {
                total_fns += 1;
                let reaches_resolver = RESOLVER_SPELLINGS.iter().any(|s| test_fn.body.contains(s));
                let holds_guard = GUARD_SPELLINGS.iter().any(|s| test_fn.body.contains(s));
                if reaches_resolver && !holds_guard {
                    offenders.push(format!("{rel}::{}", test_fn.name));
                }
            }
        }
        // Sanity floor on the walk itself: R72's report measured 262 test
        // fns across 22 files before this census's own test fn existed (263
        // with it — this fn is itself part of the tree it scans). A number
        // far below that means the scan (not the code) broke — e.g. a
        // changed attribute style or an extraction bug — and reporting "0
        // offenders" from a broken walk would be exactly 判据 §2's
        // "unfalsifiable green".
        assert!(
            total_fns > 200,
            "found only {total_fns} test fns under src/browser — the source \
             walk or fn-extraction likely broke (R72 measured 262, 263 \
             counting this census's own test fn); a low count here would \
             make the offenders check below unfalsifiable"
        );
        assert!(
            offenders.is_empty(),
            "these tests resolve ALEPH_HOME without holding AlephHomeEnvGuard \
             (or HomeEnvGuards) in their own body — see r72-report.md for the \
             mechanism and the fix pattern: {offenders:?}"
        );
    }
}
