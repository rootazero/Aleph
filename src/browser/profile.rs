// Browser profile configuration and system-level config.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::network_policy::SsrfConfig;

/// Supported browser engines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrowserType {
    #[default]
    Chromium,
    Chrome,
    Brave,
    Edge,
}

/// Driver mode for browser profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriver {
    /// Aleph launches and manages a dedicated browser instance (Playwright CLI managed via fnm).
    #[default]
    Managed,
    /// Attach to user's running Chrome via Chrome `DevTools` MCP.
    ExistingSession,
}

/// Per-profile browser configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileConfig {
    /// Which browser engine to use.
    ///
    /// Honored by both drivers. The managed driver no longer passes the engine
    /// to `playwright-cli` (it launches the browser itself); the value steers
    /// `discovery::find_chromium_preferred`. When the requested engine is not
    /// installed the search degrades to whatever is, and
    /// `PlaywrightCliDriver::ensure_chromium` warns with the engine it actually
    /// resolved — the substitution is reported by the code that performs it,
    /// which is why the old boot-time warning is gone.
    #[serde(default)]
    pub browser: BrowserType,

    /// Profile-level override for headless (None = follow global `playwright_cli.headless`).
    /// Honored by both drivers.
    #[serde(default)]
    pub headless: Option<bool>,

    /// Proxy server URL (e.g. "<socks5://127.0.0.1:1080>").
    ///
    /// Honored by both drivers: Chrome's `--proxy-server` on the
    /// existing-session launch argv, and `browser.launchOptions.proxy.server`
    /// in the generated `open --config` file on the managed side. The CLI has
    /// no proxy *flag*, which is why this was once documented as
    /// existing-session only — the surface is the config file, not the flag
    /// list.
    #[serde(default)]
    pub proxy: Option<String>,

    /// Custom user data directory for browser state isolation.
    ///
    /// Honored by both drivers: Chrome's `--user-data-dir` on the
    /// existing-session launch argv, and `browser.userDataDir` in the
    /// generated `open --config` file on the managed side. Left unset, a
    /// managed session keeps its profile in memory.
    #[serde(default)]
    pub user_data_dir: Option<String>,

    /// Extra command-line arguments passed to the browser process.
    ///
    /// Honored by both drivers: appended last to the Chrome launch argv on the
    /// existing-session side, and passed as `browser.launchOptions.args` in
    /// the generated `open --config` file on the managed side.
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Seconds of inactivity before the browser session is automatically torn
    /// down.
    ///
    /// Honored by both drivers: `ProfileManager::reap_idle` destroys the
    /// Chrome MCP session for an existing-session profile and runs
    /// `playwright-cli close` for a managed one. (The CLI does have a
    /// stop-session command — `close`, plus `close-all` / `kill-all`; this was
    /// once documented as having none, so the setting was accepted and never
    /// enforced on the managed side.) Tabs are reclaimed sooner and
    /// separately, via [`Self::tab_idle_timeout_secs`].
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,

    /// Driver mode: managed (launch dedicated browser) or existing-session (attach to user's Chrome).
    #[serde(default)]
    pub driver: BrowserDriver,

    /// Max concurrently-open tabs for this profile before the least-recently-used
    /// are reclaimed on the next sweep. Only enforced for `Managed` profiles
    /// (Aleph-owned browsers); `ExistingSession` tabs belong to the user and are
    /// never reaped. (openclaw per-session cap parity.)
    #[serde(default = "default_max_tabs")]
    pub max_tabs_per_profile: usize,

    /// Seconds a tab may sit idle before it is reclaimed. Shorter than
    /// `idle_timeout_secs` — an unused tab is cheap to reopen. `Managed` only.
    #[serde(default = "default_tab_idle_timeout")]
    pub tab_idle_timeout_secs: u64,
}

const fn default_idle_timeout() -> u64 {
    1800
}

const fn default_max_tabs() -> usize {
    super::tab_registry::DEFAULT_MAX_TABS_PER_PROFILE
}

const fn default_tab_idle_timeout() -> u64 {
    super::tab_registry::DEFAULT_TAB_IDLE_TIMEOUT_SECS
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            browser: BrowserType::default(),
            headless: None,
            proxy: None,
            user_data_dir: None,
            extra_args: Vec::new(),
            idle_timeout_secs: default_idle_timeout(),
            driver: BrowserDriver::default(),
            max_tabs_per_profile: default_max_tabs(),
            tab_idle_timeout_secs: default_tab_idle_timeout(),
        }
    }
}

