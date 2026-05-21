# Headless Browser (Playwright MCP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Playwright MCP as the default headless browser backend, reserving Chrome DevTools MCP for explicit user requests.

**Architecture:** `PlaywrightMcpDriver` manages Playwright MCP sessions (identical pattern to `ChromeMcpDriver`). `PlaywrightMcpBackend` implements `BrowserBackend` trait by routing calls through Playwright MCP tools. The `Managed` driver variant triggers headless mode; `ExistingSession` keeps the existing Chrome DevTools MCP path. Default profile uses `Managed` (headless), "user" profile keeps `ExistingSession`.

**Tech Stack:** Rust, `@anthropic/mcp-playwright` (npx), existing MCP client infrastructure (`McpClient`, `ExternalServerConfig`).

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/browser/playwright_mcp.rs` | PlaywrightMcpDriver — session lifecycle, lazy creation, tool dispatch |
| Create | `src/browser/playwright_mcp_backend.rs` | PlaywrightMcpBackend — implements BrowserBackend via Playwright MCP |
| Modify | `src/browser/manager.rs` | Add `playwright_mcp_driver` field, `get_playwright_mcp_driver()`, restore default profile to `Managed` |
| Modify | `src/browser/mod.rs` | Add module declarations and re-exports |
| Modify | `src/builtin_tools/browser_tools/open.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/snapshot.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/click.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/type_text.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/fill_form.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/select.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/navigate.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/screenshot.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/evaluate.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/tabs.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Modify | `src/builtin_tools/browser_tools/profile_tool.rs` | Wire `Managed` → PlaywrightMcpBackend |
| Delete | `src/browser/playwright_bridge.rs` | Superseded by PlaywrightMcpDriver/Backend |

---

### Task 1: PlaywrightMcpDriver

Creates the session manager for Playwright MCP — same pattern as `ChromeMcpDriver` but simpler (no Chrome discovery/launch logic needed; Playwright handles its own browser).

**Files:**
- Create: `src/browser/playwright_mcp.rs`

- [ ] **Step 1: Create PlaywrightMcpDriver**

```rust
//! PlaywrightMcpDriver — manages Playwright MCP sessions.
//!
//! Spawns `@anthropic/mcp-playwright` as a stdio MCP server.
//! Sessions are lazily created on first tool call and cached.

use std::collections::HashMap;

use tokio::sync::RwLock;

use super::error::BrowserError;
use super::profile::PlaywrightMcpConfig;
use crate::mcp::{ExternalServerConfig, McpClient};

/// A running Playwright MCP session.
struct PlaywrightMcpSession {
    client: McpClient,
}

/// Manages Playwright MCP sessions with lazy creation.
pub struct PlaywrightMcpDriver {
    sessions: RwLock<HashMap<String, PlaywrightMcpSession>>,
    config: PlaywrightMcpConfig,
}

impl PlaywrightMcpDriver {
    pub fn new(config: PlaywrightMcpConfig) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Call a tool on the Playwright MCP server.
    /// Creates the session lazily if it doesn't exist.
    pub async fn call_tool(
        &self,
        session_key: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.ensure_session(session_key).await?;

        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_key)
            .ok_or_else(|| BrowserError::PlaywrightError("Session not found after creation".into()))?;

        let server_name = format!("playwright-{session_key}");
        let full_tool_name = format!("{server_name}:{tool_name}");
        let result = match session.client.call_tool(&full_tool_name, args).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                let is_transport = err_str.contains("broken pipe")
                    || err_str.contains("connection reset")
                    || err_str.contains("process exited")
                    || err_str.contains("channel closed");
                if is_transport {
                    tracing::warn!("Playwright MCP transport error for '{session_key}': {err_str}");
                    drop(sessions);
                    self.destroy_session(session_key).await;
                }
                return Err(BrowserError::PlaywrightError(err_str));
            }
        };

        if !result.success {
            return Err(BrowserError::PlaywrightError(
                result.error.unwrap_or_else(|| "Unknown Playwright MCP error".into()),
            ));
        }
        Ok(result.content)
    }

    /// Ensure a session exists, creating one if needed.
    async fn ensure_session(&self, session_key: &str) -> Result<(), BrowserError> {
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(session_key) {
                return Ok(());
            }
        }

        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(session_key) {
            return Ok(());
        }

        let session = self.create_session(session_key).await?;
        sessions.insert(session_key.to_string(), session);
        Ok(())
    }

    /// Create a new MCP session by spawning Playwright MCP.
    async fn create_session(&self, session_key: &str) -> Result<PlaywrightMcpSession, BrowserError> {
        let server_name = format!("playwright-{session_key}");
        let config = ExternalServerConfig {
            name: server_name,
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: Some(60),
        };

        let client = McpClient::new();
        client
            .start_external_server(config)
            .await
            .map_err(|e| BrowserError::PlaywrightError(format!("Failed to start Playwright MCP: {e}")))?;

        tracing::info!("Playwright MCP session started for '{session_key}'");

        Ok(PlaywrightMcpSession { client })
    }

    /// Destroy a session (for cleanup after transport errors).
    pub async fn destroy_session(&self, session_key: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(session_key) {
            let _ = session.client.stop_all().await;
            tracing::info!("Playwright MCP session destroyed for '{session_key}'");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playwright_mcp_driver_new() {
        let config = PlaywrightMcpConfig::default();
        let driver = PlaywrightMcpDriver::new(config);
        let sessions = driver.sessions.try_read().unwrap();
        assert!(sessions.is_empty());
    }
}
```

