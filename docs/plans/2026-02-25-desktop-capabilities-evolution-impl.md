# Desktop Capabilities Evolution Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add cross-platform browser automation (CDP), Windows desktop actions, media understanding pipeline, and permission system to Aleph.

**Architecture:** Browser Runtime lives in Core (CDP is pure WebSocket protocol, no platform APIs). Windows desktop actions live in Tauri bridge (platform-specific APIs via IPC). Vision pipeline and approval layer are provider-based abstractions in Core.

**Tech Stack:** Rust + tokio-tungstenite (CDP WebSocket), chromiumoxide (CDP protocol), windows-rs (Windows APIs), schemars (JSON Schema), async-trait

**Design Doc:** `docs/plans/2026-02-25-desktop-capabilities-evolution-design.md`

---

## Phase 1: Cross-platform Browser Runtime (CDP)

### Task 1: Browser module scaffold + error types

**Files:**
- Create: `core/src/browser/mod.rs`
- Create: `core/src/browser/error.rs`
- Create: `core/src/browser/types.rs`
- Modify: `core/src/lib.rs` (add `pub mod browser`)

**Step 1: Write failing test**

Create `core/src/browser/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("Browser not running. Call 'start' first.")]
    NotRunning,

    #[error("Browser launch failed: {0}")]
    LaunchFailed(String),

    #[error("CDP connection failed: {0}")]
    ConnectionFailed(String),

    #[error("CDP protocol error: {0}")]
    Protocol(String),

    #[error("Tab not found: {0}")]
    TabNotFound(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Action failed: {0}")]
    ActionFailed(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Chromium not found. Set ALEPH_CHROME_PATH or install Chrome/Chromium.")]
    ChromiumNotFound,

    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("JavaScript evaluation error: {0}")]
    EvalError(String),
}
```

Create `core/src/browser/types.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique tab identifier (CDP target ID).
pub type TabId = String;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TabInfo {
    pub id: TabId,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserConfig {
    #[serde(default = "default_mode")]
    pub mode: LaunchMode,
    #[serde(default)]
    pub user_data_dir: Option<String>,
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

fn default_mode() -> LaunchMode { LaunchMode::Auto }
fn default_cdp_port() -> u16 { 9222 }

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            mode: LaunchMode::Auto,
            user_data_dir: None,
            cdp_port: 9222,
            headless: false,
            extra_args: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaunchMode {
    Auto,
    Connect { endpoint: String },
    Binary { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AriaSnapshot {
    pub elements: Vec<AriaElement>,
    pub page_title: String,
    pub page_url: String,
    pub focused_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AriaElement {
    pub ref_id: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub state: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ElementRect>,
    #[serde(default)]
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    Ref { ref_id: String },
    Selector { css: String },
    Coordinates { x: f64, y: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotOpts {
    #[serde(default)]
    pub full_page: bool,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_quality")]
    pub quality: u8,
}

fn default_format() -> String { "png".to_string() }
fn default_quality() -> u8 { 80 }

impl Default for ScreenshotOpts {
    fn default() -> Self {
        Self { full_page: false, format: "png".to_string(), quality: 80 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub data_base64: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Local,
    Session,
}
```

Create `core/src/browser/mod.rs`:

```rust
pub mod error;
pub mod types;

pub use error::BrowserError;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.cdp_port, 9222);
        assert!(!config.headless);
        assert!(config.extra_args.is_empty());
    }

    #[test]
    fn test_action_target_serialization() {
        let target = ActionTarget::Ref { ref_id: "button[0]".to_string() };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "ref");
        assert_eq!(json["ref_id"], "button[0]");
    }

    #[test]
    fn test_aria_element_serialization() {
        let elem = AriaElement {
            ref_id: "input[0]".to_string(),
            role: "textbox".to_string(),
            name: "Email".to_string(),
            value: Some("test@example.com".to_string()),
            state: vec!["focused".to_string()],
            bounds: Some(ElementRect { x: 10.0, y: 20.0, width: 200.0, height: 30.0 }),
            children: vec![],
        };
        let json = serde_json::to_string(&elem).unwrap();
        assert!(json.contains("textbox"));
        assert!(json.contains("focused"));
    }

    #[test]
    fn test_launch_mode_tagged_enum() {
        let mode = LaunchMode::Connect { endpoint: "ws://127.0.0.1:9222".to_string() };
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "connect");
        assert_eq!(json["endpoint"], "ws://127.0.0.1:9222");
    }
}
```

**Step 2: Add module to lib.rs**

Add `pub mod browser;` to `core/src/lib.rs` in the appropriate section.

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib browser -- --nocapture`
Expected: All 4 tests pass.

**Step 4: Commit**

```bash
git add core/src/browser/ core/src/lib.rs
git commit -m "browser: scaffold module with types and error definitions"
```

---

### Task 2: Chromium discovery

**Files:**
- Create: `core/src/browser/discovery.rs`
- Modify: `core/src/browser/mod.rs` (add `pub mod discovery`)

**Step 1: Write failing test**

Create `core/src/browser/discovery.rs`:

```rust
use std::path::PathBuf;
use tracing::debug;

use super::error::BrowserError;

/// Discover a local Chromium-based browser executable.
///
/// Search order:
/// 1. `ALEPH_CHROME_PATH` environment variable
/// 2. Platform-specific default paths (Chrome, Edge, Brave, Chromium)
/// 3. PATH lookup via `which`
pub fn find_chromium() -> Result<PathBuf, BrowserError> {
    // 1. Environment variable
    if let Ok(path) = std::env::var("ALEPH_CHROME_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            debug!("Found Chromium via ALEPH_CHROME_PATH: {}", path);
            return Ok(p);
        }
    }

    // 2. Platform-specific default paths
    for path in platform_paths() {
        if path.exists() {
            debug!("Found Chromium at: {}", path.display());
            return Ok(path);
        }
    }

    // 3. PATH lookup
    for name in &["google-chrome", "chromium", "chromium-browser"] {
        if let Ok(path) = which::which(name) {
            debug!("Found Chromium in PATH: {}", path.display());
            return Ok(path);
        }
    }

    Err(BrowserError::ChromiumNotFound)
}

#[cfg(target_os = "macos")]
fn platform_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ]
}

