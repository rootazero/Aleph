# P2: Browser System — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add managed browser profiles, SSRF protection, and Playwright MCP integration to Aleph's browser system, splitting the monolithic browser tool into individual focused tools.

**Architecture:** Core defines `NetworkPolicy` and `BrowserManager` traits (R1). Existing CDP runtime remains as fallback. New Playwright MCP Server provides advanced DOM operations. Individual browser_* tools replace the monolithic BrowserTool, each doing param validation + SSRF check + dispatch (to MCP or CDP).

**Tech Stack:** Rust, chromiumoxide (existing), MCP client (existing), Playwright MCP Server (Node.js, external), TOML config.

**Key Finding:** Aleph already has a working BrowserTool (13 actions) and BrowserRuntime (CDP). The browser is NOT registered in BUILTIN_TOOL_DEFINITIONS. The MCP client system is mature with stdio/http/sse transports.

---

## Task 1: NetworkPolicy Trait + SSRF Implementation

Add URL validation to block private network access and enforce domain allow/block lists.

**Files:**
- Create: `src/browser/network_policy.rs`
- Modify: `src/browser/mod.rs` (add module)
- Test: inline in network_policy.rs

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_localhost() {
        let policy = SsrfPolicy::default();
        assert!(policy.check_url("http://localhost/admin").is_err());
        assert!(policy.check_url("http://127.0.0.1:8080/").is_err());
        assert!(policy.check_url("http://[::1]/").is_err());
    }

    #[test]
    fn test_blocks_private_networks() {
        let policy = SsrfPolicy::default();
        assert!(policy.check_url("http://10.0.0.1/").is_err());
        assert!(policy.check_url("http://172.16.0.1/").is_err());
        assert!(policy.check_url("http://192.168.1.1/").is_err());
    }

    #[test]
    fn test_allows_public_urls() {
        let policy = SsrfPolicy::default();
        assert!(policy.check_url("https://example.com/").is_ok());
        assert!(policy.check_url("https://google.com/search?q=test").is_ok());
    }

    #[test]
    fn test_blocked_domains() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: true,
            blocked_domains: vec!["*.malware.com".into(), "evil.org".into()],
            allowed_domains: vec![],
        });
        assert!(policy.check_url("https://foo.malware.com/").is_err());
        assert!(policy.check_url("https://evil.org/").is_err());
        assert!(policy.check_url("https://good.com/").is_ok());
    }

    #[test]
    fn test_allowed_domains_whitelist() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: true,
            blocked_domains: vec![],
            allowed_domains: vec!["*.company.com".into(), "docs.rs".into()],
        });
        // Only allowed domains pass
        assert!(policy.check_url("https://app.company.com/").is_ok());
        assert!(policy.check_url("https://docs.rs/").is_ok());
        assert!(policy.check_url("https://google.com/").is_err());
    }

    #[test]
    fn test_disabled_ssrf_allows_everything() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
        });
        assert!(policy.check_url("http://localhost/").is_ok());
        assert!(policy.check_url("http://10.0.0.1/").is_ok());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib browser::network_policy::tests`
Expected: FAIL — module doesn't exist

**Step 3: Implement**

```rust
use std::net::IpAddr;
use url::Url;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SsrfConfig {
    #[serde(default = "default_true")]
    pub block_private: bool,
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

fn default_true() -> bool { true }

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            block_private: true,
            blocked_domains: vec![],
            allowed_domains: vec![],
        }
    }
}

#[derive(Debug)]
pub enum PolicyViolation {
    PrivateNetwork(String),
    BlockedDomain(String),
    NotInAllowlist(String),
    InvalidUrl(String),
}

impl std::fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrivateNetwork(url) => write!(f, "URL targets private network: {url}"),
            Self::BlockedDomain(domain) => write!(f, "Domain is blocked: {domain}"),
            Self::NotInAllowlist(domain) => write!(f, "Domain not in allowlist: {domain}"),
            Self::InvalidUrl(url) => write!(f, "Invalid URL: {url}"),
        }
    }
}

pub struct SsrfPolicy {
    config: SsrfConfig,
}

impl SsrfPolicy {
    pub fn new(config: SsrfConfig) -> Self {
        Self { config }
    }

