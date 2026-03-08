use std::sync::Arc;

use crate::a2a::domain::security::{Credentials, SecurityScheme, TrustLevel};
use crate::a2a::domain::A2AError;
use crate::a2a::port::authenticator::{A2AAction, A2AAuthContext, A2AAuthPrincipal, A2AAuthenticator};
use crate::a2a::port::A2AResult;

use super::token_store::TokenStore;

/// Tiered trust authenticator implementing localhost -> token -> OAuth2 -> reject.
///
/// Security tiers:
/// - **Local**: Loopback connections bypass auth entirely (configurable)
/// - **Trusted**: Valid bearer token or API key grants full access
/// - **Public**: OAuth2 tokens get restricted access
/// - **Reject**: No valid credentials results in Unauthorized error
pub struct TieredAuthenticator {
    local_bypass: bool,
    token_store: Arc<tokio::sync::RwLock<TokenStore>>,
    local_permissions: Vec<String>,
    trusted_permissions: Vec<String>,
    public_permissions: Vec<String>,
}

impl TieredAuthenticator {
    pub fn new(local_bypass: bool, tokens: Vec<String>) -> Self {
        Self {
            local_bypass,
            token_store: Arc::new(tokio::sync::RwLock::new(TokenStore::new(tokens))),
            local_permissions: vec!["*".to_string()],
            trusted_permissions: vec!["*".to_string()],
            public_permissions: vec!["read".to_string()],
        }
    }

    /// Configure per-level permissions
    pub fn with_permissions(
        mut self,
        local: Vec<String>,
        trusted: Vec<String>,
        public: Vec<String>,
    ) -> Self {
        self.local_permissions = local;
        self.trusted_permissions = trusted;
        self.public_permissions = public;
        self
    }

    fn permissions_for(&self, level: &TrustLevel) -> Vec<String> {
        match level {
            TrustLevel::Local => self.local_permissions.clone(),
            TrustLevel::Trusted => self.trusted_permissions.clone(),
            TrustLevel::Public => self.public_permissions.clone(),
        }
    }
}

#[async_trait::async_trait]
impl A2AAuthenticator for TieredAuthenticator {
    async fn authenticate(&self, context: &A2AAuthContext) -> A2AResult<A2AAuthPrincipal> {
        // Tier 1: localhost bypass
        if self.local_bypass && context.remote_addr.ip().is_loopback() {
            return Ok(A2AAuthPrincipal {
                agent_id: None,
                trust_level: TrustLevel::Local,
                permissions: self.permissions_for(&TrustLevel::Local),
            });
        }

        // Tier 2-3: credential-based authentication
        match &context.credentials {
            Credentials::BearerToken(token) | Credentials::ApiKey(token) => {
                let store = self.token_store.read().await;
                if store.is_valid(token) {
                    return Ok(A2AAuthPrincipal {
                        agent_id: None,
                        trust_level: TrustLevel::Trusted,
                        permissions: self.permissions_for(&TrustLevel::Trusted),
                    });
                }
                // Invalid token — fall through to reject
            }
            Credentials::OAuth2Token(_token) => {
                // OAuth2 validation would go here (e.g. JWKS verification)
                // For now, accept any OAuth2 token at Public level
                return Ok(A2AAuthPrincipal {
                    agent_id: None,
                    trust_level: TrustLevel::Public,
                    permissions: self.permissions_for(&TrustLevel::Public),
                });
            }
            Credentials::None => {}
        }

        // Tier 4: reject
        Err(A2AError::Unauthorized)
    }

    async fn authorize(
        &self,
        principal: &A2AAuthPrincipal,
        _action: &A2AAction,
    ) -> A2AResult<bool> {
        // Wildcard grants all actions
        if principal.permissions.iter().any(|p| p == "*") {
            return Ok(true);
        }
        // Any non-empty permission list allows access for now;
        // fine-grained per-action authorization can be added later
        Ok(!principal.permissions.is_empty())
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
    async fn oauth2_token_grants_public() {
        let auth = TieredAuthenticator::new(false, vec![]);
        let ctx = make_context(
            remote_addr(),
            Credentials::OAuth2Token("oauth-token".to_string()),
        );
        let principal = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(principal.trust_level, TrustLevel::Public);
        assert!(principal.permissions.contains(&"read".to_string()));
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
            agent_id: None,
            trust_level: TrustLevel::Local,
            permissions: vec!["*".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth.authorize(&principal, &A2AAction::SendMessage).await.unwrap());
        assert!(auth.authorize(&principal, &A2AAction::CancelTask).await.unwrap());
    }

    #[tokio::test]
    async fn authorize_empty_permissions_denies() {
        let principal = A2AAuthPrincipal {
            agent_id: None,
            trust_level: TrustLevel::Public,
            permissions: vec![],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(!auth.authorize(&principal, &A2AAction::SendMessage).await.unwrap());
    }

    #[tokio::test]
    async fn authorize_non_wildcard_non_empty_allows() {
        let principal = A2AAuthPrincipal {
            agent_id: None,
            trust_level: TrustLevel::Public,
            permissions: vec!["read".to_string()],
        };
        let auth = TieredAuthenticator::new(true, vec![]);
        assert!(auth.authorize(&principal, &A2AAction::GetTask).await.unwrap());
    }

    #[tokio::test]
    async fn custom_permissions_applied() {
        let auth = TieredAuthenticator::new(true, vec!["tok".to_string()]).with_permissions(
            vec!["admin".to_string()],
            vec!["read".to_string(), "write".to_string()],
            vec!["read".to_string()],
        );

        // Local
        let ctx = make_context(localhost_addr(), Credentials::None);
        let p = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(p.permissions, vec!["admin".to_string()]);

        // Trusted
        let ctx = make_context(remote_addr(), Credentials::BearerToken("tok".to_string()));
        let p = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(p.permissions, vec!["read".to_string(), "write".to_string()]);

        // Public
        let ctx = make_context(remote_addr(), Credentials::OAuth2Token("x".to_string()));
        let p = auth.authenticate(&ctx).await.unwrap();
        assert_eq!(p.permissions, vec!["read".to_string()]);
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
