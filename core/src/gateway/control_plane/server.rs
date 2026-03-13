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
        .route("/login", get(serve_login))
        .route("/{*path}", get(serve_static_or_index))
}

/// Serve a standalone login page (no WASM) for token entry.
/// This is a client-side-only form: it saves the token to localStorage
/// and redirects to `/` where the WASM panel validates via WebSocket.
async fn serve_login() -> Html<&'static str> {
    Html(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Aleph — Login</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0a0a0f;color:#e0e0e0;display:flex;justify-content:center;align-items:center;min-height:100vh}
.c{background:#14141f;border:1px solid #2a2a3a;border-radius:16px;padding:40px;max-width:400px;width:100%}
h1{font-size:24px;margin-bottom:8px}
p{color:#888;font-size:14px;margin-bottom:24px}
input{width:100%;padding:12px 16px;background:#0a0a0f;border:1px solid #2a2a3a;border-radius:8px;color:#e0e0e0;font-size:16px;margin-bottom:16px}
input:focus{outline:none;border-color:#6366f1}
button{width:100%;padding:12px;background:#6366f1;color:#fff;border:none;border-radius:8px;font-size:16px;cursor:pointer}
button:hover{background:#5558e6}
.err{background:#3b1419;border:1px solid #7f1d1d;color:#fca5a5;padding:12px;border-radius:8px;margin-bottom:16px;font-size:14px;display:none}
</style>
</head>
<body>
<div class="c">
<h1>Aleph</h1>
<p>Enter your access token to continue</p>
<div class="err" id="err"></div>
<form id="lf">
<input type="password" name="token" placeholder="Access token" autofocus required>
<button type="submit">Sign in</button>
</form>
<script>
document.getElementById('lf').addEventListener('submit',function(e){
  e.preventDefault();
  var t=this.querySelector('input[name=token]').value;
  if(!t){return}
  try{localStorage.setItem('aleph_shared_token',t)}catch(ex){}
  window.location.href='/';
});
</script>
</div>
</body>
</html>"#)
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
                    (header::CACHE_CONTROL, "no-store, must-revalidate"),
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