#[cfg(target_os = "windows")]
fn platform_paths() -> Vec<PathBuf> {
    let program_files = std::env::var("ProgramFiles")
        .unwrap_or_else(|_| "C:\\Program Files".to_string());
    let program_files_x86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join("AppData").join("Local").to_string_lossy().to_string()
        });

    vec![
        PathBuf::from(format!("{}\\Google\\Chrome\\Application\\chrome.exe", program_files)),
        PathBuf::from(format!("{}\\Google\\Chrome\\Application\\chrome.exe", program_files_x86)),
        PathBuf::from(format!("{}\\Google\\Chrome\\Application\\chrome.exe", local_app_data)),
        PathBuf::from(format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", program_files)),
        PathBuf::from(format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", program_files_x86)),
        PathBuf::from(format!("{}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", program_files)),
    ]
}

#[cfg(target_os = "linux")]
fn platform_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/snap/bin/chromium"),
        PathBuf::from("/usr/bin/microsoft-edge"),
        PathBuf::from("/usr/bin/brave-browser"),
    ]
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn platform_paths() -> Vec<PathBuf> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_chromium_returns_existing_path() {
        // On a dev machine, at least one Chromium browser should exist
        match find_chromium() {
            Ok(path) => assert!(path.exists()),
            Err(BrowserError::ChromiumNotFound) => {
                // Acceptable in CI without Chrome
                eprintln!("No Chromium found (expected in CI)");
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }
    }

    #[test]
    fn test_env_override() {
        // Set env to a known path
        std::env::set_var("ALEPH_CHROME_PATH", "/bin/sh");
        let result = find_chromium();
        std::env::remove_var("ALEPH_CHROME_PATH");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn test_platform_paths_not_empty() {
        let paths = platform_paths();
        assert!(!paths.is_empty());
    }
}
```

**Step 2: Add `which` dependency to `core/Cargo.toml`**

```toml
which = "7"
```

Note: `which` is already used by the Tauri desktop app. Check if it's already in core's Cargo.toml; if so, skip adding it.

**Step 3: Update `core/src/browser/mod.rs`**

Add `pub mod discovery;` and re-export:
```rust
pub use discovery::find_chromium;
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib browser::discovery -- --nocapture`
Expected: 3 tests pass.

**Step 5: Commit**

```bash
git add core/src/browser/discovery.rs core/src/browser/mod.rs core/Cargo.toml
git commit -m "browser: add cross-platform Chromium discovery"
```

---

### Task 3: CDP transport layer

**Files:**
- Modify: `core/Cargo.toml` (add `chromiumoxide` dependency)
- Create: `core/src/browser/runtime.rs`
- Modify: `core/src/browser/mod.rs`

**Step 1: Add dependency**

Add to `core/Cargo.toml` under `[dependencies]`:

```toml
chromiumoxide = { version = "0.7", features = ["tokio-runtime"], default-features = false }
```

`chromiumoxide` provides:
- CDP WebSocket transport (uses tokio-tungstenite internally)
- Browser launch + process management
- Tab (Page) management
- Full CDP protocol type coverage
- Built on tokio async runtime

This avoids building a CDP client from scratch while keeping full Rust-native performance.

**Step 2: Create runtime.rs**

Create `core/src/browser/runtime.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;

use super::discovery::find_chromium;
use super::error::BrowserError;
use super::types::{self, BrowserConfig, LaunchMode, TabId, TabInfo};

/// Cross-platform browser runtime powered by CDP (Chrome DevTools Protocol).
///
/// Manages a Chromium browser process and provides high-level APIs for
/// tab management, navigation, actions, and perception.
pub struct BrowserRuntime {
    browser: Browser,
    pages: Vec<Page>,
    config: BrowserConfig,
    _handle: tokio::task::JoinHandle<()>,
}

impl BrowserRuntime {
    /// Launch or connect to a Chromium browser.
    pub async fn start(config: BrowserConfig) -> Result<Self, BrowserError> {
        let cdp_config = build_cdp_config(&config)?;

        let (browser, mut handler) = Browser::launch(cdp_config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn CDP event handler in background
        let handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    warn!("CDP handler error: {}", e);
                }
            }
        });

        info!("Browser runtime started");
        Ok(Self {
            browser,
            pages: Vec::new(),
            config,
            _handle: handle,
        })
    }

    /// Check if the browser process is still alive.
    pub fn is_running(&self) -> bool {
        !self._handle.is_finished()
    }

    /// Open a new tab and navigate to URL.
    pub async fn open_tab(&mut self, url: &str) -> Result<TabId, BrowserError> {
        let page = self.browser
            .new_page(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        let target_id = page.target_id().to_string();
        self.pages.push(page);
        debug!("Opened tab: {} → {}", target_id, url);
        Ok(target_id)
    }

    /// Close a tab by ID.
    pub async fn close_tab(&mut self, tab_id: &str) -> Result<(), BrowserError> {
        let idx = self.find_page_index(tab_id)?;
        let page = self.pages.remove(idx);
        page.close().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        debug!("Closed tab: {}", tab_id);
        Ok(())
    }

    /// List all open tabs.
    pub async fn list_tabs(&self) -> Vec<TabInfo> {
        let mut tabs = Vec::new();
        for page in &self.pages {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            tabs.push(TabInfo {
                id: page.target_id().to_string(),
                url,
                title,
            });
        }
        tabs
    }

    /// Navigate a tab to a URL.
    pub async fn navigate(&mut self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        page.goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        debug!("Navigated tab {} → {}", tab_id, url);
        Ok(())
    }

    /// Take a screenshot of a tab.
    pub async fn screenshot(
        &self,
        tab_id: &str,
        opts: types::ScreenshotOpts,
    ) -> Result<types::ScreenshotResult, BrowserError> {
        let page = self.find_page(tab_id)?;

        let png_data = if opts.full_page {
            page.save_screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(true)
                    .build(),
            )
            .await
        } else {
            page.screenshot(
                chromiumoxide::page::ScreenshotParams::builder().build(),
            )
            .await
        }
        .map_err(|e| BrowserError::ScreenshotFailed(e.to_string()))?;

        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&png_data);

        Ok(types::ScreenshotResult {
            data_base64,
            width: 0,  // CDP screenshot doesn't return dimensions directly
            height: 0,
            format: "png".to_string(),
        })
    }

    /// Evaluate JavaScript in a tab and return the result.
    pub async fn evaluate(
        &self,
        tab_id: &str,
        js: &str,
    ) -> Result<serde_json::Value, BrowserError> {
        let page = self.find_page(tab_id)?;
        let result = page
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::EvalError(e.to_string()))?;
        Ok(result.into_value().unwrap_or(serde_json::Value::Null))
    }

    /// Get a page's current URL.
    pub async fn get_url(&self, tab_id: &str) -> Result<String, BrowserError> {
        let page = self.find_page(tab_id)?;
        page.url()
            .await
            .map_err(|e| BrowserError::Protocol(e.to_string()))?
            .ok_or_else(|| BrowserError::Protocol("No URL available".into()))
    }

    /// Get a page's title.
    pub async fn get_title(&self, tab_id: &str) -> Result<String, BrowserError> {
        let page = self.find_page(tab_id)?;
        page.get_title()
            .await
            .map_err(|e| BrowserError::Protocol(e.to_string()))?
            .ok_or_else(|| BrowserError::Protocol("No title available".into()))
    }

    /// Get underlying Page for advanced CDP operations.
    pub fn get_page(&self, tab_id: &str) -> Result<&Page, BrowserError> {
        self.find_page(tab_id)
    }

    /// Gracefully close the browser.
    pub async fn stop(self) -> Result<(), BrowserError> {
        // Pages are dropped automatically
        drop(self.browser);
        info!("Browser runtime stopped");
        Ok(())
    }

    // -- Internal helpers --

    fn find_page(&self, tab_id: &str) -> Result<&Page, BrowserError> {
        self.pages
            .iter()
            .find(|p| p.target_id().to_string() == tab_id)
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))
    }

    fn find_page_index(&self, tab_id: &str) -> Result<usize, BrowserError> {
        self.pages
            .iter()
            .position(|p| p.target_id().to_string() == tab_id)
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))
    }
}