// `default_true` lives in `super::network_policy` (single source); the
// serde `default = "..."` attribute on this module's bool fields uses
// a small local wrapper because serde macros require a path resolvable
// in the current module's scope.
const fn default_true() -> bool {
    super::network_policy::default_true()
}

/// Configuration for the Playwright CLI integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaywrightCliConfig {
    /// Optional override: absolute path to `playwright-cli` binary.
    /// When `None`, resolved via `fnm exec --using lts which playwright-cli`.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Global default: run headless (profile-level `headless: Option<bool>` overrides).
    #[serde(default = "default_true")]
    pub headless: bool,

    /// Timeout (seconds) for navigate / `wait_for_text`.
    #[serde(default = "default_nav_timeout")]
    pub nav_timeout_secs: u64,

    /// Timeout (seconds) for other actions (click/fill/type/etc).
    #[serde(default = "default_action_timeout")]
    pub action_timeout_secs: u64,
}

const fn default_nav_timeout() -> u64 {
    30
}
const fn default_action_timeout() -> u64 {
    10
}

impl Default for PlaywrightCliConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            headless: true,
            nav_timeout_secs: 30,
            action_timeout_secs: 10,
        }
    }
}

/// External-runtime settings for the managed driver's browser.
///
/// Chromium is deliberately NOT in any Aleph installer (D4): all three
/// artifacts stay Chromium-free and the browser is supplied at runtime, the
/// same way `playwright-cli` already is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserRuntimeConfig {
    /// Absolute path to a Chromium-family binary, pinned by the operator.
    /// Highest precedence — a pin that does not exist is a hard failure, not a
    /// fallback, because silently launching a different browser than the one
    /// named is worse than refusing.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Use a system-installed Chromium-family browser (via
    /// `discovery::find_chromium_preferred`) before Playwright's own.
    ///
    /// Default `true`: Windows almost always has Edge and macOS usually has
    /// Chrome, so the ~150 MB download is only for a clean Linux host. The
    /// Chrome spike ran system Chrome 152 against playwright-core 1.60 with no
    /// trouble, so the cross-version mixing this permits is measured, not hoped.
    #[serde(default = "default_true")]
    pub prefer_system_browser: bool,

    /// `PLAYWRIGHT_DOWNLOAD_HOST` for the install. Playwright's CDN is blocked
    /// on some networks exactly as GitHub release assets are; npmmirror carries
    /// a mirror. A config key rather than "go export a variable", because the
    /// installer runs inside the daemon.
    #[serde(default)]
    pub download_host: Option<String>,
}

impl BrowserRuntimeConfig {
    /// The pinned binary, or `None` when unset **or blank**.
    ///
    /// A cleared form field posts `""`, and `Some("")` would be spent as a path
    /// — resolving to the current directory and failing with a message that
    /// names nothing.
    #[must_use]
    pub fn pinned_binary(&self) -> Option<&str> {
        self.binary_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// The download mirror, or `None` when unset or blank. See
    /// [`Self::pinned_binary`] for why blank is not a value.
    #[must_use]
    pub fn download_host(&self) -> Option<&str> {
        self.download_host
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            prefer_system_browser: true,
            download_host: None,
        }
    }
}

/// Configuration for the Chrome `DevTools` MCP integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChromeMcpConfig {
    /// Command to launch Chrome `DevTools` MCP server.
    #[serde(default = "default_chrome_mcp_command")]
    pub command: String,

    /// Arguments for the MCP command.
    #[serde(default = "default_chrome_mcp_args")]
    pub args: Vec<String>,
}

fn default_chrome_mcp_command() -> String {
    "npx".to_string()
}

/// The default `npx` invocation for the Chrome DevTools MCP server.
///
/// `--allow-unrestricted-paths` turns OFF a guard the server added in v1.6.0,
/// and that is deliberate. Absent negotiated MCP `roots`, the server confines
/// every `filePath` argument to the OS temp directory — a boundary nobody here
/// chose: it permits anything under `/tmp` while refusing the user's own
/// Downloads folder, so `browser_upload` failed for the files people actually
/// want to upload and succeeded for scratch files nobody does.
///
/// The gate that stays is Aleph's own: `file_ops`' protected-location denylist
/// and allowed roots run over the upload path before the tool is ever called,
/// and that one is informed, configurable and tested. This is the same call
/// made for the managed driver when `outputDir` was found to have narrowed
/// playwright-cli's write roots: switch off the weaker second answer rather
/// than route around it.
///
/// Deliberately NOT solved by declaring the `roots` capability. That is the
/// protocol-correct answer, and it belongs in `src/mcp/` where it would apply
/// to every server Aleph connects to — a much larger blast radius than a
/// browser-profile default, and one that needs its own round.
///
/// Servers older than 1.6.0 ignore the unknown switch (yargs is not strict
/// here — verified against 1.5.0), so the default stays safe for a pinned
/// older version.
fn default_chrome_mcp_args() -> Vec<String> {
    vec![
        "-y".to_string(),
        "chrome-devtools-mcp@latest".to_string(),
        "--autoConnect".to_string(),
        "--experimentalStructuredContent".to_string(),
        "--allow-unrestricted-paths".to_string(),
    ]
}

