//! Authenticated Panel module delivery. Core owns declarations and bytes;
//! this ingress owns HTTP credential validation and request policy.

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, OriginalUri, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::extension::panel_publication::{PanelAssetIdentity, PublishedPanelAsset};
use crate::extension::visibility::VisibilityCtx;
use crate::extension::ExtensionManager;
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::panel_project_context::{PanelCallerContext, PanelProjectSelector};
use crate::gateway::rate_limiter::{
    RateLimitConfig, RateLimitKey, RateLimitScope, RateLimiter, WindowConfig,
};
use crate::gateway::security::SharedTokenManager;
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

use crate::gateway::handlers::connect::resolve_connection_identity;
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::gateway::server::connection::auth::wall_admits;
use crate::gateway::trusted_proxy::ResolvedClient;

/// Fields are private: request values cannot manufacture an authenticated caller.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PanelHttpAuthContext {
    user: String,
    role: &'static str,
    device_id: Option<String>,
    verified_user: Option<String>,
}

impl PanelHttpAuthContext {
    #[cfg(test)]
    pub(super) fn user(&self) -> &str {
        &self.user
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PanelHttpAuthError {
    MissingCredential,
    InvalidCredential,
    Walled,
}

/// Require a live credential on every HTTP request, including loopback.
/// Bootstrap exchange and credential-less WebSocket compatibility are not HTTP grants.
pub(super) fn authenticate(
    headers: &HeaderMap,
    client: ResolvedClient,
    device_tokens: &DeviceTokenManager,
    store: &SecurityStore,
    validate_shared: &dyn Fn(&str) -> bool,
) -> Result<PanelHttpAuthContext, PanelHttpAuthError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next().ok_or(PanelHttpAuthError::MissingCredential)?;
    if values.next().is_some() {
        return Err(PanelHttpAuthError::InvalidCredential);
    }
    let token = value
        .to_str()
        .ok()
        .and_then(extract_bearer_token)
        .filter(|token| !token.chars().any(char::is_whitespace))
        .ok_or(PanelHttpAuthError::InvalidCredential)?;

    let device_id = match device_tokens.validate_device_token(token) {
        Ok(Some(row)) => Some(row.device_id),
        Ok(None) if validate_shared(token) => None,
        _ => return Err(PanelHttpAuthError::InvalidCredential),
    };
    let (user, role) = resolve_connection_identity(client.local, device_id.as_deref(), store);
    if !wall_admits(Some(role), "") {
        return Err(PanelHttpAuthError::Walled);
    }
    let user = user.ok_or(PanelHttpAuthError::Walled)?;
    let verified_user = match device_id.as_deref() {
        Some(id) => store
            .get_device(id)
            .map_err(|_| PanelHttpAuthError::Walled)?
            .and_then(|device| device.user_id)
            .filter(|bound| bound == &user),
        None => None,
    };
    Ok(PanelHttpAuthContext { user, role, device_id, verified_user })
}

// These handles are ingress dependencies, never a second publication registry.
type ManagerReader = dyn Fn() -> Option<Arc<ExtensionManager>> + Send + Sync;
type SharedValidator = dyn Fn(&str) -> bool + Send + Sync;

pub(super) struct PanelAssetRouteState {
    device_tokens: Arc<DeviceTokenManager>,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
    manager: Arc<ManagerReader>,
    validate_shared: Arc<SharedValidator>,
    rate_limiter: RateLimiter,
    panel_projects: Option<Arc<PanelProjectSelector>>,
}

impl PanelAssetRouteState {
    pub(super) fn new(
        device_tokens: Arc<DeviceTokenManager>,
        origin_policy: Arc<OriginPolicy>,
        trusted_proxy_enabled: bool,
        trusted_proxy_ips: Vec<IpAddr>,
        allow_insecure_remote: bool,
        tls_enabled: bool,
        manager: Arc<ManagerReader>,
        validate_shared: Arc<SharedValidator>,
    ) -> Self {
        Self {
            device_tokens,
            origin_policy,
            trusted_proxy_enabled,
            trusted_proxy_ips,
            allow_insecure_remote,
            tls_enabled,
            manager,
            validate_shared,
            panel_projects: None,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                // Authentication and byte reads cannot consume RPC budgets.
                exempt_loopback: false,
                rpc_heavy: WindowConfig {
                    max_requests: 120,
                    window_secs: 60,
                    lockout_secs: None,
                },
                ..RateLimitConfig::default()
            }),
        }
    }

    fn asset(&self, caller: PanelHttpAuthContext, uri: &Uri) -> Option<(PublishedPanelAsset, u64)> {
        // Match the raw URI, not a percent-decoded client path or filesystem path.
        if uri.query().is_some() || uri.path().contains('%') {
            return None;
        }
        let manager = (self.manager)()?;
        let (context, generation) = match &self.panel_projects {
            Some(selector) => {
                let resolved = selector.resolve(&PanelCallerContext::authenticated(
                    caller.verified_user, caller.device_id,
                )).ok()?;
                (resolved.visibility, resolved.selection.generation)
            }
            None => (VisibilityCtx::default(), 0),
        };
        let snapshot = manager.panel_ui_capability_snapshot_for(&context);
        let mut declarations = snapshot.declarations.iter().filter(|declaration| {
            declaration.public_base == "./tabs/"
                && declaration.descriptor.entry.strip_prefix('.') == Some(uri.path())
        });
        let declaration = declarations.next()?;
        if declarations.next().is_some() {
            return None;
        }
        // Identity, version, entry and revision all come from the admitted record.
        // A concurrent replacement between these reads fails in the Core store.
        manager
            .panel_ui_asset(
                &PanelAssetIdentity::from_declaration(declaration),
                &declaration.descriptor.entry,
                &context,
            )
            .ok()
            .map(|asset| (asset, generation))
    }
}

