//! What it takes to launch one `playwright-cli` browser session.
//!
//! The managed driver never issued `open`. Every other subcommand refuses to
//! run without it — `playwright-cli 0.1.8` answers `tab-new` and `goto` alike
//! with *"The browser 'X' is not open, please run open first"* — so the whole
//! managed driver, which is the DEFAULT driver, could never reach a browser.
//! This module owns the launch that was missing, and with it the only place
//! the CLI accepts per-session configuration.
//!
//! Two properties of `open` shape everything here, both measured against the
//! real CLI rather than assumed:
//!
//! 1. **`open` is destructive when repeated.** A second `open` on a live
//!    session relaunches the browser under a new pid and drops every tab. So
//!    the launch must happen exactly once per session, and only when the CLI
//!    itself says the session is not open — never on a guess. That is why
//!    [`super::playwright_cli::PlaywrightCliDriver::run`] opens lazily off the
//!    CLI's own refusal instead of eagerly at backend construction.
//! 2. **`--config` is accepted only by `open`/`attach`,** not as a global
//!    option (`tab-list --config` is rejected outright). Anything configurable
//!    therefore has to ride on this one call.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

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

/// The `--browser` value for a [`BrowserType`], or `None` when the CLI has no
/// way to ask for it.
///
/// `playwright-cli open --browser` accepts `chrome | firefox | webkit | msedge`
/// (verbatim from `--help`). `Chromium` is Playwright's own default, so it is
/// expressed by *omitting* the flag rather than by naming it — passing a value
/// the CLI does not list would be a hard `Unknown option` failure.
///
/// `Brave` has no channel in Playwright and therefore no honest mapping; it
/// stays in [`super::manager::unhonored_managed_fields`] so the operator is
/// told at boot instead of silently getting Chromium.
#[must_use]
pub fn browser_flag_value(browser: &BrowserType) -> Option<&'static str> {
    match browser {
        BrowserType::Chromium | BrowserType::Brave => None,
        BrowserType::Chrome => Some("chrome"),
        BrowserType::Edge => Some("msedge"),
    }
}

