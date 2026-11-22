//! The owned process-local capability ledger forwarder and current ingress ACL.
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::capability::event_audience::{CapabilityEventAccess, TrustedEventScope};
use crate::capability::events::{CapabilityEventRecord, EventError, EventSubscription};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::gateway::event_scope::EventScopeGuard;
use crate::gateway::event_visibility::EventVisibilityIndex;
use crate::gateway::server::ConnectionState;
use crate::gateway::session_store::SessionStore;

pub struct GatewayCapabilityEventAccess {
    connection_id: String,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    store: Option<Arc<dyn SessionStore>>,
    visibility: Arc<EventVisibilityIndex>,
}
impl GatewayCapabilityEventAccess {
    pub fn for_connection(
        connection_id: String,
        connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
        store: Option<Arc<dyn SessionStore>>,
        visibility: Arc<EventVisibilityIndex>,
    ) -> TrustedEventScope {
        TrustedEventScope::from_access(Arc::new(Self {
            connection_id,
            connections,
            store,
            visibility,
        }))
    }
}
#[async_trait]
impl CapabilityEventAccess for GatewayCapabilityEventAccess {
    async fn admits(&self, record: &CapabilityEventRecord) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        let identity = {
            let connections = self.connections.read().await;
            let Some(state) = connections.get(&self.connection_id) else {
                return false;
            };
            if state.first_message
                || !crate::gateway::server::connection::wall_admits(Some(&state.caller_role), "")
                || !EventScopeGuard::default_rules()
                    .can_receive("capability.event", &state.permissions)
            {
                return false;
            }
            (
                state.caller_user.clone(),
                state.caller_role.clone(),
                state.permissions.clone(),
            )
        };
        if !self
            .visibility
            .capability_audience_admits(
                record.audience(),
                identity.0.as_deref(),
                Some(&identity.1),
                store,
            )
            .await
        {
            return false;
        }
        // A restamp during metadata I/O must not use the old identity's verdict.
        self.connections
            .read()
            .await
            .get(&self.connection_id)
            .is_some_and(|state| {
                !state.first_message
                    && state.caller_user == identity.0
                    && state.caller_role == identity.1
                    && state.permissions == identity.2
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityBridgeError {
    #[error("capability event worker join failed")]
    Join,
    #[error("capability event publication failed")]
    Publish,
}
struct BridgeWorker {
    handle: Option<JoinHandle<Result<(), CapabilityBridgeError>>>,
    result: Option<Result<(), CapabilityBridgeError>>,
}

/// Clones retain the SAME worker. Cancelling a join never takes its handle away
/// from the startup owner; another cleanup can poll that handle in place.
#[derive(Clone)]
pub struct CapabilityEventBridge {
    worker: Arc<Mutex<BridgeWorker>>,
}
impl CapabilityEventBridge {
    pub fn start(mut receiver: EventSubscription, bus: Arc<GatewayEventBus>) -> Self {
        let handle = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(record) => {
                        let data = serde_json::to_value(record.as_ref())
                            .map_err(|_| CapabilityBridgeError::Publish)?;
                        bus.publish_json(&TopicEvent::new("capability.event", data))
                            .map_err(|_| CapabilityBridgeError::Publish)?;
                    }
                    Err(EventError::Lagged(_)) => {
                        bus.publish_json(&TopicEvent::new(
                            "capability.event.status",
                            json!({"state":"lagged","incomplete":true}),
                        ))
                        .map_err(|_| CapabilityBridgeError::Publish)?;
                    }
                    Err(EventError::Closed) => return Ok(()),
                    Err(_) => return Err(CapabilityBridgeError::Publish),
                }
            }
        });
        Self {
            worker: Arc::new(Mutex::new(BridgeWorker {
                handle: Some(handle),
                result: None,
            })),
        }
    }

    pub async fn join(self) -> Result<(), CapabilityBridgeError> {
        let mut worker = self.worker.lock().await;
        if let Some(result) = &worker.result {
            return result.clone();
        }
        let result = match worker.handle.as_mut() {
            Some(handle) => handle
                .await
                .map_err(|_| CapabilityBridgeError::Join)
                .and_then(|result| result),
            None => Err(CapabilityBridgeError::Join),
        };
        worker.result = Some(result.clone());
        worker.handle = None;
        result
    }
}