- [ ] **Step 2: Add `PlaywrightError` variant to `BrowserError`**

In `src/browser/error.rs`, add:

```rust
#[error("Playwright MCP error: {0}")]
PlaywrightError(String),
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/browser/playwright_mcp.rs src/browser/error.rs
git commit -m "browser: add PlaywrightMcpDriver for headless browser sessions"
```

---

### Task 2: PlaywrightMcpBackend

Implements `BrowserBackend` by routing calls through the Playwright MCP server. Playwright MCP tool names differ from Chrome DevTools MCP — use the Playwright MCP's actual tool names (`browser_navigate`, `browser_click`, etc. prefixed with the server name).

**Reference:** Playwright MCP tool names from `@anthropic/mcp-playwright`:
- `browser_navigate` (url)
- `browser_click` (element, ref)
- `browser_type` (element, ref, text)
- `browser_snapshot` (no args)
- `browser_screenshot` (no args, raw: bool)
- `browser_tab_list`, `browser_tab_new` (url), `browser_tab_select` (index), `browser_tab_close` (index)
- `browser_evaluate` (expression)

**Files:**
- Create: `src/browser/playwright_mcp_backend.rs`

- [ ] **Step 1: Create PlaywrightMcpBackend**

```rust
//! PlaywrightMcpBackend — BrowserBackend implementation routing through Playwright MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::playwright_mcp::PlaywrightMcpDriver;
use super::types::{
    ActionTarget, AriaElement, AriaSnapshot, ScreenshotOpts, ScreenshotResult,
    ScrollDirection, TabId, TabInfo,
};

pub struct PlaywrightMcpBackend {
    driver: Arc<PlaywrightMcpDriver>,
    session_key: String,
}

impl PlaywrightMcpBackend {
    pub fn new(driver: Arc<PlaywrightMcpDriver>, session_key: String) -> Self {
        Self { driver, session_key }
    }

    async fn call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.driver
            .call_tool(&self.session_key, tool_name, args)
            .await
    }

    /// Extract text from MCP content response.
    fn extract_text(result: &serde_json::Value) -> String {
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
            }
        }
        if let Some(s) = result.as_str() {
            return s.to_string();
        }
        result.to_string()
    }

    /// Parse Playwright snapshot text into AriaSnapshot.
    /// Playwright returns an indented text format like:
    /// ```
    /// - heading "Title" [level=1]
    /// - navigation "Main"
    ///   - link "Home" [ref=s1e3]
    /// ```
    fn parse_snapshot_text(text: &str) -> AriaSnapshot {
        let mut elements = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim().trim_start_matches("- ");
            if trimmed.is_empty() {
                continue;
            }
            // Extract role (first word), name (quoted), ref (in brackets)
            let (role, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
            let name = rest
                .find('"')
                .and_then(|start| {
                    rest[start + 1..].find('"').map(|end| &rest[start + 1..start + 1 + end])
                })
                .map(|s| s.to_string());
            let ref_id = rest
                .find("[ref=")
                .and_then(|start| {
                    rest[start + 5..].find(']').map(|end| &rest[start + 5..start + 5 + end])
                })
                .unwrap_or("")
                .to_string();

            elements.push(AriaElement {
                ref_id,
                role: role.to_string(),
                name,
                value: None,
                state: vec![],
                bounds: None,
                children: vec![],
            });
        }
        AriaSnapshot {
            elements,
            page_title: None,
            page_url: None,
            focused_ref: None,
        }
    }
}

