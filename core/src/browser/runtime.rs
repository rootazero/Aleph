//! BrowserRuntime — CDP transport layer via chromiumoxide.
//!
//! Wraps a headless (or headed) Chromium instance launched through the Chrome
//! DevTools Protocol. Provides high-level tab management, navigation,
//! screenshot capture, and JavaScript evaluation.

use std::collections::HashMap;

use base64::Engine as _;
use chromiumoxide::browser::{
    Browser, BrowserConfig as CdpBrowserConfig, BrowserConfigBuilder,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::Page;
use futures::StreamExt;

use super::discovery::find_chromium;
use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, BrowserConfig, LaunchMode, ScreenshotOpts, ScreenshotResult,
    ScrollDirection, TabId, TabInfo,
};

/// A running browser instance managed through CDP.
///
/// `BrowserRuntime` owns the chromiumoxide [`Browser`], a map of open
/// [`Page`]s keyed by their target ID, and the background task that drives the
/// CDP event loop.
pub struct BrowserRuntime {
    browser: Browser,
    pages: HashMap<String, Page>,
    #[allow(dead_code)] // Retained for introspection / future reconnect logic.
    config: BrowserConfig,
    _handle: tokio::task::JoinHandle<()>,
}

impl BrowserRuntime {
    /// Launch (or connect to) a Chromium instance described by `config`.
    ///
    /// The CDP event handler is spawned onto the current Tokio runtime and
    /// stored internally. Call [`stop`] to tear everything down.
    pub async fn start(config: BrowserConfig) -> Result<Self, BrowserError> {
        let cdp_config = build_cdp_config(&config)?;

        let (browser, mut handler) = match &config.mode {
            LaunchMode::Connect { endpoint } => {
                Browser::connect(endpoint.as_str())
                    .await
                    .map_err(|e| BrowserError::ConnectionFailed(e.to_string()))?
            }
            _ => {
                Browser::launch(cdp_config)
                    .await
                    .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?
            }
        };

        // Spawn the CDP event loop so the browser stays responsive.
        let handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    tracing::warn!("CDP handler event error: {e}");
                    // Continue processing — transient errors are expected
                }
            }
            tracing::debug!("CDP handler loop exited");
        });

        Ok(Self {
            browser,
            pages: HashMap::new(),
            config,
            _handle: handle,
        })
    }

    /// Returns `true` if the CDP event handler task is still running.
    pub fn is_running(&self) -> bool {
        !self._handle.is_finished()
    }

    /// Open a new tab navigating to `url`.
    ///
    /// Returns the unique [`TabId`] (the CDP target ID) for the new tab.
    pub async fn open_tab(&mut self, url: &str) -> Result<TabId, BrowserError> {
        let page = self
            .browser
            .new_page(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;

        let tab_id: String = page.target_id().as_ref().to_string();
        self.pages.insert(tab_id.clone(), page);
        Ok(tab_id)
    }

    /// Close the tab identified by `tab_id`.
    ///
    /// The page is removed from the internal map and its CDP target is closed.
    pub async fn close_tab(&mut self, tab_id: &str) -> Result<(), BrowserError> {
        let page = self
            .pages
            .remove(tab_id)
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))?;

        page.close()
            .await
            .map_err(|e| BrowserError::ActionFailed(format!("Failed to close tab: {e}")))?;

        Ok(())
    }

    /// List all open tabs managed by this runtime.
    ///
    /// Retrieves the current URL and title for each page. Pages that fail to
    /// report metadata are still included with empty strings.
    pub async fn list_tabs(&self) -> Vec<TabInfo> {
        let mut tabs = Vec::with_capacity(self.pages.len());
        for (id, page) in &self.pages {
            let url = page
                .url()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let title = page
                .get_title()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            tabs.push(TabInfo {
                id: id.clone(),
                url,
                title,
            });
        }
        tabs
    }

    /// Navigate an existing tab to a new URL.
    pub async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        page.goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        Ok(())
    }

    /// Capture a screenshot of the given tab.
    ///
    /// The raw PNG/JPEG bytes are Base64-encoded and returned inside a
    /// [`ScreenshotResult`]. Image dimensions are extracted from the raw data
    /// when possible.
    pub async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        let page = self.find_page(tab_id)?;

        let format = match opts.format.as_str() {
            "jpeg" | "jpg" => CaptureScreenshotFormat::Jpeg,
            "webp" => CaptureScreenshotFormat::Webp,
            _ => CaptureScreenshotFormat::Png,
        };

        let params = ScreenshotParams::builder()
            .format(format)
            .quality(opts.quality as i64)
            .full_page(opts.full_page)
            .build();

        let bytes = page
            .screenshot(params)
            .await
            .map_err(|e| BrowserError::ScreenshotFailed(e.to_string()))?;

        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        // Try to extract image dimensions from the raw bytes.
        let (width, height) = extract_image_dimensions(&bytes).unwrap_or((0, 0));

        Ok(ScreenshotResult {
            data_base64,
            width,
            height,
            format: opts.format,
        })
    }

    /// Evaluate a JavaScript expression in the context of the given tab.
    ///
    /// The result is returned as a [`serde_json::Value`]. If the JS yields a
    /// non-serialisable value (e.g. `undefined`), [`serde_json::Value::Null`]
    /// is returned.
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

        // Try to deserialise the result into a generic JSON value.
        let value: serde_json::Value = result
            .into_value()
            .unwrap_or(serde_json::Value::Null);

        Ok(value)
    }

    /// Return the current URL of the given tab.
    pub async fn get_url(&self, tab_id: &str) -> Result<String, BrowserError> {
        let page = self.find_page(tab_id)?;
        page.url()
            .await
            .map_err(|e| BrowserError::Protocol(e.to_string()))?
            .ok_or_else(|| BrowserError::Protocol("Page has no URL".to_string()))
    }

    /// Return the document title of the given tab.
    pub async fn get_title(&self, tab_id: &str) -> Result<String, BrowserError> {
        let page = self.find_page(tab_id)?;
        page.get_title()
            .await
            .map_err(|e| BrowserError::Protocol(e.to_string()))?
            .ok_or_else(|| BrowserError::Protocol("Page has no title".to_string()))
    }

    /// Obtain a reference to the underlying [`Page`] for advanced CDP
    /// operations not covered by the convenience methods above.
    pub fn get_page(&self, tab_id: &str) -> Result<&Page, BrowserError> {
        self.find_page(tab_id)
    }

    // ── High-level action helpers ───────────────────────────────────────

    /// Click the element identified by `target` in the given tab.
    pub async fn click(
        &self,
        tab_id: &str,
        target: ActionTarget,
    ) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        super::actions::click(page, &target).await
    }

    /// Type (append) `text` into the element identified by `target`.
    pub async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        super::actions::type_text(page, &target, text).await
    }

    /// Fill (replace) the value of the element identified by `target`.
    pub async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        super::actions::fill(page, &target, value).await
    }

    /// Scroll the element at `target` in the given `direction`.
    pub async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        super::actions::scroll(page, &target, &direction).await
    }

    /// Hover over the element identified by `target`.
    pub async fn hover(
        &self,
        tab_id: &str,
        target: ActionTarget,
    ) -> Result<(), BrowserError> {
        let page = self.find_page(tab_id)?;
        super::actions::hover(page, &target).await
    }

    /// Take an ARIA accessibility snapshot of the given tab.
    pub async fn snapshot(
        &self,
        tab_id: &str,
    ) -> Result<AriaSnapshot, BrowserError> {
        let page = self.find_page(tab_id)?;
        super::snapshot::take_aria_snapshot(page).await
    }

    /// Shut down the browser and abort the CDP handler task.
    ///
    /// Consumes `self` — after this call the runtime is gone.
    pub async fn stop(self) -> Result<(), BrowserError> {
        // Dropping the browser triggers the CDP `Browser.close` command.
        drop(self.browser);

        // Give the handler loop a moment to finish gracefully.
        self._handle.abort();

        Ok(())
    }

    // ── Internal helpers ────────────────────────────────────────────────

    fn find_page(&self, tab_id: &str) -> Result<&Page, BrowserError> {
        self.pages
            .get(tab_id)
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))
    }

}

