//! Projection and invocation of registered remote agents through Zahir.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agents::sub_agents::SubAgentRequest;
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityHealth, CapabilityId,
    CapabilityKind, CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::facade::Reference;
use crate::capability::ownership::{
    LifetimeScope, OwnerRef, OwnershipTree, RegistrationToken, VisibilityScope,
};
use crate::capability::policy_runtime::{CapabilityCallContext, ResourceUse};
use crate::capability::projection_host::ProjectionHost;
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
};
use crate::capability::special::{
    canonical_schema_bytes, CapabilityDispatchError, CapabilityHandler, CapabilityHandlerCall,
    CapabilityHandlerRegistry, SpecialRegistration, SpecialRegistrationHandle,
};
use crate::capability::special_dispatch::CapabilityInvocationOrigin;
use crate::capability::zahir_facade::ZahirFacade;

use super::port::{AgentHealth, RegisteredAgent};
use super::service::CardRegistry;
use super::sub_agent::{A2ASubAgent, DelegationOutcome};

fn capability_id(peer_id: &str) -> CapabilityId {
    CapabilityId {
        namespace: "a2a".into(),
        name: peer_id.into(),
    }
}

struct RemoteAgentHandler {
    registry: Arc<CardRegistry>,
    sub_agent: Weak<A2ASubAgent>,
    peer_id: String,
    generation: Uuid,
}

#[async_trait::async_trait]
impl CapabilityHandler for RemoteAgentHandler {
    fn source_available(&self) -> bool {
        self.registry.is_current(&self.peer_id, self.generation)
    }

    fn projected_health(&self) -> Option<CapabilityHealth> {
        self.registry
            .projected_health(&self.peer_id, self.generation)
            .map(|health| match health {
                AgentHealth::Healthy => CapabilityHealth::Healthy,
                AgentHealth::Degraded => CapabilityHealth::Degraded,
                AgentHealth::Unreachable => CapabilityHealth::Unreachable,
            })
    }

    async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        let prompt = input["prompt"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| CapabilityDispatchError::InvalidInput("prompt is empty".into()))?;
        let sub_agent = self
            .sub_agent
            .upgrade()
            .ok_or(CapabilityDispatchError::Unavailable)?;
        let mut request = SubAgentRequest::new(prompt);
        request.id = call.context().call_id().to_owned();
        request.parent_agent_id = crate::tools::turn_context::current_agent_id();
        request.parent_session_id = call.context().session_key().map(str::to_owned);
        let target = self
            .registry
            .dispatch_target(&self.peer_id, self.generation)
            .map_err(|_| CapabilityDispatchError::Unavailable)?;
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(CapabilityDispatchError::Cancelled),
            result = sub_agent.execute_registered_delegation(&target, &request) => result,
        };
        serde_json::to_value(outcome)
            .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))
    }
}

pub(crate) async fn invoke_remote_agent(
    facade: &ZahirFacade,
    registry: &CardRegistry,
    target: &RegisteredAgent,
    request: &SubAgentRequest,
) -> crate::error::Result<DelegationOutcome> {
    let context = CapabilityCallContext::for_current_turn(request.id.clone());
    let reference = Reference {
        id: capability_id(&target.card.id),
        visibility: VisibilityScope::default(),
    };
    let lease = facade
        .resolve_special_for_call(&reference, &context)
        .map_err(|error| {
            crate::error::AlephError::tool(format!("A2A agent unavailable: {error:?}"))
        })?;
    if !registry.is_current(&target.card.id, target.invocation_generation()) {
        return Err(crate::error::AlephError::tool(
            "A2A agent registration changed",
        ));
    }
    let result = facade
        .invoke_special(
            &lease,
            json!({"prompt": request.prompt}),
            CapabilityHandlerCall::new(context, Vec::new()),
            CapabilityInvocationOrigin::CanonicalIngress,
            ResourceUse::Allocate,
            CancellationToken::new(),
        )
        .await
        .map_err(|error| crate::error::AlephError::tool(error.to_string()))?;
    serde_json::from_value(result)
        .map_err(|error| crate::error::AlephError::tool(format!("Invalid A2A result: {error}")))
}