#[async_trait]
impl BrowserBackend for PlaywrightMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("browser_tab_new", json!({ "url": url })).await?;
        // Return tab index from response, or default to "0"
        let text = Self::extract_text(&result);
        // Try to extract tab index
        Ok(text
            .lines()
            .find_map(|l| l.trim().parse::<u32>().ok())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "0".to_string()))
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let index: u32 = tab_id.parse().unwrap_or(0);
        self.call("browser_tab_close", json!({ "index": index })).await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        let result = self.call("browser_tab_list", json!({})).await?;
        let text = Self::extract_text(&result);
        // Parse "Tab N: URL (title)" format
        let mut tabs = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("Tab ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let id = rest[..colon_pos].trim().to_string();
                    let url = rest[colon_pos + 2..].trim().to_string();
                    tabs.push(TabInfo {
                        id,
                        url,
                        title: String::new(),
                    });
                }
            }
        }
        if tabs.is_empty() {
            // Fallback: single default tab
            tabs.push(TabInfo {
                id: "0".to_string(),
                url: String::new(),
                title: String::new(),
            });
        }
        Ok(tabs)
    }

    async fn navigate(&self, _tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.call("browser_navigate", json!({ "url": url })).await?;
        Ok(())
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let args = match target {
            ActionTarget::Ref { ref_id } => json!({ "element": "element", "ref": ref_id }),
            ActionTarget::Selector { css } => json!({ "element": css }),
            ActionTarget::Coordinates { x, y } => json!({ "element": format!("coords({x},{y})") }),
        };
        self.call("browser_click", args).await?;
        Ok(())
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let ref_id = match &target {
            ActionTarget::Ref { ref_id } => ref_id.clone(),
            _ => String::new(),
        };
        self.call("browser_type", json!({
            "element": "element",
            "ref": ref_id,
            "text": text,
        })).await?;
        Ok(())
    }

    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.type_text(tab_id, target, value).await
    }

    async fn hover(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let args = match target {
            ActionTarget::Ref { ref_id } => json!({ "element": "element", "ref": ref_id }),
            ActionTarget::Selector { css } => json!({ "element": css }),
            ActionTarget::Coordinates { x, y } => json!({ "element": format!("coords({x},{y})") }),
        };
        self.call("browser_hover", args).await?;
        Ok(())
    }

    async fn scroll(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        let key = match direction {
            ScrollDirection::Up => "PageUp",
            ScrollDirection::Down => "PageDown",
            ScrollDirection::Left => "Home",
            ScrollDirection::Right => "End",
        };
        self.call("browser_press_key", json!({ "key": key })).await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        let result = self.call("browser_screenshot", json!({ "raw": true })).await?;
        // Check for image content in MCP response
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                    let data = item.get("data").and_then(|v| v.as_str()).unwrap_or("");
                    return Ok(ScreenshotResult {
                        data_base64: data.to_string(),
                        width: 0,
                        height: 0,
                        format: "png".to_string(),
                    });
                }
            }
        }
        let text = Self::extract_text(&result);
        Ok(ScreenshotResult {
            data_base64: text,
            width: 0,
            height: 0,
            format: "png".to_string(),
        })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        let result = self.call("browser_snapshot", json!({})).await?;
        let text = Self::extract_text(&result);
        Ok(Self::parse_snapshot_text(&text))
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError> {
        self.call("browser_evaluate", json!({ "expression": js })).await
    }

    async fn select(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let ref_id = match &target {
            ActionTarget::Ref { ref_id } => ref_id.clone(),
            _ => String::new(),
        };
        self.call("browser_select", json!({
            "element": "element",
            "ref": ref_id,
            "value": value,
        })).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_text_basic() {
        let text = r#"- heading "Welcome" [level=1]
- navigation "Main"
  - link "Home" [ref=s1e3]
  - link "About" [ref=s1e4]
- textbox "Search" [ref=s1e5]"#;

        let snapshot = PlaywrightMcpBackend::parse_snapshot_text(text);
        assert_eq!(snapshot.elements.len(), 5);
        assert_eq!(snapshot.elements[0].role, "heading");
        assert_eq!(snapshot.elements[0].name.as_deref(), Some("Welcome"));
        assert_eq!(snapshot.elements[2].role, "link");
        assert_eq!(snapshot.elements[2].name.as_deref(), Some("Home"));
        assert_eq!(snapshot.elements[2].ref_id, "s1e3");
    }

    #[test]
    fn test_parse_snapshot_text_empty() {
        let snapshot = PlaywrightMcpBackend::parse_snapshot_text("");
        assert!(snapshot.elements.is_empty());
    }

    #[test]
    fn test_extract_text_mcp_format() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "hello world"}]
        });
        assert_eq!(PlaywrightMcpBackend::extract_text(&result), "hello world");
    }
}
```

- [ ] **Step 2: Add module declarations to `mod.rs`**

In `src/browser/mod.rs`, add:

```rust
pub mod playwright_mcp;
pub mod playwright_mcp_backend;
```

And add re-exports:

```rust
pub use playwright_mcp::PlaywrightMcpDriver;
pub use playwright_mcp_backend::PlaywrightMcpBackend;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib playwright_mcp`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/browser/playwright_mcp_backend.rs src/browser/mod.rs
git commit -m "browser: add PlaywrightMcpBackend implementing BrowserBackend"
```

