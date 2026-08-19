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
    SnapshotOutput, TabId, WaitCondition,
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
        // steal the id of the tab we just opened. List inline (not via the
        // public `list_tabs`, which would re-acquire the same lock and
        // deadlock).
        let _guard = self.profile_guard().await;
        self.call("new_page", json!({ "url": url })).await?;
        let tabs_text = Self::extract_text(&self.call("list_pages", json!({})).await?);
        // `new_page` also *selects* the page it opened, so the driver's own
        // `[selected]` marker names our tab; last-listed is the fallback for a
        // listing that carries no marker. Shared parser, so the
        // chrome-devtools-mcp "N: URL" and playwright-cli "Tab N: URL" formats
        // both yield the same id.
        let new_id = super::tab_registry::active_tab_id(&tabs_text);

        // Post-navigation audit on the listing already fetched above (no extra
        // round trip): a redirect may have landed the new tab on a blocked
        // origin the navigation-time guard never saw. The quarantine (closing
        // the tab) lives in `post_nav`, not here — `close_tab` takes no profile
        // lock, so running it under `_guard` cannot deadlock.
        super::post_nav::audit_listing(self, &self.ssrf_guard, &tabs_text, new_id.as_deref())
            .await?;

        new_id.ok_or_else(|| {
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
        // Post-navigation audit: a redirect may have landed the tab on a
        // blocked origin the navigation-time guard never saw. Runs outside
        // the profile lock (list/close take none), so no deadlock.
        super::post_nav::audit_landed_tab(self, &self.ssrf_guard, Some(tab_id)).await
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

    /// Delegates to `fill` (clear-then-set), not keystroke simulation.
    ///
    /// The server *does* expose a `type_text` tool — an earlier note here said
    /// it did not — but its schema is `{text, submitKey}` with **no element
    /// argument**: it types into whatever currently has focus. `browser_type`
    /// takes a target, so routing it through `type_text` would silently drop
    /// the caller's ref and type into some other element. `fill` is the only
    /// targeted write this server offers; the cost is that the write is not
    /// character-by-character, which is what this comment exists to say.
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
        let text = Self::extract_text(&result);
        // Hand back the VALUE, not the server's prose about it. The managed
        // driver had this same defect and it was fixed there; leaving it here
        // meant the two drivers answered the same `browser_evaluate` call with
        // two different shapes — one a JSON scalar, the other
        // "Script ran on page and returned:\n```json\n<value>\n```" — so any
        // caller that compared a value (rather than searching a substring) was
        // right on one driver and wrong on the other.
        Ok(parse_evaluate_value(&text).unwrap_or(text))
    }

    /// Set a `<select>`'s value.
    ///
    /// NOT `fill`. chrome-devtools-mcp's `fill` waits for the element to become
    /// "interactive" in the sense a text field is, which a `<select>` never
    /// does: every call came back
    /// `Failed to interact with the element with uid … within the configured
    /// timeout`, so `browser_select` on an existing-session profile had never
    /// once changed a dropdown. `fill_form` fails identically — it is the same
    /// locator underneath — so batching was not an escape either.
    ///
    /// The server exposes no select primitive, so the write goes through
    /// `evaluate_script`, whose `args` are **element uids resolved by the
    /// server**. That matters: the alternative was to synthesise a CSS selector
    /// from the ref, which is the guessing this driver's whole uid scheme
    /// exists to avoid.
    ///
    /// Assigning a value that matches no `<option>` leaves `el.value` as the
    /// empty string and raises nothing, so the function returns what it landed
    /// on and a mismatch is reported as a failure. Silently answering "selected"
    /// for an option that does not exist is the failure mode this whole driver
    /// keeps being caught by.
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        let element = Self::extract_element_ref(&target)?;
        let result = self
            .select_and_call(
                tab_id,
                "evaluate_script",
                select_script_args(&element, value),
            )
            .await?;
        let landed = Self::extract_text(&result);
        let landed = parse_evaluate_value(&landed)
            .and_then(|v| serde_json::from_str::<String>(&v).ok())
            .unwrap_or_default();
        if landed == value {
            return Ok(());
        }
        Err(BrowserError::ActionFailed(format!(
            "select did not take: asked for '{value}', the element now holds '{landed}' \
             (no matching <option>?)"
        )))
    }

    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.select_and_call(tab_id, "press_key", json!({ "key": key }))
            .await?;
        Ok(())
    }

    /// `Text` waits use the MCP-native `wait_for` tool (server-side wait, no
    /// polling round-trips). The MCP tool has no selector/URL arms, so those
    /// conditions fall back to the shared evaluate-polling loop.
    ///
    /// Lock scope is deliberately narrower than every other action here: the
    /// per-profile guard is held across `select_page` and then released before
    /// the wait is issued. The wait binds to the page the server has selected
    /// when the request arrives, and it can legitimately run for the full
    /// clamped budget (up to 120s) — holding the guard for that long freezes
    /// every other operation on the profile, including the `close_tab` or
    /// navigation that would end the wait. A concurrent op that re-selects
    /// inside the one round-trip window is the residual cost, and it is a
    /// cheaper one than a two-minute profile-wide stall.
    async fn wait_for(
        &self,
        tab_id: &str,
        condition: &WaitCondition,
        timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        let WaitCondition::Text(text) = condition else {
            return super::wait_probe::poll_wait_for(self, tab_id, condition, timeout_ms).await;
        };
        {
            let _guard = self.profile_guard().await;
            self.select_page(tab_id).await?;
        }
        let outcome = self.call("wait_for", wait_for_args(text, timeout_ms)).await;
        match outcome {
            Ok(_) => Ok(true),
            Err(e) => classify_wait_error(e, tab_id),
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
                .await
                .map_err(explain_path_denial)?;
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
        let count = form_fields.len();
        self.select_and_call(tab_id, "fill_form", fill_form_args(form_fields))
            .await?;
        Ok(count)
    }
}