struct PublishedAgent {
    registration: SpecialRegistrationHandle,
    ownership: RegistrationToken,
    generation: Uuid,
    health: AgentHealth,
}

struct AgentProjection {
    registrations: HashMap<String, PublishedAgent>,
    specials: CapabilityHandlerRegistry,
    ownership: Arc<OwnershipTree>,
    registry: Arc<CardRegistry>,
    sub_agent: Arc<A2ASubAgent>,
}

impl AgentProjection {
    fn retire(&mut self, peer_id: &str) {
        if let Some(publication) = self.registrations.remove(peer_id) {
            let _ = publication.registration.dispose();
            self.ownership.rollback_if_matches(&publication.ownership);
        }
    }

    fn publish(&mut self, peer_id: &str, generation: Uuid, health: AgentHealth) {
        if let Some(existing) = self.registrations.get_mut(peer_id) {
            if existing.generation == generation {
                if existing.health != health {
                    existing.registration.publish_projection_change();
                    existing.health = health;
                }
                return;
            }
        }
        self.retire(peer_id);
        if let Err(error) = self.register(peer_id, generation, health) {
            tracing::warn!(%peer_id, %error, "could not project remote A2A Agent capability");
        }
    }

    async fn refresh(&mut self) {
        let (agents, _) = self.registry.snapshot_with_revision().await;
        let obsolete: Vec<_> = self
            .registrations
            .keys()
            .filter(|id| !agents.iter().any(|agent| &agent.card.id == *id))
            .cloned()
            .collect();
        for id in obsolete {
            self.retire(&id);
        }
        for agent in agents {
            self.publish(&agent.card.id, agent.invocation_generation(), agent.health);
        }
    }

    fn register(
        &mut self,
        peer_id: &str,
        generation: Uuid,
        health: AgentHealth,
    ) -> Result<(), String> {
        let id = capability_id(peer_id);
        let schema = json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {"prompt": {"type": "string", "minLength": 1}},
            "additionalProperties": false,
        });
        let descriptor = CapabilityDescriptor {
            id: id.clone(),
            kind: CapabilityKind::Agent,
            schema: SchemaRef {
                version: 1,
                fingerprint: Sha256::digest(canonical_schema_bytes(&schema)).into(),
            },
            revision: CapabilityRevision(0),
            lifetime: LifetimeScope::Runtime,
            visibility: VisibilityScope::default(),
            owner: OwnerRef::Runtime,
            services: Vec::new(),
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new()
                .with(CapabilityFacet::Callable)
                .with(CapabilityFacet::ExternalEffect),
        };
        let provider_id =
            ProviderId::new(format!("a2a/{peer_id}")).map_err(|error| error.to_string())?;
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            provider_id,
            ProviderTransport::InProcess,
        ));
        let handler = Arc::new(RemoteAgentHandler {
            registry: self.registry.clone(),
            sub_agent: Arc::downgrade(&self.sub_agent),
            peer_id: peer_id.to_owned(),
            generation,
        });
        let ownership = self
            .ownership
            .register_if_absent_with_token(
                id,
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                VisibilityScope::default(),
            )
            .map_err(|error| format!("{error:?}"))?
            .ok_or_else(|| "the peer already has an ownership binding".to_owned())?;
        let registration = match self.specials.register_with_handle(SpecialRegistration::new(
            descriptor, schema, handler, provider,
        )) {
            Ok(registration) => registration,
            Err(error) => {
                self.ownership.rollback_if_matches(&ownership);
                return Err(error.to_string());
            }
        };
        self.registrations.insert(
            peer_id.to_owned(),
            PublishedAgent {
                registration,
                ownership,
                generation,
                health,
            },
        );
        Ok(())
    }
}

