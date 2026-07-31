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
    #[serde(default)]
    pub browser: BrowserType,

    /// Profile-level override for headless (None = follow global `playwright_cli.headless`).
    #[serde(default)]
    pub headless: Option<bool>,

    /// UI indicator color for this profile.
    #[serde(default)]
    pub color: Option<String>,

    /// Proxy server URL (e.g. "<socks5://127.0.0.1:1080>").
    #[serde(default)]
    pub proxy: Option<String>,

    /// Custom user data directory for browser state isolation.
    #[serde(default)]
    pub user_data_dir: Option<String>,

    /// Extra command-line arguments passed to the browser process.
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Seconds of inactivity before the browser is automatically stopped.
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
            color: None,
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

const fn default_true() -> bool {
    true
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

fn default_chrome_mcp_args() -> Vec<String> {
    vec![
        "-y".to_string(),
        "chrome-devtools-mcp@latest".to_string(),
        "--autoConnect".to_string(),
        "--experimentalStructuredContent".to_string(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_defaults() {
        let config = ProfileConfig::default();
        assert_eq!(config.browser, BrowserType::Chromium);
        assert_eq!(config.headless, None);
        assert!(config.color.is_none());
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
        assert_eq!(work.color.as_deref(), Some("#ff0000"));
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
        assert_eq!(user.color.as_deref(), Some("#00AA00"));
        assert_eq!(config.chrome_mcp.command, "npx");
    }
}
