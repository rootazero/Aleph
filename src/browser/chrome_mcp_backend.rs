//! ChromeMcpBackend — BrowserBackend implementation routing through Chrome DevTools MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use base64::Engine as _;

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::error::BrowserError;
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
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
        self.call("select_page", json!({ "pageId": page_id }))
            .await?;
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
}

#[async_trait]
impl BrowserBackend for ChromeMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("new_page", json!({ "url": url })).await?;
        let text = Self::extract_text(&result);
        // Re-list to find the new page's ID
        let tabs_text = self.list_tabs().await?;
        // Parse last numeric id from "N: URL" lines
        let last_id = tabs_text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let colon_pos = line.find(": ")?;
                let id_str = line.get(..colon_pos)?.trim();
                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                    Some(id_str.to_string())
                } else {
                    None
                }
            })
            .last();
        Ok(last_id.unwrap_or_else(|| {
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
        self.call("close_page", json!({ "pageId": page_id }))
            .await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        let result = self.call("list_pages", json!({})).await?;
        Ok(Self::extract_text(&result))
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
    ) -> Result<ScreenshotOutput, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("take_screenshot", json!({})).await?;
        // Check if result has image content type with base64 data
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                    let data = item.get("data").and_then(|v| v.as_str()).unwrap_or("");
                    let png_bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map_err(|e| BrowserError::ScreenshotFailed(format!("base64 decode: {e}")))?;
                    return Ok(ScreenshotOutput { png_bytes });
                }
            }
        }
        // Fallback: treat text as base64
        let text = Self::extract_text(&result);
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(&text)
            .map_err(|e| BrowserError::ScreenshotFailed(format!("base64 decode: {e}")))?;
        Ok(ScreenshotOutput { png_bytes })
    }

    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("take_snapshot", json!({})).await?;
        let snapshot_text = Self::extract_text(&result);
        // Best-effort: parse page URL and title from snapshot header lines
        let (page_url, page_title) = parse_snapshot_header(&snapshot_text);
        Ok(SnapshotOutput {
            snapshot_text,
            page_url,
            page_title,
        })
    }

    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self
            .call("evaluate_script", json!({ "function": js }))
            .await?;
        Ok(Self::extract_text(&result))
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

    async fn wait_for_text(
        &self,
        tab_id: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        self.select_page(tab_id).await?;
        self.call("wait_for", json!({ "text": text, "timeout": timeout_ms }))
            .await?;
        Ok(true)
    }

    async fn console_messages(&self, tab_id: &str) -> Result<String, BrowserError> {
        self.select_page(tab_id).await?;
        let result = self.call("list_console_messages", json!({})).await?;
        Ok(Self::extract_text(&result))
    }

    async fn fill_form(
        &self,
        tab_id: &str,
        fields: &[(ActionTarget, String)],
    ) -> Result<usize, BrowserError> {
        if fields.is_empty() {
            return Ok(0);
        }
        self.select_page(tab_id).await?;
        let form_fields: Vec<_> = fields
            .iter()
            .filter_map(|(target, value)| {
                let uid = Self::extract_element_ref(target).ok()?;
                Some(json!({ "uid": uid, "value": value }))
            })
            .collect();
        if form_fields.is_empty() {
            return Err(BrowserError::ActionFailed(
                "No valid ref_id targets for fill_form".into(),
            ));
        }
        self.call("fill_form", json!({ "fields": form_fields }))
            .await?;
        Ok(form_fields.len())
    }
}

/// Best-effort extraction of page URL and title from the first few lines of a snapshot.
/// Chrome DevTools MCP snapshot text begins with header lines like:
///   - Page URL: https://example.com/
///   - Page Title: Hello
/// Returns empty strings when the fields are absent (graceful degradation).
fn parse_snapshot_header(text: &str) -> (String, String) {
    let mut url = String::new();
    let mut title = String::new();
    for line in text.lines().take(10) {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("- Page URL:") {
            url = rest.trim().to_string();
        } else if let Some(rest) = t.strip_prefix("- Page Title:") {
            title = rest.trim().to_string();
        }
    }
    (url, title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_header_extracts_url_and_title() {
        let text =
            "- Page URL: https://example.com/\n- Page Title: Hello\n- button \"OK\" [ref=e1]";
        let (url, title) = parse_snapshot_header(text);
        assert_eq!(url, "https://example.com/");
        assert_eq!(title, "Hello");
    }

    #[test]
    fn test_parse_snapshot_header_returns_empty_when_missing() {
        let text = "- button \"OK\" [ref=e1]";
        let (url, title) = parse_snapshot_header(text);
        assert_eq!(url, "");
        assert_eq!(title, "");
    }
}
