//! What it takes to hand one `playwright-cli` session a browser Aleph launched.
//!
//! The managed driver used to launch the browser through `playwright-cli open`.
//! It no longer does: Aleph spawns Chromium itself (`chromium_launch`) and the
//! CLI joins it with `attach --cdp <http-url>`. Two measurements forced the
//! change and one forbids going back:
//!
//! 1. **A CLI-launched Chrome's debug port is not a contract.** It is random
//!    per launch, a caller's own `--remote-debugging-port` loses to
//!    Playwright's (Chrome takes the last duplicate), no `DevToolsActivePort`
//!    file is written, and `playwright-cli list` prints no endpoint.
//! 2. **`close` under `cdpEndpoint` only disconnects** — nine Chrome processes
//!    before and after, endpoint still serving, page state intact. So the
//!    browser's lifetime is Aleph's to manage, which is the whole point.
//! 3. **`open` clobbers the page it reuses**, issuing `goto('about:blank')` on
//!    it; `attach` does not. Never emit `open` against a handed-over browser.
//!
//! `--config` is accepted by `attach` (verified against playwright-cli 0.1.8's
//! own `--help`), so the containment this module set up on the `open` path —
//! `outputDir` plus `allowUnrestrictedFileAccess` — survives unchanged. Every
//! other key it used to write moved onto the Chrome argv Aleph now builds.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::profile::{BrowserType, ProfileConfig};

/// Everything the CLI needs at launch time for one session.
///
/// Built from a [`ProfileConfig`] by [`Self::from_profile`] and carried by the
/// backend, because the driver is shared across sessions and does not know
/// which profile a session key belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLaunch {
    pub headless: bool,
    pub browser: BrowserType,
    pub user_data_dir: Option<String>,
    pub proxy: Option<String>,
    pub extra_args: Vec<String>,
}

impl SessionLaunch {
    /// `headless` is resolved by the caller because it merges a profile-level
    /// `Option<bool>` with the global default; the rest come straight off the
    /// profile.
    #[must_use]
    pub fn from_profile(cfg: &ProfileConfig, headless: bool) -> Self {
        Self {
            headless,
            browser: cfg.browser.clone(),
            user_data_dir: cfg.user_data_dir.clone(),
            proxy: cfg.proxy.clone(),
            extra_args: cfg.extra_args.clone(),
        }
    }

    /// A launch that configures nothing — the shape a default profile has.
    /// Note this still gets an explicit config file written for it; see
    /// [`config_path_for`].
    #[must_use]
    pub fn headless_default() -> Self {
        Self {
            headless: true,
            browser: BrowserType::default(),
            user_data_dir: None,
            proxy: None,
            extra_args: Vec::new(),
        }
    }
}

/// Whether a call is allowed to bring a browser into existence.
///
/// The lazy launch made every subcommand a potential browser launcher, which
/// is wrong for anything that is *observing* rather than driving: the idle-tab
/// reaper listing the tabs of a session with no browser must report "nothing
/// there", not create the thing it was sweeping. (It did — a unit-test sweep
/// launched a real Chrome.) A sensor must not create what it measures, so the
/// permission is stated per call instead of being a property of the driver.
///
/// This also matches the contract the tool layer already had: every verb other
/// than `browser_open` answers a browserless session with "No tabs open. Use
/// browser_open first."
#[derive(Debug, Clone, Copy)]
pub enum LaunchPolicy<'a> {
    /// The caller is asking for a browser: launch one if the CLI reports the
    /// session is not open.
    OpenIfNeeded(&'a SessionLaunch),
    /// The caller is observing an existing session; a missing browser is an
    /// answer, not something to fix.
    Refuse,
}

impl<'a> LaunchPolicy<'a> {
    /// The launch to use, or `None` when this call may not open anything.
    #[must_use]
    pub const fn launch(self) -> Option<&'a SessionLaunch> {
        match self {
            Self::OpenIfNeeded(l) => Some(l),
            Self::Refuse => None,
        }
    }
}

