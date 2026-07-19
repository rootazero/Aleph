//! `ChromeMcpBackend` — `BrowserBackend` implementation routing through Chrome `DevTools` MCP.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use base64::Engine as _;

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::types::{
    ActionTarget, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput, ScrollDirection,
    SnapshotOutput, TabId,
};

pub struct ChromeMcpBackend {
    driver: Arc<ChromeMcpDriver>,
    profile_name: String,
    ssrf_guard: Arc<BrowserSsrfGuard>,
}

impl ChromeMcpBackend {
    pub const fn new(
        driver: Arc<ChromeMcpDriver>,
        profile_name: String,
        ssrf_guard: Arc<BrowserSsrfGuard>,
    ) -> Self {
        Self {
            driver,
            profile_name,
            ssrf_guard,
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

    /// Acquire the per-profile serialization lock. Held across a
    /// `select_page` → action sequence so concurrent same-profile operations
    /// can't interleave the server-side page selection.
    async fn profile_guard(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.driver
            .profile_lock(&self.profile_name)
            .lock_owned()
            .await
    }

    /// Atomically (under the per-profile lock) select `tab_id` then invoke
    /// `tool` with `args`. This is the common shape of nearly every backend
    /// action; holding the guard across both round-trips closes the
    /// select-then-act interleave race.
    async fn select_and_call(
        &self,
        tab_id: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        let _guard = self.profile_guard().await;
        self.select_page(tab_id).await?;
        self.call(tool, args).await
    }

    /// Select a page by its index before performing operations on it.
    /// Chrome `DevTools` MCP uses `pageId` (number) for page selection.
    async fn select_page(&self, tab_id: &str) -> Result<(), BrowserError> {
        let page_id: u32 = tab_id.parse().map_err(|_| {
            BrowserError::ActionFailed(format!(
                "Invalid tab ID '{tab_id}': expected numeric page ID"
            ))
        })?;
        self.call("select_page", json!({ "pageId": page_id }))
            .await?;
        Ok(())
    }

    /// Extract text content from Chrome `DevTools` MCP response.
    /// MCP responses have format: {"content": [{"text": "...", "type": "text"}]}
    /// Returns empty string when no text content is present — callers must NOT
    /// dump raw JSON (image / binary frames) into snapshot output downstream.
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
        String::new()
    }
}

#[async_trait]
impl BrowserBackend for ChromeMcpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.ssrf_guard
            .check_navigation(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        // Hold the per-profile lock across new_page + re-list so a concurrent
        // open on the same profile can't append a tab between the two calls and
        // steal our "newest is last" id. List inline (not via the public
        // `list_tabs`, which would re-acquire the same lock and deadlock).
        let _guard = self.profile_guard().await;
        let _result = self.call("new_page", json!({ "url": url })).await?;
        let tabs_text = Self::extract_text(&self.call("list_pages", json!({})).await?);
        // Parse last numeric id from "N: URL" lines (newest tab is last)
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
            .next_back();

        last_id.ok_or_else(|| {
            BrowserError::TabNotFound(format!("Could not determine tab ID after opening {url}"))
        })
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let page_id: u32 = tab_id.parse().map_err(|_| {
            BrowserError::TabNotFound(format!(
                "Invalid tab ID '{tab_id}': expected numeric page ID"
            ))
        })?;
        self.call("close_page", json!({ "pageId": page_id }))
            .await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        let result = self.call("list_pages", json!({})).await?;
        Ok(Self::extract_text(&result))
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.ssrf_guard
            .check_navigation(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        self.select_and_call(tab_id, "navigate_page", json!({ "url": url }))
            .await?;
        Ok(())
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_and_call(tab_id, "click", json!({ "uid": element }))
            .await?;
        Ok(())
    }

    async fn dblclick(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_and_call(tab_id, "click", json!({ "uid": element, "dblClick": true }))
            .await?;
        Ok(())
    }

    async fn history(&self, tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError> {
        // Native history navigation — `navigate_page` waits for the load,
        // unlike a fire-and-forget `history.back()` eval.
        let nav_type = match nav {
            HistoryNav::Back => "back",
            HistoryNav::Forward => "forward",
            HistoryNav::Refresh => "reload",
        };
        self.select_and_call(tab_id, "navigate_page", json!({ "type": nav_type }))
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
        self.select_and_call(tab_id, "fill", json!({ "uid": element, "value": text }))
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
        self.select_and_call(tab_id, "fill", json!({ "uid": element, "value": value }))
            .await?;
        Ok(())
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        self.select_and_call(tab_id, "hover", json!({ "uid": element }))
            .await?;
        Ok(())
    }

    async fn scroll(
        &self,
        tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        // Vertical scrolling: PageUp/PageDown are reliable across pages.
        // Horizontal scrolling: Home/End would jump to start/end-of-document, which
        // is NOT lateral scroll — fall back to window.scrollBy(±SCROLL_STEP_PX, 0) via JS.
        let (tool, args) = match direction {
            ScrollDirection::Up => ("press_key", json!({ "key": "PageUp" })),
            ScrollDirection::Down => ("press_key", json!({ "key": "PageDown" })),
            ScrollDirection::Left | ScrollDirection::Right => {
                let (dx, _) = direction.wheel_delta();
                (
                    "evaluate_script",
                    json!({ "function": format!("() => window.scrollBy({dx}, 0)") }),
                )
            }
        };
        self.select_and_call(tab_id, tool, args).await?;
        Ok(())
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        let result = self
            .select_and_call(
                tab_id,
                "take_screenshot",
                json!({ "fullPage": opts.full_page }),
            )
            .await?;
        // Check if result has image content type with base64 data
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                    if let Some(data) = item.get("data").and_then(|v| v.as_str()) {
                        let png_bytes = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .map_err(|e| {
                                BrowserError::ScreenshotFailed(format!("base64 decode: {e}"))
                            })?;
                        return Ok(ScreenshotOutput { png_bytes });
                    }
                }
            }
        }
        // Fallback: treat text as base64. An empty string means the MCP
        // response carried neither image content nor text — treat as failure
        // rather than silently returning a zero-byte screenshot.
        let text = Self::extract_text(&result);
        if text.is_empty() {
            return Err(BrowserError::ScreenshotFailed(
                "Chrome MCP returned no image data".into(),
            ));
        }
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(&text)
            .map_err(|e| BrowserError::ScreenshotFailed(format!("base64 decode: {e}")))?;
        Ok(ScreenshotOutput { png_bytes })
    }

    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        let result = self
            .select_and_call(tab_id, "take_snapshot", json!({}))
            .await?;
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
        let result = self
            .select_and_call(tab_id, "evaluate_script", json!({ "function": js }))
            .await?;
        Ok(Self::extract_text(&result))
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        // Inline `fill`'s body rather than delegating: calling the public `fill`
        // here would re-acquire the per-profile lock and deadlock.
        let element = Self::extract_element_ref(&target)?;
        self.select_and_call(tab_id, "fill", json!({ "uid": element, "value": value }))
            .await?;
        Ok(())
    }

    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.select_and_call(tab_id, "press_key", json!({ "key": key }))
            .await?;
        Ok(())
    }

    async fn wait_for_text(
        &self,
        tab_id: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        match self
            .select_and_call(
                tab_id,
                "wait_for",
                json!({ "text": text, "timeout": timeout_ms }),
            )
            .await
        {
            Ok(_) => Ok(true),
            // A genuine wait *timeout* means "text did not appear" → Ok(false).
            // A structured TabNotFound (tab closed/navigated away mid-wait) or any
            // other transport error is a real failure and must propagate — do NOT
            // fold it into Ok(false). Note the previous broad `contains("not found")`
            // collided with the "Tab not found" message and silently swallowed it.
            Err(BrowserError::TabNotFound(_)) => Err(BrowserError::TabNotFound(format!(
                "tab '{tab_id}' disappeared while waiting for text"
            ))),
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("timeout")
                    || msg.contains("timed out")
                    || msg.contains("did not appear")
                    || msg.contains("exceeded")
                {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn console_messages(&self, tab_id: &str) -> Result<String, BrowserError> {
        let result = self
            .select_and_call(tab_id, "list_console_messages", json!({}))
            .await?;
        Ok(Self::extract_text(&result))
    }

    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.select_page(tab_id).await
    }

    async fn handle_dialog(
        &self,
        tab_id: &str,
        action: &str,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        let action_norm = match action.to_ascii_lowercase().as_str() {
            "accept" | "ok" | "confirm" => "accept",
            "dismiss" | "cancel" | "reject" => "dismiss",
            other => {
                return Err(BrowserError::ActionFailed(format!(
                    "unknown dialog action '{other}' — expected 'accept' or 'dismiss'"
                )));
            }
        };
        let mut args = json!({ "action": action_norm });
        if let Some(text) = prompt_text {
            args.as_object_mut()
                .ok_or_else(|| BrowserError::ActionFailed("args is not a JSON object".to_string()))?
                .insert("promptText".to_string(), json!(text));
        }
        self.select_and_call(tab_id, "handle_dialog", args).await?;
        Ok(())
    }

    async fn network_log(&self, tab_id: &str) -> Result<String, BrowserError> {
        let result = self
            .select_and_call(tab_id, "list_network_requests", json!({}))
            .await?;
        Ok(Self::extract_text(&result))
    }

    async fn drag(
        &self,
        tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError> {
        let from_uid = Self::extract_element_ref(&from)?;
        let to_uid = Self::extract_element_ref(&to)?;
        self.select_and_call(
            tab_id,
            "drag",
            json!({ "from_uid": from_uid, "to_uid": to_uid }),
        )
        .await?;
        Ok(())
    }

    async fn upload(
        &self,
        tab_id: &str,
        target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError> {
        let target = target.ok_or_else(|| {
            BrowserError::ActionFailed(
                "upload in existing-session mode requires a ref_id for the file input element \
                 (from browser_snapshot)"
                    .into(),
            )
        })?;
        let uid = Self::extract_element_ref(&target)?;
        if paths.is_empty() {
            return Err(BrowserError::ActionFailed(
                "upload requires at least one file path".into(),
            ));
        }
        // Hold the lock across select_page + the per-file loop so the whole
        // multi-file upload runs against the selected page atomically.
        let _guard = self.profile_guard().await;
        self.select_page(tab_id).await?;
        // chrome-devtools-mcp's `upload_file` is single-file; apply each path in order.
        for path in paths {
            self.call("upload_file", json!({ "uid": uid, "filePath": path }))
                .await?;
        }
        Ok(())
    }

    async fn resize(&self, tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError> {
        self.select_and_call(
            tab_id,
            "resize_page",
            json!({ "width": width, "height": height }),
        )
        .await?;
        Ok(())
    }

    async fn emulate(&self, tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError> {
        opts.validate().map_err(BrowserError::ActionFailed)?;
        // chrome-devtools-mcp exposes a single `emulate` tool covering every
        // override; build its argument object from the set fields only.
        let mut args = serde_json::Map::new();
        if let Some(cs) = opts.color_scheme {
            args.insert("colorScheme".into(), json!(cs.as_mcp()));
        }
        if let Some(geo) = &opts.geolocation {
            args.insert(
                "geolocation".into(),
                json!(format!("{},{}", geo.latitude, geo.longitude)),
            );
        }
        if let Some(nc) = opts.network_condition {
            // `Online` is expressed by omitting the field (no throttling).
            if let Some(v) = nc.as_mcp() {
                args.insert("networkConditions".into(), json!(v));
            }
        }
        if let Some(rate) = opts.cpu_throttle {
            args.insert("cpuThrottlingRate".into(), json!(rate));
        }
        if let Some(headers) = &opts.extra_http_headers {
            // The MCP contract wants the header map as a JSON *string*.
            let encoded = serde_json::to_string(headers).map_err(|e| {
                BrowserError::ActionFailed(format!("encode extra_http_headers: {e}"))
            })?;
            args.insert("extraHttpHeaders".into(), json!(encoded));
        }
        if let Some(ua) = &opts.user_agent {
            args.insert("userAgent".into(), json!(ua));
        }
        self.select_and_call(tab_id, "emulate", serde_json::Value::Object(args))
            .await?;
        Ok(())
    }

    async fn fill_form(
        &self,
        tab_id: &str,
        fields: &[(ActionTarget, String)],
    ) -> Result<usize, BrowserError> {
        if fields.is_empty() {
            return Ok(0);
        }
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
        self.select_and_call(tab_id, "fill_form", json!({ "fields": form_fields }))
            .await?;
        Ok(form_fields.len())
    }
}

/// Best-effort extraction of page URL and title from the first few lines of a snapshot.
/// Chrome `DevTools` MCP snapshot text begins with header lines like:
///
///   - Page URL: <https://example.com>/
///   - Page Title: Hello
///
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

    #[test]
    fn test_extract_text_reads_content_text() {
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": "hello world" }]
        });
        assert_eq!(ChromeMcpBackend::extract_text(&result), "hello world");
    }

    #[test]
    fn test_extract_text_plain_string() {
        let result = serde_json::json!("plain");
        assert_eq!(ChromeMcpBackend::extract_text(&result), "plain");
    }

    #[test]
    fn test_extract_text_no_text_returns_empty_not_json() {
        // Image-only response — must NOT dump raw JSON into the result.
        let result = serde_json::json!({
            "content": [{ "type": "image", "data": "aGVsbG8=" }]
        });
        assert_eq!(ChromeMcpBackend::extract_text(&result), "");
    }
}
