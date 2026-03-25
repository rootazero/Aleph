//! ChromeMcpBackend — BrowserBackend implementation routing through Chrome DevTools MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_snapshot::convert_chrome_mcp_snapshot;
use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ConsoleMessage, ScreenshotOpts, ScreenshotResult, ScrollDirection,
    TabId, TabInfo,
};

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

    async fn call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.driver
            .call_tool(&self.profile_name, tool_name, args)
            .await
    }

    /// Select a page by its index before performing operations on it.
    /// Chrome DevTools MCP uses `pageId` (number) for page selection.
    async fn select_page(&self, tab_id: &str) -> Result<(), BrowserError> {
        let page_id: u32 = tab_id.parse().unwrap_or(1);
        self.call("select_page", json!({ "pageId": page_id })).await?;
        Ok(())
    }

    /// Extract text content from Chrome DevTools MCP response.
    /// MCP responses have format: {"content": [{"text": "...", "type": "text"}]}
    fn extract_text(result: &serde_json::Value) -> String {
        // Try content[0].text (standard MCP format)
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
            }
        }
        // Try as plain string
        if let Some(s) = result.as_str() {
            return s.to_string();
        }
        // Fallback to JSON serialization
        result.to_string()
    }

    /// Parse "N: URL [selected]" lines from list_pages text output.
    fn parse_pages_text(text: &str) -> Vec<TabInfo> {
        let mut tabs = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            // Match lines like "1: https://example.com [selected]" or "2: https://example.com"
            if let Some(colon_pos) = line.find(": ") {
                let id_str = line[..colon_pos].trim();
                // Verify it starts with a number
                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                    let rest = &line[colon_pos + 2..];
                    let (url, _selected) = if let Some(bracket_pos) = rest.rfind(" [") {
                        (&rest[..bracket_pos], true)
                    } else {
                        (rest, false)
                    };
                    tabs.push(TabInfo {
                        id: id_str.to_string(),
                        url: url.to_string(),
                        title: String::new(),
                    });
                }
            }
        }
        tabs
    }
}

#[async_trait]
impl BrowserBackend for ChromeMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("new_page", json!({ "url": url })).await?;
        // After opening, list pages to find the new page's index
        let text = Self::extract_text(&result);
        // The response usually confirms the new page — re-list to get its ID
        let tabs = self.list_tabs().await?;
        Ok(tabs.last().map(|t| t.id.clone()).unwrap_or_else(|| {
            // Fallback: try to extract from response text
            text.lines()
                .filter(|l| l.contains(url))
                .filter_map(|l| l.split(':').next())
                .next()
                .unwrap_or("1")
                .trim()
                .to_string()
        }))
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let page_id: u32 = tab_id.parse().unwrap_or(1);
        self.call("close_page", json!({ "pageId": page_id })).await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        let result = self.call("list_pages", json!({})).await?;
        // Chrome DevTools MCP returns text format: "## Pages\n1: URL [selected]\n2: URL\n..."
        let text = Self::extract_text(&result);
        let tabs = Self::parse_pages_text(&text);
        if tabs.is_empty() {
            // Fallback: try JSON parsing for future structured content support
            if let Some(pages) = result.as_array()
                .or_else(|| result.get("pages").and_then(|v| v.as_array()))
            {
                return Ok(pages.iter().map(|page| TabInfo {
                    id: page.get("pageId").or_else(|| page.get("id"))
                        .and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                    url: page.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    title: page.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                }).collect());
            }
        }
        Ok(tabs)
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.select_page(tab_id).await?;
        self.call("navigate_page", json!({ "url": url })).await?;
        Ok(())
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_page(tab_id).await?;
        self.call("click", json!({ "uid": element })).await?;
        Ok(())
    }

    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_page(tab_id).await?;
        self.call("fill", json!({ "uid": element, "value": text }))
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
        self.select_page(tab_id).await?;
        self.call("fill", json!({ "uid": element, "value": value }))
            .await?;
        Ok(())
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_page(tab_id).await?;
        self.call("hover", json!({ "uid": element })).await?;
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
        self.select_page(tab_id).await?;
        self.call("press_key", json!({ "key": key })).await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("take_screenshot", json!({})).await?;
        // Screenshot response may have base64 in content[0].data or content[0].text
        let text = Self::extract_text(&result);
        // Check if result has image content type
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
        Ok(ScreenshotResult {
            data_base64: text,
            width: 0,
            height: 0,
            format: "png".to_string(),
        })
    }

    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("take_snapshot", json!({})).await?;

        // MCP responses wrap content as: {"content": [{"type": "text", "text": "..."}]}
        // The snapshot tree may be a JSON string inside the text field.
        // Try multiple extraction strategies:

        // 1. Direct "snapshot" key (future structured format)
        if let Some(snapshot_data) = result.get("snapshot") {
            return convert_chrome_mcp_snapshot(snapshot_data);
        }

        // 2. Extract text from MCP wrapper and parse as JSON
        let text = Self::extract_text(&result);
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
            return convert_chrome_mcp_snapshot(&parsed);
        }

        // 3. Fallback: try the raw result
        convert_chrome_mcp_snapshot(&result)
    }

    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError> {
        self.select_page(tab_id).await?;
        self.call("evaluate_script", json!({ "function": js }))
            .await
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.fill(tab_id, target, value).await
    }

    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.select_page(tab_id).await?;
        self.call("press_key", json!({ "key": key })).await?;
        Ok(())
    }

    async fn wait_for_text(&self, tab_id: &str, text: &str, timeout_ms: u64) -> Result<bool, BrowserError> {
        self.select_page(tab_id).await?;
        self.call("wait_for", json!({ "text": text, "timeout": timeout_ms })).await?;
        Ok(true)
    }

    async fn console_messages(&self, tab_id: &str) -> Result<Vec<ConsoleMessage>, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("list_console_messages", json!({})).await?;
        let text = Self::extract_text(&result);
        Ok(parse_console_messages(&text))
    }

    async fn fill_form(&self, tab_id: &str, fields: &[(ActionTarget, String)]) -> Result<usize, BrowserError> {
        if fields.is_empty() {
            return Ok(0);
        }
        self.select_page(tab_id).await?;
        let form_fields: Vec<_> = fields.iter().filter_map(|(target, value)| {
            let uid = Self::extract_element_ref(target).ok()?;
            Some(json!({ "uid": uid, "value": value }))
        }).collect();
        if form_fields.is_empty() {
            return Err(BrowserError::ActionFailed("No valid ref_id targets for fill_form".into()));
        }
        self.call("fill_form", json!({ "fields": form_fields })).await?;
        Ok(form_fields.len())
    }
}

/// Parse console messages from Chrome DevTools MCP text output.
/// Console output format: "[level] message" per line.
fn parse_console_messages(text: &str) -> Vec<ConsoleMessage> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() { return None; }
        // Try to extract [level] prefix
        if line.starts_with('[') {
            if let Some(end) = line.find(']') {
                let level = line.get(1..end).unwrap_or("log").to_lowercase();
                let msg = line.get(end + 1..).unwrap_or("").trim().to_string();
                return Some(ConsoleMessage { level, text: msg });
            }
        }
        // Fallback: treat as log
        Some(ConsoleMessage { level: "log".to_string(), text: line.to_string() })
    }).collect()
}
