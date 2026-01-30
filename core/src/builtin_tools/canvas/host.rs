//! Canvas Host HTTP Server
//!
//! Serves static files for the canvas rendering system.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, warn};

/// Canvas host configuration
#[derive(Debug, Clone)]
pub struct CanvasHostConfig {
    /// Port to listen on
    pub port: u16,
    /// Bind address
    pub bind: String,
    /// Root directory for user content
    pub root_dir: Option<PathBuf>,
    /// Directory for A2UI assets
    pub a2ui_dir: Option<PathBuf>,
    /// Enable live reload
    pub live_reload: bool,
}

impl Default for CanvasHostConfig {
    fn default() -> Self {
        Self {
            port: 18793,
            bind: "127.0.0.1".to_string(),
            root_dir: None,
            a2ui_dir: None,
            live_reload: true,
        }
    }
}

/// Canvas host state
pub struct CanvasHostState {
    config: CanvasHostConfig,
}

impl CanvasHostState {
    /// Create new host state
    pub fn new(config: CanvasHostConfig) -> Self {
        Self { config }
    }
}

/// Create the canvas host router
pub fn create_router(state: Arc<CanvasHostState>) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/__moltbot__/a2ui/*path", get(a2ui_handler))
        .route("/__moltbot__/canvas/*path", get(canvas_handler))
        .route("/__moltbot__/health", get(health_handler))
        .with_state(state)
}

/// Index handler
async fn index_handler(State(state): State<Arc<CanvasHostState>>) -> impl IntoResponse {
    let html = if state.config.live_reload {
        include_str!("host_index.html").replace(
            "<!-- LIVE_RELOAD -->",
            r#"<script>
            (function() {
                const ws = new WebSocket('ws://' + location.host + '/__moltbot__/ws');
                ws.onmessage = function(e) {
                    if (e.data === 'reload') location.reload();
                };
                ws.onclose = function() {
                    setTimeout(function() { location.reload(); }, 1000);
                };
            })();
            </script>"#,
        )
    } else {
        include_str!("host_index.html").replace("<!-- LIVE_RELOAD -->", "")
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
}

/// A2UI assets handler
async fn a2ui_handler(
    State(state): State<Arc<CanvasHostState>>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    // Sanitize path
    let path = sanitize_path(&path);
    if path.is_none() {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }
    let path = path.unwrap();

    // Get A2UI directory
    let a2ui_dir = match &state.config.a2ui_dir {
        Some(dir) => dir.clone(),
        None => {
            // Default to embedded assets or error
            return (StatusCode::NOT_FOUND, "A2UI assets not configured").into_response();
        }
    };

    let file_path = a2ui_dir.join(&path);
    serve_file(&file_path).await
}

/// Canvas content handler
async fn canvas_handler(
    State(state): State<Arc<CanvasHostState>>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    // Sanitize path
    let path = sanitize_path(&path);
    if path.is_none() {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }
    let path = path.unwrap();

    // Get root directory
    let root_dir = match &state.config.root_dir {
        Some(dir) => dir.clone(),
        None => {
            return (StatusCode::NOT_FOUND, "Canvas root not configured").into_response();
        }
    };

    let file_path = root_dir.join(&path);
    serve_file(&file_path).await
}

/// Health check handler
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// Serve a static file
async fn serve_file(path: &std::path::Path) -> Response {
    debug!("Serving file: {}", path.display());

    // Check if file exists
    match fs::metadata(path).await {
        Ok(meta) => {
            if !meta.is_file() {
                return (StatusCode::NOT_FOUND, "Not a file").into_response();
            }
        }
        Err(_) => {
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
    }

    // Read file
    let content = match fs::read(path).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read file: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response();
        }
    };

    // Detect MIME type
    let mime = mime_from_path(path);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(content))
        .unwrap()
}

/// Sanitize a path to prevent directory traversal
fn sanitize_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');

    // Check for path traversal attempts
    if path.contains("..") || path.contains("//") {
        return None;
    }

    // Check for absolute paths
    if path.starts_with('/') {
        return None;
    }

    // Check for null bytes
    if path.contains('\0') {
        return None;
    }

    Some(path.to_string())
}

/// Get MIME type from file path
fn mime_from_path(path: &std::path::Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        // Web
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "wasm" => "application/wasm",

        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",

        // Fonts
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",

        // Media
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogg" => "audio/ogg",

        // Documents
        "pdf" => "application/pdf",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",

        // Data
        "csv" => "text/csv; charset=utf-8",
        "jsonl" => "application/x-ndjson",

        // Default
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_valid() {
        assert_eq!(sanitize_path("index.html"), Some("index.html".to_string()));
        assert_eq!(
            sanitize_path("assets/style.css"),
            Some("assets/style.css".to_string())
        );
        assert_eq!(
            sanitize_path("/leading/slash"),
            Some("leading/slash".to_string())
        );
    }

    #[test]
    fn test_sanitize_path_traversal() {
        assert_eq!(sanitize_path("../etc/passwd"), None);
        assert_eq!(sanitize_path("foo/../bar"), None);
        assert_eq!(sanitize_path("foo//bar"), None);
    }

    #[test]
    fn test_sanitize_path_null_byte() {
        assert_eq!(sanitize_path("foo\0bar"), None);
    }

    #[test]
    fn test_mime_from_path() {
        assert_eq!(
            mime_from_path(std::path::Path::new("test.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_from_path(std::path::Path::new("style.css")),
            "text/css; charset=utf-8"
        );
        assert_eq!(
            mime_from_path(std::path::Path::new("script.js")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_from_path(std::path::Path::new("image.png")),
            "image/png"
        );
        assert_eq!(
            mime_from_path(std::path::Path::new("unknown.xyz")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_config_default() {
        let config = CanvasHostConfig::default();
        assert_eq!(config.port, 18793);
        assert_eq!(config.bind, "127.0.0.1");
        assert!(config.live_reload);
    }
}