fn build_cdp_config(config: &BrowserConfig) -> Result<CdpBrowserConfig, BrowserError> {
    let mut builder = CdpBrowserConfig::builder();

    match &config.mode {
        LaunchMode::Auto => {
            let path = find_chromium()?;
            builder = builder.chrome_executable(path);
        }
        LaunchMode::Binary { path } => {
            builder = builder.chrome_executable(PathBuf::from(path));
        }
        LaunchMode::Connect { endpoint: _ } => {
            // chromiumoxide connect mode handled separately
            // For now, fall back to auto
            let path = find_chromium()?;
            builder = builder.chrome_executable(path);
        }
    }

    if config.headless {
        builder = builder.arg("--headless=new");
    }

    builder = builder.arg(format!("--remote-debugging-port={}", config.cdp_port));

    if let Some(ref dir) = config.user_data_dir {
        builder = builder.user_data_dir(PathBuf::from(dir));
    }

    for arg in &config.extra_args {
        builder = builder.arg(arg);
    }

    // Common safety args
    builder = builder
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking");

    builder.build().map_err(|e| BrowserError::LaunchFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cdp_config_headless() {
        let config = BrowserConfig {
            headless: true,
            ..Default::default()
        };
        // This may fail if no Chrome installed, which is fine
        let _result = build_cdp_config(&config);
    }

    #[tokio::test]
    #[ignore = "requires Chromium installed"]
    async fn test_browser_lifecycle() {
        let config = BrowserConfig {
            headless: true,
            ..Default::default()
        };
        let mut runtime = BrowserRuntime::start(config).await.unwrap();
        assert!(runtime.is_running());

        let tab_id = runtime.open_tab("about:blank").await.unwrap();
        assert!(!tab_id.is_empty());

        let tabs = runtime.list_tabs().await;
        assert!(!tabs.is_empty());

        runtime.close_tab(&tab_id).await.unwrap();
        runtime.stop().await.unwrap();
    }
}
```

**Step 3: Update `core/src/browser/mod.rs`**

Add `pub mod runtime;` and re-export `pub use runtime::BrowserRuntime;`.

**Step 4: Add `base64` to dependencies if not present**

Check `core/Cargo.toml` for `base64`. If missing, add:
```toml
base64 = "0.22"
```

**Step 5: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add core/src/browser/runtime.rs core/src/browser/mod.rs core/Cargo.toml
git commit -m "browser: add BrowserRuntime with CDP transport via chromiumoxide"
```

---

### Task 4: ARIA snapshot extraction

**Files:**
- Create: `core/src/browser/snapshot.rs`
- Modify: `core/src/browser/mod.rs`

**Step 1: Create snapshot.rs**

The ARIA snapshot is built by injecting JavaScript into the page that walks the accessibility tree and returns structured element data. This is the key innovation from OpenClaw.

```rust
use super::error::BrowserError;
use super::types::{AriaElement, AriaSnapshot, ElementRect};

use chromiumoxide::page::Page;

/// JavaScript that extracts ARIA accessibility tree from the DOM.
/// Returns a JSON array of elements with ref IDs, roles, names, values, states, and bounds.
const ARIA_SNAPSHOT_JS: &str = r#"
(() => {
    const elements = [];
    const counters = {};

    function getRefId(role) {
        if (!counters[role]) counters[role] = 0;
        const id = `${role}[${counters[role]}]`;
        counters[role]++;
        return id;
    }

    function getState(el) {
        const states = [];
        if (el === document.activeElement) states.push('focused');
        if (el.disabled) states.push('disabled');
        if (el.checked) states.push('checked');
        if (el.getAttribute('aria-expanded') === 'true') states.push('expanded');
        if (el.getAttribute('aria-selected') === 'true') states.push('selected');
        if (el.getAttribute('aria-hidden') === 'true') states.push('hidden');
        if (el.required) states.push('required');
        if (el.readOnly) states.push('readonly');
        return states;
    }

    function getRole(el) {
        if (el.getAttribute('role')) return el.getAttribute('role');
        const tag = el.tagName.toLowerCase();
        const roleMap = {
            'a': 'link', 'button': 'button', 'input': 'textbox',
            'select': 'combobox', 'textarea': 'textbox', 'img': 'img',
            'h1': 'heading', 'h2': 'heading', 'h3': 'heading',
            'h4': 'heading', 'h5': 'heading', 'h6': 'heading',
            'nav': 'navigation', 'main': 'main', 'aside': 'complementary',
            'form': 'form', 'table': 'table', 'li': 'listitem',
            'ul': 'list', 'ol': 'list',
        };
        if (tag === 'input') {
            const type = el.type || 'text';
            if (type === 'checkbox') return 'checkbox';
            if (type === 'radio') return 'radio';
            if (type === 'submit' || type === 'button') return 'button';
            return 'textbox';
        }
        return roleMap[tag] || null;
    }

    function getName(el) {
        return el.getAttribute('aria-label')
            || el.getAttribute('alt')
            || el.getAttribute('title')
            || el.getAttribute('placeholder')
            || (el.labels && el.labels[0] && el.labels[0].textContent)
            || el.textContent?.trim().substring(0, 100)
            || '';
    }

    function getValue(el) {
        if (el.value !== undefined && el.value !== '') return String(el.value);
        if (el.getAttribute('aria-valuenow')) return el.getAttribute('aria-valuenow');
        return null;
    }

    function walk(el) {
        const role = getRole(el);
        if (!role) {
            // No semantic role — still walk children
            for (const child of el.children) walk(child);
            return;
        }

        const state = getState(el);
        if (state.includes('hidden')) return;

        const rect = el.getBoundingClientRect();
        if (rect.width === 0 && rect.height === 0) return;

        const refId = getRefId(role);
        const childRefs = [];

        for (const child of el.children) {
            const prevLen = elements.length;
            walk(child);
            for (let i = prevLen; i < elements.length; i++) {
                if (!childRefs.includes(elements[i].ref_id)) {
                    childRefs.push(elements[i].ref_id);
                }
            }
        }

        elements.push({
            ref_id: refId,
            role: role,
            name: getName(el),
            value: getValue(el),
            state: state,
            bounds: {
                x: Math.round(rect.x),
                y: Math.round(rect.y),
                width: Math.round(rect.width),
                height: Math.round(rect.height),
            },
            children: childRefs,
        });
    }

    walk(document.body);
    return {
        elements: elements,
        page_title: document.title,
        page_url: window.location.href,
        focused_ref: null,
    };
})()
"#;

/// Take an ARIA snapshot of a page, returning structured element data
/// that agents can use for targeted actions (click by ref_id, not coordinates).
pub async fn take_aria_snapshot(page: &Page) -> Result<AriaSnapshot, BrowserError> {
    let result = page
        .evaluate(ARIA_SNAPSHOT_JS)
        .await
        .map_err(|e| BrowserError::EvalError(format!("ARIA snapshot failed: {}", e)))?;

    let value = result.into_value::<serde_json::Value>()
        .ok_or_else(|| BrowserError::EvalError("ARIA snapshot returned null".into()))?;

    let snapshot: AriaSnapshot = serde_json::from_value(value)
        .map_err(|e| BrowserError::EvalError(format!("Failed to parse ARIA snapshot: {}", e)))?;

    Ok(snapshot)
}

/// Find an element by ref_id in a snapshot and return its center coordinates.
/// Used to translate ActionTarget::Ref into coordinates for CDP input events.
pub fn resolve_ref_to_point(snapshot: &AriaSnapshot, ref_id: &str) -> Option<(f64, f64)> {
    snapshot.elements.iter().find(|e| e.ref_id == ref_id).and_then(|e| {
        e.bounds.as_ref().map(|b| (b.x + b.width / 2.0, b.y + b.height / 2.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_ref_to_point() {
        let snapshot = AriaSnapshot {
            elements: vec![
                AriaElement {
                    ref_id: "button[0]".to_string(),
                    role: "button".to_string(),
                    name: "Submit".to_string(),
                    value: None,
                    state: vec![],
                    bounds: Some(ElementRect { x: 100.0, y: 200.0, width: 80.0, height: 30.0 }),
                    children: vec![],
                },
            ],
            page_title: "Test".to_string(),
            page_url: "about:blank".to_string(),
            focused_ref: None,
        };

        let point = resolve_ref_to_point(&snapshot, "button[0]");
        assert!(point.is_some());
        let (x, y) = point.unwrap();
        assert!((x - 140.0).abs() < 0.1); // 100 + 80/2
        assert!((y - 215.0).abs() < 0.1); // 200 + 30/2
    }

    #[test]
    fn test_resolve_ref_not_found() {
        let snapshot = AriaSnapshot {
            elements: vec![],
            page_title: "Empty".to_string(),
            page_url: "about:blank".to_string(),
            focused_ref: None,
        };
        assert!(resolve_ref_to_point(&snapshot, "nonexistent").is_none());
    }
}
```

**Step 2: Update mod.rs**

Add `pub mod snapshot;` and re-export `pub use snapshot::{take_aria_snapshot, resolve_ref_to_point};`.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib browser::snapshot -- --nocapture`
Expected: 2 tests pass.

**Step 4: Commit**

```bash
git add core/src/browser/snapshot.rs core/src/browser/mod.rs
git commit -m "browser: add ARIA snapshot extraction with ref_id resolution"
```

---

### Task 5: Browser actions (click, type, navigate)

**Files:**
- Create: `core/src/browser/actions.rs`
- Modify: `core/src/browser/runtime.rs` (add action methods)
- Modify: `core/src/browser/mod.rs`

**Step 1: Create actions.rs**

```rust
use chromiumoxide::page::Page;
use tracing::debug;

use super::error::BrowserError;
use super::snapshot::{resolve_ref_to_point, take_aria_snapshot};
use super::types::ActionTarget;

/// Resolve an ActionTarget to (x, y) coordinates on the page.
async fn resolve_target(page: &Page, target: &ActionTarget) -> Result<(f64, f64), BrowserError> {
    match target {
        ActionTarget::Coordinates { x, y } => Ok((*x, *y)),
        ActionTarget::Ref { ref_id } => {
            let snapshot = take_aria_snapshot(page).await?;
            resolve_ref_to_point(&snapshot, ref_id)
                .ok_or_else(|| BrowserError::ActionFailed(format!("Element not found: {}", ref_id)))
        }
        ActionTarget::Selector { css } => {
            // Use CDP to find element bounds by CSS selector
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector({selector});
                    if (!el) return null;
                    const r = el.getBoundingClientRect();
                    return {{ x: r.x + r.width / 2, y: r.y + r.height / 2 }};
                }})()"#,
                selector = serde_json::to_string(css).unwrap_or_default()
            );
            let result = page.evaluate(&js)
                .await
                .map_err(|e| BrowserError::EvalError(e.to_string()))?;
            let val = result.into_value::<serde_json::Value>()
                .ok_or_else(|| BrowserError::ActionFailed(format!("Selector not found: {}", css)))?;
            let x = val["x"].as_f64().ok_or_else(|| BrowserError::ActionFailed("No x coordinate".into()))?;
            let y = val["y"].as_f64().ok_or_else(|| BrowserError::ActionFailed("No y coordinate".into()))?;
            Ok((x, y))
        }
    }
}