/// The `--config` JSON for a launch, following the documented
/// `.playwright/cli.config.json` schema.
///
/// `proxy` and `extra_args` land in `browser.launchOptions`, which the schema
/// defines as Playwright's `LaunchOptions` — that type carries both `proxy`
/// and `args`. This is the surface an earlier round looked for among the CLI
/// *flags* and, not finding it there, recorded as "no equivalent exists".
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
pub fn launch_config_json(launch: &SessionLaunch, output_dir: &Path) -> Value {
    let mut browser = Map::new();
    if let Some(dir) = &launch.user_data_dir {
        browser.insert("userDataDir".into(), json!(dir));
    }

    let mut launch_options = Map::new();
    if let Some(proxy) = &launch.proxy {
        launch_options.insert("proxy".into(), json!({ "server": proxy }));
    }
    if !launch.extra_args.is_empty() {
        launch_options.insert("args".into(), json!(launch.extra_args));
    }
    if !launch_options.is_empty() {
        browser.insert("launchOptions".into(), Value::Object(launch_options));
    }

    json!({
        "browser": Value::Object(browser),
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

/// The `open` argv (after the `-s=<session>` flag the driver always prepends).
///
/// `--headed` belongs to `open` and to nothing else: it used to be prepended
/// to the `tab-new` argv, where the CLI rejects it outright
/// (`Unknown option: --headed`, exit 1) — so headed mode was not a degraded
/// mode, it was a hard failure on every call.
///
/// No URL is passed: `open` alone lands on `about:blank`, which keeps the
/// launch out of the SSRF guard's way. The caller navigates afterwards through
/// the normal, guarded path.
#[must_use]
pub fn open_argv(launch: &SessionLaunch, config_path: &Path) -> Vec<String> {
    let mut argv = vec![
        "open".to_string(),
        "--config".to_string(),
        config_path.to_string_lossy().into_owned(),
    ];
    if !launch.headless {
        argv.push("--headed".to_string());
    }
    if let Some(value) = browser_flag_value(&launch.browser) {
        argv.push("--browser".to_string());
        argv.push(value.to_string());
    }
    argv
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
fn browser_state_dir(leaf: &str) -> Result<PathBuf, super::error::BrowserError> {
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
/// Rewritten on every launch rather than written once: the profile's proxy /
/// user-data-dir / extra args can change in config between launches, and the
/// file is only read at `open` time, so the cheapest correct thing is to make
/// the file a pure function of the current launch.
pub async fn write_launch_config(
    session_key: &str,
    launch: &SessionLaunch,
) -> Result<PathBuf, super::error::BrowserError> {
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
    let body = launch_config_json(launch, &output_dir).to_string();
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

    #[test]
    fn a_configuring_launch_maps_onto_the_documented_schema() {
        let launch = SessionLaunch {
            headless: true,
            browser: BrowserType::Chromium,
            user_data_dir: Some("/tmp/udd".into()),
            proxy: Some("socks5://127.0.0.1:1080".into()),
            extra_args: vec!["--disable-gpu".into()],
        };
        assert_eq!(
            launch_config_json(&launch, Path::new("/tmp/out")),
            json!({
                "browser": {
                    "userDataDir": "/tmp/udd",
                    "launchOptions": {
                        "proxy": { "server": "socks5://127.0.0.1:1080" },
                        "args": ["--disable-gpu"],
                    }
                },
                "outputDir": "/tmp/out",
                // Not decoration: naming `outputDir` without this makes the CLI
                // refuse every caller-supplied path outside it — see the fn doc.
                "allowUnrestrictedFileAccess": true,
            })
        );
    }

    /// A profile that configures nothing still yields a config object — the
    /// point of writing it is to *displace* the ambient
    /// `.playwright/cli.config.json`, so "nothing to say" must not collapse
    /// into "do not pass --config".
    #[test]
    fn a_default_launch_still_produces_a_config_to_displace_the_ambient_one() {
        let json = launch_config_json(&SessionLaunch::headless_default(), Path::new("/tmp/out"));
        assert_eq!(
            json,
            json!({
                "browser": {},
                "outputDir": "/tmp/out",
                "allowUnrestrictedFileAccess": true,
            })
        );
    }

    #[test]
    fn launch_options_is_omitted_rather_than_emitted_empty() {
        let launch = SessionLaunch {
            user_data_dir: Some("/tmp/udd".into()),
            ..SessionLaunch::headless_default()
        };
        let json = launch_config_json(&launch, Path::new("/tmp/out"));
        assert!(json["browser"].get("launchOptions").is_none());
    }

    #[test]
    fn headed_puts_the_flag_on_open_where_the_cli_accepts_it() {
        let launch = SessionLaunch {
            headless: false,
            ..SessionLaunch::headless_default()
        };
        let argv = open_argv(&launch, Path::new("/tmp/c.json"));
        assert_eq!(argv[0], "open");
        assert!(argv.contains(&"--headed".to_string()));
    }

    #[test]
    fn headless_omits_the_headed_flag() {
        let argv = open_argv(&SessionLaunch::headless_default(), Path::new("/tmp/c.json"));
        assert!(!argv.contains(&"--headed".to_string()));
    }

    /// `--config` rides on every launch, including the one that configures
    /// nothing.
    #[test]
    fn every_launch_carries_an_explicit_config() {
        for launch in [
            SessionLaunch::headless_default(),
            SessionLaunch {
                headless: false,
                browser: BrowserType::Chrome,
                ..SessionLaunch::headless_default()
            },
        ] {
            let argv = open_argv(&launch, Path::new("/tmp/c.json"));
            let i = argv
                .iter()
                .position(|a| a == "--config")
                .expect("--config must always be passed");
            assert_eq!(argv[i + 1], "/tmp/c.json");
        }
    }

    /// Only the values the CLI's own `--help` lists may be passed; anything
    /// else is `Unknown option` + exit 1, so `Chromium` (Playwright's default)
    /// is expressed by omission and `Brave` has no mapping at all.
    #[test]
    fn browser_flag_only_carries_values_the_cli_accepts() {
        assert_eq!(browser_flag_value(&BrowserType::Chromium), None);
        assert_eq!(browser_flag_value(&BrowserType::Brave), None);
        assert_eq!(browser_flag_value(&BrowserType::Chrome), Some("chrome"));
        assert_eq!(browser_flag_value(&BrowserType::Edge), Some("msedge"));

        let argv = open_argv(
            &SessionLaunch {
                browser: BrowserType::Edge,
                ..SessionLaunch::headless_default()
            },
            Path::new("/tmp/c.json"),
        );
        let i = argv.iter().position(|a| a == "--browser").unwrap();
        assert_eq!(argv[i + 1], "msedge");
    }

    /// The property that matters is positional, not lexical: whatever the
    /// name ends up looking like, the path must stay a single component
    /// directly under `cli-config`. Asserting "the name has no `..` in it"
    /// would be checking spelling — `p_.._.._etc_passwd.json` is a perfectly
    /// safe file name.
    /// Page snapshots and console logs must land under `~/.aleph`, never in
    /// whatever directory the server was started in. The CLI's default is
    /// `.playwright-cli/` relative to the process cwd, and the driver sets no
    /// cwd — which put a full accessibility tree of a browsed site into a git
    /// checkout before this was set.
    #[test]
    fn browsed_page_content_is_contained_under_aleph_home() {
        let out = output_dir_for("default").expect("home resolves");
        let json = launch_config_json(&SessionLaunch::headless_default(), &out);
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
