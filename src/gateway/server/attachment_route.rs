//! `GET /api/memory/sessions/attachment/{id}` — attachment download route.
//!
//! Serves binary attachments from the Markdown session store.

use std::net::{IpAddr, SocketAddr};

use crate::gateway::caller_identity::with_caller_identity;
use crate::gateway::handlers::connect::resolve_connection_identity;
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::router::SessionKey;
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::security::{DeviceTokenManager, SharedTokenManager};
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::session::RawSessionStore;
use crate::sync_primitives::Arc;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

/// State for the attachment route.
pub struct AttachmentRouteState {
    pub store: Arc<MarkdownSessionStore>,
    pub gateway_store: Arc<dyn SessionStore>,
    /// Device tokens, when boot installed the manager. `None` ⇒ only the
    /// shared token is accepted.
    pub device_tokens: Option<Arc<DeviceTokenManager>>,
    pub shared_token: Option<Arc<SharedTokenManager>>,
    pub trusted_proxy_enabled: bool,
    pub trusted_proxy_ips: Vec<IpAddr>,
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

/// Resolve the HTTP credential into the identity this download runs under.
///
/// `None` ⇒ no usable credential, so the caller is denied. A shared token is
/// the operator's own credential (single-user deployments carry the implicit
/// owner); a device token resolves through that device's user binding, using
/// the same resolution the socket handshake uses so an HTTP download and a
/// socket see the same owner.
fn authenticate(
    headers: &HeaderMap,
    device_tokens: Option<&DeviceTokenManager>,
    shared_token: Option<&SharedTokenManager>,
    client_is_local: bool,
) -> Option<(String, &'static str)> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let token = value.to_str().ok().and_then(extract_bearer_token)?.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(manager) = shared_token {
        if manager.validate(token).unwrap_or(false) {
            // The shared token IS the operator's credential: it carries no device
            // binding, so it resolves to the implicit owner.
            return Some((OWNER_USER_ID.to_string(), "operator"));
        }
    }
    let manager = device_tokens?;
    let row = manager.validate_device_token(token).ok().flatten()?;
    let (user, role) = resolve_connection_identity(
        client_is_local,
        Some(row.device_id.as_str()),
        manager.store().as_ref(),
    );
    Some((user?, role))
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    // Parse attachment ID: format is "{session_id}:{attachment_id}"
    let Some((session_id, attachment_id)) = id.rsplit_once(':') else {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid attachment ID format. Expected: session_id:attachment_id",
        )
            .into_response();
    };

    // Reject traversal before touching either storage backend.
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || std::path::Path::new(session_id).is_absolute()
        || std::path::Path::new(session_id).components().count() != 1
    {
        return (StatusCode::NOT_FOUND, "Attachment not found").into_response();
    }

    // Authenticate before touching either storage backend. HTTP dispatch
    // never scopes a `CALLER_USER` task-local, so an unauthenticated caller
    // would read as "internal, unrestricted" and receive another user's
    // bytes. A missing or invalid credential is denied, never read as
    // internal.
    let client = crate::gateway::trusted_proxy::resolve_client(
        peer.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let device_tokens = state.device_tokens.clone();
    let shared_token = state.shared_token.clone();
    let identity = tokio::task::spawn_blocking(move || {
        authenticate(
            &headers,
            device_tokens.as_deref(),
            shared_token.as_deref(),
            client.local,
        )
    })
    .await
    .ok()
    .flatten();
    let Some((caller_user, caller_role)) = identity else {
        return (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
    };

    let Some(key) = SessionKey::parse(session_id) else {
        return (StatusCode::NOT_FOUND, "Attachment not found").into_response();
    };
    // Scope the resolved identity so the owner predicate below judges the
    // same caller the socket dispatch would scope.
    let visible = with_caller_identity(
        Some(caller_role.to_string()),
        Some(caller_user),
        client.local,
        None,
        async {
            match state.gateway_store.get_metadata(&key).await {
                Ok(Some(metadata)) => visibility::session_visible(&metadata),
                Ok(None) | Err(_) => false,
            }
        },
    )
    .await;
    if !visible {
        return (StatusCode::NOT_FOUND, "Attachment not found").into_response();
    }

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

    attachment_response(attachment)
}

fn attachment_response(attachment: crate::memory::store::session::Attachment) -> Response {
    // Build response with correct MIME type
    let mut response = Response::new(attachment.data.into());
    let mime_value = attachment
        .mime_type
        .parse()
        .unwrap_or_else(|_| "application/octet-stream".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, mime_value);

    let mime_lower = attachment.mime_type.to_ascii_lowercase();
    let active_document =
        mime_lower.starts_with("text/html") || mime_lower.starts_with("image/svg");
    let disposition_type = if active_document {
        "attachment"
    } else {
        "inline"
    };
    if active_document {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            super::artifact_route::ARTIFACT_DOCUMENT_CSP
                .parse()
                .unwrap(),
        );
        // Enforce isolation even without a usable original filename.
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, "attachment".parse().unwrap());
    }
    if let Some(original_name) = attachment.original_name {
        if let Ok(disposition) =
            format!("{}; filename=\"{}\"", disposition_type, original_name).parse()
        {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, disposition);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::{SecurityStore, UserRole};
    use crate::memory::store::session::Attachment;

    #[test]
    fn attachment_auth_uses_instance_credentials_and_device_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store.clone(), dir.path().join("vault"));
        let token = manager.generate_token().unwrap();
        let foreign = SharedTokenManager::new(
            Arc::new(SecurityStore::in_memory().unwrap()),
            dir.path().join("other-vault"),
        );
        let foreign_token = foreign.generate_token().unwrap();
        let headers = |token: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
            headers
        };
        assert!(authenticate(&HeaderMap::new(), None, Some(&manager), true).is_none());
        assert!(authenticate(&headers(&foreign_token), None, Some(&manager), false).is_none());
        assert!(authenticate(&headers(&token), None, None, false).is_none());
        assert_eq!(
            authenticate(&headers(&token), None, Some(&manager), false),
            Some((OWNER_USER_ID.to_string(), "operator"))
        );
        manager.generate_token().unwrap();
        assert!(authenticate(&headers(&token), None, Some(&manager), false).is_none());

        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let devices = DeviceTokenManager::new(store.clone());
        let ticket = devices
            .create_bootstrap_ticket(None, Some("u-alice"))
            .unwrap();
        let device = devices
            .exchange_bootstrap_ticket(&ticket, Some("panel-alice".into()), None, None)
            .unwrap();
        // A credential-validated local device still resolves to its bound user.
        for local in [false, true] {
            assert_eq!(
                authenticate(
                    &headers(&device.device_token),
                    Some(&devices),
                    Some(&manager),
                    local
                ),
                Some(("u-alice".to_string(), "member"))
            );
        }
        devices.revoke_panel_device("panel-alice").unwrap();
        assert!(authenticate(
            &headers(&device.device_token),
            Some(&devices),
            Some(&manager),
            false
        )
        .is_none());
    }

    #[tokio::test]
    async fn active_attachments_are_sandboxed_without_requiring_a_filename() {
        for mime in ["text/html", "TEXT/HTML; charset=utf-8", "image/svg+xml"] {
            for name in [None, Some("page.html"), Some("bad\nname")] {
                let response = attachment_response(Attachment {
                    data: b"<script>bad()</script>".to_vec(),
                    mime_type: mime.to_string(),
                    original_name: name.map(str::to_string),
                });
                assert_eq!(
                    response.headers()[header::CONTENT_SECURITY_POLICY],
                    super::super::artifact_route::ARTIFACT_DOCUMENT_CSP
                );
                assert!(response.headers()[header::CONTENT_DISPOSITION]
                    .to_str()
                    .unwrap()
                    .starts_with("attachment"));
                assert_eq!(
                    axum::body::to_bytes(response.into_body(), 1024)
                        .await
                        .unwrap(),
                    &b"<script>bad()</script>"[..]
                );
            }
        }
        let image = attachment_response(Attachment {
            data: vec![1, 2, 3],
            mime_type: "image/png".to_string(),
            original_name: Some("photo.png".to_string()),
        });
        assert_eq!(image.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            image.headers()[header::CONTENT_DISPOSITION],
            "inline; filename=\"photo.png\""
        );
        assert!(!image
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY));
    }
}
