//! HTTP Server for ControlPlane
//!
//! Provides HTTP routes for serving ControlPlane static assets.

use axum::{
    Router,
    routing::get,
    response::{Html, IntoResponse, Response},
    http::{StatusCode, header},
    extract::Path as AxumPath,
};

use super::assets::ControlPlaneAssets;

/// Create the ControlPlane router
pub fn create_control_plane_router() -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/{*path}", get(serve_static_or_index))
}

/// Serve the index.html file
async fn serve_index() -> impl IntoResponse {
    match ControlPlaneAssets::get_index_html() {
        Some(content) => Html(content).into_response(),
        None => (StatusCode::NOT_FOUND, "ControlPlane index.html not found").into_response(),
    }
}

/// Serve static assets or index.html for SPA routing
pub async fn serve_static_asset(AxumPath(path): AxumPath<String>) -> Response {
    serve_static_or_index(AxumPath(path)).await
}

/// Serve static assets or index.html for SPA routing (internal)
async fn serve_static_or_index(AxumPath(path): AxumPath<String>) -> Response {
    // If path is empty, just "/", or ends with "/", serve index.html
    if path.is_empty() || path == "/" || path.ends_with('/') {
        return serve_index().await.into_response();
    }

    // Try to serve as static asset first
    match ControlPlaneAssets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream();

            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.as_ref()),
                    (header::CACHE_CONTROL, "public, max-age=31536000"),
                ],
                content.data,
            ).into_response()
        }
        None => {
            // For SPA routing, return index.html for non-file paths
            if !path.contains('.') {
                return serve_index().await.into_response();
            }
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_router() {
        let _router = create_control_plane_router();
        // Just check that it compiles
    }
}