/// Pull the value out of chrome-devtools-mcp's `evaluate_script` transcript.
///
/// The server answers a successful evaluation with
///
/// ````text
/// Script ran on page and returned:
/// ```json
/// <the JSON-encoded value>
/// ```
/// ````
///
/// and a thrown script with a bare `Error: …` carrying no fence at all. So the
/// fence is the anchor, and its absence means "there is no value here" — the
/// caller then passes the text on unchanged rather than inventing one. Same
/// contract as the managed driver's `playwright_cli::parse_result_value`, which
/// exists for the same reason on the other side.
///
/// A JSON scalar is always a single line (strings escape their newlines), and a
/// returned string containing a fence arrives quoted (`"```"`), so requiring the
/// closing line to be exactly ``` cannot be satisfied by the payload.
fn parse_evaluate_value(text: &str) -> Option<String> {
    let mut lines = text.lines();
    lines.by_ref().find(|l| l.trim() == "```json")?;
    let mut value = String::new();
    for line in lines {
        if line.trim() == "```" {
            return Some(value.trim().to_string());
        }
        if !value.is_empty() {
            value.push('\n');
        }
        value.push_str(line);
    }
    // An opening fence with no close is a truncated answer, not a value.
    None
}

/// Arguments for the `evaluate_script` call that stands in for a select
/// primitive. Split out so the shape is pinned by a test rather than reviewed.
fn select_script_args(uid: &str, value: &str) -> serde_json::Value {
    // The value is baked into the function source, not passed through `args`:
    // `args` items are *element uids* the server resolves, so a plain string
    // there comes back as `Element uid "b" not found on page`.
    let encoded = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let function = format!(
        "(el) => {{ el.value = {encoded}; \
         el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
         el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
         return el.value; }}"
    );
    json!({ "function": function, "args": [uid] })
}

/// Turn chrome-devtools-mcp's path refusal into one that names the remedy.
///
/// From 1.6.0 the server restricts every `filePath` argument to the OS temp
/// directory unless the client negotiated MCP `roots` or the operator passed
/// `--allow-unrestricted-paths`. Aleph declares `sampling` only
/// (`mcp::modern::aleph_client_capabilities`), so uploading anything from
/// outside the temp dir — a user's Downloads folder, say — came back as a bare
/// "Access denied: path … is not within any of the configured workspace roots",
/// which names neither the server that refused nor the switch that lifts it.
///
/// [`super::profile::default_chrome_mcp_args`] now passes that switch, so this
/// only fires for an operator who has overridden `args` — exactly the person
/// who can act on the advice.
fn explain_path_denial(err: BrowserError) -> BrowserError {
    let text = err.to_string();
    if !text.contains("Access denied") || !text.contains("workspace roots") {
        return err;
    }
    BrowserError::ActionFailed(format!(
        "{text}\n\nThis is chrome-devtools-mcp's own path restriction, not Aleph's: from v1.6.0 \
         it confines file arguments to the OS temp directory for clients that do not negotiate \
         MCP roots. Add \"--allow-unrestricted-paths\" to \
         [general.browser.chrome_mcp] args, or copy the file into the temp directory first."
    ))
}