/// The `--config` JSON for a launch, following the documented
/// `.playwright/cli.config.json` schema.
///
/// `outputDir` is not a setting anyone asked for; it is a containment. The CLI
/// writes page snapshots and console logs to `.playwright-cli/` **relative to
/// the process cwd**, and the driver never sets a cwd, so the browser wrote
/// page content into whatever directory the server happened to be started in —
/// observed in practice as a full accessibility tree of a visited site landing
/// in a git checkout. Naming a directory under `~/.aleph` keeps browsed page
/// content inside Aleph's own storage.
///
/// `allowUnrestrictedFileAccess` turns off a *second*, weaker answer to a
/// question Aleph already answers. Naming `outputDir` has a side effect the
/// setting's own purpose does not advertise: the CLI then refuses any
/// explicitly-supplied path outside `outputDir ∪ process-cwd`
/// (`checkFile` in playwright-core), **and reports the refusal with exit 0**.
/// Every artifact verb was silently affected — `browser_pdf` and
/// `browser_session(save)` answered success over a file that was never written,
/// `browser_screenshot` failed reading a file the CLI had declined to create,
/// and `browser_upload` could attach nothing outside those two directories.
///
/// The containment that was actually wanted is unaffected: page snapshots and
/// console logs still land in `outputDir`, which is the whole reason it is set.
/// What the flag disables applies only to paths a caller names, and there are
/// exactly four such paths in the backend — two of them Aleph's own, two of them
/// already gated:
///
/// * `screenshot --filename` — a temp path this crate chooses;
/// * `state-save` / `state-load` — resolved under Aleph's browser state dir from
///   a name that is validated to contain no separators;
/// * `pdf --filename` — model-supplied, and `browser_tools::pdf` runs the file
///   layer's protected-location deny check first (test:
///   `test_pdf_refuses_a_protected_output_path`);
/// * `upload <files…>` — model-supplied, same check in `browser_tools::upload`
///   (test: `test_upload_refuses_a_protected_path_before_the_gate`).
///
/// A fifth path-taking verb has to answer the same question before it is added.
///
/// Aleph's guard is strictly better informed than the CLI's: `outputDir ∪ cwd`
/// would happily admit the entire directory the server was started from (a git
/// checkout, in practice) while refusing `/tmp`, which is not a boundary anyone
/// chose.
///
/// The object is emitted even when it configures nothing, because passing
/// `--config` unconditionally is what stops the CLI from reading an ambient
/// one; see [`config_path_for`].
#[must_use]
pub fn launch_config_json(output_dir: &Path) -> Value {
    json!({
        "outputDir": output_dir.to_string_lossy(),
        "allowUnrestrictedFileAccess": true,
    })
}

/// Where this session's page snapshots and console logs are written.
///
/// See [`launch_config_json`] for why this is set at all.
pub fn output_dir_for(session_key: &str) -> Result<PathBuf, super::error::BrowserError> {
    Ok(browser_state_dir("cli-output")?.join(sanitize_session_key(session_key)))
}

/// The `attach` argv (after the `-s=<session>` flag the driver always prepends).
///
/// `--cdp` takes the http form of the endpoint. Both `http://…` and
/// `ws://…/devtools/browser/<id>` were accepted in the spike; the http one is
/// chosen because it does not embed a browser id that changes on every launch,
/// so a retry does not have to re-read the port file to rebuild the URL.
///
/// No `--headed` and no `--browser`: neither is an option of `attach`
/// (verified against the CLI's own `--help`), and headedness and engine choice
/// are now properties of the Chrome argv Aleph builds — see
/// [`super::chromium_launch::ChromiumLaunchSpec::argv`] and
/// [`super::chromium_resolve::resolve_binary`].
#[must_use]
pub(crate) fn attach_argv(
    endpoint: &super::chromium_launch::CdpEndpoint,
    config_path: &Path,
) -> Vec<String> {
    vec![
        "attach".to_string(),
        "--cdp".to_string(),
        endpoint.http_url.clone(),
        "--config".to_string(),
        config_path.to_string_lossy().into_owned(),
    ]
}

