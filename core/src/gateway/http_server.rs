//! HTTP Server with Static File Serving
//!
//! Provides an HTTP server that serves both WebSocket connections and static files
//! for the WebChat UI.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::{
    body::Body,
    http::{Response, StatusCode, Uri},
    Router,
};
use tower_http::services::ServeDir;
use tracing::{debug, error, info};

/// Configuration for the HTTP server
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    /// Address to bind to
    pub addr: SocketAddr,
    /// Path to static files directory
    pub static_dir: Option<PathBuf>,
    /// Fallback file for SPA routing (typically index.html)
    pub fallback_file: Option<String>,
    /// Enable CORS for development
    pub enable_cors: bool,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            addr: ([127, 0, 0, 1], 18789).into(),
            static_dir: None,
            fallback_file: Some("index.html".to_string()),
            enable_cors: true,
        }
    }
}

/// HTTP Server with static file serving capabilities
pub struct HttpServer {
    config: HttpServerConfig,
}

impl HttpServer {
    /// Create a new HTTP server
    pub fn new(config: HttpServerConfig) -> Self {
        Self { config }
    }

    /// Create the Axum router for static file serving
    pub fn create_static_router(&self) -> Option<Router> {
        let static_dir = self.config.static_dir.as_ref()?;

        if !static_dir.exists() {
            debug!("Static directory not found: {}", static_dir.display());
            return None;
        }

        // Create serve directory service
        let serve_dir = ServeDir::new(static_dir.clone())
            .append_index_html_on_directories(true);

        // If we have a fallback file, use it for SPA routing
        let router = if let Some(fallback) = &self.config.fallback_file {
            let fallback_path = static_dir.join(fallback);
            if fallback_path.exists() {
                let fallback_service = ServeDir::new(static_dir.clone())
                    .append_index_html_on_directories(true)
                    .fallback(tower_http::services::ServeFile::new(fallback_path));

                Router::new()
                    .fallback_service(fallback_service)
            } else {
                Router::new()
                    .fallback_service(serve_dir)
            }
        } else {
            Router::new()
                .fallback_service(serve_dir)
        };

        info!("Static file serving enabled: {}", static_dir.display());
        Some(router)
    }

    /// Get the static directory path
    pub fn static_dir(&self) -> Option<&PathBuf> {
        self.config.static_dir.as_ref()
    }
}

/// Helper function to check if a path should be handled as a static file
pub fn is_static_file_request(uri: &Uri) -> bool {
    let path = uri.path();

    // Check for file extensions
    let has_extension = path.rfind('.').map(|i| {
        let ext = &path[i + 1..];
        matches!(
            ext.to_lowercase().as_str(),
            "html" | "css" | "js" | "mjs" | "json" | "svg" | "png" | "jpg" | "jpeg" | "gif" | "ico" | "woff" | "woff2" | "ttf" | "eot" | "map"
        )
    }).unwrap_or(false);

    // Check for common static paths
    let is_static_path = path.starts_with("/assets/")
        || path.starts_with("/static/")
        || path == "/favicon.ico"
        || path == "/aleph.svg"
        || path == "/";

    has_extension || is_static_path
}

/// Serve a static file from the directory
pub async fn serve_static_file(
    static_dir: &PathBuf,
    uri: &Uri,
    fallback_file: Option<&str>,
) -> Response<Body> {
    let path = uri.path();

    // Strip leading slash and construct full path
    let file_path = if path == "/" {
        static_dir.join("index.html")
    } else {
        static_dir.join(path.trim_start_matches('/'))
    };

    // Try to serve the file
    if file_path.exists() && file_path.is_file() {
        match tokio::fs::read(&file_path).await {
            Ok(content) => {
                let mime = mime_guess::from_path(&file_path)
                    .first_or_octet_stream()
                    .to_string();

                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", mime)
                    .header("Cache-Control", "public, max-age=3600")
                    .body(Body::from(content))
                    .unwrap_or_else(|_| not_found())
            }
            Err(e) => {
                error!("Failed to read file {}: {}", file_path.display(), e);
                not_found()
            }
        }
    } else if let Some(fallback) = fallback_file {
        // SPA fallback - serve index.html for non-file routes
        let fallback_path = static_dir.join(fallback);
        if fallback_path.exists() {
            match tokio::fs::read(&fallback_path).await {
                Ok(content) => {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/html; charset=utf-8")
                        .body(Body::from(content))
                        .unwrap_or_else(|_| not_found())
                }
                Err(_) => not_found(),
            }
        } else {
            not_found()
        }
    } else {
        not_found()
    }
}

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not Found"))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_static_file_request() {
        assert!(is_static_file_request(&"/index.html".parse().unwrap()));
        assert!(is_static_file_request(&"/assets/main.js".parse().unwrap()));
        assert!(is_static_file_request(&"/favicon.ico".parse().unwrap()));
        assert!(is_static_file_request(&"/".parse().unwrap()));
        assert!(!is_static_file_request(&"/api/health".parse().unwrap()));
        assert!(!is_static_file_request(&"/ws".parse().unwrap()));
    }
}
