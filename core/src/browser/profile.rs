// Browser profile configuration, state machine, and system-level config.

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

/// Per-profile browser configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileConfig {
    /// Which browser engine to use.
    #[serde(default)]
    pub browser: BrowserType,

    /// CDP debugging port.
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,

    /// Run browser in headless mode.
    #[serde(default)]
    pub headless: bool,

    /// UI indicator color for this profile.
    #[serde(default)]
    pub color: Option<String>,

    /// Proxy server URL (e.g. "socks5://127.0.0.1:1080").
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
}

fn default_cdp_port() -> u16 {
    18800
}

fn default_idle_timeout() -> u64 {
    1800
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            browser: BrowserType::default(),
            cdp_port: default_cdp_port(),
            headless: false,
            color: None,
            proxy: None,
            user_data_dir: None,
            extra_args: Vec::new(),
            idle_timeout_secs: default_idle_timeout(),
        }
    }
}

/// Runtime state of a browser profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileState {
    /// No browser process running.
    Idle,
    /// Browser process is being launched.
    Starting,
    /// Browser process is running with the given PID and CDP port.
    Running { pid: u32, port: u16 },
    /// Browser process is being shut down.
    Stopping,
}

impl ProfileState {
    /// Whether the profile can transition to Starting.
    pub fn can_start(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Whether the browser process is currently running.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

/// Configuration for the Playwright MCP integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaywrightMcpConfig {
    /// Whether Playwright MCP is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Command to launch the MCP server.
    #[serde(default = "default_mcp_command")]
    pub command: String,

    /// Arguments for the MCP command.
    #[serde(default = "default_mcp_args")]
    pub args: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_mcp_command() -> String {
    "npx".to_string()
}

fn default_mcp_args() -> Vec<String> {
    vec!["@anthropic/mcp-playwright".to_string()]
}

impl Default for PlaywrightMcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: default_mcp_command(),
            args: default_mcp_args(),
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

    /// Playwright MCP integration settings.
    #[serde(default)]
    pub playwright_mcp: PlaywrightMcpConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_defaults() {
        let config = ProfileConfig::default();
        assert_eq!(config.browser, BrowserType::Chromium);
        assert_eq!(config.cdp_port, 18800);
        assert!(!config.headless);
        assert!(config.color.is_none());
        assert!(config.proxy.is_none());
        assert!(config.user_data_dir.is_none());
        assert!(config.extra_args.is_empty());
        assert_eq!(config.idle_timeout_secs, 1800);
    }

    #[test]
    fn test_profile_state_can_start() {
        assert!(ProfileState::Idle.can_start());
        assert!(!ProfileState::Starting.can_start());
        assert!(!(ProfileState::Running { pid: 1, port: 9222 }).can_start());
        assert!(!ProfileState::Stopping.can_start());
    }

    #[test]
    fn test_profile_state_is_running() {
        assert!(!ProfileState::Idle.is_running());
        assert!(!ProfileState::Starting.is_running());
        assert!((ProfileState::Running { pid: 42, port: 18800 }).is_running());
        assert!(!ProfileState::Stopping.is_running());
    }

    #[test]
    fn test_browser_system_config_toml_deserialization() {
        let toml_str = r##"
[profiles.work]
browser = "chrome"
cdp_port = 19000
headless = true
color = "#ff0000"
proxy = "socks5://127.0.0.1:1080"
extra_args = ["--disable-gpu"]
idle_timeout_secs = 3600

[profiles.personal]
browser = "brave"
cdp_port = 19001

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
        assert_eq!(work.cdp_port, 19000);
        assert!(work.headless);
        assert_eq!(work.color.as_deref(), Some("#ff0000"));
        assert_eq!(work.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(work.extra_args, vec!["--disable-gpu"]);
        assert_eq!(work.idle_timeout_secs, 3600);

        // Personal profile
        let personal = config.profiles.get("personal").unwrap();
        assert_eq!(personal.browser, BrowserType::Brave);
        assert_eq!(personal.cdp_port, 19001);
        assert!(!personal.headless); // default
        assert_eq!(personal.idle_timeout_secs, 1800); // default

        // Policy
        assert!(config.policy.block_private);
        assert_eq!(config.policy.blocked_domains, vec!["*.malware.com"]);

        // Playwright MCP
        assert!(!config.playwright_mcp.enabled);
        assert_eq!(config.playwright_mcp.command, "node");
        assert_eq!(config.playwright_mcp.args, vec!["./mcp-server.js"]);
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
        assert_eq!(serde_json::to_string(&BrowserType::Chromium).unwrap(), "\"chromium\"");
        assert_eq!(serde_json::to_string(&BrowserType::Chrome).unwrap(), "\"chrome\"");
        assert_eq!(serde_json::to_string(&BrowserType::Brave).unwrap(), "\"brave\"");
        assert_eq!(serde_json::to_string(&BrowserType::Edge).unwrap(), "\"edge\"");
    }

    #[test]
    fn test_playwright_mcp_config_defaults() {
        let config = PlaywrightMcpConfig::default();
        assert!(config.enabled);
        assert_eq!(config.command, "npx");
        assert_eq!(config.args, vec!["@anthropic/mcp-playwright"]);
    }

    #[test]
    fn test_browser_system_config_defaults() {
        let config = BrowserSystemConfig::default();
        assert!(config.profiles.is_empty());
        assert!(config.policy.block_private);
        assert!(config.playwright_mcp.enabled);
    }
}
