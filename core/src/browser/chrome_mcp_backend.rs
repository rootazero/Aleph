//! ChromeMcpBackend — BrowserBackend implementation routing through Chrome DevTools MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_snapshot::convert_chrome_mcp_snapshot;
use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection, TabId, TabInfo,
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
}

#[async_trait]
impl BrowserBackend for ChromeMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let result = self.call("new_page", json!({ "url": url })).await?;
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
        self.call("close_page", json!({ "pageId": tab_id }))
            .await?;
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
        self.call("navigate_page", json!({ "pageId": tab_id, "url": url }))
            .await?;
        Ok(())
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("click", json!({ "pageId": tab_id, "element": element }))
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
        self.call(
            "fill",
            json!({ "pageId": tab_id, "element": element, "value": text }),
        )
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
        self.call(
            "fill",
            json!({ "pageId": tab_id, "element": element, "value": value }),
        )
        .await?;
        Ok(())
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.call("hover", json!({ "pageId": tab_id, "element": element }))
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
        self.call("press_key", json!({ "pageId": tab_id, "key": key }))
            .await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        let result = self
            .call("take_screenshot", json!({ "pageId": tab_id }))
            .await?;
        let data_base64 = result
            .get("data")
            .or_else(|| result.get("image"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(ScreenshotResult {
            data_base64,
            width: 0,
            height: 0,
            format: "png".to_string(),
        })
    }

    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        let result = self
            .call("take_snapshot", json!({ "pageId": tab_id }))
            .await?;
        let snapshot_data = result.get("snapshot").unwrap_or(&result);
        convert_chrome_mcp_snapshot(snapshot_data)
    }

    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError> {
        self.call("evaluate_script", json!({ "pageId": tab_id, "script": js }))
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
}
