use crate::a2a::domain::security::{Credentials, SecurityScheme, TrustLevel};
use crate::a2a::domain::A2AError;
use crate::a2a::port::authenticator::{
    A2AAction, A2AAuthContext, A2AAuthPrincipal, A2AAuthenticator, TaskOwner,
};
use crate::a2a::port::A2AResult;

use super::token_store::TokenStore;

/// Authenticates explicit credentials and optionally allows direct loopback requests.
///
/// A valid bearer token or API key grants Trusted permissions. A direct
/// loopback request without credentials can qualify for Local permissions.
pub struct TieredAuthenticator {
    local_bypass: bool,
    token_store: TokenStore,
    local_permissions: Vec<String>,
    trusted_permissions: Vec<String>,
    public_permissions: Vec<String>,
}

impl TieredAuthenticator {
    #[must_use]
    pub fn new(local_bypass: bool, tokens: Vec<String>) -> Self {
        Self {
            local_bypass,
            token_store: TokenStore::new(tokens),
            local_permissions: vec!["*".to_string()],
            trusted_permissions: vec!["*".to_string()],
            public_permissions: vec!["read".to_string()],
        }
    }

    fn permissions_for(&self, level: &TrustLevel) -> Vec<String> {
        match level {
            TrustLevel::Local => self.local_permissions.clone(),
            TrustLevel::Trusted => self.trusted_permissions.clone(),
            TrustLevel::Public => self.public_permissions.clone(),
        }
    }
}

/// Headers set by reverse proxies when they relay a request.
const FORWARDING_HEADERS: [&str; 3] = ["x-forwarded-for", "x-forwarded-host", "forwarded"];

/// Returns `true` when the request carries a reverse-proxy forwarding header.
///
/// A relayed request reaches the listener from the proxy's own loopback
/// socket, so the peer address alone no longer proves the caller is local.
fn is_forwarded(context: &A2AAuthContext) -> bool {
    context.headers.keys().any(|name| {
        FORWARDING_HEADERS
            .iter()
            .any(|header| name.eq_ignore_ascii_case(header))
    })
}

#[async_trait::async_trait]
impl A2AAuthenticator for TieredAuthenticator {
    async fn authenticate(&self, context: &A2AAuthContext) -> A2AResult<A2AAuthPrincipal> {
        match &context.credentials {
            Credentials::BearerToken(token) | Credentials::ApiKey(token) => {
                let id = self
                    .token_store
                    .credential_id(token)
                    .ok_or(A2AError::Unauthorized)?;
                Ok(A2AAuthPrincipal {
                    owner: TaskOwner::Credential(id),
                    trust_level: TrustLevel::Trusted,
                    permissions: self.permissions_for(&TrustLevel::Trusted),
                })
            }
            Credentials::None
                if self.local_bypass
                    && context.remote_addr.ip().is_loopback()
                    && !is_forwarded(context)
                    && !context.headers.keys().any(|name| {
                        name.eq_ignore_ascii_case("authorization")
                            || name.eq_ignore_ascii_case("x-api-key")
                    }) =>
            {
                Ok(A2AAuthPrincipal {
                    owner: TaskOwner::Loopback,
                    trust_level: TrustLevel::Local,
                    permissions: self.permissions_for(&TrustLevel::Local),
                })
            }
            _ => Err(A2AError::Unauthorized),
        }
    }

    async fn authorize(&self, principal: &A2AAuthPrincipal, action: &A2AAction) -> A2AResult<bool> {
        // Wildcard grants all actions
        if principal.permissions.iter().any(|p| p == "*") {
            return Ok(true);
        }
        // Map actions to required permission
        let required = match action {
            A2AAction::GetTask | A2AAction::ListTasks | A2AAction::GetExtendedCard => "read",
            A2AAction::SendMessage | A2AAction::CancelTask | A2AAction::Subscribe => "write",
            A2AAction::ManagePushConfig => "admin",
        };
        Ok(principal.permissions.iter().any(|p| p == required))
    }

    fn supported_schemes(&self) -> Vec<SecurityScheme> {
        vec![SecurityScheme::Http {
            scheme: "bearer".to_string(),
            bearer_format: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn localhost_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
    }

    fn remote_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 8080)
    }

    fn make_context(addr: SocketAddr, creds: Credentials) -> A2AAuthContext {
        A2AAuthContext {
            remote_addr: addr,
            headers: HashMap::new(),
            credentials: creds,
        }
    }

    #[tokio::test]
    async fn localhost_with_bypass_grants_local_trust() {
        let auth = TieredAuthenticator::new(true, vec![]);
        let ctx = make_context(localhost_addr(), Credentials::None);
        let principal = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(principal.trust_level, TrustLevel::Local);
        assert!(principal.permissions.contains(&"*".to_string()));
    }

    #[tokio::test]
    async fn forwarded_loopback_request_is_not_local() {
        // A reverse proxy relays the call from its own loopback socket, so the
        // peer address must not be treated as proof of a local caller.
        let auth = TieredAuthenticator::new(true, vec![]);
        let mut ctx = make_context(localhost_addr(), Credentials::None);
        ctx.headers
            .insert("x-forwarded-for".to_string(), "203.0.113.7".to_string());
        let result = auth.authenticate(&ctx).await;
        assert!(matches!(result.unwrap_err(), A2AError::Unauthorized));
    }

    #[tokio::test]
    async fn forwarded_loopback_request_with_token_is_trusted() {
        let auth = TieredAuthenticator::new(true, vec!["my-token".to_string()]);
        let mut ctx = make_context(
            localhost_addr(),
            Credentials::BearerToken("my-token".to_string()),
        );
        ctx.headers
            .insert("forwarded".to_string(), "for=203.0.113.7".to_string());
        let principal = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(principal.trust_level, TrustLevel::Trusted);
    }