// ── Free functions ──────────────────────────────────────────────────────────

/// Translate our [`BrowserConfig`] into chromiumoxide's [`CdpBrowserConfig`].
///
/// Handles the three [`LaunchMode`] variants:
/// - **Auto** — discover a Chromium binary via [`find_chromium`]
/// - **Binary** — use the supplied path directly
/// - **Connect** — the config is not used for launching (caller dials
///   `Browser::connect` instead) so we still build a valid placeholder config
fn build_cdp_config(config: &BrowserConfig) -> Result<CdpBrowserConfig, BrowserError> {
    let mut builder: BrowserConfigBuilder = CdpBrowserConfig::builder();

    // Executable path
    match &config.mode {
        LaunchMode::Auto => {
            let path = find_chromium()?;
            builder = builder.chrome_executable(path);
        }
        LaunchMode::Binary { path } => {
            builder = builder.chrome_executable(std::path::PathBuf::from(path));
        }
        LaunchMode::Connect { .. } => {
            // When connecting to an existing browser we don't launch one, but
            // chromiumoxide still requires a valid config.  Try to locate a
            // binary anyway so the config builds without errors.
            if let Ok(path) = find_chromium() {
                builder = builder.chrome_executable(path);
            }
        }
    }

    // Headless mode
    if config.headless {
        // Default builder is headless; calling `with_head()` disables it.
        // We do nothing here to keep headless.
    } else {
        builder = builder.with_head();
    }

    // Port
    if config.cdp_port != 0 {
        builder = builder.port(config.cdp_port);
    }

    // User data directory
    if let Some(ref dir) = config.user_data_dir {
        builder = builder.user_data_dir(std::path::PathBuf::from(dir));
    }

    // Safety / automation args
    builder = builder
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--disable-infobars");

    // Extra user-supplied args
    for arg in &config.extra_args {
        builder = builder.arg(arg);
    }

    builder
        .build()
        .map_err(|e| BrowserError::LaunchFailed(e.to_string()))
}

