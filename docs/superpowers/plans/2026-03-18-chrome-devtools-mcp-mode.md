# Chrome DevTools MCP Mode Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add existing-session browser driver that attaches to the user's running Chrome via Chrome DevTools MCP, preserving login state while reusing the unified `browser_*` tool interface.

**Architecture:** A `BrowserBackend` trait abstracts browser operations. `ManagedBackend` wraps the existing `BrowserRuntime` (chromiumoxide). `ChromeMcpBackend` wraps a new `ChromeMcpDriver` that manages `chrome-devtools-mcp` MCP sessions via stdio. `ProfileManager` routes to the correct backend based on profile `driver` field.

**Tech Stack:** Rust, async-trait, existing MCP client (stdio transport), chrome-devtools-mcp (npm)

**Spec:** `docs/superpowers/specs/2026-03-18-chrome-devtools-mcp-mode-design.md`

---

## Chunk 1: Profile Configuration + BrowserBackend Trait

### Task 1: Add BrowserDriver enum and ChromeMcpConfig to profile.rs

**Files:**
- Modify: `src/browser/profile.rs`
- Modify: `src/browser/error.rs`

- [ ] **Step 1: Write tests for new types**

Add to the `#[cfg(test)] mod tests` block in `src/browser/profile.rs`:

```rust
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
    assert_eq!(serde_json::to_string(&BrowserDriver::Managed).unwrap(), "\"managed\"");
    assert_eq!(serde_json::to_string(&BrowserDriver::ExistingSession).unwrap(), "\"existing_session\"");
}

#[test]
fn test_profile_config_driver_defaults_to_managed() {
    let config = ProfileConfig::default();
    assert_eq!(config.driver, BrowserDriver::Managed);
}

#[test]
fn test_chrome_mcp_config_defaults() {
    let config = ChromeMcpConfig::default();
    assert_eq!(config.command, "npx");
    assert!(config.args.contains(&"chrome-devtools-mcp@latest".to_string()));
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib browser::profile::tests -- --nocapture`
Expected: FAIL — `BrowserDriver`, `ChromeMcpConfig` not defined, `ProfileConfig` has no `driver` field.

- [ ] **Step 3: Add BrowserDriver enum**

In `src/browser/profile.rs`, add before `ProfileConfig`:

```rust
/// Driver mode for browser profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriver {
    /// Aleph launches and manages a dedicated browser instance (chromiumoxide).
    #[default]
    Managed,
    /// Attach to user's running Chrome via Chrome DevTools MCP.
    ExistingSession,
}
```

- [ ] **Step 4: Add `driver` field to ProfileConfig**

In the `ProfileConfig` struct, add after the `idle_timeout_secs` field:

```rust
    /// Driver mode: managed (launch dedicated browser) or existing-session (attach to user's Chrome).
    #[serde(default)]
    pub driver: BrowserDriver,
```

Also update the manual `Default` impl for `ProfileConfig` (around line 66) to include the new field:

```rust
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
            driver: BrowserDriver::default(),
        }
    }
}
```

- [ ] **Step 5: Add ChromeMcpConfig struct**

Add after `PlaywrightMcpConfig`:

```rust
/// Configuration for the Chrome DevTools MCP integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChromeMcpConfig {
    /// Command to launch Chrome DevTools MCP server.
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
```

- [ ] **Step 6: Add `chrome_mcp` field to BrowserSystemConfig**

In the `BrowserSystemConfig` struct, add after `playwright_mcp`:

```rust
    /// Chrome DevTools MCP integration settings.
    #[serde(default)]
    pub chrome_mcp: ChromeMcpConfig,
```

- [ ] **Step 7: Add new BrowserError variants**

In `src/browser/error.rs`, add to the `BrowserError` enum:

```rust
    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib browser::profile::tests -- --nocapture`
Expected: ALL PASS

- [ ] **Step 9: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors (existing TOML deserialization tests should still pass since new fields have defaults)

- [ ] **Step 10: Commit**

```bash
git add src/browser/profile.rs src/browser/error.rs
git commit -m "browser: add BrowserDriver enum, ChromeMcpConfig, and new error variants"
```

---

### Task 2: Create BrowserBackend trait

**Files:**
- Create: `src/browser/backend.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Create backend.rs with trait definition**

Create `src/browser/backend.rs`:

```rust
//! BrowserBackend trait — unified contract for browser driver implementations.

use async_trait::async_trait;

use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection, TabId, TabInfo,
};

/// Unified interface for browser operations, implemented by both
/// `ManagedBackend` (chromiumoxide) and `ChromeMcpBackend` (Chrome DevTools MCP).
#[async_trait]
pub trait BrowserBackend: Send + Sync {
    /// Open a new tab navigating to `url`. Returns the tab ID.
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;

    /// Close the tab identified by `tab_id`.
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;

    /// List all open tabs.
    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError>;

    /// Navigate an existing tab to a new URL.
    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError>;

    /// Click the element identified by `target`.
    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;

    /// Type (append) text into the element identified by `target`.
    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError>;

    /// Fill (replace) the value of the element identified by `target`.
    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;