/// Click on an element.
pub async fn click(page: &Page, target: &ActionTarget) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;
    debug!("Click at ({}, {})", x, y);

    // Use CDP Input.dispatchMouseEvent
    page.evaluate(format!(
        r#"
        (() => {{
            const el = document.elementFromPoint({x}, {y});
            if (el) el.click();
        }})()
        "#,
        x = x, y = y
    ))
    .await
    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

    Ok(())
}

/// Type text into a focused element or a specific target.
pub async fn type_text(page: &Page, target: &ActionTarget, text: &str) -> Result<(), BrowserError> {
    // First click to focus
    click(page, target).await?;

    // Then type character by character using CDP
    for ch in text.chars() {
        page.evaluate(format!(
            r#"document.activeElement.value += '{}'"#,
            ch.to_string().replace('\\', "\\\\").replace('\'', "\\'")
        ))
        .await
        .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
    }

    // Dispatch input event
    page.evaluate(
        r#"document.activeElement.dispatchEvent(new Event('input', { bubbles: true }))"#,
    )
    .await
    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

    debug!("Typed {} chars", text.len());
    Ok(())
}

/// Fill a form field (clear + set value + dispatch events).
pub async fn fill(page: &Page, target: &ActionTarget, value: &str) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let escaped = serde_json::to_string(value).unwrap_or_default();
    page.evaluate(format!(
        r#"
        (() => {{
            const el = document.elementFromPoint({x}, {y});
            if (!el) throw new Error('Element not found');
            el.focus();
            el.value = {value};
            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true }}));
        }})()
        "#,
        x = x, y = y, value = escaped
    ))
    .await
    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

    debug!("Filled field at ({}, {}) with {} chars", x, y, value.len());
    Ok(())
}

