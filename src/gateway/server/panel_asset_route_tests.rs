use std::net::IpAddr;

use axum::http::{header::AUTHORIZATION, HeaderMap};

use crate::gateway::security::{DeviceTokenManager, SecurityStore, SharedTokenManager};
use crate::gateway::trusted_proxy::ResolvedClient;
use crate::sync_primitives::Arc;

use super::panel_asset_route::{authenticate, PanelHttpAuthError};

fn store() -> Arc<SecurityStore> {
    Arc::new(SecurityStore::in_memory().expect("security store"))
}

fn local_client() -> ResolvedClient {
    ResolvedClient {
        ip: "127.0.0.1".parse::<IpAddr>().expect("loopback"),
        secure: false,
        local: true,
    }
}

fn remote_client() -> ResolvedClient {
    ResolvedClient {
        ip: "203.0.113.7".parse::<IpAddr>().expect("remote"),
        secure: true,
        local: false,
    }
}

fn headers(value: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = value {
        headers.insert(AUTHORIZATION, value.parse().expect("authorization"));
    }
    headers
}

#[test]
fn loopback_without_explicit_credential_is_refused() {
    let store = store();
    let device_tokens = DeviceTokenManager::new(store.clone());

    assert_eq!(
        authenticate(
            &headers(None),
            local_client(),
            &device_tokens,
            &store,
            &|_| false,
        ),
        Err(PanelHttpAuthError::MissingCredential),
    );
}

#[test]
fn loopback_with_invalid_credential_is_refused() {
    let store = store();
    let device_tokens = DeviceTokenManager::new(store.clone());

    assert_eq!(
        authenticate(
            &headers(Some("Bearer invalid")),
            local_client(),
            &device_tokens,
            &store,
            &|_| false,
        ),
        Err(PanelHttpAuthError::InvalidCredential),
    );
}

#[test]
fn valid_shared_bearer_creates_route_owned_context() {
    let store = store();
    let device_tokens = DeviceTokenManager::new(store.clone());

    let context = authenticate(
        &headers(Some("bEaReR shared-secret")),
        remote_client(),
        &device_tokens,
        &store,
        &|token| token == "shared-secret",
    )
    .expect("valid shared bearer");

    assert_eq!(
        context.user(),
        crate::gateway::security::store::OWNER_USER_ID
    );
}

#[test]
fn shared_token_rotation_refuses_the_previous_bearer() {
    let root = tempfile::tempdir().unwrap();
    let store = store();
    let tokens = DeviceTokenManager::new(store.clone());
    let shared = SharedTokenManager::with_secret(store.clone(), [7; 32], root.path().join("vault"));
    let first = shared.generate_token().unwrap();
    let validate = |token: &str| shared.validate(token).unwrap_or(false);
    assert!(authenticate(
        &headers(Some(&format!("Bearer {first}"))),
        remote_client(),
        &tokens,
        &store,
        &validate
    )
    .is_ok());
    let second = shared.generate_token().unwrap();
    assert_eq!(
        authenticate(
            &headers(Some(&format!("Bearer {first}"))),
            remote_client(),
            &tokens,
            &store,
            &validate
        ),
        Err(PanelHttpAuthError::InvalidCredential)
    );
    assert!(authenticate(
        &headers(Some(&format!("Bearer {second}"))),
        remote_client(),
        &tokens,
        &store,
        &validate
    )
    .is_ok());
}

#[test]
fn device_credentials_bind_current_user_and_revocation() {
    use crate::gateway::security::store::{UserRole, UserStatus};

    let store = store();
    store
        .create_user("panel-member", "Panel member", UserRole::Member)
        .unwrap();
    let tokens = DeviceTokenManager::new(store.clone());
    let ticket = tokens
        .create_bootstrap_ticket(None, Some("panel-member"))
        .unwrap();
    let issued = tokens
        .exchange_bootstrap_ticket(
            &ticket,
            Some("panel-device".into()),
            Some("Panel".into()),
            None,
        )
        .unwrap();
    let request = headers(Some(&format!("Bearer {}", issued.device_token)));
    let auth = authenticate(&request, local_client(), &tokens, &store, &|_| false).unwrap();
    assert_eq!(auth.user(), "panel-member");

    store
        .update_user("panel-member", None, None, Some(UserStatus::Deactivated))
        .unwrap();
    assert_eq!(
        authenticate(&request, local_client(), &tokens, &store, &|_| false),
        Err(PanelHttpAuthError::Walled)
    );
    store
        .update_user("panel-member", None, None, Some(UserStatus::Active))
        .unwrap();
    tokens.revoke_device(&issued.device_id).unwrap();
    assert_eq!(
        authenticate(&request, local_client(), &tokens, &store, &|_| false),
        Err(PanelHttpAuthError::InvalidCredential)
    );
}

#[test]
fn duplicate_and_empty_credentials_cannot_create_context() {
    let store = store();
    let tokens = DeviceTokenManager::new(store.clone());
    let mut request = headers(Some("Bearer shared-secret"));
    request.append(AUTHORIZATION, "Bearer shared-secret".parse().unwrap());
    assert_eq!(
        authenticate(&request, remote_client(), &tokens, &store, &|_| true),
        Err(PanelHttpAuthError::InvalidCredential)
    );
    assert_eq!(
        authenticate(
            &headers(Some("Bearer ")),
            local_client(),
            &tokens,
            &store,
            &|_| true
        ),
        Err(PanelHttpAuthError::InvalidCredential)
    );
}

#[test]
fn malformed_authorization_is_not_an_implicit_loopback_grant() {
    let store = store();
    let device_tokens = DeviceTokenManager::new(store.clone());

    assert_eq!(
        authenticate(
            &headers(Some("Basic shared-secret")),
            local_client(),
            &device_tokens,
            &store,
            &|_| true,
        ),
        Err(PanelHttpAuthError::InvalidCredential),
    );
}