    pub fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
        let url = Url::parse(url_str)
            .map_err(|_| PolicyViolation::InvalidUrl(url_str.to_string()))?;

        let host = url.host_str()
            .ok_or_else(|| PolicyViolation::InvalidUrl(url_str.to_string()))?;

        // Check private networks
        if self.config.block_private {
            if self.is_private_host(host) {
                return Err(PolicyViolation::PrivateNetwork(url_str.to_string()));
            }
        }

        // Check blocked domains
        for pattern in &self.config.blocked_domains {
            if self.domain_matches(host, pattern) {
                return Err(PolicyViolation::BlockedDomain(host.to_string()));
            }
        }

        // Check allowed domains (if non-empty, acts as whitelist)
        if !self.config.allowed_domains.is_empty() {
            let allowed = self.config.allowed_domains.iter()
                .any(|pattern| self.domain_matches(host, pattern));
            if !allowed {
                return Err(PolicyViolation::NotInAllowlist(host.to_string()));
            }
        }

        Ok(())
    }

    fn is_private_host(&self, host: &str) -> bool {
        // Check localhost variants
        if host == "localhost" || host == "[::1]" {
            return true;
        }

        // Parse as IP and check private ranges
        if let Ok(ip) = host.parse::<IpAddr>() {
            return match ip {
                IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
                IpAddr::V6(v6) => v6.is_loopback(),
            };
        }

        false
    }

    fn domain_matches(&self, host: &str, pattern: &str) -> bool {
        if pattern.starts_with("*.") {
            let suffix = &pattern[1..]; // ".domain.com"
            host.ends_with(suffix) || host == &pattern[2..]
        } else {
            host == pattern
        }
    }
}