/// Scroll the page or a specific element.
pub async fn scroll(
    page: &Page,
    target: &ActionTarget,
    direction: &super::types::ScrollDirection,
) -> Result<(), BrowserError> {
    use super::types::ScrollDirection;
    let (dx, dy) = match direction {
        ScrollDirection::Up => (0, -300),
        ScrollDirection::Down => (0, 300),
        ScrollDirection::Left => (-300, 0),
        ScrollDirection::Right => (300, 0),
    };

    let (x, y) = resolve_target(page, target).await.unwrap_or((0.0, 0.0));

    page.evaluate(format!(
        r#"
        (() => {{
            const el = document.elementFromPoint({x}, {y}) || document.documentElement;
            el.scrollBy({{ left: {dx}, top: {dy}, behavior: 'smooth' }});
        }})()
        "#,
        x = x, y = y, dx = dx, dy = dy
    ))
    .await
    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

    debug!("Scrolled ({}, {}) at ({}, {})", dx, dy, x, y);
    Ok(())
}

/// Hover over an element.
pub async fn hover(page: &Page, target: &ActionTarget) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    page.evaluate(format!(
        r#"
        (() => {{
            const el = document.elementFromPoint({x}, {y});
            if (el) {{
                el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));
                el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));
            }}
        }})()
        "#,
        x = x, y = y
    ))
    .await
    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

    debug!("Hover at ({}, {})", x, y);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_target_coordinates() {
        // Static test — just verify types work
        let target = ActionTarget::Coordinates { x: 100.0, y: 200.0 };
        match &target {
            ActionTarget::Coordinates { x, y } => {
                assert_eq!(*x, 100.0);
                assert_eq!(*y, 200.0);
            }
            _ => panic!("Wrong variant"),
        }
    }
}
```

**Step 2: Add convenience methods to runtime.rs**

Add these methods to `impl BrowserRuntime`:

```rust
pub async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
    let page = self.find_page(tab_id)?;
    actions::click(page, &target).await
}

pub async fn type_text(&self, tab_id: &str, target: ActionTarget, text: &str) -> Result<(), BrowserError> {
    let page = self.find_page(tab_id)?;
    actions::type_text(page, &target, text).await
}

pub async fn fill(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError> {
    let page = self.find_page(tab_id)?;
    actions::fill(page, &target, value).await
}

pub async fn scroll(&self, tab_id: &str, target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError> {
    let page = self.find_page(tab_id)?;
    actions::scroll(page, &target, &direction).await
}

pub async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
    let page = self.find_page(tab_id)?;
    actions::hover(page, &target).await
}

pub async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
    let page = self.find_page(tab_id)?;
    snapshot::take_aria_snapshot(page).await
}
```

**Step 3: Update mod.rs**

Add `pub mod actions;`.

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`

**Step 5: Commit**

```bash
git add core/src/browser/actions.rs core/src/browser/runtime.rs core/src/browser/mod.rs
git commit -m "browser: add click, type, fill, scroll, hover actions with ARIA ref targeting"
```

---

### Task 6: BrowserTool (AlephTool implementation)

**Files:**
- Create: `core/src/builtin_tools/browser.rs`
- Modify: `core/src/builtin_tools/mod.rs` (add module)
- Modify: `core/src/tools/builtin.rs` (add `.with_browser()`)

**Step 1: Create browser.rs**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::browser::{
    ActionTarget, AriaSnapshot, BrowserConfig, BrowserError, BrowserRuntime,
    ScreenshotOpts, ScreenshotResult, ScrollDirection, StorageKind, TabInfo,
};
use crate::tools::AlephTool;

/// Built-in browser automation tool powered by CDP (Chrome DevTools Protocol).
///
/// Provides cross-platform browser control: tab management, navigation,
/// element interaction (via ARIA ref IDs), screenshots, and ARIA snapshots.
#[derive(Clone)]
pub struct BrowserTool {
    runtime: Arc<Mutex<Option<BrowserRuntime>>>,
}

impl BrowserTool {
    pub fn new() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BrowserArgs {
    /// The action to perform.
    pub action: BrowserAction,
    /// Target tab ID (from open_tab or list_tabs). Required for most actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// URL for navigate/open_tab actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// ARIA element ref_id (e.g., "button[0]", "textbox[1]") for targeted actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    /// CSS selector (fallback if ref_id not available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Text to type or fill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// JavaScript code to evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,
    /// Scroll direction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<ScrollDirection>,
    /// Whether to launch headless (default: false).
    #[serde(default)]
    pub headless: bool,
    /// Screenshot options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_page: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Launch the browser. Call this first before other actions.
    Start,
    /// Close the browser.
    Stop,
    /// Open a new tab. Requires `url`.
    OpenTab,
    /// Close a tab. Requires `tab_id`.
    CloseTab,
    /// List all open tabs.
    ListTabs,
    /// Navigate to URL. Requires `tab_id` and `url`.
    Navigate,
    /// Click on an element. Requires `tab_id` and (`ref_id` or `selector`).
    Click,
    /// Type text. Requires `tab_id`, (`ref_id` or `selector`), and `text`.
    Type,
    /// Fill a form field (clear + set). Requires `tab_id`, (`ref_id` or `selector`), and `text`.
    Fill,
    /// Scroll. Requires `tab_id` and `direction`.
    Scroll,
    /// Hover over element. Requires `tab_id` and (`ref_id` or `selector`).
    Hover,
    /// Take screenshot. Requires `tab_id`.
    Screenshot,
    /// Get ARIA accessibility snapshot. Requires `tab_id`. Returns element list with ref_ids.
    Snapshot,
    /// Evaluate JavaScript. Requires `tab_id` and `js`.
    Evaluate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl BrowserOutput {
    fn ok(data: impl Serialize) -> Self {
        Self {
            success: true,
            message: None,
            data: serde_json::to_value(data).ok(),
        }
    }

    fn ok_msg(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
        }
    }
}

#[async_trait]
impl AlephTool for BrowserTool {
    const NAME: &'static str = "browser";
    const DESCRIPTION: &'static str = "Control a web browser via CDP. Actions: start, stop, open_tab, close_tab, list_tabs, navigate, click, type, fill, scroll, hover, screenshot, snapshot, evaluate. Use 'snapshot' to get ARIA element refs, then target elements by ref_id (e.g., 'button[0]').";