impl Default for ChromeMcpConfig {
    fn default() -> Self {
        Self {
            command: default_chrome_mcp_command(),
            args: default_chrome_mcp_args(),
        }
    }
}

/// Top-level browser system configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSystemConfig {
    /// Named browser profiles.
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    /// SSRF protection policy.
    #[serde(default)]
    pub policy: SsrfConfig,

    /// Reads both `[playwright_cli]` and legacy `[playwright_mcp]` (unknown fields dropped).
    #[serde(default, alias = "playwright_mcp")]
    pub playwright_cli: PlaywrightCliConfig,

    /// Chrome `DevTools` MCP integration settings.
    #[serde(default)]
    pub chrome_mcp: ChromeMcpConfig,

    /// External-runtime supply for the managed driver's Chromium.
    #[serde(default)]
    pub runtime: BrowserRuntimeConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_defaults() {
        let config = ProfileConfig::default();
        assert_eq!(config.browser, BrowserType::Chromium);
        assert_eq!(config.headless, None);
        assert!(config.proxy.is_none());
        assert!(config.user_data_dir.is_none());
        assert!(config.extra_args.is_empty());
        assert_eq!(config.idle_timeout_secs, 1800);
    }

    #[test]
    fn test_browser_system_config_toml_deserialization() {
        let toml_str = r##"
[profiles.work]
browser = "chrome"
headless = true
color = "#ff0000"
proxy = "socks5://127.0.0.1:1080"
extra_args = ["--disable-gpu"]
idle_timeout_secs = 3600

[profiles.personal]
browser = "brave"

[policy]
block_private = true
blocked_domains = ["*.malware.com"]

[playwright_mcp]
enabled = false
command = "node"
args = ["./mcp-server.js"]
"##;

        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();

        // Work profile
        let work = config.profiles.get("work").unwrap();
        assert_eq!(work.browser, BrowserType::Chrome);
        assert_eq!(work.headless, Some(true));
        assert_eq!(work.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(work.extra_args, vec!["--disable-gpu"]);
        assert_eq!(work.idle_timeout_secs, 3600);

        // Personal profile
        let personal = config.profiles.get("personal").unwrap();
        assert_eq!(personal.browser, BrowserType::Brave);
        assert_eq!(personal.headless, None); // default
        assert_eq!(personal.idle_timeout_secs, 1800); // default

        // Policy
        assert!(config.policy.block_private);
        assert_eq!(config.policy.blocked_domains, vec!["*.malware.com"]);

        // Playwright CLI (legacy [playwright_mcp] section still maps to
        // playwright_cli via the serde alias; unknown legacy keys are ignored,
        // surviving fields fall back to defaults).
        assert!(config.playwright_cli.headless);
        assert_eq!(config.playwright_cli.nav_timeout_secs, 30);
    }

    /// The three `[browser.runtime]` keys, and the one property that matters
    /// about all of them: an EMPTY string is not a value.
    ///
    /// `download_host = ""` is what the spec's own config sample shows, and
    /// what a Panel form posts when the operator clears the field. Handing that
    /// to the installer as `PLAYWRIGHT_DOWNLOAD_HOST=` is not "no mirror", it is
    /// "the mirror is the empty host" — every download then fails with a URL
    /// error that names nothing. Same for a `binary_path` cleared to "".
    #[test]
    fn browser_runtime_reads_its_three_keys_and_treats_empty_as_unset() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = "/opt/chromium/chrome"
prefer_system_browser = false
download_host = "https://npmmirror.com/mirrors/playwright"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.runtime.pinned_binary(), Some("/opt/chromium/chrome"));
        assert!(!cfg.runtime.prefer_system_browser);
        assert_eq!(
            cfg.runtime.download_host(),
            Some("https://npmmirror.com/mirrors/playwright")
        );

        let cleared: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = ""