/// Best-effort extraction of image width/height from raw PNG/JPEG bytes.
fn extract_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // PNG: width and height are at bytes 16-23 in the IHDR chunk.
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height));
    }

    // JPEG: scan for SOF0 (0xFFC0) marker.
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            // SOF0, SOF1, SOF2 markers
            if marker == 0xC0 || marker == 0xC1 || marker == 0xC2 {
                let height =
                    u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let width =
                    u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((width, height));
            }
            // Skip to next marker using segment length.
            let seg_len =
                u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            i += 2 + seg_len;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cdp_config_headless() {
        let config = BrowserConfig {
            mode: LaunchMode::Auto,
            headless: true,
            cdp_port: 0,
            user_data_dir: None,
            extra_args: vec!["--no-first-run".to_string()],
        };

        // This may fail if no Chromium is installed (find_chromium error),
        // which is acceptable in CI environments.
        match build_cdp_config(&config) {
            Ok(cdp_config) => {
                // If we got here, the config was built successfully.
                tracing::debug!("CDP config built: {:?}", cdp_config);
            }
            Err(BrowserError::ChromiumNotFound) => {
                eprintln!("No Chromium found — acceptable in CI");
            }
            Err(e) => {
                panic!("Unexpected error building CDP config: {e}");
            }
        }
    }

    #[test]
    fn test_extract_png_dimensions() {
        // Minimal valid PNG header (1x1 pixel)
        #[rustfmt::skip]
        let png_header: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR chunk length
            b'I', b'H', b'D', b'R', // IHDR marker
            0x00, 0x00, 0x03, 0x20, // width = 800
            0x00, 0x00, 0x02, 0x58, // height = 600
        ];
        let (w, h) = extract_image_dimensions(&png_header).unwrap();
        assert_eq!(w, 800);
        assert_eq!(h, 600);
    }

    #[tokio::test]
    #[ignore] // Integration test: requires a Chromium binary on the system.
    async fn test_browser_lifecycle() {
        let config = BrowserConfig {
            mode: LaunchMode::Auto,
            headless: true,
            cdp_port: 0,
            user_data_dir: None,
            extra_args: vec![],
        };

        // Start
        let mut runtime = BrowserRuntime::start(config)
            .await
            .expect("Failed to start browser");
        assert!(runtime.is_running());

        // Open tab
        let tab_id = runtime
            .open_tab("about:blank")
            .await
            .expect("Failed to open tab");
        assert!(!tab_id.is_empty());

        // List tabs
        let tabs = runtime.list_tabs().await;
        assert!(!tabs.is_empty());
        assert!(tabs.iter().any(|t| t.id == tab_id));

        // Navigate
        runtime
            .navigate(&tab_id, "data:text/html,<h1>Hello</h1>")
            .await
            .expect("Failed to navigate");

        // Evaluate JS
        let result = runtime
            .evaluate(&tab_id, "document.title")
            .await
            .expect("Failed to evaluate JS");
        // data: URLs may have an empty title
        assert!(result.is_string() || result.is_null());

        // Close tab
        runtime
            .close_tab(&tab_id)
            .await
            .expect("Failed to close tab");
        let tabs_after = runtime.list_tabs().await;
        assert!(!tabs_after.iter().any(|t| t.id == tab_id));

        // Stop
        runtime.stop().await.expect("Failed to stop browser");
    }
}
