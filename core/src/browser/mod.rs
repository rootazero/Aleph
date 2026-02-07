//! Browser Control Service
//!
//! Provides browser automation via Chrome DevTools Protocol (CDP).
//!
//! # Features
//!
//! - Launch and manage Chrome/Chromium instances
//! - Navigate to URLs
//! - Take screenshots (viewport or full page)
//! - Get page accessibility snapshots
//! - Click elements, type text
//! - Run JavaScript evaluation
//! - Tab management
//! - Multi-context management (BrowserPool)
//! - Persistent session support
//!
//! # Usage
//!
//! ## Legacy BrowserService (single-session)
//!
//! ```rust,ignore
//! use alephcore::browser::{BrowserService, BrowserConfig};
//!
//! let config = BrowserConfig::default();
//! let mut service = BrowserService::new(config)?;
//!
//! // Start browser
//! service.start().await?;
//!
//! // Navigate
//! service.navigate("https://example.com").await?;
//!
//! // Take screenshot
//! let screenshot = service.screenshot(Default::default()).await?;
//!
//! // Get snapshot
//! let snapshot = service.snapshot().await?;
//!
//! // Stop browser
//! service.stop().await?;
//! ```
//!
//! ## BrowserPool (multi-context, recommended)
//!
//! ```rust,ignore
//! use alephcore::browser::{BrowserPool, BrowserConfig, AllocationPolicy};
//!
//! let config = BrowserConfig::default();
//! let mut pool = BrowserPool::new(config, AllocationPolicy::Adaptive)?;
//!
//! // Start primary instance
//! pool.start().await?;
//!
//! // Get primary context for user operations
//! let primary_ctx = pool.get_primary_context().await?;
//!
//! // Create ephemeral context for isolated task
//! let task_ctx = pool.create_ephemeral_context("task-123").await?;
//! ```

pub mod config;
pub mod context_registry;
pub mod resource_monitor;

pub use config::{
    ActionResult, BrowserConfig, ClickOptions, PageSnapshot, ScreenshotOptions,
    SnapshotNode, TabInfo, TypeOptions,
};

use std::collections::HashMap;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;

#[cfg(feature = "browser")]
use chromiumoxide::{
    browser::{Browser, BrowserConfig as CdpBrowserConfig},
    cdp::browser_protocol::page::CaptureScreenshotFormat,
    handler::viewport::Viewport,
    Page,
};
#[cfg(feature = "browser")]
use futures::StreamExt;

/// Result type for browser operations
pub type BrowserResult<T> = Result<T, BrowserError>;