---

### Task 3: Wire ProfileManager

Add Playwright MCP driver to `ProfileManager` and restore default profile to `Managed` (headless).

**Files:**
- Modify: `src/browser/manager.rs`

- [ ] **Step 1: Add `playwright_mcp_driver` to ProfileManager**

In `ProfileManager` struct, add field:

```rust
playwright_mcp_driver: Arc<PlaywrightMcpDriver>,
```

In `ProfileManager::new()`, create the driver:

```rust
let playwright_mcp_driver = Arc::new(PlaywrightMcpDriver::new(config.playwright_mcp.clone()));
```

Add it to the `Self { ... }` struct.

Add accessor:

```rust
/// Get the shared Playwright MCP driver instance.
pub fn get_playwright_mcp_driver(&self) -> Arc<PlaywrightMcpDriver> {
    self.playwright_mcp_driver.clone()
}
```

- [ ] **Step 2: Restore default profile to `Managed` (headless)**

Change the auto-inject loop in `new()` from:

```rust
for name in &["default", "user"] {
```

To differentiated logic:

```rust
// "default" profile → Managed (headless Playwright)
if !profiles.contains_key("default") {
    profiles.insert(
        "default".into(),
        ManagedProfile {
            config: ProfileConfig {
                browser: BrowserType::Chrome,
                driver: BrowserDriver::Managed,
                ..Default::default()
            },
            state: ProfileState::Idle,
            last_activity: std::time::Instant::now(),
        },
    );
}
// "user" profile → ExistingSession (Chrome DevTools MCP)
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
```

Also revert the empty-config branch to use `Managed`:

```rust
if config.profiles.is_empty() {
    profiles.insert(
        "default".into(),
        ManagedProfile {
            config: ProfileConfig::default(), // default() → Managed
            state: ProfileState::Idle,
            last_activity: std::time::Instant::now(),
        },
    );
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/browser/manager.rs
git commit -m "browser: wire PlaywrightMcpDriver into ProfileManager, restore Managed default"
```

---