    type Args = BrowserArgs;
    type Output = BrowserOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"browser(action="start")"#.to_string(),
            r#"browser(action="open_tab", url="https://example.com")"#.to_string(),
            r#"browser(action="snapshot", tab_id="...")"#.to_string(),
            r#"browser(action="click", tab_id="...", ref_id="button[0]")"#.to_string(),
            r#"browser(action="fill", tab_id="...", ref_id="textbox[0]", text="hello")"#.to_string(),
            r#"browser(action="screenshot", tab_id="...")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> anyhow::Result<Self::Output> {
        match args.action {
            BrowserAction::Start => {
                let mut lock = self.runtime.lock().await;
                if lock.is_some() {
                    return Ok(BrowserOutput::ok_msg("Browser already running"));
                }
                let config = BrowserConfig {
                    headless: args.headless,
                    ..Default::default()
                };
                match BrowserRuntime::start(config).await {
                    Ok(rt) => {
                        *lock = Some(rt);
                        Ok(BrowserOutput::ok_msg("Browser started"))
                    }
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Stop => {
                let mut lock = self.runtime.lock().await;
                if let Some(rt) = lock.take() {
                    rt.stop().await.ok();
                }
                Ok(BrowserOutput::ok_msg("Browser stopped"))
            }

            BrowserAction::OpenTab => {
                let url = args.url.as_deref().unwrap_or("about:blank");
                let mut lock = self.runtime.lock().await;
                let rt = lock.as_mut().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.open_tab(url).await {
                    Ok(tab_id) => Ok(BrowserOutput::ok(serde_json::json!({ "tab_id": tab_id }))),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::CloseTab => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let mut lock = self.runtime.lock().await;
                let rt = lock.as_mut().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.close_tab(tab_id).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg("Tab closed")),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::ListTabs => {
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                let tabs = rt.list_tabs().await;
                Ok(BrowserOutput::ok(tabs))
            }

            BrowserAction::Navigate => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let url = args.url.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("url required"))?;
                let mut lock = self.runtime.lock().await;
                let rt = lock.as_mut().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.navigate(tab_id, url).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg(format!("Navigated to {}", url))),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Click => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let target = resolve_action_target(&args)?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.click(tab_id, target).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg("Clicked")),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Type => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let text = args.text.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("text required"))?;
                let target = resolve_action_target(&args)?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.type_text(tab_id, target, text).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg(format!("Typed {} chars", text.len()))),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Fill => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let text = args.text.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("text required"))?;
                let target = resolve_action_target(&args)?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.fill(tab_id, target, text).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg("Field filled")),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Scroll => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let direction = args.direction.unwrap_or(ScrollDirection::Down);
                let target = resolve_action_target(&args)
                    .unwrap_or(ActionTarget::Coordinates { x: 0.0, y: 0.0 });
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.scroll(tab_id, target, direction).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg("Scrolled")),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Hover => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let target = resolve_action_target(&args)?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.hover(tab_id, target).await {
                    Ok(()) => Ok(BrowserOutput::ok_msg("Hovered")),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Screenshot => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let opts = ScreenshotOpts {
                    full_page: args.full_page.unwrap_or(false),
                    ..Default::default()
                };
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.screenshot(tab_id, opts).await {
                    Ok(result) => Ok(BrowserOutput::ok(result)),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Snapshot => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.snapshot(tab_id).await {
                    Ok(snapshot) => Ok(BrowserOutput::ok(snapshot)),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }

            BrowserAction::Evaluate => {
                let tab_id = args.tab_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("tab_id required"))?;
                let js = args.js.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("js required"))?;
                let lock = self.runtime.lock().await;
                let rt = lock.as_ref().ok_or_else(|| anyhow::anyhow!("Browser not running"))?;
                match rt.evaluate(tab_id, js).await {
                    Ok(val) => Ok(BrowserOutput::ok(val)),
                    Err(e) => Ok(BrowserOutput::err(e.to_string())),
                }
            }
        }
    }
}

fn resolve_action_target(args: &BrowserArgs) -> anyhow::Result<ActionTarget> {
    if let Some(ref ref_id) = args.ref_id {
        Ok(ActionTarget::Ref { ref_id: ref_id.clone() })
    } else if let Some(ref css) = args.selector {
        Ok(ActionTarget::Selector { css: css.clone() })
    } else {
        Err(anyhow::anyhow!("ref_id or selector required"))
    }
}
```

**Step 2: Register in builtin.rs**

Add to `core/src/tools/builtin.rs`:

```rust
pub fn with_browser(self) -> Self {
    self.tool(crate::builtin_tools::browser::BrowserTool::new())
}
```

Add `pub mod browser;` to `core/src/builtin_tools/mod.rs`.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: Compiles.

**Step 4: Commit**

```bash
git add core/src/builtin_tools/browser.rs core/src/builtin_tools/mod.rs core/src/tools/builtin.rs
git commit -m "browser: add BrowserTool with AlephTool implementation"
```

---

### Task 7: Integration test — full browser workflow

**Files:**
- Create: `core/tests/browser_integration.rs`

**Step 1: Write integration test**

```rust
//! Integration test for the browser runtime.
//! Requires Chromium installed. Run with:
//!   cargo test --test browser_integration -- --nocapture --ignored

#[cfg(test)]
mod tests {
    use alephcore::browser::{BrowserConfig, BrowserRuntime, ActionTarget, ScreenshotOpts};

    #[tokio::test]
    #[ignore = "requires Chromium installed"]
    async fn test_full_browser_workflow() {
        // 1. Start browser
        let config = BrowserConfig {
            headless: true,
            ..Default::default()
        };
        let mut runtime = BrowserRuntime::start(config).await
            .expect("Failed to start browser");

        // 2. Open tab
        let tab_id = runtime.open_tab("https://example.com").await
            .expect("Failed to open tab");
        assert!(!tab_id.is_empty());

        // 3. Wait a moment for page load
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // 4. List tabs
        let tabs = runtime.list_tabs().await;
        assert!(!tabs.is_empty());
        assert!(tabs.iter().any(|t| t.url.contains("example.com")));

        // 5. Take ARIA snapshot
        let snapshot = runtime.snapshot(&tab_id).await
            .expect("Failed to take snapshot");
        assert!(!snapshot.elements.is_empty());
        assert!(snapshot.page_title.contains("Example"));

        // 6. Take screenshot
        let screenshot = runtime.screenshot(&tab_id, ScreenshotOpts::default()).await
            .expect("Failed to take screenshot");
        assert!(!screenshot.data_base64.is_empty());

        // 7. Evaluate JS
        let result = runtime.evaluate(&tab_id, "document.title").await
            .expect("Failed to evaluate JS");
        assert!(result.as_str().unwrap_or("").contains("Example"));

        // 8. Close tab
        runtime.close_tab(&tab_id).await
            .expect("Failed to close tab");

        // 9. Stop browser
        runtime.stop().await.expect("Failed to stop browser");
    }
}
```

**Step 2: Run integration test**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --test browser_integration -- --nocapture --ignored`
Expected: PASS (if Chromium is installed).

