use std::collections::HashMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::a2a::domain::{Credentials, SecurityScheme, TrustLevel};

use super::task_manager::A2AResult;

/// Context extracted from an incoming A2A request.
/// Uses plain `HashMap` for headers to avoid coupling port traits to axum/http.
pub struct A2AAuthContext {
    pub remote_addr: SocketAddr,
    pub headers: HashMap<String, String>,
    pub credentials: Credentials,
}

impl Clone for A2AAuthContext {
    fn clone(&self) -> Self {
        Self {
            remote_addr: self.remote_addr,
            headers: self.headers.clone(),
            credentials: self.credentials.clone(),
        }
    }
}

impl std::fmt::Debug for A2AAuthContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A2AAuthContext")
            .field("remote_addr", &self.remote_addr)
            .field(
                "headers",
                &self
                    .headers
                    .keys()
                    .map(|k| {
                        if k.eq_ignore_ascii_case("authorization") {
                            format!("{k}: [REDACTED]")
                        } else {
                            format!("{k}: [PRESENT]")
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .field("credentials", &self.credentials)
            .finish()
    }
}

/// Authenticated principal with trust level and permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AAuthPrincipal {
    pub agent_id: Option<String>,
    pub trust_level: TrustLevel,
    pub permissions: Vec<String>,
}

/// Actions that can be authorized
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2AAction {
    SendMessage,
    GetTask,
    CancelTask,
    ListTasks,
    Subscribe,
    ManagePushConfig,
    /// Fetch the authenticated extended agent card
    /// (`agent/getAuthenticatedExtendedCard`).
    GetExtendedCard,
}

/// Port for authentication and authorization of A2A requests.
///
/// Implements a tiered security model based on trust levels:
/// - Local: no auth required
/// - Trusted: token-based auth
/// - Public: OAuth2/mTLS
#[async_trait::async_trait]
pub trait A2AAuthenticator: Send + Sync {
    /// Authenticate an incoming request and return the principal
    async fn authenticate(&self, context: &A2AAuthContext) -> A2AResult<A2AAuthPrincipal>;

    /// Check if a principal is authorized to perform an action
    async fn authorize(&self, principal: &A2AAuthPrincipal, action: &A2AAction) -> A2AResult<bool>;

    /// Return the security schemes this authenticator supports
    fn supported_schemes(&self) -> Vec<SecurityScheme>;
}