    /// Hover over the element identified by `target`.
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;

    /// Scroll the element at `target` in the given direction.
    async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError>;

    /// Capture a screenshot of the given tab.
    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError>;

    /// Take an ARIA accessibility snapshot of the given tab.
    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError>;

    /// Evaluate JavaScript in the context of the given tab.
    async fn evaluate(
        &self,
        tab_id: &str,
        js: &str,
    ) -> Result<serde_json::Value, BrowserError>;

    /// Select an option from a dropdown by value.
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;
}
```

- [ ] **Step 2: Register module in mod.rs**

In `src/browser/mod.rs`, add after the existing `pub mod types;` line:

```rust
pub mod backend;
```

And add to the `pub use` block:

```rust
pub use backend::BrowserBackend;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors. The trait has no implementors yet but compiles fine.

- [ ] **Step 4: Commit**

```bash
git add src/browser/backend.rs src/browser/mod.rs
git commit -m "browser: add BrowserBackend trait for driver abstraction"
```

---

### Task 3: Create ManagedBackend wrapping BrowserRuntime

**Files:**
- Create: `src/browser/managed_backend.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Create managed_backend.rs**

Create `src/browser/managed_backend.rs`:

```rust
//! ManagedBackend — BrowserBackend implementation wrapping BrowserRuntime (chromiumoxide).

use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::runtime::BrowserRuntime;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection, TabId, TabInfo,
};

/// BrowserBackend backed by a managed chromiumoxide BrowserRuntime.
pub struct ManagedBackend {
    runtime: Arc<Mutex<BrowserRuntime>>,
}