/// Where this session's generated `--config` file lives.
///
/// Passing `--config` on every launch is a security property, not a
/// convenience: `playwright-cli` otherwise auto-loads
/// `.playwright/cli.config.json` **relative to the process cwd**, and the
/// driver never sets a cwd, so the child inherits the server's. That schema
/// carries `initScript` (JavaScript evaluated in every page before the page's
/// own scripts), `cdpEndpoint`, and `userDataDir` — i.e. a directory Aleph
/// happens to be running in could silently redirect or instrument the agent's
/// browser. An explicit `--config` fully replaces the ambient file (measured:
/// the ambient `userDataDir` is not merged, not even as a fallback), so the
/// file is written even when it configures nothing.
pub fn config_path_for(session_key: &str) -> Result<PathBuf, super::error::BrowserError> {
    Ok(
        browser_state_dir("cli-config")?
            .join(format!("{}.json", sanitize_session_key(session_key))),
    )
}

/// `~/.aleph/data/browser/<leaf>`, resolved through the one home-dir helper.
pub(super) fn browser_state_dir(leaf: &str) -> Result<PathBuf, super::error::BrowserError> {
    Ok(crate::discovery::aleph_home_dir()
        .map_err(|e| {
            super::error::BrowserError::PlaywrightCliError(format!(
                "cannot resolve aleph home for the browser launch config: {e}"
            ))
        })?
        .join("data")
        .join("browser")
        .join(leaf))
}

/// A session key reduced to one safe path component.
///
/// The key is a profile name, which the config layer constrains; sanitize
/// anyway so a name can never escape the directory.
///
/// `pub(super)` so the sibling `chrome_mcp` module can reuse it when it
/// builds its per-profile user-data-dir; without this both paths would have
/// to keep the same shape in sync to honour the BROWSER-R4-05 isolation
/// guarantee.
pub(super) fn sanitize_session_key(session_key: &str) -> String {
    let safe: String = session_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.starts_with('.') {
        format!("p_{safe}")
    } else {
        safe
    }
}