download_host = "   "
"#,
        )
        .expect("parse");
        assert_eq!(cleared.runtime.pinned_binary(), None, "empty pin is unset");
        assert_eq!(cleared.runtime.download_host(), None, "blank host is unset");
        // The `[runtime]` table is PRESENT here and the key is absent, so this
        // exercises serde's field-level `default = "default_true"` — which is a
        // different mechanism from `Default::default()` and the one that would
        // silently flip to `false` if the attribute were dropped.
        assert!(
            cleared.runtime.prefer_system_browser,
            "a system browser is preferred unless the operator says otherwise: \
             Windows almost always has Edge and macOS usually has Chrome, so the \
             download is for clean Linux servers"
        );
    }

    /// A config with no `[runtime]` table at all must still produce the
    /// defaults — this section is new, and every config file on every existing
    /// install predates it.
    #[test]
    fn a_config_without_the_runtime_table_still_gets_the_defaults() {
        let cfg: BrowserSystemConfig =
            toml::from_str("[policy]\nblock_private = true\n").expect("parse");
        assert!(cfg.runtime.prefer_system_browser);
        assert_eq!(cfg.runtime.pinned_binary(), None);
        assert_eq!(cfg.runtime.download_host(), None);
    }

    #[test]
    fn test_browser_type_serde_roundtrip() {
        let types = vec![
            BrowserType::Chromium,
            BrowserType::Chrome,
            BrowserType::Brave,
            BrowserType::Edge,
        ];

        for bt in types {
            let json = serde_json::to_string(&bt).unwrap();
            let deserialized: BrowserType = serde_json::from_str(&json).unwrap();
            assert_eq!(bt, deserialized);
        }

        // Verify lowercase serialization
        assert_eq!(
            serde_json::to_string(&BrowserType::Chromium).unwrap(),
            "\"chromium\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Chrome).unwrap(),
            "\"chrome\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Brave).unwrap(),
            "\"brave\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Edge).unwrap(),
            "\"edge\""
        );
    }

    #[test]
    fn test_browser_system_config_defaults() {
        let config = BrowserSystemConfig::default();
        assert!(config.profiles.is_empty());
        assert!(config.policy.block_private);
        assert!(config.playwright_cli.headless);
    }

    #[test]
    fn test_browser_driver_default_is_managed() {
        let driver = BrowserDriver::default();
        assert_eq!(driver, BrowserDriver::Managed);
    }

    #[test]
    fn test_browser_driver_serde_roundtrip() {
        let drivers = vec![BrowserDriver::Managed, BrowserDriver::ExistingSession];
        for d in drivers {
            let json = serde_json::to_string(&d).unwrap();
            let deserialized: BrowserDriver = serde_json::from_str(&json).unwrap();
            assert_eq!(d, deserialized);
        }
        assert_eq!(
            serde_json::to_string(&BrowserDriver::Managed).unwrap(),
            "\"managed\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserDriver::ExistingSession).unwrap(),
            "\"existing_session\""
        );
    }

    #[test]
    fn test_profile_config_driver_defaults_to_managed() {
        let config = ProfileConfig::default();
        assert_eq!(config.driver, BrowserDriver::Managed);
    }

    #[test]
    fn test_old_playwright_mcp_toml_deserializes_to_playwright_cli() {
        let toml_str = r##"
[playwright_mcp]
enabled = true
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
"##;
        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        assert!(config.playwright_cli.headless);
        assert_eq!(config.playwright_cli.nav_timeout_secs, 30);
    }

    #[test]
    fn test_playwright_cli_defaults() {
        let config = PlaywrightCliConfig::default();
        assert!(config.binary_path.is_none());
        assert!(config.headless);
        assert_eq!(config.nav_timeout_secs, 30);
        assert_eq!(config.action_timeout_secs, 10);
    }

    #[test]
    fn test_profile_config_headless_option_compat() {
        let toml_str = r##"
[profiles.default]
browser = "chromium"
headless = true
"##;
        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        let p = config.profiles.get("default").unwrap();
        assert_eq!(p.headless, Some(true));
    }

    #[test]
    fn test_chrome_mcp_config_defaults() {
        let config = ChromeMcpConfig::default();
        assert_eq!(config.command, "npx");
        assert!(config
            .args
            .contains(&"chrome-devtools-mcp@latest".to_string()));
        assert!(config.args.contains(&"--autoConnect".to_string()));
    }

    #[test]
    fn test_browser_system_config_with_chrome_mcp() {
        let toml_str = r##"
[profiles.user]
browser = "chrome"
driver = "existing_session"
color = "#00AA00"

[chrome_mcp]
command = "npx"
args = ["-y", "chrome-devtools-mcp@latest", "--autoConnect"]
"##;

        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        let user = config.profiles.get("user").unwrap();
        assert_eq!(user.browser, BrowserType::Chrome);
        assert_eq!(user.driver, BrowserDriver::ExistingSession);
        assert_eq!(config.chrome_mcp.command, "npx");
    }
}