/// Arguments for chrome-devtools-mcp's native `fill_form`.
///
/// The array key is **`elements`**, not `fields`. Aleph sent `fields`, and the
/// server's schema marks `elements` as *required* while leaving
/// `additionalProperties: true` — so the stray key was tolerated and the
/// missing one rejected the call outright. `browser_fill_form` on an
/// existing-session profile had therefore never once filled a form; every
/// invocation came back `MCP error -32602: Input validation error`.
///
/// Same shape as the `wait_for` string-vs-list defect directly below, on the
/// same driver, for the same reason: a wire contract with an external server
/// cannot be checked by a fake backend, because a fake answers with whatever
/// the code hoped for. Only the real `tools/list` schema — or a real call —
/// settles it.
fn fill_form_args(elements: Vec<serde_json::Value>) -> serde_json::Value {
    json!({ "elements": elements })
}

/// Arguments for chrome-devtools-mcp's native `wait_for`.
///
/// `text` is a **list** — `zod.array(zod.string()).min(1)`, "resolves when any
/// value appears" — in every published version of the server, including the
/// oldest one still on this machine. Aleph sent a bare string, so the call was
/// answered with `MCP error -32602: Input validation error` and
/// `browser_wait_for(text=…)` on an existing-session profile has never
/// completed. It failed loudly (`classify_wait_error` refuses to read a
/// validation error as "not found"), which is why it survived: a loud failure
/// on a driver nobody had exercised on a real machine looks like the driver
/// being unavailable.
///
/// `timeout` above the server's own ceiling is likewise a validation error
/// rather than a timeout result — the tool layer's clamp (≤120 s) keeps us
/// inside the accepted range.
fn wait_for_args(text: &str, timeout_ms: u64) -> serde_json::Value {
    json!({ "text": [text], "timeout": timeout_ms })
}