/// Materialize this session's `--config` file and return its path.
///
/// Rewritten on every attach rather than written once: `outputDir` is derived
/// from the session key and the home dir, both of which a restart can move, and
/// the file is only read at attach time — so the cheapest correct thing is to
/// make it a pure function of the current session.
pub async fn write_launch_config(session_key: &str) -> Result<PathBuf, super::error::BrowserError> {
    let path = config_path_for(session_key)?;
    let output_dir = output_dir_for(session_key)?;
    tokio::fs::create_dir_all(&output_dir).await.map_err(|e| {
        super::error::BrowserError::PlaywrightCliError(format!(
            "cannot create the browser output dir {}: {e}",
            output_dir.display()
        ))
    })?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| {
            super::error::BrowserError::PlaywrightCliError(format!(
                "cannot create the browser launch-config dir {}: {e}",
                dir.display()
            ))
        })?;
    }
    let body = launch_config_json(&output_dir).to_string();
    tokio::fs::write(&path, body).await.map_err(|e| {
        super::error::BrowserError::PlaywrightCliError(format!(
            "cannot write the browser launch config {}: {e}",
            path.display()
        ))
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config file's key set, exactly. Everything that used to live under
    /// `browser` moved onto the Chrome argv Aleph now builds itself
    /// (`chromium_launch::ChromiumLaunchSpec::argv`), so a `userDataDir` or
    /// `launchOptions` surviving here would be a SECOND answer to where the
    /// profile directory and the proxy come from — and the CLI's copy would be
    /// the one nothing honours, because it no longer launches anything.
    #[test]
    fn the_attach_config_carries_exactly_the_two_keys_the_cli_still_owns() {
        let json = launch_config_json(Path::new("/tmp/out"));
        assert_eq!(
            json,
            json!({
                "outputDir": "/tmp/out",
                // Not decoration: naming `outputDir` without this makes the CLI
                // refuse every caller-supplied path outside it — see the fn doc.
                "allowUnrestrictedFileAccess": true,
            })
        );
        let obj = json.as_object().expect("object");
        for gone in ["browser", "userDataDir", "launchOptions", "cdpEndpoint"] {
            assert!(
                !obj.contains_key(gone),
                "{gone} must not be in the attach config"
            );
        }
    }

    /// `--config` rides on the attach for the same reason it rode on the open:
    /// passing it is what DISPLACES the ambient `.playwright/cli.config.json`
    /// the CLI would otherwise load from the process cwd — a file that can
    /// carry `initScript` and `cdpEndpoint`, i.e. a directory Aleph happens to
    /// run in could instrument or redirect the agent's browser.
    #[test]
    fn attach_argv_names_the_endpoint_and_always_carries_an_explicit_config() {
        let endpoint = crate::browser::chromium_launch::CdpEndpoint {
            http_url: "http://127.0.0.1:58363".into(),
            ws_url: "ws://127.0.0.1:58363/devtools/browser/abc".into(),
            pid: 4242,
        };
        let argv = attach_argv(&endpoint, Path::new("/tmp/c.json"));
        assert_eq!(
            argv,
            vec![
                "attach",
                "--cdp",
                "http://127.0.0.1:58363",
                "--config",
                "/tmp/c.json"
            ]
        );
    }

    /// The OTHER caller of `launch_config_json`, which the arity change would
    /// otherwise leave uncompilable. Its containment assertions are about
    /// `output_dir_for` and are unaffected — only the call loses an argument.
    #[test]
    fn browsed_page_content_is_contained_under_aleph_home() {
        let out = output_dir_for("default").expect("home resolves");
        let json = launch_config_json(&out);
        assert_eq!(json["outputDir"], json!(out.to_string_lossy()));
        assert!(
            out.to_string_lossy().contains("browser"),
            "expected a path under the browser state dir, got {}",
            out.display()
        );
        // Same containment property as the config file: one component, under
        // the state dir, whatever the session key looks like.
        let dir = out.parent().expect("has a parent").to_path_buf();
        for hostile in ["../../etc", "/etc", "..", "", "a/b"] {
            let p = output_dir_for(hostile).expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
        }
    }

    /// `open` is destructive to a browser handed over this way: it issues
    /// `page.goto('about:blank')` on the page it reuses, silently clobbering
    /// whatever was displayed (measured, spike STEP 3). `attach` left the page
    /// untouched. Nothing in this module may emit the verb again.
    #[test]
    fn the_launch_verb_is_attach_and_never_open() {
        let endpoint = crate::browser::chromium_launch::CdpEndpoint {
            http_url: "http://127.0.0.1:1".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
            pid: 1,
        };
        let argv = attach_argv(&endpoint, Path::new("/tmp/c.json"));
        assert_eq!(argv.first().map(String::as_str), Some("attach"));
        assert!(!argv.iter().any(|a| a == "open"));
        // Neither flag belongs to `attach`; passing one is `Unknown option`,
        // exit 1 — a hard failure on every call, which is how `--headed` broke
        // `tab-new` before.
        assert!(!argv.iter().any(|a| a == "--headed" || a == "--browser"));
    }

    #[test]
    fn config_path_cannot_escape_its_directory() {
        let dir = config_path_for("default")
            .expect("home resolves")
            .parent()
            .expect("has a parent")
            .to_path_buf();
        for hostile in ["../../etc/passwd", "/etc/passwd", "..", "", "a/b"] {
            let p = config_path_for(hostile).expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
            assert_eq!(
                p.components().count(),
                dir.components().count() + 1,
                "not a single component for {hostile:?}"
            );
        }
    }
}