**Step 3: Commit**

```bash
git add core/tests/browser_integration.rs
git commit -m "browser: add integration test for full browser workflow"
```

---

## Phase 2: Windows Desktop Actions

> Phase 2 implements the Windows-specific desktop actions in the Tauri bridge app.
> These are compile-gated with `#[cfg(target_os = "windows")]`.

### Task 8: Input simulation (click, type, key combo)

**Files:**
- Create: `apps/desktop/src-tauri/src/bridge/action.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs` (wire dispatch)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add windows-rs features)

**Step 1: Add windows-rs features**

In `apps/desktop/src-tauri/Cargo.toml`, ensure the `windows` dependency has required features:

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_Foundation",
    "Win32_Graphics_Gdi",
] }
```

**Step 2: Create action.rs**

```rust
use serde_json::{json, Value};
use aleph_protocol::desktop_bridge::*;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::*;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::POINT;

pub fn handle_click(params: Value) -> Result<Value, (i32, String)> {
    let x = params["x"].as_f64().ok_or((ERR_INTERNAL, "x required".into()))? as i32;
    let y = params["y"].as_f64().ok_or((ERR_INTERNAL, "y required".into()))? as i32;
    let button = params["button"].as_str().unwrap_or("left");

    #[cfg(target_os = "windows")]
    {
        use std::mem::size_of;
        unsafe {
            // Move cursor
            let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);

            // Determine button flags
            let (down_flag, up_flag) = match button {
                "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
                "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
                _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            };

            let inputs = [
                INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0,
                            mouseData: 0,
                            dwFlags: down_flag,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0, dy: 0,
                            mouseData: 0,
                            dwFlags: up_flag,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            ];

            SendInput(&inputs, size_of::<INPUT>() as i32);
        }
        Ok(json!({ "clicked": true, "x": x, "y": y, "button": button }))
    }

    #[cfg(not(target_os = "windows"))]
    Err((ERR_NOT_IMPLEMENTED, "Click only available on Windows".into()))
}

pub fn handle_type_text(params: Value) -> Result<Value, (i32, String)> {
    let text = params["text"].as_str().ok_or((ERR_INTERNAL, "text required".into()))?;

    #[cfg(target_os = "windows")]
    {
        use std::mem::size_of;
        for ch in text.encode_utf16() {
            unsafe {
                let inputs = [
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(0),
                                wScan: ch,
                                dwFlags: KEYEVENTF_UNICODE,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(0),
                                wScan: ch,
                                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                ];
                SendInput(&inputs, size_of::<INPUT>() as i32);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(json!({ "typed": true, "length": text.len() }))
    }

    #[cfg(not(target_os = "windows"))]
    Err((ERR_NOT_IMPLEMENTED, "TypeText only available on Windows".into()))
}

pub fn handle_key_combo(params: Value) -> Result<Value, (i32, String)> {
    let keys = params["keys"].as_array()
        .ok_or((ERR_INTERNAL, "keys array required".into()))?;

    #[cfg(target_os = "windows")]
    {
        use std::mem::size_of;

        let vk_codes: Vec<VIRTUAL_KEY> = keys.iter()
            .filter_map(|k| k.as_str())
            .map(|k| key_name_to_vk(k))
            .collect();

        // Press all keys down
        let mut inputs: Vec<INPUT> = Vec::new();
        for &vk in &vk_codes {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYBD_EVENT_FLAGS(0),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
        // Release all keys (reverse order)
        for &vk in vk_codes.iter().rev() {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        unsafe {
            SendInput(&inputs, size_of::<INPUT>() as i32);
        }

        let key_names: Vec<&str> = keys.iter().filter_map(|k| k.as_str()).collect();
        Ok(json!({ "combo": key_names.join("+") }))
    }

    #[cfg(not(target_os = "windows"))]
    Err((ERR_NOT_IMPLEMENTED, "KeyCombo only available on Windows".into()))
}

#[cfg(target_os = "windows")]
fn key_name_to_vk(name: &str) -> VIRTUAL_KEY {
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => VK_CONTROL,
        "alt" | "opt" | "option" => VK_MENU,
        "shift" => VK_SHIFT,
        "cmd" | "win" | "super" | "meta" => VK_LWIN,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" | "esc" => VK_ESCAPE,
        "space" => VK_SPACE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "f1" => VK_F1, "f2" => VK_F2, "f3" => VK_F3, "f4" => VK_F4,
        "f5" => VK_F5, "f6" => VK_F6, "f7" => VK_F7, "f8" => VK_F8,
        "f9" => VK_F9, "f10" => VK_F10, "f11" => VK_F11, "f12" => VK_F12,
        s if s.len() == 1 => {
            let ch = s.chars().next().unwrap().to_ascii_uppercase();
            VIRTUAL_KEY(ch as u16)
        }
        _ => VK_SPACE, // fallback
    }
}
```

**Step 3: Wire into dispatch**

In `apps/desktop/src-tauri/src/bridge/mod.rs`, replace the stub calls for `METHOD_CLICK`, `METHOD_TYPE_TEXT`, and `METHOD_KEY_COMBO` with calls to `action::handle_click`, `action::handle_type_text`, and `action::handle_key_combo`.

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/apps/desktop && cargo check` (on macOS, Windows stubs return ERR_NOT_IMPLEMENTED)

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/action.rs apps/desktop/src-tauri/src/bridge/mod.rs apps/desktop/src-tauri/Cargo.toml
git commit -m "desktop-bridge: implement Windows input simulation (click, type, key combo)"
```

---

### Task 9: Window management (list, focus, launch)

**Files:**
- Create: `apps/desktop/src-tauri/src/bridge/window_mgmt.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Step 1: Create window_mgmt.rs**

Implement `handle_window_list`, `handle_focus_window`, and `handle_launch_app` using Windows APIs (`EnumWindows`, `SetForegroundWindow`, `ShellExecuteW`). Follow the same pattern as `action.rs` — `#[cfg(target_os = "windows")]` with `ERR_NOT_IMPLEMENTED` fallback.

**Step 2: Wire dispatch and verify**

**Step 3: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/window_mgmt.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "desktop-bridge: implement Windows window management (list, focus, launch)"
```

---

### Task 10: OCR via Windows.Media.Ocr

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs`
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add WinRT features)

**Step 1: Add WinRT features**

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    # ... existing features ...
    "Media_Ocr",
    "Globalization",
    "Graphics_Imaging",
    "Storage_Streams",
] }
```

**Step 2: Implement `handle_ocr` in perception.rs**

Use `Windows::Media::Ocr::OcrEngine` with `Language::CreateLanguage("zh-Hans")`. Decode PNG bytes to `SoftwareBitmap`, call `RecognizeAsync`, extract text + bounding boxes.

**Step 3: Verify and commit**

```bash
git commit -m "desktop-bridge: implement Windows OCR via WinRT Media.Ocr"
```

---

### Task 11: UI Automation (AX Tree)

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs`
- Modify: `apps/desktop/src-tauri/Cargo.toml`

