//! Browser Control Handlers
//!
//! JSON-RPC handlers for browser automation via CDP.

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(feature = "browser")]
use crate::browser::{
    BrowserConfig, BrowserService, ClickOptions, ScreenshotOptions, TypeOptions,
};

/// Browser service state for handlers
#[cfg(feature = "browser")]
pub struct BrowserState {
    pub service: RwLock<BrowserService>,
}

#[cfg(feature = "browser")]
impl BrowserState {
    pub fn new(config: BrowserConfig) -> Result<Self, String> {
        let service = BrowserService::new(config).map_err(|e| e.to_string())?;
        Ok(Self {
            service: RwLock::new(service),
        })
    }
}

/// Parameters for browser.start
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StartParams {
    #[serde(default)]
    pub headless: Option<bool>,
}

/// Parameters for browser.navigate
#[derive(Debug, Clone, Deserialize)]
pub struct NavigateParams {
    pub url: String,
}

/// Parameters for browser.click
#[derive(Debug, Clone, Deserialize)]
pub struct ClickParams {
    /// Element reference (e1, e2) or CSS selector
    pub target: String,
    #[serde(default)]
    pub double_click: bool,
    #[serde(default = "default_button")]
    pub button: String,
}

fn default_button() -> String {
    "left".to_string()
}

/// Parameters for browser.type
#[derive(Debug, Clone, Deserialize)]
pub struct TypeParams {
    /// Element reference or CSS selector
    pub target: String,
    /// Text to type
    pub text: String,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub submit: bool,
}

/// Parameters for browser.screenshot
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScreenshotParams {
    #[serde(default)]
    pub full_page: bool,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_format() -> String {
    "png".to_string()
}

/// Parameters for browser.evaluate
#[derive(Debug, Clone, Deserialize)]
pub struct EvaluateParams {
    pub script: String,
}

/// Parameters for browser.tab
#[derive(Debug, Clone, Deserialize)]
pub struct TabParams {
    pub url: Option<String>,
}

/// Handle browser.start - Launch browser
#[cfg(feature = "browser")]
pub async fn handle_start(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let _params: StartParams = match request.params {
        Some(p) => serde_json::from_value(p).unwrap_or_default(),
        None => StartParams { headless: None },
    };

    let mut service = state.service.write().await;

    if service.is_running() {
        return JsonRpcResponse::success(
            request.id,
            json!({"ok": true, "message": "Browser already running"}),
        );
    }

    match service.start().await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({"ok": true, "message": "Browser started"}),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to start browser: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_start(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.stop - Terminate browser
#[cfg(feature = "browser")]
pub async fn handle_stop(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let mut service = state.service.write().await;

    match service.stop().await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({"ok": true, "message": "Browser stopped"}),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to stop browser: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_stop(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.status - Get browser status
#[cfg(feature = "browser")]
pub async fn handle_status(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let service = state.service.read().await;
    let running = service.is_running();

    let mut result = json!({
        "ok": true,
        "running": running,
    });

    if running {
        if let Ok(url) = service.current_url().await {
            result["url"] = json!(url);
        }
        if let Ok(title) = service.current_title().await {
            result["title"] = json!(title);
        }
    }

    JsonRpcResponse::success(request.id, result)
}

#[cfg(not(feature = "browser"))]
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(request.id, json!({"ok": true, "running": false, "enabled": false}))
}

/// Handle browser.navigate - Navigate to URL
#[cfg(feature = "browser")]
pub async fn handle_navigate(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: NavigateParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid params: {}", e)),
        },
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing url parameter"),
    };

    let mut service = state.service.write().await;

    match service.navigate(&params.url).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Navigation failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_navigate(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.screenshot - Capture screenshot
#[cfg(feature = "browser")]
pub async fn handle_screenshot(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: ScreenshotParams = match request.params.clone() {
        Some(p) => serde_json::from_value(p).unwrap_or_default(),
        None => ScreenshotParams::default(),
    };

    let service = state.service.read().await;

    let options = ScreenshotOptions {
        full_page: params.full_page,
        format: params.format,
        ..Default::default()
    };

    match service.screenshot(options).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Screenshot failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_screenshot(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.snapshot - Get page accessibility snapshot
#[cfg(feature = "browser")]
pub async fn handle_snapshot(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let mut service = state.service.write().await;

    match service.snapshot().await {
        Ok(snapshot) => JsonRpcResponse::success(request.id, json!({
            "ok": true,
            "url": snapshot.url,
            "title": snapshot.title,
            "nodes": snapshot.nodes,
            "total_elements": snapshot.total_elements,
            "interactive_count": snapshot.interactive_count,
            "truncated": snapshot.truncated,
        })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Snapshot failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_snapshot(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.click - Click element
#[cfg(feature = "browser")]
pub async fn handle_click(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: ClickParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid params: {}", e)),
        },
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing target parameter"),
    };

    let service = state.service.read().await;

    let options = ClickOptions {
        double_click: params.double_click,
        button: params.button,
        ..Default::default()
    };

    match service.click(&params.target, options).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Click failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_click(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.type - Type text into element
#[cfg(feature = "browser")]
pub async fn handle_type(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: TypeParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid params: {}", e)),
        },
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing parameters"),
    };

    let service = state.service.read().await;

    let options = TypeOptions {
        clear: params.clear,
        submit: params.submit,
        ..Default::default()
    };

    match service.type_text(&params.target, &params.text, options).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Type failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_type(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.evaluate - Run JavaScript
#[cfg(feature = "browser")]
pub async fn handle_evaluate(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: EvaluateParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid params: {}", e)),
        },
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing script parameter"),
    };

    let service = state.service.read().await;

    match service.evaluate(&params.script).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Evaluate failed: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_evaluate(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.tabs - List open tabs
#[cfg(feature = "browser")]
pub async fn handle_tabs(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let service = state.service.read().await;

    match service.list_tabs().await {
        Ok(tabs) => JsonRpcResponse::success(request.id, json!({
            "ok": true,
            "tabs": tabs,
            "count": tabs.len(),
        })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to list tabs: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_tabs(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.newTab - Open new tab
#[cfg(feature = "browser")]
pub async fn handle_new_tab(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let params: TabParams = match request.params.clone() {
        Some(p) => serde_json::from_value(p).unwrap_or(TabParams { url: None }),
        None => TabParams { url: None },
    };

    let mut service = state.service.write().await;

    match service.new_tab(params.url.as_deref()).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to open tab: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_new_tab(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}

/// Handle browser.closeTab - Close current tab
#[cfg(feature = "browser")]
pub async fn handle_close_tab(
    request: JsonRpcRequest,
    state: Arc<BrowserState>,
) -> JsonRpcResponse {
    let mut service = state.service.write().await;

    match service.close_tab().await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to close tab: {}", e)),
    }
}

#[cfg(not(feature = "browser"))]
pub async fn handle_close_tab(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Browser feature not enabled")
}
