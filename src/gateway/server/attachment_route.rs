//! `GET /api/memory/sessions/attachment/{id}` — attachment download route.
//!
//! Serves binary attachments from the Markdown session store.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::session::RawSessionStore;
use crate::sync_primitives::Arc;

/// State for the attachment route.
pub struct AttachmentRouteState {
    pub store: Arc<MarkdownSessionStore>,
}

/// The attachment download route, ready to merge into the gateway router.
pub fn attachment_routes(state: Arc<AttachmentRouteState>) -> Router {
    Router::new()
        .route(
            "/api/memory/sessions/attachment/{id}",
            get(serve_attachment),
        )
        .with_state(state)
}

/// Serve an attachment by ID.
///
/// **Route**: `GET /api/memory/sessions/attachment/{id}`
///
/// **Path Parameters**:
/// - `id`: Attachment ID (format: `{session_id}:{attachment_id}`)
///
/// **Returns**: Binary attachment data with correct MIME type.
async fn serve_attachment(
    State(state): State<Arc<AttachmentRouteState>>,
    Path(id): Path<String>,
) -> Response {
    // Parse attachment ID: format is "{session_id}:{attachment_id}"
    let Some((session_id, attachment_id)) = id.rsplit_once(':') else {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid attachment ID format. Expected: session_id:attachment_id",
        )
            .into_response();
    };

    // Get attachment from store
    let attachment = match state.store.get_attachment(session_id, attachment_id).await {
        Ok(attachment) => attachment,
        Err(e) => {
            tracing::warn!(
                "Failed to get attachment {}:{}: {}",
                session_id,
                attachment_id,
                e
            );
            return (StatusCode::NOT_FOUND, "Attachment not found").into_response();
        }
    };

    // Build response with correct MIME type
    let mut response = Response::new(attachment.data.into());
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, attachment.mime_type.parse().unwrap());

    // Add Content-Disposition with original filename if available
    if let Some(original_name) = attachment.original_name {
        if let Ok(disposition) = format!("inline; filename=\"{}\"", original_name).parse() {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, disposition);
        }
    }

    response
}