/// Turn a failed MCP `wait_for` call into a wait outcome.
///
/// Only ONE failure means "the text did not appear": the tool answered, and its
/// answer was its own timeout. That folds to `Ok(false)` — an absent condition
/// is an answer, not an error.
///
/// Everything else propagates:
/// - [`BrowserError::ChromeMcpTransport`] — nothing ever looked at the page
///   (dead pipe, or the MCP client's own 60s request timeout firing under a
///   120s wait budget). Its message contains the word "timeout" too, which is
///   exactly why the classification cannot be a string match: reporting "the
///   text is not on the page" when the transport died is a confident lie.
/// - [`BrowserError::TabNotFound`] — the tab went away mid-wait.
fn classify_wait_error(err: BrowserError, tab_id: &str) -> Result<bool, BrowserError> {
    match err {
        BrowserError::TabNotFound(_) => Err(BrowserError::TabNotFound(format!(
            "tab '{tab_id}' disappeared while waiting for text"
        ))),
        BrowserError::ChromeMcpError(ref msg) => {
            let lower = msg.to_lowercase();
            if lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("did not appear")
                || lower.contains("exceeded")
            {
                Ok(false)
            } else {
                Err(err)
            }
        }
        other => Err(other),
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

    /// The fold, both ways. A timeout the *tool* reported is the answer
    /// "not found"; the same words arriving as a *transport* failure must stay
    /// an error, because nothing looked at the page.
    #[test]
    fn a_tool_timeout_folds_to_not_found_and_a_transport_timeout_does_not() {
        let verdict = BrowserError::ChromeMcpError(
            "Tool 'wait_for' returned error: Error: Timed out after waiting 2000ms".into(),
        );
        assert!(matches!(
            super::classify_wait_error(verdict, "t1"),
            Ok(false)
        ));

        let transport =
            BrowserError::ChromeMcpTransport("I/O error: request timed out; broken pipe".into());
        assert!(super::classify_wait_error(transport, "t1").is_err());
    }

    /// Shapes copied verbatim from a live chrome-devtools-mcp 1.7.0 — a
    /// hand-imagined transcript here would reproduce the very mistake the
    /// parser exists to correct.
    #[test]
    fn parse_evaluate_value_takes_the_value_out_of_the_transcript() {
        let t = "Script ran on page and returned:\n```json\n0\n```";
        assert_eq!(super::parse_evaluate_value(t).as_deref(), Some("0"));
        let t = "Script ran on page and returned:\n```json\n{\"a\":1,\"b\":[1,2]}\n```";
        assert_eq!(
            super::parse_evaluate_value(t).as_deref(),
            Some("{\"a\":1,\"b\":[1,2]}")
        );
        // A returned string that is itself a fence: it arrives quoted, so the
        // closing-fence line stays unambiguous.
        let t = "Script ran on page and returned:\n```json\n\"```\"\n```";
        assert_eq!(super::parse_evaluate_value(t).as_deref(), Some("\"```\""));
    }

    /// A thrown script carries no fence. `None` means "pass the text on", not
    /// "the value was empty" — folding the two would hand the model an empty
    /// string where an error message belongs.
    #[test]
    fn parse_evaluate_value_declines_a_transcript_with_no_value() {
        assert_eq!(super::parse_evaluate_value("Error: boom"), None);
        assert_eq!(super::parse_evaluate_value(""), None);
        // Opening fence, no close: truncated, not a value.
        assert_eq!(
            super::parse_evaluate_value("Script ran on page and returned:\n```json\n1"),
            None
        );
    }

    /// The value belongs in the function source. `args` items are element uids
    /// the server resolves, so a bare string there comes back as
    /// `Element uid "b" not found on page`.
    #[test]
    fn select_passes_only_the_uid_as_an_arg() {
        let args = super::select_script_args("1_6", "b");
        assert_eq!(args["args"], serde_json::json!(["1_6"]));
        let function = args["function"].as_str().expect("function is a string");
        assert!(function.contains("el.value = \"b\""), "{function}");
        assert!(function.contains("'change'"), "{function}");
    }

    /// The refusal must name the switch that lifts it. Anything else passes
    /// through untouched — a wrapper that fired on every error would bury the
    /// real message under advice about a setting that is not the problem.
    #[test]
    fn a_path_denial_names_the_switch_and_nothing_else_does() {
        let denied = BrowserError::ChromeMcpError(
            "Access denied: path /home/u/a.txt (canonical: /home/u/a.txt) is not within any of \
             the configured workspace roots."
                .into(),
        );
        let text = super::explain_path_denial(denied).to_string();
        assert!(text.contains("--allow-unrestricted-paths"), "{text}");

        let unrelated = BrowserError::ChromeMcpError("Element uid \"1_6\" not found".into());
        let text = super::explain_path_denial(unrelated).to_string();
        assert!(!text.contains("--allow-unrestricted-paths"), "{text}");
    }

    /// Pins the argument shape against the server's schema. The key is
    /// `elements`; `fields` reads just as plausibly and is what shipped, which
    /// is exactly why it needs pinning rather than reviewing.
    #[test]
    fn fill_form_sends_its_array_under_elements() {
        let args = super::fill_form_args(vec![serde_json::json!({"uid": "1_2", "value": "x"})]);
        assert_eq!(
            args,
            serde_json::json!({ "elements": [{"uid": "1_2", "value": "x"}] }),
            "chrome-devtools-mcp's fill_form requires `elements`"
        );
    }

    /// Pins the argument shape against the server's schema, which is the only
    /// thing that made this call fail — the code read fine.
    #[test]
    fn wait_for_sends_the_text_as_a_list() {
        let args = super::wait_for_args("Loading done", 5_000);
        assert_eq!(
            args,
            serde_json::json!({ "text": ["Loading done"], "timeout": 5_000 }),
            "chrome-devtools-mcp's wait_for takes zod.array(zod.string()).min(1)"
        );
    }
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
    fn wait_error_folds_only_the_tools_own_timeout() {
        // The tool answered "I waited and the text never showed" → Ok(false).
        let found = classify_wait_error(
            BrowserError::ChromeMcpError("Timeout 5000ms exceeded".into()),
            "1",
        )
        .expect("the tool's own timeout is an answer, not an error");
        assert!(!found);
    }

    #[test]
    fn wait_error_never_folds_a_transport_failure() {
        // Same word, opposite meaning: nothing looked at the page. Folding this
        // into Ok(false) tells the model the text is not on the page.
        let err = classify_wait_error(
            BrowserError::ChromeMcpTransport("request timeout after 60s".into()),
            "1",
        )
        .expect_err("a transport failure is not a wait verdict");
        assert!(
            matches!(err, BrowserError::ChromeMcpTransport(_)),
            "{err:?}"
        );
    }

    #[test]
    fn wait_error_reports_a_vanished_tab() {
        let err = classify_wait_error(BrowserError::TabNotFound("gone".into()), "7")
            .expect_err("a closed tab is a failure");
        assert!(err.to_string().contains("tab '7' disappeared"), "{err}");
        // A non-timeout tool error is a real failure too.
        assert!(
            classify_wait_error(BrowserError::ChromeMcpError("invalid args".into()), "1").is_err()
        );
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