/// Publish the initial remote Agent capabilities, then follow the card source.
/// The caller owns the returned worker and joins it at shutdown.
pub async fn mount_remote_agents(
    host: &ProjectionHost,
    ownership: Arc<OwnershipTree>,
    registry: Arc<CardRegistry>,
    sub_agent: Arc<A2ASubAgent>,
) -> tokio::task::JoinHandle<()> {
    let mut changes = registry.subscribe();
    let mut projection = AgentProjection {
        registrations: HashMap::new(),
        specials: host.specials(),
        ownership,
        registry,
        sub_agent,
    };
    projection.refresh().await;
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            projection.refresh().await;
        }
    })
}

impl Drop for AgentProjection {
    fn drop(&mut self) {
        for (_, publication) in self.registrations.drain() {
            let _ = publication.registration.dispose();
            self.ownership.rollback_if_matches(&publication.ownership);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::adapter::client::A2AClientPool;
    use crate::a2a::domain::{A2ATask, AgentCard, TaskState, TrustLevel};
    use crate::a2a::port::AgentResolver;
    use crate::capability::facade::{Scope, Zahir};
    use crate::security::ssrf::SsrfPolicy;
    use crate::tools::registry::ToolHandlerRegistry;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn mounted_agent_delegates_and_withdraws_the_same_registration() {
        let peer = MockServer::start().await;
        let mut task = A2ATask::new("remote-task", "remote-context");
        task.status.state = TaskState::Completed;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": task,
            })))
            .expect(1)
            .mount(&peer)
            .await;
        let policy = SsrfPolicy {
            enabled: false,
            ..SsrfPolicy::default()
        };
        let registry = Arc::new(CardRegistry::with_ssrf_policy(policy.clone()));
        let card: AgentCard = serde_json::from_value(json!({
            "id": "peer", "name": "Peer", "version": "1", "interfaces": [],
        }))
        .unwrap();
        registry
            .register_with_token(card, &peer.uri(), TrustLevel::Trusted, None)
            .await
            .unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(ToolHandlerRegistry::new(), tree.clone());
        assert!(host.wait_until_ready().await);
        let facade = host.facade();
        let sub_agent = Arc::new(
            A2ASubAgent::new(
                registry.clone(),
                Arc::new(A2AClientPool::with_ssrf_policy(policy)),
            )
            .with_capability_facade(&facade),
        );
        let worker = mount_remote_agents(&host, tree, registry.clone(), sub_agent.clone()).await;
        let reference = Reference {
            id: capability_id("peer"),
            visibility: VisibilityScope::default(),
        };
        let lease = facade.resolve(reference.clone()).unwrap();
        assert_eq!(lease.descriptor.kind, CapabilityKind::Agent);
        let outcome = sub_agent
            .execute_delegation("test", Some("Peer"), None, None)
            .await
            .unwrap();
        assert!(outcome.result.success, "{outcome:?}");

        let selected = registry.resolve_by_id("peer").await.unwrap().unwrap();
        registry
            .record_health_if_current(&selected, AgentHealth::Degraded)
            .await;
        let snapshot = facade.describe(Scope::default());
        let found = snapshot
            .capabilities
            .iter()
            .find(|descriptor| descriptor.id == reference.id)
            .unwrap();
        assert_eq!(found.metadata.health, Some(CapabilityHealth::Degraded));
        facade.validate_lease(&lease).unwrap();

        registry.unregister("peer").await.unwrap();
        assert!(facade.resolve(reference).is_err());
        assert!(facade.validate_lease(&lease).is_err());
        let result = invoke_remote_agent(
            &facade,
            &registry,
            &selected,
            &SubAgentRequest::new("stale"),
        )
        .await;
        assert!(result.is_err());
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        host.close_and_await().await;
    }
}