    #[tokio::test]
    async fn localhost_without_bypass_and_no_creds_rejects() {
        let auth = TieredAuthenticator::new(false, vec![]);
        let ctx = make_context(localhost_addr(), Credentials::None);
        let result = auth.authenticate(&ctx).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), A2AError::Unauthorized));
    }

    #[tokio::test]
    async fn valid_bearer_token_grants_trusted() {
        let auth = TieredAuthenticator::new(false, vec!["my-token".to_string()]);
        let ctx = make_context(
            remote_addr(),
            Credentials::BearerToken("my-token".to_string()),
        );
        let principal = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(principal.trust_level, TrustLevel::Trusted);
    }

    #[tokio::test]
    async fn valid_api_key_grants_trusted() {
        let auth = TieredAuthenticator::new(false, vec!["api-key-123".to_string()]);
        let ctx = make_context(
            remote_addr(),
            Credentials::ApiKey("api-key-123".to_string()),
        );
        let principal = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(principal.trust_level, TrustLevel::Trusted);
    }

    #[tokio::test]
    async fn invalid_bearer_token_rejects() {
        let auth = TieredAuthenticator::new(false, vec!["correct-token".to_string()]);
        let ctx = make_context(
            remote_addr(),
            Credentials::BearerToken("wrong-token".to_string()),
        );
        let result = auth.authenticate(&ctx).await;
        assert!(matches!(result.unwrap_err(), A2AError::Unauthorized));
    }

    #[tokio::test]
    async fn oauth2_token_rejected_until_jwks_implemented() {
        let auth = TieredAuthenticator::new(false, vec![]);
        let ctx = make_context(
            remote_addr(),
            Credentials::OAuth2Token("oauth-token".to_string()),
        );
        let result = auth.authenticate(&ctx).await;
        assert!(matches!(result.unwrap_err(), A2AError::Unauthorized));
    }

    #[tokio::test]
    async fn no_credentials_rejects() {
        let auth = TieredAuthenticator::new(false, vec![]);
        let ctx = make_context(remote_addr(), Credentials::None);
        let result = auth.authenticate(&ctx).await;
        assert!(matches!(result.unwrap_err(), A2AError::Unauthorized));
    }

    #[tokio::test]
    async fn authorize_wildcard_allows_all() {
        let principal = A2AAuthPrincipal {
            owner: TaskOwner::Loopback,
            trust_level: TrustLevel::Local,
            permissions: vec!["*".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth
            .authorize(&principal, &A2AAction::SendMessage)
            .await
            .unwrap());
        assert!(auth
            .authorize(&principal, &A2AAction::CancelTask)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn authorize_empty_permissions_denies() {
        let principal = A2AAuthPrincipal {
            owner: TaskOwner::Loopback,
            trust_level: TrustLevel::Public,
            permissions: vec![],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(!auth
            .authorize(&principal, &A2AAction::SendMessage)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn authorize_read_permission_allows_get_task() {
        let principal = A2AAuthPrincipal {
            owner: TaskOwner::Loopback,
            trust_level: TrustLevel::Public,
            permissions: vec!["read".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth
            .authorize(&principal, &A2AAction::GetTask)
            .await
            .unwrap());
        assert!(auth
            .authorize(&principal, &A2AAction::ListTasks)
            .await
            .unwrap());
        // read permission should NOT allow write actions
        assert!(!auth
            .authorize(&principal, &A2AAction::SendMessage)
            .await
            .unwrap());
        assert!(!auth
            .authorize(&principal, &A2AAction::CancelTask)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn authorize_write_permission_allows_send_and_cancel() {
        let principal = A2AAuthPrincipal {
            owner: TaskOwner::Loopback,
            trust_level: TrustLevel::Trusted,
            permissions: vec!["write".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth
            .authorize(&principal, &A2AAction::SendMessage)
            .await
            .unwrap());
        assert!(auth
            .authorize(&principal, &A2AAction::CancelTask)
            .await
            .unwrap());
        // write should NOT allow read
        assert!(!auth
            .authorize(&principal, &A2AAction::GetTask)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn authorize_admin_permission_allows_push_config() {
        let principal = A2AAuthPrincipal {
            owner: TaskOwner::Loopback,
            trust_level: TrustLevel::Local,
            permissions: vec!["admin".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth
            .authorize(&principal, &A2AAction::ManagePushConfig)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn default_permissions_per_tier() {
        let auth = TieredAuthenticator::new(true, vec!["tok".to_string()]);

        // Local
        let ctx = make_context(localhost_addr(), Credentials::None);
        let p = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(p.permissions, vec!["*".to_string()]);

        // Trusted
        let ctx = make_context(remote_addr(), Credentials::BearerToken("tok".to_string()));
        let p = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(p.permissions, vec!["*".to_string()]);

        // Public (OAuth2 rejected until JWKS implemented)
        let ctx = make_context(remote_addr(), Credentials::OAuth2Token("x".to_string()));
        assert!(auth.authenticate(&ctx).await.is_err());
    }

    #[tokio::test]
    async fn supported_schemes_includes_bearer() {
        let auth = TieredAuthenticator::new(true, vec![]);
        let schemes = auth.supported_schemes();
        assert_eq!(schemes.len(), 1);
        assert!(matches!(
            &schemes[0],
            SecurityScheme::Http { scheme, .. } if scheme == "bearer"
        ));
    }
}