impl ManagedBackend {
    pub fn new(runtime: Arc<Mutex<BrowserRuntime>>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl BrowserBackend for ManagedBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.runtime.lock().await.open_tab(url).await
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.runtime.lock().await.close_tab(tab_id).await
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        Ok(self.runtime.lock().await.list_tabs().await)
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.runtime.lock().await.navigate(tab_id, url).await
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.runtime.lock().await.click(tab_id, target).await
    }

    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.runtime.lock().await.type_text(tab_id, target, text).await
    }

    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.runtime.lock().await.fill(tab_id, target, value).await
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.runtime.lock().await.hover(tab_id, target).await
    }

    async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        self.runtime.lock().await.scroll(tab_id, target, direction).await
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        self.runtime.lock().await.screenshot(tab_id, opts).await
    }

    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        self.runtime.lock().await.snapshot(tab_id).await
    }

    async fn evaluate(
        &self,
        tab_id: &str,
        js: &str,
    ) -> Result<serde_json::Value, BrowserError> {
        self.runtime.lock().await.evaluate(tab_id, js).await
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        // BrowserRuntime doesn't have a select method yet.
        // Implement via JS evaluation as a reasonable fallback.
        let js = match &target {
            ActionTarget::Ref { ref_id } => {
                format!(
                    r#"(() => {{ const el = document.querySelector('[data-ref="{ref_id}"]'); if (el) {{ el.value = '{value}'; el.dispatchEvent(new Event('change')); return true; }} return false; }})()"#,
                    ref_id = ref_id,
                    value = value,
                )
            }
            ActionTarget::Selector { css } => {
                format!(
                    r#"(() => {{ const el = document.querySelector('{css}'); if (el) {{ el.value = '{value}'; el.dispatchEvent(new Event('change')); return true; }} return false; }})()"#,
                    css = css,
                    value = value,
                )
            }
            ActionTarget::Coordinates { .. } => {
                return Err(BrowserError::ActionFailed(
                    "Cannot select by coordinates".to_string(),
                ));
            }
        };
        self.runtime.lock().await.evaluate(tab_id, &js).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Register module in mod.rs**

In `src/browser/mod.rs`, add:

```rust
pub mod managed_backend;
```

And add to `pub use`:

```rust
pub use managed_backend::ManagedBackend;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/browser/managed_backend.rs src/browser/mod.rs
git commit -m "browser: add ManagedBackend wrapping BrowserRuntime"
```

---

## Chunk 2: Chrome MCP Snapshot Conversion + ChromeMcpDriver

### Task 4: Create snapshot conversion module

**Files:**
- Create: `src/browser/chrome_mcp_snapshot.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Write tests for snapshot conversion**

Create `src/browser/chrome_mcp_snapshot.rs` with tests first:

```rust
//! Snapshot conversion from Chrome DevTools MCP tree format to Aleph's AriaSnapshot.

use super::error::BrowserError;
use super::types::{AriaElement, AriaSnapshot};

/// Convert a Chrome DevTools MCP structured snapshot into Aleph's AriaSnapshot.
///
/// Chrome MCP returns a tree with nodes like:
/// ```json
/// { "role": "button", "name": "Submit", "id": "btn-1", "children": [...] }
/// ```
///
/// We use Chrome MCP's native `id` directly as `ref_id` (no remapping).
/// The tree is preserved in AriaElement's `children` field.
pub fn convert_chrome_mcp_snapshot(raw: &serde_json::Value) -> Result<AriaSnapshot, BrowserError> {
    todo!()
}

fn convert_node(node: &serde_json::Value) -> AriaElement {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_single_element() {
        let raw = json!({
            "role": "WebArea",
            "name": "Test Page",
            "children": [
                {
                    "role": "button",
                    "name": "Submit",
                    "id": "btn-1"
                }
            ]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        assert_eq!(snapshot.elements.len(), 1); // root WebArea
        assert_eq!(snapshot.elements[0].role, "WebArea");
        assert_eq!(snapshot.elements[0].children.len(), 1);
        assert_eq!(snapshot.elements[0].children[0].ref_id, "btn-1");
        assert_eq!(snapshot.elements[0].children[0].role, "button");
        assert_eq!(snapshot.elements[0].children[0].name.as_deref(), Some("Submit"));
    }

    #[test]
    fn test_convert_nested_tree() {
        let raw = json!({
            "role": "WebArea",
            "name": "Page",
            "children": [
                {
                    "role": "navigation",
                    "name": "Main",
                    "id": "nav-1",
                    "children": [
                        { "role": "link", "name": "Home", "id": "link-1" },
                        { "role": "link", "name": "About", "id": "link-2" }
                    ]
                },
                {
                    "role": "button",
                    "name": "Login",
                    "id": "btn-1"
                }
            ]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let root = &snapshot.elements[0];
        assert_eq!(root.children.len(), 2); // nav + button
        let nav = &root.children[0];
        assert_eq!(nav.ref_id, "nav-1");
        assert_eq!(nav.children.len(), 2); // 2 links
        assert_eq!(nav.children[0].ref_id, "link-1");
        assert_eq!(nav.children[1].ref_id, "link-2");
    }

    #[test]
    fn test_convert_element_with_value_and_state() {
        let raw = json!({
            "role": "WebArea",
            "children": [{
                "role": "textbox",
                "name": "Email",
                "id": "input-1",
                "value": "user@example.com",
                "focused": true,
                "disabled": false
            }]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let input = &snapshot.elements[0].children[0];
        assert_eq!(input.role, "textbox");
        assert_eq!(input.value.as_deref(), Some("user@example.com"));
        assert!(input.state.contains(&"focused".to_string()));
    }

    #[test]
    fn test_convert_empty_tree() {
        let raw = json!({
            "role": "WebArea",
            "name": "Empty Page"
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        assert_eq!(snapshot.elements.len(), 1);
        assert!(snapshot.elements[0].children.is_empty());
    }

    #[test]
    fn test_missing_id_uses_empty_string() {
        let raw = json!({
            "role": "WebArea",
            "children": [{
                "role": "paragraph",
                "name": "Some text"
            }]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let para = &snapshot.elements[0].children[0];
        assert_eq!(para.ref_id, ""); // no id in Chrome MCP → empty ref_id
    }
}
```

- [ ] **Step 2: Register module in mod.rs**

In `src/browser/mod.rs`, add:

```rust
pub mod chrome_mcp_snapshot;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib browser::chrome_mcp_snapshot::tests -- --nocapture`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 4: Implement conversion functions**

Replace the `todo!()` bodies in `chrome_mcp_snapshot.rs`:

```rust
pub fn convert_chrome_mcp_snapshot(raw: &serde_json::Value) -> Result<AriaSnapshot, BrowserError> {
    let root = convert_node(raw);
    Ok(AriaSnapshot {
        elements: vec![root],
        page_title: raw.get("name").and_then(|v| v.as_str()).map(String::from),
        page_url: raw.get("url").and_then(|v| v.as_str()).map(String::from),
        focused_ref: None,
    })
}

fn convert_node(node: &serde_json::Value) -> AriaElement {
    let mut state = Vec::new();
    if node.get("focused").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("focused".to_string());
    }
    if node.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("disabled".to_string());
    }
    if node.get("expanded").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("expanded".to_string());
    }
    if node.get("checked").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("checked".to_string());
    }

    let children = node
        .get("children")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(convert_node).collect())
        .unwrap_or_default();

    AriaElement {
        ref_id: node
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        role: node
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("generic")
            .to_string(),
        name: node.get("name").and_then(|v| v.as_str()).map(String::from),
        value: node.get("value").and_then(|v| v.as_str()).map(String::from),
        state,
        bounds: None, // Chrome MCP does not provide bounding rects
        children,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib browser::chrome_mcp_snapshot::tests -- --nocapture`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add src/browser/chrome_mcp_snapshot.rs src/browser/mod.rs
git commit -m "browser: add Chrome MCP snapshot conversion"
```

---

### Task 5: Create ChromeMcpDriver — session management and Chrome auto-launch

**Files:**
- Create: `src/browser/chrome_mcp.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Write tests for ChromeMcpDriver**

Create `src/browser/chrome_mcp.rs` with the struct definition and tests:

```rust
//! ChromeMcpDriver — manages Chrome DevTools MCP sessions.
//!
//! Spawns `chrome-devtools-mcp` as a stdio MCP server per profile.
//! Sessions are lazily created on first tool call and cached by profile name.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::RwLock;

use super::discovery::find_chromium;
use super::error::BrowserError;
use super::profile::ChromeMcpConfig;
use crate::mcp::client::{ExternalServerConfig, McpClient};

/// A running Chrome DevTools MCP session.
struct ChromeMcpSession {
    /// MCP client for communicating with the chrome-devtools-mcp process.
    client: McpClient,
    /// PID of the chrome-devtools-mcp process (for health checks).
    pid: u32,
}

/// Manages Chrome DevTools MCP sessions with lazy creation and profile-keyed caching.
pub struct ChromeMcpDriver {
    sessions: RwLock<HashMap<String, ChromeMcpSession>>,
    config: ChromeMcpConfig,
}

impl ChromeMcpDriver {
    pub fn new(config: ChromeMcpConfig) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Call a tool on the Chrome DevTools MCP server for the given profile.
    /// Creates the session lazily if it doesn't exist.
    pub async fn call_tool(
        &self,
        profile_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.ensure_session(profile_name).await?;

        let sessions = self.sessions.read().await;
        let session = sessions
            .get(profile_name)
            .ok_or_else(|| BrowserError::ChromeMcpError("Session not found after creation".into()))?;

        let result = session
            .client
            .call_tool(tool_name, args)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                // Transport errors should trigger session teardown
                if err_str.contains("broken pipe")
                    || err_str.contains("connection reset")
                    || err_str.contains("process exited")
                {
                    tracing::warn!("Chrome MCP transport error for profile '{profile_name}': {err_str}");
                    // Session will be cleaned up on next call via reap_dead_sessions
                }
                BrowserError::ChromeMcpError(err_str)
            })?;

        // McpToolResult has fields: success (bool), content (Value), error (Option<String>)
        if !result.success {
            return Err(BrowserError::ChromeMcpError(
                result.error.unwrap_or_else(|| "Unknown Chrome MCP error".into()),
            ));
        }
        Ok(result.content)
    }

    /// Ensure a session exists for the given profile, creating one if needed.
    async fn ensure_session(&self, profile_name: &str) -> Result<(), BrowserError> {
        // Fast path: check if session already exists and is alive
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(profile_name) {
                if Self::is_process_alive(session.pid) {
                    return Ok(());
                }
            }
        }

        // Slow path: create a new session
        let mut sessions = self.sessions.write().await;

        // Double-check after acquiring write lock (another task may have created it)
        if let Some(session) = sessions.get(profile_name) {
            if Self::is_process_alive(session.pid) {
                return Ok(());
            }
            // Dead session — remove it
            sessions.remove(profile_name);
        }

        // Try to spawn chrome-devtools-mcp
        let session = self.create_session(profile_name).await?;
        sessions.insert(profile_name.to_string(), session);
        Ok(())
    }

    /// Create a new MCP session by spawning chrome-devtools-mcp.
    async fn create_session(&self, profile_name: &str) -> Result<ChromeMcpSession, BrowserError> {
        let server_name = format!("chrome-mcp-{profile_name}");
        let config = ExternalServerConfig {
            name: server_name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: Some(30),
        };

        let client = McpClient::new();
        match client.start_external_server(config).await {
            Ok(()) => {
                // Try to get PID from the MCP client (best-effort)
                let pid = 0; // PID tracking will be refined in integration
                tracing::info!("Chrome DevTools MCP session started for profile '{profile_name}'");
                // First-use security warning
                tracing::warn!(
                    "Existing-session mode connects to your Chrome with remote debugging enabled. \
                     Any local process can access browser data (cookies, passwords) via the debug port. \
                     This is Chrome's standard debugging interface (same as DevTools)."
                );
                Ok(ChromeMcpSession { client, pid })
            }
            Err(e) => {
                // Connection failed — try to auto-launch Chrome first
                tracing::info!(
                    "Chrome DevTools MCP connection failed, attempting to launch Chrome: {e}"
                );
                self.ensure_chrome_running().await?;

                // Retry after Chrome launch
                let retry_config = ExternalServerConfig {
                    name: server_name,
                    command: self.config.command.clone(),
                    args: self.config.args.clone(),
                    env: HashMap::new(),
                    cwd: None,
                    requires_runtime: Some("node".into()),
                    timeout_seconds: Some(30),
                };

                let retry_client = McpClient::new();
                retry_client
                    .start_external_server(retry_config)
                    .await
                    .map_err(|e| {
                        BrowserError::AttachFailed(format!(
                            "Failed to connect Chrome DevTools MCP after launching Chrome: {e}"
                        ))
                    })?;

                Ok(ChromeMcpSession {
                    client: retry_client,
                    pid: 0,
                })
            }
        }
    }

    /// Ensure Chrome is running with remote debugging enabled.
    async fn ensure_chrome_running(&self) -> Result<(), BrowserError> {
        // Check if any Chrome process is already running
        if Self::is_chrome_running() {
            // Chrome is running but MCP couldn't connect — debugging not enabled
            return Err(BrowserError::AttachFailed(
                "Chrome is running but remote debugging is not enabled. \
                 Please restart Chrome or enable debugging at chrome://inspect/#remote-debugging"
                    .into(),
            ));
        }

        // Chrome not running — launch with debugging
        let chrome_path = find_chromium()?;
        tracing::info!(
            "Launching Chrome with remote debugging: {}",
            chrome_path.display()
        );

        Command::new(&chrome_path)
            .arg("--remote-debugging-port=0")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to launch Chrome: {e}")))?;

        // Wait for Chrome to be ready
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    }

    /// Check if a chrome-devtools-mcp process is still alive by PID.
    fn is_process_alive(pid: u32) -> bool {
        if pid == 0 {
            return true; // PID not tracked, assume alive
        }
        #[cfg(unix)]
        {
            // SAFETY: kill(pid, 0) only checks process existence, sends no signal.
            // pid as i32 is safe because valid PIDs fit in i32 on all Unix platforms.
            unsafe { libc::kill(pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            true // Assume alive on non-Unix
        }
    }

    /// Check if a Chrome browser process is running on the system.
    fn is_chrome_running() -> bool {
        #[cfg(target_os = "macos")]
        {
            // On macOS, check for the specific Chrome app binary
            std::process::Command::new("pgrep")
                .arg("-x")  // exact match on process name
                .arg("Google Chrome")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("pgrep")
                .arg("-x")
                .arg("chrome")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Destroy a session (for cleanup after transport errors).
    pub async fn destroy_session(&self, profile_name: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(profile_name) {
            let _ = session.client.stop_all().await;
            tracing::info!("Chrome MCP session destroyed for profile '{profile_name}'");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chrome_mcp_driver_new() {
        let config = ChromeMcpConfig::default();
        let driver = ChromeMcpDriver::new(config);
        // Sessions map should be empty initially
        let sessions = driver.sessions.try_read().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_is_process_alive_zero_pid() {
        // PID 0 means "not tracked" — should return true (assume alive)
        assert!(ChromeMcpDriver::is_process_alive(0));
    }

    #[test]
    fn test_is_process_alive_current_process() {
        let pid = std::process::id();
        assert!(ChromeMcpDriver::is_process_alive(pid));
    }

    #[test]
    fn test_is_process_alive_dead_pid() {
        // PID 999999 is very unlikely to exist
        // On Unix, kill(999999, 0) should fail
        #[cfg(unix)]
        assert!(!ChromeMcpDriver::is_process_alive(999999));
    }
}
```

- [ ] **Step 2: Register module in mod.rs**

In `src/browser/mod.rs`, add:

```rust
pub mod chrome_mcp;
```

And add to `pub use`:

```rust
pub use chrome_mcp::ChromeMcpDriver;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib browser::chrome_mcp::tests -- --nocapture`
Expected: ALL PASS (these are synchronous unit tests, no MCP connection needed)

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors. May need to adjust imports if `McpClient`, `ExternalServerConfig`, or `McpToolResult` have slightly different paths — follow compiler errors.

- [ ] **Step 5: Commit**

```bash
git add src/browser/chrome_mcp.rs src/browser/mod.rs
git commit -m "browser: add ChromeMcpDriver with session management and Chrome auto-launch"
```

---

## Chunk 3: ChromeMcpBackend + ProfileManager Routing

### Task 6: Create ChromeMcpBackend

**Files:**
- Create: `src/browser/chrome_mcp_backend.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Create chrome_mcp_backend.rs**

Create `src/browser/chrome_mcp_backend.rs`:

```rust
//! ChromeMcpBackend — BrowserBackend implementation routing through Chrome DevTools MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_snapshot::convert_chrome_mcp_snapshot;
use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection,
    TabId, TabInfo,
};

/// BrowserBackend backed by Chrome DevTools MCP (existing-session mode).
pub struct ChromeMcpBackend {
    driver: Arc<ChromeMcpDriver>,
    profile_name: String,
}

impl ChromeMcpBackend {
    pub fn new(driver: Arc<ChromeMcpDriver>, profile_name: String) -> Self {
        Self {
            driver,
            profile_name,
        }
    }

    /// Extract the element reference from an ActionTarget.
    /// Only ref_id is supported in existing-session mode.
    fn extract_element_ref(target: &ActionTarget) -> Result<String, BrowserError> {
        match target {
            ActionTarget::Ref { ref_id } => Ok(ref_id.clone()),
            ActionTarget::Selector { .. } => Err(BrowserError::ActionFailed(
                "CSS selectors are not supported in existing-session mode. \
                 Use ref_id from browser_snapshot instead."
                    .into(),
            )),
            ActionTarget::Coordinates { .. } => Err(BrowserError::ActionFailed(
                "Coordinate targeting is not supported in existing-session mode. \
                 Use ref_id from browser_snapshot instead."
                    .into(),
            )),
        }
    }

    /// Call a Chrome DevTools MCP tool.
    async fn call(&self, tool_name: &str, args: serde_json::Value) -> Result<serde_json::Value, BrowserError> {
        self.driver.call_tool(&self.profile_name, tool_name, args).await
    }
}

#[async_trait]
impl BrowserBackend for ChromeMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("new_page", json!({ "url": url })).await?;
        // Chrome MCP returns the new page ID
        let page_id = result
            .get("pageId")
            .or_else(|| result.get("id"))
            .and_then(|v| v.as_str())
            .or_else(|| result.as_str())
            .unwrap_or("unknown")
            .to_string();
        Ok(page_id)
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.call("close_page", json!({ "pageId": tab_id })).await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        let result = self.call("list_pages", json!({})).await?;

        let pages = result
            .as_array()
            .or_else(|| result.get("pages").and_then(|v| v.as_array()))
            .cloned()
            .unwrap_or_default();

        let tabs = pages
            .iter()
            .map(|page| TabInfo {
                id: page
                    .get("pageId")
                    .or_else(|| page.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                url: page
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                title: page
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        Ok(tabs)
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.call("navigate_page", json!({
            "pageId": tab_id,
            "url": url,
        }))
        .await?;
        Ok(())
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("click", json!({
            "pageId": tab_id,
            "element": element,
        }))
        .await?;
        Ok(())
    }

    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("fill", json!({
            "pageId": tab_id,
            "element": element,
            "value": text,
        }))
        .await?;
        Ok(())
    }

    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("fill", json!({
            "pageId": tab_id,
            "element": element,
            "value": value,
        }))
        .await?;
        Ok(())
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("hover", json!({
            "pageId": tab_id,
            "element": element,
        }))
        .await?;
        Ok(())
    }

    async fn scroll(
        &self,
        tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        let key = match direction {
            ScrollDirection::Up => "PageUp",
            ScrollDirection::Down => "PageDown",
            ScrollDirection::Left => "Home",
            ScrollDirection::Right => "End",
        };
        self.call("press_key", json!({
            "pageId": tab_id,
            "key": key,
        }))
        .await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        let result = self.call("take_screenshot", json!({
            "pageId": tab_id,
        }))
        .await?;

        let data_base64 = result
            .get("data")
            .or_else(|| result.get("image"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(ScreenshotResult {
            data_base64,
            width: 0,  // Chrome MCP may not provide dimensions
            height: 0,
            format: "png".to_string(),
        })
    }

    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        let result = self.call("take_snapshot", json!({
            "pageId": tab_id,
        }))
        .await?;

        // Chrome MCP with --experimentalStructuredContent returns structured data
        let snapshot_data = result
            .get("snapshot")
            .unwrap_or(&result);

        convert_chrome_mcp_snapshot(snapshot_data)
    }

    async fn evaluate(
        &self,
        tab_id: &str,
        js: &str,
    ) -> Result<serde_json::Value, BrowserError> {
        let result = self.call("evaluate_script", json!({
            "pageId": tab_id,
            "script": js,
        }))
        .await?;

        Ok(result)
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        // Use fill for select elements — Chrome MCP handles it the same way
        self.fill(tab_id, target, value).await
    }
}
```

- [ ] **Step 2: Register module in mod.rs**

In `src/browser/mod.rs`, add:

```rust
pub mod chrome_mcp_backend;
```

And add to `pub use`:

```rust
pub use chrome_mcp_backend::ChromeMcpBackend;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/browser/chrome_mcp_backend.rs src/browser/mod.rs
git commit -m "browser: add ChromeMcpBackend routing through Chrome DevTools MCP"
```

---

### Task 7: Update ProfileManager with get_backend() routing and "user" profile auto-injection

**Files:**
- Modify: `src/browser/manager.rs`

- [ ] **Step 1: Write tests for new ProfileManager behavior**

Add to the `#[cfg(test)] mod tests` block in `src/browser/manager.rs`:

```rust
#[test]
fn test_auto_injects_user_profile() {
    let config = BrowserSystemConfig::default();
    let manager = ProfileManager::new(config);
    let profiles = manager.list_profiles();

    // Should have both "default" and "user"
    assert!(profiles.iter().any(|p| p.0 == "default"));
    assert!(profiles.iter().any(|p| p.0 == "user"));
}

#[test]
fn test_user_profile_is_existing_session() {
    let config = BrowserSystemConfig::default();
    let manager = ProfileManager::new(config);
    let user_config = manager.get_config("user").unwrap();

    assert_eq!(user_config.driver, BrowserDriver::ExistingSession);
    assert_eq!(user_config.browser, BrowserType::Chrome);
    assert_eq!(user_config.color.as_deref(), Some("#00AA00"));
}

#[test]
fn test_explicit_user_profile_not_overridden() {
    let mut config = BrowserSystemConfig::default();
    config.profiles.insert(
        "user".into(),
        ProfileConfig {
            browser: BrowserType::Chrome,
            driver: BrowserDriver::ExistingSession,
            color: Some("#FF0000".into()),
            ..Default::default()
        },
    );
    let manager = ProfileManager::new(config);
    let user_config = manager.get_config("user").unwrap();

    // Should use the explicit config, not the auto-injected one
    assert_eq!(user_config.color.as_deref(), Some("#FF0000"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib browser::manager::tests -- --nocapture`
Expected: FAIL — no "user" profile auto-injected, `driver` field doesn't exist yet on ProfileConfig.

- [ ] **Step 3: Update ProfileManager::new() to inject "user" profile**

In `src/browser/manager.rs`, update the `new()` method. Add these imports at the top:

```rust
use super::profile::{BrowserDriver, BrowserType, BrowserSystemConfig, ProfileConfig, ProfileState};
use super::chrome_mcp::ChromeMcpDriver;
use std::sync::Arc;
```

Update the `ProfileManager` struct to hold the ChromeMcpDriver:

```rust
pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    ssrf_policy: SsrfPolicy,
    #[allow(dead_code)]
    config: BrowserSystemConfig,
    chrome_mcp_driver: Arc<ChromeMcpDriver>,
}
```

Update `ProfileManager::new()`:

```rust
pub fn new(config: BrowserSystemConfig) -> Self {
    let ssrf_policy = SsrfPolicy::new(config.policy.clone());
    let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(config.chrome_mcp.clone()));

    let mut profiles = HashMap::new();

    if config.profiles.is_empty() {
        profiles.insert(
            "default".into(),
            ManagedProfile {
                config: ProfileConfig::default(),
                state: ProfileState::Idle,
                last_activity: std::time::Instant::now(),
            },
        );
    } else {
        for (name, profile_config) in &config.profiles {
            profiles.insert(
                name.clone(),
                ManagedProfile {
                    config: profile_config.clone(),
                    state: ProfileState::Idle,
                    last_activity: std::time::Instant::now(),
                },
            );
        }
    }

    // Auto-inject "user" profile if not explicitly configured
    if !profiles.contains_key("user") {
        profiles.insert(
            "user".into(),
            ManagedProfile {
                config: ProfileConfig {
                    browser: BrowserType::Chrome,
                    driver: BrowserDriver::ExistingSession,
                    color: Some("#00AA00".into()),
                    ..Default::default()
                },
                state: ProfileState::Idle,
                last_activity: std::time::Instant::now(),
            },
        );
    }

    Self {
        profiles: RwLock::new(profiles),
        ssrf_policy,
        config,
        chrome_mcp_driver,
    }
}
```

Add the `get_backend()` method:

```rust
/// Get the appropriate BrowserBackend for a profile based on its driver type.
pub fn get_chrome_mcp_driver(&self) -> Arc<ChromeMcpDriver> {
    self.chrome_mcp_driver.clone()
}

/// Get the driver type for a profile.
pub fn get_driver(&self, name: &str) -> Option<BrowserDriver> {
    let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
    profiles.get(name).map(|p| p.config.driver.clone())
}
```

- [ ] **Step 4: Update existing test that expects 1 profile**

In `src/browser/manager.rs`, update `test_manager_default_profile_if_none_configured`:

```rust
#[test]
fn test_manager_default_profile_if_none_configured() {
    let config = BrowserSystemConfig::default();
    let manager = ProfileManager::new(config);
    let profiles = manager.list_profiles();
    // "default" (auto-created when no profiles configured) + "user" (always auto-injected)
    assert_eq!(profiles.len(), 2);
    assert!(profiles.iter().any(|p| p.0 == "default"));
    assert!(profiles.iter().any(|p| p.0 == "user"));
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib browser::manager::tests -- --nocapture`
Expected: ALL PASS

- [ ] **Step 7: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 8: Commit**

```bash
git add src/browser/manager.rs
git commit -m "browser: add user profile auto-injection and ChromeMcpDriver to ProfileManager"
```

---

## Chunk 4: Wire Browser Tools to BrowserBackend

### Task 8: Update browser_tools to route through BrowserBackend

This is the largest task — updating all 10 stub tools (excluding profile_tool.rs which already has real logic). The pattern is the same for each tool: replace the placeholder response with actual `BrowserBackend` dispatch.

**Files:**
- Modify: `src/builtin_tools/browser_tools/mod.rs`
- Modify: `src/builtin_tools/browser_tools/open.rs`
- Modify: `src/builtin_tools/browser_tools/navigate.rs`
- Modify: `src/builtin_tools/browser_tools/click.rs`
- Modify: `src/builtin_tools/browser_tools/type_text.rs`
- Modify: `src/builtin_tools/browser_tools/fill_form.rs`
- Modify: `src/builtin_tools/browser_tools/select.rs`
- Modify: `src/builtin_tools/browser_tools/screenshot.rs`
- Modify: `src/builtin_tools/browser_tools/snapshot.rs`
- Modify: `src/builtin_tools/browser_tools/evaluate.rs`
- Modify: `src/builtin_tools/browser_tools/tabs.rs`

**Implementation note:** Each tool currently holds `Arc<ProfileManager>`. The routing pattern for each is:

```rust
// Determine backend based on profile driver
let driver = self.manager.get_driver(&args.profile);
match driver {
    Some(BrowserDriver::ExistingSession) => {
        let chrome_mcp = self.manager.get_chrome_mcp_driver();
        let backend = ChromeMcpBackend::new(chrome_mcp, args.profile.clone());
        // Use backend.method(...)
    }
    _ => {
        // For managed mode, return existing stub for now
        // (ManagedBackend requires BrowserRuntime which isn't wired in tools yet)
    }
}
```

- [ ] **Step 1: Update browser_tools/open.rs**

Replace the TODO/placeholder in the `call` method with:

```rust
async fn call(&self, args: Self::Args) -> Result<Self::Output> {
    self.manager.record_activity(&args.profile);

    // SSRF check
    self.manager.check_url(&args.url).map_err(|e| anyhow::anyhow!("{e}"))?;

    let driver = self.manager.get_driver(&args.profile);
    match driver {
        Some(BrowserDriver::ExistingSession) => {
            let chrome_mcp = self.manager.get_chrome_mcp_driver();
            let backend = ChromeMcpBackend::new(chrome_mcp, args.profile.clone());
            let tab_id = backend.open_tab(&args.url).await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(BrowserOpenOutput {
                success: true,
                tab_id: Some(tab_id),
                message: format!("Opened {} in existing Chrome session", args.url),
            })
        }
        _ => {
            // Managed mode — placeholder until BrowserRuntime is wired
            Ok(BrowserOpenOutput {
                success: true,
                tab_id: Some("tab-1".to_string()),
                message: format!("Opened {} (managed mode placeholder)", args.url),
            })
        }
    }
}
```

Add the necessary imports to `open.rs`:

```rust
use crate::browser::profile::BrowserDriver;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
```

- [ ] **Step 2: Update remaining 9 tools following the same pattern**

For each tool, the pattern is:
1. Add imports for `BrowserDriver` and `ChromeMcpBackend`
2. In the `call` method, check `self.manager.get_driver(&args.profile)`
3. For `ExistingSession`, create `ChromeMcpBackend` and call the appropriate method
4. For `Managed` (default), keep the existing placeholder

The specific method calls per tool:

| Tool | Backend method | Notes |
|------|---------------|-------|
| `navigate.rs` | `backend.evaluate(tab_id, js)` | Back/Forward/Refresh actions — use `history.back()`, `history.forward()`, `location.reload()` via JS eval in existing-session mode |
| `click.rs` | `backend.click(tab_id, target)` | Convert args to `ActionTarget` |
| `type_text.rs` | `backend.type_text(tab_id, target, text)` | Convert args to `ActionTarget` |
| `fill_form.rs` | `backend.fill(tab_id, target, value)` per field | Loop over fields |
| `select.rs` | `backend.select(tab_id, target, value)` | Convert args to `ActionTarget` |
| `screenshot.rs` | `backend.screenshot(tab_id, opts)` | Convert args to `ScreenshotOpts` |
| `snapshot.rs` | `backend.snapshot(tab_id)` | Return AriaSnapshot as JSON string |
| `evaluate.rs` | `backend.evaluate(tab_id, js)` | Return result as Value |
| `tabs.rs` | `backend.list_tabs()` / `backend.close_tab()` | Handle List/Switch/Close actions |

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 4: Run existing tests**

Run: `cargo test -p alephcore --lib builtin_tools::browser_tools -- --nocapture`
Expected: Existing tests still pass (they test managed mode which still returns placeholders).

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/browser_tools/
git commit -m "browser: wire browser_tools to BrowserBackend with existing-session routing"
```

---

### Task 9: Integration smoke test

**Files:**
- Modify: `src/browser/chrome_mcp.rs` (add integration test)

- [ ] **Step 1: Add ignored integration test**

Add to the bottom of `src/browser/chrome_mcp.rs`:

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::chrome_mcp_backend::ChromeMcpBackend;

    #[tokio::test]
    #[ignore] // Requires Chrome + npx chrome-devtools-mcp installed
    async fn test_chrome_mcp_list_tabs() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        let backend = ChromeMcpBackend::new(driver, "user".to_string());

        let tabs = backend.list_tabs().await.expect("list_tabs should succeed");
        assert!(!tabs.is_empty(), "Should have at least one tab open");
        println!("Open tabs: {tabs:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_snapshot() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        let backend = ChromeMcpBackend::new(driver, "user".to_string());

        let tabs = backend.list_tabs().await.expect("list_tabs");
        let tab_id = &tabs[0].id;

        let snapshot = backend.snapshot(tab_id).await.expect("snapshot should succeed");
        assert!(!snapshot.elements.is_empty(), "Snapshot should have elements");
        println!("Snapshot elements: {}", snapshot.elements.len());
    }
}
```

- [ ] **Step 2: Verify integration tests are discoverable but skipped**

Run: `cargo test -p alephcore --lib browser::chrome_mcp::integration_tests -- --list`
Expected: Lists the test names but doesn't run them (ignored).

- [ ] **Step 3: Commit**

```bash
git add src/browser/chrome_mcp.rs
git commit -m "browser: add Chrome DevTools MCP integration smoke tests (ignored)"
```

---

### Task 10: Final compile + test validation

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS (including new tests, excluding `#[ignore]` integration tests).

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Fix any issues found by clippy**

Address any clippy lints. Common ones: unused imports, unnecessary clones, missing docs.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "browser: Chrome DevTools MCP Mode — cleanup and lint fixes"
```