/// Errors that can occur in browser operations
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("Browser not started")]
    NotStarted,

    #[error("Browser already running")]
    AlreadyRunning,

    #[error("Chrome executable not found")]
    ExecutableNotFound,

    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("CDP connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Element not found: {0}")]
    ElementNotFound(String),

    #[error("Action failed: {0}")]
    ActionFailed(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Element reference cache for stable targeting
#[derive(Debug, Clone)]
pub struct ElementRef {
    pub ref_id: String,
    pub selector: String,
    pub role: String,
    pub name: String,
}

/// Browser service for Chrome/Chromium automation
pub struct BrowserService {
    /// Configuration
    config: BrowserConfig,
    /// Chrome process (when launched by us)
    #[allow(dead_code)]
    process: Option<Child>,
    /// CDP browser handle
    #[cfg(feature = "browser")]
    browser: Option<Browser>,
    /// Current page
    #[cfg(feature = "browser")]
    page: Option<Page>,
    /// Element reference cache
    element_refs: Arc<RwLock<HashMap<String, ElementRef>>>,
    /// Reference counter for generating IDs
    ref_counter: Arc<RwLock<u32>>,
}

impl BrowserService {
    /// Create a new browser service
    pub fn new(config: BrowserConfig) -> BrowserResult<Self> {
        config.validate().map_err(BrowserError::ConfigError)?;

        Ok(Self {
            config,
            process: None,
            #[cfg(feature = "browser")]
            browser: None,
            #[cfg(feature = "browser")]
            page: None,
            element_refs: Arc::new(RwLock::new(HashMap::new())),
            ref_counter: Arc::new(RwLock::new(0)),
        })
    }

    /// Check if browser is running
    pub fn is_running(&self) -> bool {
        #[cfg(feature = "browser")]
        {
            self.browser.is_some()
        }
        #[cfg(not(feature = "browser"))]
        {
            false
        }
    }

    /// Start the browser
    #[cfg(feature = "browser")]
    pub async fn start(&mut self) -> BrowserResult<()> {
        if self.browser.is_some() {
            return Err(BrowserError::AlreadyRunning);
        }

        let executable = self.config.find_executable()
            .ok_or(BrowserError::ExecutableNotFound)?;

        let user_data_dir = self.config.expand_user_data_dir();

        // Ensure user data directory exists
        if let Err(e) = std::fs::create_dir_all(&user_data_dir) {
            return Err(BrowserError::LaunchFailed(format!(
                "Failed to create user data dir: {}", e
            )));
        }

        tracing::info!(
            "Starting browser: {} (headless: {}, port: {})",
            executable.display(),
            self.config.headless,
            self.config.cdp_port
        );

        // Build browser config
        let mut builder = CdpBrowserConfig::builder()
            .chrome_executable(executable)
            .arg(format!("--remote-debugging-port={}", self.config.cdp_port))
            .arg(format!("--user-data-dir={}", user_data_dir.display()))
            .arg("--no-first-run")
            .arg("--disable-sync")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-features=Translate,MediaRouter");

        if self.config.headless {
            builder = builder.arg("--headless=new");
        }

        // Add extra args
        for arg in &self.config.extra_args {
            builder = builder.arg(arg);
        }

        // Set viewport
        builder = builder.viewport(Viewport {
            width: self.config.viewport_width,
            height: self.config.viewport_height,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: true,
            has_touch: false,
        });

        let browser_config = builder.build()
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn handler task
        tokio::spawn(async move {
            loop {
                match handler.next().await {
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        });

        // Get initial page
        let page = browser.new_page("about:blank")
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        self.browser = Some(browser);
        self.page = Some(page);

        tracing::info!("Browser started successfully");
        Ok(())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn start(&mut self) -> BrowserResult<()> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Stop the browser
    #[cfg(feature = "browser")]
    pub async fn stop(&mut self) -> BrowserResult<()> {
        if let Some(mut browser) = self.browser.take() {
            // Close all pages
            self.page = None;

            // Close browser
            if let Err(e) = browser.close().await {
                tracing::warn!("Error closing browser: {}", e);
            }

            // Kill process if needed
            if let Some(mut proc) = self.process.take() {
                let _ = proc.kill();
            }

            // Clear element refs
            self.element_refs.write().await.clear();
            *self.ref_counter.write().await = 0;

            tracing::info!("Browser stopped");
        }
        Ok(())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn stop(&mut self) -> BrowserResult<()> {
        Ok(())
    }

    /// Navigate to URL
    #[cfg(feature = "browser")]
    pub async fn navigate(&mut self, url: &str) -> BrowserResult<ActionResult> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Navigating to: {}", url);

        page.goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        // Wait for page to load
        sleep(Duration::from_millis(500)).await;

        // Clear element refs on navigation
        self.element_refs.write().await.clear();
        *self.ref_counter.write().await = 0;

        let title = page.get_title()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?
            .unwrap_or_default();

        Ok(ActionResult::success_with_data(serde_json::json!({
            "url": url,
            "title": title,
        })))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn navigate(&mut self, _url: &str) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Take screenshot
    #[cfg(feature = "browser")]
    pub async fn screenshot(&self, options: ScreenshotOptions) -> BrowserResult<ActionResult> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Taking screenshot (full_page: {})", options.full_page);

        let format = match options.format.as_str() {
            "jpeg" | "jpg" => CaptureScreenshotFormat::Jpeg,
            _ => CaptureScreenshotFormat::Png,
        };

        let screenshot = if options.full_page {
            page.screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(format)
                    .full_page(true)
                    .build(),
            )
            .await
        } else {
            page.screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(format)
                    .build(),
            )
            .await
        };

        let data = screenshot.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);

        Ok(ActionResult::success_with_data(serde_json::json!({
            "image": base64_data,
            "format": options.format,
            "size": data.len(),
        })))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn screenshot(&self, _options: ScreenshotOptions) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Get page snapshot (accessibility tree)
    #[cfg(feature = "browser")]
    pub async fn snapshot(&mut self) -> BrowserResult<PageSnapshot> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Getting page snapshot");

        let url = page.url()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?
            .unwrap_or_default();

        let title = page.get_title()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?
            .unwrap_or_default();

        // Get accessibility tree via JavaScript
        let js = r#"
            (function() {
                const nodes = [];
                const walk = (node, depth) => {
                    if (depth > 10) return; // Limit depth

                    const role = node.getAttribute?.('role') || node.tagName?.toLowerCase() || '';
                    const ariaLabel = node.getAttribute?.('aria-label') || '';
                    const innerText = node.innerText?.slice(0, 100) || '';

                    const interactive = ['a', 'button', 'input', 'select', 'textarea'].includes(node.tagName?.toLowerCase()) ||
                        node.getAttribute?.('onclick') ||
                        node.getAttribute?.('role') === 'button' ||
                        node.getAttribute?.('role') === 'link';

                    if (role || ariaLabel || (interactive && innerText)) {
                        nodes.push({
                            role: role || node.tagName?.toLowerCase() || 'generic',
                            name: ariaLabel || innerText.trim().slice(0, 50),
                            value: node.value || null,
                            depth: depth,
                            interactive: interactive,
                            tagName: node.tagName?.toLowerCase() || '',
                        });
                    }

                    if (nodes.length < 500) { // Limit total nodes
                        for (const child of node.children || []) {
                            walk(child, depth + 1);
                        }
                    }
                };
                walk(document.body, 0);
                return nodes;
            })()
        "#;

        let result: Vec<serde_json::Value> = page.evaluate(js)
            .await
            .map_err(|e| BrowserError::ActionFailed(format!("Failed to get snapshot: {}", e)))?
            .into_value()
            .map_err(|e| BrowserError::ActionFailed(format!("Failed to parse snapshot: {}", e)))?;

        // Convert to SnapshotNodes with refs
        let mut nodes = Vec::new();
        let mut interactive_count = 0;
        let mut refs = self.element_refs.write().await;
        let mut counter = self.ref_counter.write().await;

        for item in &result {
            *counter += 1;
            let ref_id = format!("e{}", *counter);

            let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("generic").to_string();
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let value = item.get("value").and_then(|v| v.as_str()).map(|s| s.to_string());
            let depth = item.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let interactive = item.get("interactive").and_then(|v| v.as_bool()).unwrap_or(false);
            let tag_name = item.get("tagName").and_then(|v| v.as_str()).unwrap_or("").to_string();

            if interactive {
                interactive_count += 1;
            }

            // Build selector for this element
            let selector = if !name.is_empty() {
                format!("[aria-label=\"{}\"], {}:contains(\"{}\")", name, tag_name, name)
            } else {
                tag_name.clone()
            };

            refs.insert(ref_id.clone(), ElementRef {
                ref_id: ref_id.clone(),
                selector,
                role: role.clone(),
                name: name.clone(),
            });

            nodes.push(SnapshotNode {
                ref_id,
                role,
                name,
                value,
                depth,
                interactive,
            });
        }

        let truncated = result.len() >= 500;

        Ok(PageSnapshot {
            url,
            title,
            total_elements: nodes.len(),
            interactive_count,
            nodes,
            truncated,
        })
    }

    #[cfg(not(feature = "browser"))]
    pub async fn snapshot(&mut self) -> BrowserResult<PageSnapshot> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Click element by ref or selector
    #[cfg(feature = "browser")]
    pub async fn click(&self, target: &str, options: ClickOptions) -> BrowserResult<ActionResult> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Clicking: {} (double: {})", target, options.double_click);

        // Resolve target to selector
        let selector = self.resolve_target(target).await?;

        // Find element
        let element = page.find_element(&selector)
            .await
            .map_err(|_| BrowserError::ElementNotFound(target.to_string()))?;

        // Click
        if options.double_click {
            element.click().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
            sleep(Duration::from_millis(100)).await;
            element.click().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        } else {
            element.click().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        }

        if options.delay_ms > 0 {
            sleep(Duration::from_millis(options.delay_ms)).await;
        }

        Ok(ActionResult::success())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn click(&self, _target: &str, _options: ClickOptions) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Type text into element
    #[cfg(feature = "browser")]
    pub async fn type_text(&self, target: &str, text: &str, options: TypeOptions) -> BrowserResult<ActionResult> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Typing into: {}", target);

        // Resolve target to selector
        let selector = self.resolve_target(target).await?;

        // Find element
        let element = page.find_element(&selector)
            .await
            .map_err(|_| BrowserError::ElementNotFound(target.to_string()))?;

        // Clear if requested
        if options.clear {
            element.click().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
            page.evaluate("document.activeElement.value = ''")
                .await
                .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        }

        // Type text
        if options.slowly {
            for c in text.chars() {
                element.type_str(&c.to_string())
                    .await
                    .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
                sleep(Duration::from_millis(options.keystroke_delay_ms)).await;
            }
        } else {
            element.type_str(text)
                .await
                .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        }

        // Submit if requested
        if options.submit {
            element.press_key("Enter")
                .await
                .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        }

        Ok(ActionResult::success())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn type_text(&self, _target: &str, _text: &str, _options: TypeOptions) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Evaluate JavaScript
    #[cfg(feature = "browser")]
    pub async fn evaluate(&self, script: &str) -> BrowserResult<ActionResult> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;

        tracing::debug!("Evaluating JavaScript");

        let result: serde_json::Value = page.evaluate(script)
            .await
            .map_err(|e| BrowserError::ActionFailed(format!("JS evaluation failed: {}", e)))?
            .into_value()
            .unwrap_or(serde_json::Value::Null);

        Ok(ActionResult::success_with_data(serde_json::json!({
            "result": result,
        })))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn evaluate(&self, _script: &str) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// List open tabs
    #[cfg(feature = "browser")]
    pub async fn list_tabs(&self) -> BrowserResult<Vec<TabInfo>> {
        let browser = self.browser.as_ref().ok_or(BrowserError::NotStarted)?;

        let pages = browser.pages()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

        let mut tabs = Vec::new();
        let current_url = self.page.as_ref()
            .and_then(|p| futures::executor::block_on(p.url()).ok())
            .flatten();

        for page in pages {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            let active = current_url.as_ref() == Some(&url);

            tabs.push(TabInfo {
                id: url.clone(), // Use URL as ID for simplicity
                url,
                title,
                active,
            });
        }

        Ok(tabs)
    }

    #[cfg(not(feature = "browser"))]
    pub async fn list_tabs(&self) -> BrowserResult<Vec<TabInfo>> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Open new tab
    #[cfg(feature = "browser")]
    pub async fn new_tab(&mut self, url: Option<&str>) -> BrowserResult<ActionResult> {
        let browser = self.browser.as_ref().ok_or(BrowserError::NotStarted)?;

        let url = url.unwrap_or("about:blank");
        let page = browser.new_page(url)
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

        // Clear element refs for new tab
        self.element_refs.write().await.clear();
        *self.ref_counter.write().await = 0;

        self.page = Some(page);

        Ok(ActionResult::success_with_data(serde_json::json!({
            "url": url,
        })))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn new_tab(&mut self, _url: Option<&str>) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Close current tab
    #[cfg(feature = "browser")]
    pub async fn close_tab(&mut self) -> BrowserResult<ActionResult> {
        if let Some(page) = self.page.take() {
            page.close().await.map_err(|e| BrowserError::ActionFailed(e.to_string()))?;

            // Clear element refs
            self.element_refs.write().await.clear();
            *self.ref_counter.write().await = 0;

            // Get another page if available
            if let Some(browser) = &self.browser {
                if let Ok(pages) = browser.pages().await {
                    if let Some(p) = pages.into_iter().next() {
                        self.page = Some(p);
                    }
                }
            }
        }

        Ok(ActionResult::success())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn close_tab(&mut self) -> BrowserResult<ActionResult> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Resolve target (ref or selector) to CSS selector
    async fn resolve_target(&self, target: &str) -> BrowserResult<String> {
        // Check if it's a ref (e1, e2, ...)
        if target.starts_with('e') && target[1..].parse::<u32>().is_ok() {
            let refs = self.element_refs.read().await;
            if let Some(elem_ref) = refs.get(target) {
                return Ok(elem_ref.selector.clone());
            }
        }

        // Otherwise treat as CSS selector
        Ok(target.to_string())
    }

    /// Get current page URL
    #[cfg(feature = "browser")]
    pub async fn current_url(&self) -> BrowserResult<String> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;
        page.url()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?
            .ok_or(BrowserError::ActionFailed("No URL".to_string()))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn current_url(&self) -> BrowserResult<String> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Get current page title
    #[cfg(feature = "browser")]
    pub async fn current_title(&self) -> BrowserResult<String> {
        let page = self.page.as_ref().ok_or(BrowserError::NotStarted)?;
        page.get_title()
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?
            .ok_or(BrowserError::ActionFailed("No title".to_string()))
    }

    #[cfg(not(feature = "browser"))]
    pub async fn current_title(&self) -> BrowserResult<String> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }
}

impl Drop for BrowserService {
    fn drop(&mut self) {
        // Kill process if still running
        if let Some(mut proc) = self.process.take() {
            let _ = proc.kill();
        }
    }
}

#[cfg(all(test, feature = "browser"))]
mod tests {
    use super::*;

    #[test]
    fn test_browser_service_creation() {
        let config = BrowserConfig::default();
        let service = BrowserService::new(config);
        assert!(service.is_ok());
    }

    #[test]
    fn test_resolve_target() {
        // This would need an async runtime to test properly
    }
}

// ============================================================================
// BrowserPool - Multi-Context Browser Management
// ============================================================================

use context_registry::{ContextRegistry, TaskId};
use resource_monitor::ResourceMonitor;

/// Allocation policy for browser instances
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationPolicy {
    /// All contexts share one browser process
    SingleInstance,
    /// Each context gets a dedicated browser process
    MultiInstance,
    /// Automatically decide based on system resources
    Adaptive,
}

/// Browser pool for managing multiple browser instances and contexts
pub struct BrowserPool {
    /// Configuration
    config: BrowserConfig,

    /// Allocation policy
    allocation_policy: AllocationPolicy,

    /// Primary browser instance (persistent user context)
    #[cfg(feature = "browser")]
    primary_instance: Arc<RwLock<Option<Browser>>>,

    /// Shared browser instance pool (for normal tasks)
    #[cfg(feature = "browser")]
    shared_instances: Arc<RwLock<Vec<Browser>>>,

    /// Dedicated browser instances (for high-risk tasks)
    #[cfg(feature = "browser")]
    dedicated_instances: Arc<RwLock<HashMap<TaskId, Browser>>>,

    /// Context registry
    context_registry: Arc<ContextRegistry>,

    /// Resource monitor
    resource_monitor: Arc<ResourceMonitor>,

    /// Chrome processes
    processes: Arc<RwLock<Vec<Child>>>,
}

impl BrowserPool {
    /// Create a new browser pool
    pub fn new(config: BrowserConfig, allocation_policy: AllocationPolicy) -> BrowserResult<Self> {
        config.validate().map_err(BrowserError::ConfigError)?;

        Ok(Self {
            config,
            allocation_policy,
            #[cfg(feature = "browser")]
            primary_instance: Arc::new(RwLock::new(None)),
            #[cfg(feature = "browser")]
            shared_instances: Arc::new(RwLock::new(Vec::new())),
            #[cfg(feature = "browser")]
            dedicated_instances: Arc::new(RwLock::new(HashMap::new())),
            context_registry: Arc::new(ContextRegistry::new()),
            resource_monitor: Arc::new(ResourceMonitor::new()),
            processes: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Start the browser pool (launches primary instance)
    #[cfg(feature = "browser")]
    pub async fn start(&mut self) -> BrowserResult<()> {
        // Update resource monitor
        self.resource_monitor.update().await;

        // Launch primary browser instance
        let executable = self.config.find_executable()
            .ok_or(BrowserError::ExecutableNotFound)?;

        let user_data_dir = self.config.expand_user_data_dir();

        // Ensure user data directory exists
        if let Err(e) = std::fs::create_dir_all(&user_data_dir) {
            return Err(BrowserError::LaunchFailed(format!(
                "Failed to create user data dir: {}", e
            )));
        }

        tracing::info!(
            "Starting primary browser instance: {} (headless: {}, port: {})",
            executable.display(),
            self.config.headless,
            self.config.cdp_port
        );

        // Build browser config
        let mut builder = CdpBrowserConfig::builder()
            .chrome_executable(executable)
            .arg(format!("--remote-debugging-port={}", self.config.cdp_port))
            .arg(format!("--user-data-dir={}", user_data_dir.display()))
            .arg("--no-first-run")
            .arg("--disable-sync")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-features=Translate,MediaRouter");

        if self.config.headless {
            builder = builder.arg("--headless=new");
        }

        // Add extra args
        for arg in &self.config.extra_args {
            builder = builder.arg(arg);
        }

        // Set viewport
        builder = builder.viewport(Viewport {
            width: self.config.viewport_width,
            height: self.config.viewport_height,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: true,
            has_touch: false,
        });

        let browser_config = builder.build()
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn handler task
        tokio::spawn(async move {
            loop {
                match handler.next().await {
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        });

        // Get default page (primary context)
        let primary_page = browser.new_page("about:blank")
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        self.context_registry.set_primary_context(
            Arc::new(primary_page),
            Some(user_data_dir)
        ).await;

        // Store primary instance
        *self.primary_instance.write().await = Some(browser);

        // Update resource monitor
        self.resource_monitor.set_active_instances(1).await;

        tracing::info!("Primary browser instance started successfully");
        Ok(())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn start(&mut self) -> BrowserResult<()> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Stop the browser pool
    #[cfg(feature = "browser")]
    pub async fn stop(&mut self) -> BrowserResult<()> {
        // Close primary instance
        if let Some(mut browser) = self.primary_instance.write().await.take() {
            if let Err(e) = browser.close().await {
                tracing::warn!("Error closing primary browser: {}", e);
            }
        }

        // Close shared instances
        let mut shared = self.shared_instances.write().await;
        for mut browser in shared.drain(..) {
            if let Err(e) = browser.close().await {
                tracing::warn!("Error closing shared browser: {}", e);
            }
        }

        // Close dedicated instances
        let mut dedicated = self.dedicated_instances.write().await;
        for (task_id, mut browser) in dedicated.drain() {
            tracing::debug!("Closing dedicated browser for task: {}", task_id);
            if let Err(e) = browser.close().await {
                tracing::warn!("Error closing dedicated browser: {}", e);
            }
        }

        // Kill processes
        let mut processes = self.processes.write().await;
        for mut proc in processes.drain(..) {
            let _ = proc.kill();
        }

        // Clear context registry
        self.context_registry.clear_ephemeral_contexts().await;

        // Update resource monitor
        self.resource_monitor.set_active_instances(0).await;

        tracing::info!("Browser pool stopped");
        Ok(())
    }

    #[cfg(not(feature = "browser"))]
    pub async fn stop(&mut self) -> BrowserResult<()> {
        Ok(())
    }

    /// Get the primary context
    pub async fn get_primary_context(&self) -> BrowserResult<context_registry::ContextHandle> {
        self.context_registry.get_primary_context().await
            .ok_or(BrowserError::NotStarted)
    }

    /// Create an ephemeral context for a task
    #[cfg(feature = "browser")]
    pub async fn create_ephemeral_context(&self, task_id: TaskId) -> BrowserResult<context_registry::ContextHandle> {
        // Check allocation policy
        let should_use_dedicated = match self.allocation_policy {
            AllocationPolicy::SingleInstance => false,
            AllocationPolicy::MultiInstance => true,
            AllocationPolicy::Adaptive => {
                self.resource_monitor.can_handle_multi_instance().await
            }
        };

        if should_use_dedicated {
            // Create dedicated browser instance
            // TODO: Implement dedicated instance creation
            return Err(BrowserError::Internal("Dedicated instances not yet implemented".to_string()));
        }

        // Use primary instance to create new page
        let primary = self.primary_instance.read().await;
        let browser = primary.as_ref().ok_or(BrowserError::NotStarted)?;

        let page = browser.new_page("about:blank")
            .await
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        let context_handle = Arc::new(page);

        self.context_registry.create_ephemeral_context(task_id, context_handle.clone()).await;

        Ok(context_handle)
    }

    #[cfg(not(feature = "browser"))]
    pub async fn create_ephemeral_context(&self, _task_id: TaskId) -> BrowserResult<context_registry::ContextHandle> {
        Err(BrowserError::Internal("Browser feature not enabled".to_string()))
    }

    /// Get an ephemeral context by task ID
    pub async fn get_ephemeral_context(&self, task_id: &TaskId) -> Option<context_registry::ContextHandle> {
        self.context_registry.get_ephemeral_context(task_id).await
    }

    /// Remove an ephemeral context
    pub async fn remove_ephemeral_context(&self, task_id: &TaskId) -> Option<context_registry::ContextHandle> {
        self.context_registry.remove_ephemeral_context(task_id).await
    }

    /// Get the context registry
    pub fn context_registry(&self) -> &Arc<ContextRegistry> {
        &self.context_registry
    }

    /// Get the resource monitor
    pub fn resource_monitor(&self) -> &Arc<ResourceMonitor> {
        &self.resource_monitor
    }

    /// Get current allocation policy
    pub fn allocation_policy(&self) -> AllocationPolicy {
        self.allocation_policy
    }

    /// Update allocation policy
    pub fn set_allocation_policy(&mut self, policy: AllocationPolicy) {
        self.allocation_policy = policy;
    }
}

impl Drop for BrowserPool {
    fn drop(&mut self) {
        // Kill processes if still running
        if let Ok(mut processes) = self.processes.try_write() {
            for mut proc in processes.drain(..) {
                let _ = proc.kill();
            }
        }
    }
}