### Task 4: Wire Browser Tools to PlaywrightMcpBackend

Replace all `_ => { // Placeholder for managed mode }` branches in 11 browser tools with actual `PlaywrightMcpBackend` calls.

**Files:** All files in `src/builtin_tools/browser_tools/` (except `mod.rs`)

The pattern for every tool is the same. Replace:

```rust
_ => {
    // Placeholder for managed mode
    Ok(SomeOutput { success: true, ... })
}
```

With:

```rust
Some(BrowserDriver::Managed) | None => {
    let playwright = self.manager.get_playwright_mcp_driver();
    let backend = PlaywrightMcpBackend::new(playwright, args.profile.clone());
    // ... call the corresponding backend method ...
}
```

- [ ] **Step 1: Add import to each tool file**

Add at the top of each tool file:

```rust
use crate::browser::playwright_mcp_backend::PlaywrightMcpBackend;
```

- [ ] **Step 2: Wire `open.rs`**

Replace the `_ =>` branch with:

```rust
Some(BrowserDriver::Managed) | None => {
    let playwright = self.manager.get_playwright_mcp_driver();
    let backend = PlaywrightMcpBackend::new(playwright, args.profile.clone());
    match backend.open_tab(&args.url).await {
        Ok(tab_id) => Ok(BrowserOpenOutput {
            success: true,
            tab_id: Some(tab_id),
            message: Some(format!("Opened {} in profile '{}' (headless)", args.url, args.profile)),
        }),
        Err(e) => Ok(BrowserOpenOutput {
            success: false,
            tab_id: None,
            message: Some(format!("Failed to open tab: {e}")),
        }),
    }
}
```

- [ ] **Step 3: Wire `snapshot.rs`**

Replace the `_ =>` placeholder branch. Same pattern: create `PlaywrightMcpBackend`, call `get_active_tab`, then `backend.snapshot(&tab_id)`.

- [ ] **Step 4: Wire remaining 9 tools**

Apply the same pattern to: `click.rs`, `type_text.rs`, `fill_form.rs`, `select.rs`, `navigate.rs`, `screenshot.rs`, `evaluate.rs`, `tabs.rs`, `profile_tool.rs`.

Each tool already has a working `ExistingSession` branch. The `Managed` branch mirrors it but uses `PlaywrightMcpBackend` instead of `ChromeMcpBackend`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/browser_tools/
git commit -m "browser: wire all browser tools to PlaywrightMcpBackend for headless mode"
```

---

### Task 5: Cleanup and Delete Playwright Bridge Stub

**Files:**
- Delete: `src/browser/playwright_bridge.rs`
- Modify: `src/browser/mod.rs` (remove `pub mod playwright_bridge;`)

- [ ] **Step 1: Remove old module**

Delete `playwright_bridge.rs` and remove `pub mod playwright_bridge;` from `mod.rs`.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (pre-existing failures in `markdown_skill` are known and acceptable)

- [ ] **Step 4: Commit**

```bash
git add -A src/browser/
git commit -m "browser: remove superseded playwright_bridge stub"
```

---

### Task 6: Integration Test

- [ ] **Step 1: Build and start Aleph**

```bash
cargo build --bin aleph
pkill -f "target/debug/aleph" 2>/dev/null; sleep 2
target/debug/aleph start
```

- [ ] **Step 2: Send test message via Telegram**

Send: "打开 youtube.com 搜索 rust programming"

Expected behavior:
- `browser_open` uses `Managed` driver → `PlaywrightMcpBackend`
- Playwright MCP session starts (check log: "Playwright MCP session started")
- No Chrome window opens (headless)
- Snapshot returns real page content (not placeholder)

- [ ] **Step 3: Test explicit Chrome request**

Send: "使用 devtool 打开 chrome，进入 github.com"

Expected behavior:
- LLM selects `profile: "user"` (or tool routes to ExistingSession)
- Chrome DevTools MCP session starts
- Chrome window opens visually

- [ ] **Step 4: Commit final state**

```bash
git add -A
git commit -m "browser: headless Playwright MCP as default, Chrome DevTools on explicit request"
```