**Step 1: Add UI Automation features**

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    # ... existing features ...
    "Win32_UI_Accessibility",
    "Win32_System_Com",
] }
```

**Step 2: Implement `handle_ax_tree`**

Use `IUIAutomation::GetFocusedElement()` or `ElementFromHandle(hwnd)`, walk tree with `CreateTreeWalker`, extract role/name/bounds/children up to 5 levels deep.

**Step 3: Commit**

```bash
git commit -m "desktop-bridge: implement Windows UI Automation accessibility tree"
```

---

### Task 12: Canvas overlay via Tauri WebView

**Files:**
- Create: `apps/desktop/src-tauri/src/bridge/canvas.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Step 1: Implement canvas handlers**

Use Tauri's window API to create/show/hide overlay windows and evaluate JavaScript for A2UI patches. The Tauri app already has `halo` and `settings` windows defined — Canvas uses a similar pattern with a dynamic WebView window.

**Step 2: Wire dispatch and commit**

```bash
git commit -m "desktop-bridge: implement Canvas overlay via Tauri WebView"
```

---

## Phase 3: Media Understanding Pipeline

### Task 13: Vision module scaffold + trait

**Files:**
- Create: `core/src/vision/mod.rs`
- Create: `core/src/vision/provider.rs`
- Create: `core/src/vision/types.rs`
- Create: `core/src/vision/error.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create types and trait**

Define `VisionProvider` trait, `ImageInput` enum, `VisionResult`, `OcrResult` types as specified in the design doc. Add `#[cfg(test)]` module with mock provider tests.

**Step 2: Commit**

```bash
git commit -m "vision: scaffold module with VisionProvider trait and types"
```

---

### Task 14: Claude Vision provider

**Files:**
- Create: `core/src/vision/providers/mod.rs`
- Create: `core/src/vision/providers/claude.rs`

**Step 1: Implement ClaudeVisionProvider**

Uses the existing `providers/` infrastructure (Claude API client) to send images with prompts. Returns `VisionResult` with description and identified elements.

**Step 2: Commit**

```bash
git commit -m "vision: add Claude Vision provider implementation"
```

---

### Task 15: Platform OCR provider + VisionTool

**Files:**
- Create: `core/src/vision/providers/platform_ocr.rs`
- Create: `core/src/builtin_tools/vision.rs`
- Modify: `core/src/builtin_tools/mod.rs`
- Modify: `core/src/tools/builtin.rs`

**Step 1: Create PlatformOcrProvider**

Delegates to Desktop Bridge `desktop.ocr` method. Acts as adapter between VisionProvider trait and existing Desktop Bridge OCR.

**Step 2: Create VisionTool**

AlephTool implementation that wraps VisionPipeline. Actions: `understand`, `ocr`. Takes `image_base64` or uses Desktop Bridge screenshot.

**Step 3: Commit**

```bash
git commit -m "vision: add Platform OCR provider and VisionTool"
```

---

## Phase 4: Permission & Approval

### Task 16: Approval module scaffold

**Files:**
- Create: `core/src/approval/mod.rs`
- Create: `core/src/approval/policy.rs`
- Create: `core/src/approval/types.rs`
- Create: `core/src/approval/config.rs`
- Modify: `core/src/lib.rs`

**Step 1: Define ApprovalPolicy trait and types**

As specified in design doc. Include `ActionType` enum covering browser, desktop, and shell actions. Implement `ConfigApprovalPolicy` that loads from `~/.aleph/approval-policy.json`.

**Step 2: Add tests for allowlist/blocklist pattern matching**

**Step 3: Commit**

```bash
git commit -m "approval: scaffold module with ApprovalPolicy trait and config"
```

---

### Task 17: Wire approval into BrowserTool + DesktopTool

**Files:**
- Modify: `core/src/builtin_tools/browser.rs`
- Modify: `core/src/builtin_tools/desktop.rs`

**Step 1: Add approval checks**

Before executing browser navigation or desktop actions, check `ApprovalPolicy::check()`. If `Ask`, return a message to the agent requesting user confirmation.

**Step 2: Run all tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test`

**Step 3: Commit**

```bash
git commit -m "approval: integrate approval checks into BrowserTool and DesktopTool"
```

---

## Summary

| Task | Phase | Component | Commit Message |
|------|-------|-----------|---------------|
| 1 | 1 | Browser scaffold + types | `browser: scaffold module with types and error definitions` |
| 2 | 1 | Chromium discovery | `browser: add cross-platform Chromium discovery` |
| 3 | 1 | CDP runtime | `browser: add BrowserRuntime with CDP transport` |
| 4 | 1 | ARIA snapshot | `browser: add ARIA snapshot extraction` |
| 5 | 1 | Browser actions | `browser: add click, type, fill, scroll, hover actions` |
| 6 | 1 | BrowserTool | `browser: add BrowserTool with AlephTool implementation` |
| 7 | 1 | Integration test | `browser: add integration test for full browser workflow` |
| 8 | 2 | Windows input sim | `desktop-bridge: implement Windows input simulation` |
| 9 | 2 | Windows window mgmt | `desktop-bridge: implement Windows window management` |
| 10 | 2 | Windows OCR | `desktop-bridge: implement Windows OCR via WinRT` |
| 11 | 2 | Windows UI Automation | `desktop-bridge: implement Windows UI Automation AX tree` |
| 12 | 2 | Canvas overlay | `desktop-bridge: implement Canvas overlay via Tauri WebView` |
| 13 | 3 | Vision scaffold | `vision: scaffold module with VisionProvider trait` |
| 14 | 3 | Claude Vision | `vision: add Claude Vision provider` |
| 15 | 3 | VisionTool | `vision: add Platform OCR provider and VisionTool` |
| 16 | 4 | Approval scaffold | `approval: scaffold module with ApprovalPolicy trait` |
| 17 | 4 | Approval integration | `approval: integrate into BrowserTool and DesktopTool` |
