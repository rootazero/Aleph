//! Trusted observation audience and request-owned live access, not another ACL store.
use crate::capability::events::CapabilityEventRecord;
use crate::capability::policy_runtime::CapabilityCallContext;
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;

/// Captured only at a trusted producer boundary. Clients cannot deserialize authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventAudience {
    OperatorOnly,
    Principal(PrincipalEventAudience),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrincipalEventAudience {
    user_id: String,
    session_key: Option<String>,
    workspace: Option<String>,
    source_incarnation: Option<String>,
}
impl PrincipalEventAudience {
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn session_key(&self) -> Option<&str> {
        self.session_key.as_deref()
    }
    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }
    pub fn source_incarnation(&self) -> Option<&str> {
        self.source_incarnation.as_deref()
    }
}
impl EventAudience {
    /// Decode ONLY the trusted process-local producer envelope. This is not an
    /// ingress authority constructor; malformed or extended authority fails closed.
    pub(crate) fn from_produced_frame(data: &serde_json::Value) -> Option<Self> {
        #[derive(serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum ProducedAudience {
            OperatorOnly,
            Principal {
                user_id: String,
                session_key: Option<String>,
                workspace: Option<String>,
                source_incarnation: Option<String>,
            },
        }
        let envelope = data.get("audience")?;
        let fields = envelope.as_object()?;
        let expected: &[&str] = match fields.get("kind")?.as_str()? {
            "operator_only" => &["kind"],
            "principal" => &[
                "kind",
                "user_id",
                "session_key",
                "workspace",
                "source_incarnation",
            ],
            _ => return None,
        };
        if fields.len() != expected.len() || expected.iter().any(|name| !fields.contains_key(*name))
        {
            return None;
        }
        match serde_json::from_value(envelope.clone()).ok()? {
            ProducedAudience::OperatorOnly => Some(Self::OperatorOnly),
            ProducedAudience::Principal {
                user_id,
                session_key,
                workspace,
                source_incarnation,
            } => {
                if user_id.trim().is_empty()
                    || [&session_key, &workspace, &source_incarnation]
                        .into_iter()
                        .any(|value| value.as_ref().is_some_and(|value| value.trim().is_empty()))
                {
                    return None;
                }
                Some(Self::Principal(PrincipalEventAudience {
                    user_id,
                    session_key,
                    workspace,
                    source_incarnation,
                }))
            }
        }
    }

    /// No principal is an operator-only observation, never a public broadcast.
    pub(crate) fn from_call(
        call: &CapabilityCallContext,
        source_incarnation: Option<String>,
    ) -> Self {
        match call.principal() {
            Some(principal) => Self::Principal(PrincipalEventAudience {
                user_id: principal.to_owned(),
                session_key: call.session_key().map(str::to_owned),
                workspace: call.workspace().map(str::to_owned),
                source_incarnation,
            }),
            None => Self::OperatorOnly,
        }
    }
}

/// The owning authenticated ingress implements the existing login/role, audience,
/// selection and visibility checks against CURRENT facts on every record.
#[async_trait]
pub trait CapabilityEventAccess: Send + Sync {
    async fn admits(&self, record: &CapabilityEventRecord) -> bool;
}

/// Opaque authenticated request scope. Neither a filter nor a client role claim.
#[derive(Clone)]
pub struct TrustedEventScope {
    access: Arc<dyn CapabilityEventAccess>,
}
impl TrustedEventScope {
    pub(crate) fn from_access(access: Arc<dyn CapabilityEventAccess>) -> Self {
        Self { access }
    }
    pub(crate) async fn admits(&self, record: &CapabilityEventRecord) -> bool {
        self.access.admits(record).await
    }
}