pub(super) fn production_state(
    device_tokens: Arc<DeviceTokenManager>,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
    panel_projects: Option<Arc<PanelProjectSelector>>,
) -> Arc<PanelAssetRouteState> {
    let mut state = PanelAssetRouteState::new(
        device_tokens,
        origin_policy,
        trusted_proxy_enabled,
        trusted_proxy_ips,
        allow_insecure_remote,
        tls_enabled,
        Arc::new(|| crate::extension::try_extension_manager().cloned()),
        Arc::new(SharedTokenManager::global_validate),
    );
    state.panel_projects = panel_projects;
    Arc::new(state)
}

pub(super) fn panel_asset_routes(state: impl Into<Option<Arc<PanelAssetRouteState>>>) -> Router {
    Router::new()
        .route("/tabs", get(serve_asset))
        .route("/tabs/", get(serve_asset))
        .route("/tabs/{*entry}", get(serve_asset))
        .with_state(state.into())
}

fn denied() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "private, no-store")],
        "Panel asset unavailable",
    )
        .into_response()
}

async fn serve_asset(
    State(state): State<Option<Arc<PanelAssetRouteState>>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    // Built-in modules already belong to the public build asset owner. Preserve
    // that delivery without turning a missing community entry into an SPA page.
    // Authenticated requests always take the Core publication path below.
    let public_path = uri.path().trim_start_matches('/');
    if !headers.contains_key(header::AUTHORIZATION)
        && uri.query().is_none()
        && aleph_protocol::panel::valid_relative_entry(&format!("./{public_path}"))
        && crate::gateway::control_plane::assets::ControlPlaneAssets::get(public_path).is_some()
    {
        return crate::gateway::control_plane::server::serve_static_asset(
            headers,
            axum::extract::Path(public_path.to_owned()),
        )
        .await;
    }
    let Some(state) = state else {
        return denied();
    };
    let client = resolve_client(
        peer.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    if super::handler::refuse_insecure_remote(
        client.local,
        state.tls_enabled || client.secure,
        state.allow_insecure_remote,
    ) {
        return denied();
    }
    // Malformed or duplicate Origin/Host values must not become an absent header.
    if [header::ORIGIN, header::HOST].iter().any(|name| {
        headers.get_all(name).iter().count() > 1
            || headers
                .get(name)
                .is_some_and(|value| value.to_str().is_err())
    }) {
        return denied();
    }
    if !state.origin_policy.is_allowed(
        headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok()),
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) || headers.contains_key("x-aleph-project")
        || headers.contains_key("x-project-id")
    {
        return denied();
    }
    if state
        .rate_limiter
        .check_and_record(&RateLimitKey::new(
            &client.ip.to_string(),
            RateLimitScope::RpcHeavy,
        ))
        .is_err()
    {
        return denied();
    }
    let caller = match authenticate(
        &headers,
        client,
        &state.device_tokens,
        state.device_tokens.store(),
        state.validate_shared.as_ref(),
    ) {
        Ok(caller) => caller,
        Err(_) => return denied(),
    };
    let Some((asset, generation)) = state.asset(caller, &uri) else {
        return denied();
    };
    let mut response = (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::VARY, "Authorization, Origin"),
        ],
        Body::from(asset.bytes.as_ref().to_vec()),
    )
        .into_response();
    response.headers_mut().insert(
        aleph_protocol::panel::PANEL_PROJECT_GENERATION_HEADER,
        generation.to_string().parse().expect("integer header"),
    );
    response
}
