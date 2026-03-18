//! PlaywrightMcpBackend — BrowserBackend implementation routing through Playwright MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::playwright_mcp::PlaywrightMcpDriver;
use super::types::{
    ActionTarget, AriaElement, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection,
    TabId, TabInfo,
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

    /// Parse Playwright's indented ARIA snapshot format into flat elements.
    ///
    /// Input format example:
    /// ```text
    /// - heading "Title" [level=1]
    /// - navigation "Main"
    ///   - link "Home" [ref=s1e3]
    /// ```
    fn parse_snapshot_text(text: &str) -> Vec<AriaElement> {
        let mut elements = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim();
            // Lines start with "- " followed by role and optional quoted name
            let content = if let Some(stripped) = trimmed.strip_prefix("- ") {
                stripped
            } else {
                continue;
            };

            // Parse: role "name" [ref=xxx] or role "name" [attr=val]
            let (role, rest) = match content.find(' ') {
                Some(pos) => (&content[..pos], content.get(pos + 1..).unwrap_or("")),
                None => (content, ""),
            };

            // Extract quoted name
            let name = if rest.starts_with('"') {
                // Find closing quote
                rest.get(1..)
                    .and_then(|s| s.find('"').map(|end| &s[..end]))
                    .map(|s| s.to_string())
            } else {
                None
            };

            // Extract ref from [ref=xxx]
            let ref_id = extract_bracket_value(rest, "ref");

            elements.push(AriaElement {
                ref_id: ref_id.unwrap_or_default(),
                role: role.to_string(),
                name,
                value: None,
                state: Vec::new(),
                bounds: None,
                children: Vec::new(),
            });
        }

        elements
    }

    /// Map an ActionTarget to Playwright MCP args (element description + ref).
    fn target_to_args(target: &ActionTarget) -> Result<(String, String), BrowserError> {
        match target {
            ActionTarget::Ref { ref_id } => Ok(("element".to_string(), ref_id.clone())),
            ActionTarget::Selector { css } => Ok((css.clone(), String::new())),
            ActionTarget::Coordinates { x, y } => {
                Ok((format!("coordinates ({x}, {y})"), String::new()))
            }
        }
    }
}

/// Extract a value from bracket attributes like [ref=s1e3] or [level=1].
fn extract_bracket_value(text: &str, key: &str) -> Option<String> {
    let pattern = format!("[{key}=");
    if let Some(start) = text.find(&pattern) {
        let after = text.get(start + pattern.len()..)?;
        let end = after.find(']')?;
        Some(after[..end].to_string())
    } else {
        None
    }
}

#[async_trait]
impl BrowserBackend for PlaywrightMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("browser_tab_new", json!({ "url": url })).await?;
        let text = Self::extract_text(&result);
        // Return the tab index from response, or fallback to listing tabs
        let tabs = self.list_tabs().await?;
        Ok(tabs
            .last()
            .map(|t| t.id.clone())
            .unwrap_or_else(|| text.trim().to_string()))
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let index: u32 = tab_id.parse().unwrap_or(0);
        self.call("browser_tab_close", json!({ "index": index }))
            .await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        let result = self.call("browser_tab_list", json!({})).await?;
        let text = Self::extract_text(&result);
        // Parse "Tab N: URL" lines
        let mut tabs = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("Tab ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let id_str = rest[..colon_pos].trim();
                    let url = rest[colon_pos + 2..].trim();
                    tabs.push(TabInfo {
                        id: id_str.to_string(),
                        url: url.to_string(),
                        title: String::new(),
                    });
                }
            }
        }
        Ok(tabs)
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
    ) -> Result<ScreenshotResult, BrowserError> {
        let result = self.call("browser_screenshot", json!({ "raw": true })).await?;
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
        let elements = Self::parse_snapshot_text(&text);
        Ok(AriaSnapshot {
            elements,
            page_title: None,
            page_url: None,
            focused_ref: None,
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError> {
        self.call("browser_evaluate", json!({ "expression": js }))
            .await
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_text_basic() {
        let text = r#"- heading "Title" [level=1]
- navigation "Main"
  - link "Home" [ref=s1e3]
  - link "About" [ref=s1e4]
- textbox "Search" [ref=s1e5]"#;

        let elements = PlaywrightMcpBackend::parse_snapshot_text(text);
        assert_eq!(elements.len(), 5);

        assert_eq!(elements[0].role, "heading");
        assert_eq!(elements[0].name.as_deref(), Some("Title"));
        assert!(elements[0].ref_id.is_empty()); // no ref attribute

        assert_eq!(elements[1].role, "navigation");
        assert_eq!(elements[1].name.as_deref(), Some("Main"));

        assert_eq!(elements[2].role, "link");
        assert_eq!(elements[2].name.as_deref(), Some("Home"));
        assert_eq!(elements[2].ref_id, "s1e3");

        assert_eq!(elements[3].role, "link");
        assert_eq!(elements[3].name.as_deref(), Some("About"));
        assert_eq!(elements[3].ref_id, "s1e4");

        assert_eq!(elements[4].role, "textbox");
        assert_eq!(elements[4].name.as_deref(), Some("Search"));
        assert_eq!(elements[4].ref_id, "s1e5");
    }

    #[test]
    fn test_parse_snapshot_text_empty() {
        let elements = PlaywrightMcpBackend::parse_snapshot_text("");
        assert!(elements.is_empty());

        let elements = PlaywrightMcpBackend::parse_snapshot_text("no elements here");
        assert!(elements.is_empty());
    }

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
