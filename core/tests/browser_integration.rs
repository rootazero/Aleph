//! Integration test for the browser runtime.
//! Requires Chromium installed. Run with:
//!   cargo test --test browser_integration -- --nocapture --ignored --test-threads=1
//!
//! Tests must run sequentially (--test-threads=1) to avoid Chromium
//! singleton lock conflicts when multiple instances launch concurrently.

#[cfg(test)]
mod tests {
    use alephcore::browser::{BrowserConfig, BrowserRuntime, ScreenshotOpts};

    #[tokio::test]
    #[ignore = "requires Chromium installed"]
    async fn test_full_browser_workflow() {
        // 1. Start browser (headless)
        let config = BrowserConfig {
            headless: true,
            ..Default::default()
        };
        let mut runtime = BrowserRuntime::start(config)
            .await
            .expect("Failed to start browser");
        assert!(runtime.is_running());

        // 2. Open tab to example.com
        let tab_id = runtime
            .open_tab("https://example.com")
            .await
            .expect("Failed to open tab");
        assert!(!tab_id.is_empty());

        // 3. Wait for page load
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // 4. List tabs — should have at least one
        let tabs = runtime.list_tabs().await;
        assert!(!tabs.is_empty());

        // 5. Take ARIA snapshot — should find elements
        let snapshot = runtime
            .snapshot(&tab_id)
            .await
            .expect("Failed to take snapshot");
        assert!(!snapshot.elements.is_empty());

        // 6. Take screenshot — should return base64 data
        let screenshot = runtime
            .screenshot(&tab_id, ScreenshotOpts::default())
            .await
            .expect("Failed to take screenshot");
        assert!(!screenshot.data_base64.is_empty());

        // 7. Evaluate JavaScript — get document.title
        let title = runtime
            .evaluate(&tab_id, "document.title")
            .await
            .expect("Failed to evaluate JS");
        let title_str = title.as_str().unwrap_or("");
        assert!(!title_str.is_empty(), "Title should not be empty");

        // 8. Navigate to another URL
        runtime
            .navigate(&tab_id, "https://httpbin.org/html")
            .await
            .expect("Failed to navigate");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // 9. Close tab
        runtime
            .close_tab(&tab_id)
            .await
            .expect("Failed to close tab");

        // 10. Stop browser — note: `stop` consumes `self`
        runtime.stop().await.expect("Failed to stop browser");
    }

    #[tokio::test]
    #[ignore = "requires Chromium installed"]
    async fn test_browser_screenshot_formats() {
        let config = BrowserConfig {
            headless: true,
            ..Default::default()
        };
        let mut runtime = BrowserRuntime::start(config).await.unwrap();
        let tab_id = runtime.open_tab("https://example.com").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // PNG screenshot
        let png = runtime
            .screenshot(
                &tab_id,
                ScreenshotOpts {
                    full_page: false,
                    format: "png".to_string(),
                    quality: 80,
                },
            )
            .await
            .unwrap();
        assert_eq!(png.format, "png");
        assert!(!png.data_base64.is_empty());

        runtime.close_tab(&tab_id).await.ok();
        runtime.stop().await.ok();
    }
}
