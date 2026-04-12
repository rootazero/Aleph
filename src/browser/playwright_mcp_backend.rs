//! PlaywrightMcpBackend — BrowserBackend implementation routing through Playwright MCP.
//! NOTE: This backend is pending deletion in Task 11. Methods are stubbed to satisfy the
//! new BrowserBackend trait where the old return types no longer exist.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::playwright_mcp::PlaywrightMcpDriver;
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
};

pub struct PlaywrightMcpBackend {
    driver: Arc<PlaywrightMcpDriver>,
    session_key: String,
}

impl PlaywrightMcpBackend {
    pub fn new(driver: Arc<PlaywrightMcpDriver>, session_key: String) -> Self {
        Self {
            driver,
            session_key,
        }
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

    /// Extract text content from Playwright MCP response.
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

    /// Map an ActionTarget to Playwright MCP args (element description + ref).
    fn target_to_args(target: &ActionTarget) -> Result<(String, String), BrowserError> {
        match target {
            ActionTarget::Ref { ref_id } => Ok(("element".to_string(), ref_id.clone())),
            ActionTarget::Coordinates { x, y } => {
                Ok((format!("coordinates ({x}, {y})"), String::new()))
            }
        }
    }
}

#[async_trait]
impl BrowserBackend for PlaywrightMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("browser_tab_new", json!({ "url": url })).await?;
        let text = Self::extract_text(&result);
        // Return the tab index from response, or fallback to listing tabs
        let tabs_text = self.list_tabs().await?;
        // Parse the first tab id from the text list
        let first_id = tabs_text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix("Tab ")
                    .and_then(|rest| rest.split(':').next())
                    .map(|s| s.trim().to_string())
            })
            .last();
        Ok(first_id.unwrap_or_else(|| text.trim().to_string()))
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let index: u32 = tab_id.parse().unwrap_or(0);
        self.call("browser_tab_close", json!({ "index": index }))
            .await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        let result = self.call("browser_tab_list", json!({})).await?;
        Ok(Self::extract_text(&result))
    }

    async fn navigate(&self, _tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.call("browser_navigate", json!({ "url": url })).await?;
        Ok(())
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let (element, ref_id) = Self::target_to_args(&target)?;
        let mut args = json!({ "element": element });
        if !ref_id.is_empty() {
            args["ref"] = json!(ref_id);
        }
        self.call("browser_click", args).await?;
        Ok(())
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        let (element, ref_id) = Self::target_to_args(&target)?;
        let mut args = json!({ "element": element, "text": text });
        if !ref_id.is_empty() {
            args["ref"] = json!(ref_id);
        }
        self.call("browser_type", args).await?;
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
        let (element, ref_id) = Self::target_to_args(&target)?;
        let mut args = json!({ "element": element });
        if !ref_id.is_empty() {
            args["ref"] = json!(ref_id);
        }
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
        self.call("browser_press_key", json!({ "key": key }))
            .await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        Err(BrowserError::ActionFailed("pending migration".into()))
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        let result = self.call("browser_snapshot", json!({})).await?;
        let text = Self::extract_text(&result);
        Ok(SnapshotOutput {
            snapshot_text: text,
            page_url: String::new(),
            page_title: String::new(),
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        let result = self
            .call("browser_evaluate", json!({ "expression": js }))
            .await?;
        Ok(Self::extract_text(&result))
    }

    async fn select(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let (element, ref_id) = Self::target_to_args(&target)?;
        let mut args = json!({ "element": element, "value": value });
        if !ref_id.is_empty() {
            args["ref"] = json!(ref_id);
        }
        self.call("browser_select", args).await?;
        Ok(())
    }

    async fn press_key(&self, _tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.call("browser_press_key", json!({ "key": key }))
            .await?;
        Ok(())
    }

    async fn wait_for_text(
        &self,
        _tab_id: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        self.call(
            "browser_wait_for_text",
            json!({ "text": text, "timeout": timeout_ms }),
        )
        .await?;
        Ok(true)
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        let result = self.call("browser_console_messages", json!({})).await?;
        Ok(Self::extract_text(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_mcp_format() {
        // Standard MCP format
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Hello World"}]
        });
        assert_eq!(PlaywrightMcpBackend::extract_text(&result), "Hello World");

        // Plain string
        let result = serde_json::json!("plain text");
        assert_eq!(PlaywrightMcpBackend::extract_text(&result), "plain text");

        // Fallback to JSON
        let result = serde_json::json!({"key": "value"});
        assert_eq!(
            PlaywrightMcpBackend::extract_text(&result),
            r#"{"key":"value"}"#
        );
    }
}