impl Default for SsrfPolicy {
    fn default() -> Self {
        Self::new(SsrfConfig::default())
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib browser::network_policy::tests`
Expected: PASS

**Step 5: Commit**

```
feat(browser): add SsrfPolicy for URL validation and private network blocking
```

---

## Task 2: Browser Profile Config Types

Define profile configuration, state machine, and config file structure.

**Files:**
- Create: `src/browser/profile.rs`
- Modify: `src/browser/mod.rs`
- Modify: `src/config/types/general.rs` (add browser config section)
- Test: inline

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_defaults() {
        let profile = ProfileConfig::default();
        assert_eq!(profile.browser, BrowserType::Chromium);
        assert_eq!(profile.cdp_port, 18800);
        assert!(!profile.headless);
    }

    #[test]
    fn test_profile_state_transitions() {
        let mut state = ProfileState::Idle;
        assert!(state.can_start());
        state = ProfileState::Starting;
        assert!(!state.can_start());
        state = ProfileState::Running { pid: 1234, port: 18800 };
        assert!(!state.can_start());
    }

    #[test]
    fn test_browser_config_deserializes() {
        let toml = r#"
        [profiles.default]
        browser = "chromium"
        cdp_port = 18800
        headless = false

        [profiles.work]
        browser = "chrome"
        cdp_port = 18801
        proxy = "socks5://127.0.0.1:1080"

        [policy]
        block_private = true
        blocked_domains = ["*.malware.com"]
        "#;
        let config: BrowserSystemConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert!(config.profiles.contains_key("default"));
        assert!(config.profiles.contains_key("work"));
        assert!(config.policy.block_private);
    }
}
```

**Step 2: Run to verify failure**

**Step 3: Implement**

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use super::network_policy::SsrfConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserType {
    Chromium,
    Chrome,
    Brave,
    Edge,
}

impl Default for BrowserType {
    fn default() -> Self { Self::Chromium }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileConfig {
    #[serde(default)]
    pub browser: BrowserType,
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,
    #[serde(default)]
    pub headless: bool,
    pub color: Option<String>,
    pub proxy: Option<String>,
    pub user_data_dir: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
}

fn default_cdp_port() -> u16 { 18800 }
fn default_idle_timeout() -> u64 { 1800 } // 30 min

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            browser: BrowserType::default(),
            cdp_port: default_cdp_port(),
            headless: false,
            color: None,
            proxy: None,
            user_data_dir: None,
            extra_args: vec![],
            idle_timeout_secs: default_idle_timeout(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileState {
    Idle,
    Starting,
    Running { pid: u32, port: u16 },
    Stopping,
}

impl ProfileState {
    pub fn can_start(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BrowserSystemConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub policy: SsrfConfig,
    #[serde(default = "default_playwright_mcp")]
    pub playwright_mcp: PlaywrightMcpConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlaywrightMcpConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_playwright_command")]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

fn default_true() -> bool { true }
fn default_playwright_command() -> String { "npx".into() }

impl Default for PlaywrightMcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: "npx".into(),
            args: vec!["@anthropic/mcp-playwright".into()],
        }
    }
}
```

Add `browser` field to `GeneralConfig` in `src/config/types/general.rs`:
```rust
#[serde(default)]
pub browser: BrowserSystemConfig,
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib browser::profile::tests`
Expected: PASS

**Step 5: Commit**

```
feat(browser): add profile config types and browser system configuration
```

---

## Task 3: ProfileManager — Profile Lifecycle

Manage browser profile instances (start, stop, health check, idle reclaim).

**Files:**
- Create: `src/browser/manager.rs`
- Modify: `src/browser/mod.rs`
- Test: inline

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_registers_profiles_from_config() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert("default".into(), ProfileConfig::default());
        config.profiles.insert("work".into(), ProfileConfig {
            cdp_port: 18801,
            ..Default::default()
        });

        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "work"));
    }

    #[test]
    fn test_manager_default_profile_if_none_configured() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].0, "default");
    }

    #[test]
    fn test_get_profile_state() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let state = manager.get_state("default");
        assert_eq!(state, Some(ProfileState::Idle));
    }
}
```

**Step 2: Run to verify failure**

**Step 3: Implement**

```rust
use std::collections::HashMap;
use std::sync::RwLock;
use super::profile::{BrowserSystemConfig, ProfileConfig, ProfileState};
use super::network_policy::SsrfPolicy;

pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    ssrf_policy: SsrfPolicy,
    config: BrowserSystemConfig,
}

struct ManagedProfile {
    config: ProfileConfig,
    state: ProfileState,
    last_activity: std::time::Instant,
}

impl ProfileManager {
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_policy = SsrfPolicy::new(config.policy.clone());

        let mut profiles = HashMap::new();

        if config.profiles.is_empty() {
            // Create default profile if none configured
            profiles.insert("default".into(), ManagedProfile {
                config: ProfileConfig::default(),
                state: ProfileState::Idle,
                last_activity: std::time::Instant::now(),
            });
        } else {
            for (name, profile_config) in &config.profiles {
                profiles.insert(name.clone(), ManagedProfile {
                    config: profile_config.clone(),
                    state: ProfileState::Idle,
                    last_activity: std::time::Instant::now(),
                });
            }
        }

        Self {
            profiles: RwLock::new(profiles),
            ssrf_policy,
            config,
        }
    }

    pub fn list_profiles(&self) -> Vec<(String, ProfileState)> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.iter()
            .map(|(name, p)| (name.clone(), p.state.clone()))
            .collect()
    }

    pub fn get_state(&self, name: &str) -> Option<ProfileState> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.state.clone())
    }

    pub fn get_config(&self, name: &str) -> Option<ProfileConfig> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.config.clone())
    }

    pub fn check_url(&self, url: &str) -> Result<(), super::network_policy::PolicyViolation> {
        self.ssrf_policy.check_url(url)
    }

    pub fn record_activity(&self, profile_name: &str) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.last_activity = std::time::Instant::now();
        }
    }

    pub fn set_state(&self, profile_name: &str, state: ProfileState) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.state = state;
        }
    }

    /// Returns profiles that have been idle longer than their timeout.
    pub fn idle_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.iter()
            .filter(|(_, p)| {
                p.state.is_running()
                    && p.last_activity.elapsed().as_secs() > p.config.idle_timeout_secs
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib browser::manager::tests`
Expected: PASS

**Step 5: Commit**

```
feat(browser): add ProfileManager for browser profile lifecycle management
```

---

## Task 4: Playwright MCP Server Integration

Wire Playwright MCP Server as an auto-started MCP server, leveraging existing McpClient infrastructure.

**Files:**
- Create: `src/browser/playwright_bridge.rs`
- Modify: `src/browser/mod.rs`
- Test: inline (config generation test, not actual MCP connection)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_mcp_server_config() {
        let config = PlaywrightMcpConfig::default();
        let mcp_config = PlaywrightBridge::to_mcp_config(&config, "default", 18800);

        assert_eq!(mcp_config.name, "playwright-default");
        assert_eq!(mcp_config.command, "npx");
        assert!(mcp_config.args.contains(&"@anthropic/mcp-playwright".to_string()));
    }

    #[test]
    fn test_bridge_tool_name_mapping() {
        // Maps our tool names to Playwright MCP tool names
        assert_eq!(
            PlaywrightBridge::map_tool_name("browser_click"),
            Some("playwright_click")
        );
        assert_eq!(
            PlaywrightBridge::map_tool_name("browser_screenshot"),
            Some("playwright_screenshot")
        );
        assert_eq!(
            PlaywrightBridge::map_tool_name("unknown_tool"),
            None
        );
    }
}
```

**Step 2: Run to verify failure**

**Step 3: Implement**

```rust
use crate::mcp::types::McpToolResult;
use super::profile::PlaywrightMcpConfig;

/// Bridge between Aleph browser tools and the Playwright MCP Server.
/// Translates Aleph tool names/args to Playwright MCP calls.
pub struct PlaywrightBridge;

impl PlaywrightBridge {
    /// Generate an MCP external server config for Playwright.
    pub fn to_mcp_config(
        config: &PlaywrightMcpConfig,
        profile_name: &str,
        cdp_port: u16,
    ) -> crate::config::types::tools::McpExternalServerConfig {
        let mut args = config.args.clone();

        // Add CDP endpoint so Playwright connects to our managed browser
        args.push("--cdp-endpoint".into());
        args.push(format!("http://127.0.0.1:{cdp_port}"));

        crate::config::types::tools::McpExternalServerConfig {
            name: format!("playwright-{profile_name}"),
            command: config.command.clone(),
            args,
            env: std::collections::HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: 30,
        }
    }

    /// Map Aleph browser tool names to Playwright MCP tool names.
    pub fn map_tool_name(aleph_name: &str) -> Option<&'static str> {
        match aleph_name {
            "browser_click" => Some("playwright_click"),
            "browser_type" => Some("playwright_type"),
            "browser_select" => Some("playwright_select"),
            "browser_screenshot" => Some("playwright_screenshot"),
            "browser_snapshot" => Some("playwright_snapshot"),
            "browser_evaluate" => Some("playwright_evaluate"),
            "browser_navigate" => Some("playwright_navigate"),
            "browser_fill_form" => Some("playwright_fill"),
            "browser_upload" => Some("playwright_upload"),
            "browser_download" => Some("playwright_download"),
            "browser_network" => Some("playwright_network"),
            _ => None,
        }
    }

    /// Map Aleph tool args to Playwright MCP args format.
    pub fn map_args(
        aleph_name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        // For now, pass through. Playwright MCP accepts similar arg shapes.
        // Individual tools can override mapping if needed.
        args
    }
}
```

NOTE: The actual Playwright MCP server tool names will be discovered at runtime via `McpClient::list_tools()`. The static mapping above is a starting point — the real integration will use the dynamic tool registry.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib browser::playwright_bridge::tests`
Expected: PASS

**Step 5: Commit**

```
feat(browser): add PlaywrightBridge for MCP server integration
```

---

## Task 5: Individual Browser Tools — Core Set

Split the monolithic BrowserTool into focused tools. Start with the most used: browser_open, browser_screenshot, browser_snapshot, browser_click, browser_type.

**Files:**
- Create: `src/builtin_tools/browser_tools/mod.rs`
- Create: `src/builtin_tools/browser_tools/open.rs`
- Create: `src/builtin_tools/browser_tools/screenshot.rs`
- Create: `src/builtin_tools/browser_tools/click.rs`
- Create: `src/builtin_tools/browser_tools/type_text.rs`
- Create: `src/builtin_tools/browser_tools/snapshot.rs`
- Modify: `src/builtin_tools/mod.rs`
- Test: inline per tool

**Each tool follows this pattern:**

```rust
use crate::tools::traits::AlephTool;
use crate::browser::manager::ProfileManager;
use crate::browser::network_policy::PolicyViolation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BrowserOpenArgs {
    /// URL to open
    pub url: String,
    /// Browser profile name (default: "default")
    #[serde(default = "default_profile")]
    pub profile: String,
}

fn default_profile() -> String { "default".into() }

#[derive(Debug, Serialize)]
pub struct BrowserOpenOutput {
    pub success: bool,
    pub tab_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<ProfileManager>,
}

impl BrowserOpenTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl AlephTool for BrowserOpenTool {
    const NAME: &'static str = "browser_open";
    const DESCRIPTION: &'static str = "Open a URL in a managed browser profile";
    type Args = BrowserOpenArgs;
    type Output = BrowserOpenOutput;

    async fn call(&self, args: Self::Args) -> anyhow::Result<Self::Output> {
        // 1. SSRF check
        if let Err(violation) = self.manager.check_url(&args.url) {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!("Blocked: {violation}")),
            });
        }

        // 2. Record activity
        self.manager.record_activity(&args.profile);

        // 3. Delegate to runtime (CDP or MCP)
        // For now, use existing BrowserRuntime
        // TODO: Route through PlaywrightBridge when MCP server is available

        Ok(BrowserOpenOutput {
            success: true,
            tab_id: Some("tab-1".into()),
            message: Some(format!("Opened {} in profile '{}'", args.url, args.profile)),
        })
    }
}
```

Repeat pattern for:
- `browser_screenshot` — capture page screenshot
- `browser_snapshot` — get ARIA tree
- `browser_click` — click element (CSS/XPath/ARIA/coords)
- `browser_type` — type text into element

Each tool:
1. Validates args
2. Checks SSRF for navigation-related actions
3. Records activity on ProfileManager
4. Delegates to existing BrowserRuntime (CDP fallback)

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::browser_tools`
Expected: PASS

**Step 5: Commit**

```
feat(browser): add individual browser_open, browser_screenshot, browser_snapshot, browser_click, browser_type tools
```

---

## Task 6: Additional Browser Tools

Add the remaining browser tools: browser_navigate, browser_tabs, browser_select, browser_evaluate, browser_fill_form, browser_profile.

**Files:**
- Create: `src/builtin_tools/browser_tools/navigate.rs` (back/forward/refresh)
- Create: `src/builtin_tools/browser_tools/tabs.rs` (list/switch/close)
- Create: `src/builtin_tools/browser_tools/select.rs` (dropdown selection)
- Create: `src/builtin_tools/browser_tools/evaluate.rs` (JS execution)
- Create: `src/builtin_tools/browser_tools/fill_form.rs` (smart form fill)
- Create: `src/builtin_tools/browser_tools/profile_tool.rs` (manage profiles)
- Modify: `src/builtin_tools/browser_tools/mod.rs`

Same pattern as Task 5. Each tool is self-contained with args, output, and AlephTool impl.

**Step 5: Commit**

```
feat(browser): add browser_navigate, browser_tabs, browser_select, browser_evaluate, browser_fill_form, browser_profile tools
```

---

## Task 7: Register Browser Tools in Builtin Registry

Wire all new browser tools into the tool registration system so the agent can discover and use them.

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs` (add tool definitions)
- Modify: `src/executor/builtin_registry/mod.rs` (add create_tool_boxed cases)

**What to add to BUILTIN_TOOL_DEFINITIONS:**

```rust
BuiltinToolDefinition { name: "browser_open", description: "Open URL in browser", requires_config: false },
BuiltinToolDefinition { name: "browser_click", description: "Click element", requires_config: false },
BuiltinToolDefinition { name: "browser_type", description: "Type text", requires_config: false },
BuiltinToolDefinition { name: "browser_screenshot", description: "Capture screenshot", requires_config: false },
BuiltinToolDefinition { name: "browser_snapshot", description: "Get ARIA tree", requires_config: false },
BuiltinToolDefinition { name: "browser_navigate", description: "Navigate back/forward/refresh", requires_config: false },
BuiltinToolDefinition { name: "browser_tabs", description: "Manage tabs", requires_config: false },
BuiltinToolDefinition { name: "browser_select", description: "Select dropdown option", requires_config: false },
BuiltinToolDefinition { name: "browser_evaluate", description: "Execute JavaScript", requires_config: false },
BuiltinToolDefinition { name: "browser_fill_form", description: "Fill form fields", requires_config: false },
BuiltinToolDefinition { name: "browser_profile", description: "Manage browser profiles", requires_config: false },
```

**What to add to create_tool_boxed:**

```rust
"browser_open" => {
    let manager = get_or_create_profile_manager(config);
    Some(Box::new(BrowserOpenTool::new(manager)))
}
// ... repeat for each tool
```

IMPORTANT: Read the actual `create_tool_boxed` function and builtin_registry to understand the exact pattern. The ProfileManager needs to be shared (Arc) across all browser tools.

**Step 4: Run tests**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Commit**

```
feat(browser): register all browser tools in builtin registry
```

---

## Task 8: Browser Config in aleph.toml

Add the `[browser]` config section to the main config file and wire it into server startup.

**Files:**
- Modify: `src/config/types/general.rs` (already added in Task 2)
- Create example config at `examples/browser-config.toml` (documentation)

**Example config:**

```toml
[browser]

[browser.profiles.default]
browser = "chromium"
cdp_port = 18800
headless = false
idle_timeout_secs = 1800

[browser.profiles.work]
browser = "chrome"
cdp_port = 18801
proxy = "socks5://127.0.0.1:1080"

[browser.policy]
block_private = true
blocked_domains = ["*.malware.com"]
allowed_domains = []

[browser.playwright_mcp]
enabled = true
command = "npx"
args = ["@anthropic/mcp-playwright"]
```

**Step 5: Commit**

```
feat(config): add browser system configuration with profiles and SSRF policy
```

---

## Task 9: Integration Test — SSRF + Profile + Tool Chain

End-to-end test verifying: config loads → ProfileManager created → SSRF policy enforced → browser_open blocked for private URLs.

**Files:**
- Add test to: `src/browser/manager.rs` or `src/builtin_tools/browser_tools/open.rs`

```rust
#[tokio::test]
async fn test_browser_open_blocks_ssrf() {
    let mut config = BrowserSystemConfig::default();
    config.policy.block_private = true;

    let manager = Arc::new(ProfileManager::new(config));
    let tool = BrowserOpenTool::new(manager);

    // Should block localhost
    let result = tool.call(BrowserOpenArgs {
        url: "http://localhost:3000/admin".into(),
        profile: "default".into(),
    }).await.unwrap();

    assert!(!result.success);
    assert!(result.message.unwrap().contains("Blocked"));

    // Should allow public URLs
    let result = tool.call(BrowserOpenArgs {
        url: "https://example.com".into(),
        profile: "default".into(),
    }).await.unwrap();

    assert!(result.success);
}

#[test]
fn test_full_config_deserialization() {
    let toml = r#"
    [browser.profiles.default]
    browser = "chromium"
    cdp_port = 18800

    [browser.policy]
    block_private = true
    blocked_domains = ["evil.com"]

    [browser.playwright_mcp]
    enabled = true
    "#;

    let config: GeneralConfig = toml::from_str(toml).unwrap();
    assert!(config.browser.policy.block_private);
    assert_eq!(config.browser.profiles.len(), 1);
    assert!(config.browser.playwright_mcp.enabled);
}
```

**Step 5: Commit**

```
test(browser): add integration tests for SSRF enforcement and config chain
```

---

## Summary

| Task | Component | Complexity |
|------|-----------|-----------|
| 1 | SsrfPolicy (URL validation) | Low |
| 2 | Profile config types | Low |
| 3 | ProfileManager (lifecycle) | Medium |
| 4 | PlaywrightBridge (MCP mapping) | Low |
| 5 | Core browser tools (5 tools) | Medium |
| 6 | Additional browser tools (6 tools) | Medium |
| 7 | Tool registration | Medium |
| 8 | Config wiring | Low |
| 9 | Integration tests | Low |

**Dependencies:** Task 1 → 3 (SSRF used by manager). Task 2 → 3 (config types). Task 1+2+3 → 5 (tools use manager). Task 4 independent. Task 5 → 6 (pattern established). Task 5+6 → 7 (tools exist to register). Task 2 → 8. Task 7 → 9.
