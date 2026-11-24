//! QA-only observations emitted after the production Gateway has made an auth decision.
//!
//! This module is compiled only for the isolated native acceptance helper. It
//! carries typed, bounded facts across the Core/helper boundary; it never
//! exposes a request, response body, credential, or authorization primitive.

use crate::sync_primitives::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeQaRpcOperation {
    Connect,
    Select,
    Discovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeQaRpcResult {
    Authorized,
    Refused,
    SnapshotOk,
    SnapshotError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeQaRpcReceipt {
    pub operation: NativeQaRpcOperation,
    pub user: Option<String>,
    pub device: Option<String>,
    /// The server-owned project identifier selected by the request or returned
    /// by the contextual snapshot. The helper maps it to a fixed test label.
    pub project_id: Option<String>,
    pub result: NativeQaRpcResult,
}

pub trait NativeQaRpcObserver: Send + Sync {
    fn record(&self, receipt: NativeQaRpcReceipt);
}

pub type NativeQaRpcObserverHandle = Arc<dyn NativeQaRpcObserver>;

#[derive(Clone, Default)]
pub struct NativeQaRpcObserverSlot {
    observer: Option<NativeQaRpcObserverHandle>,
}

impl NativeQaRpcObserverSlot {
    #[must_use]
    pub const fn empty() -> Self {
        Self { observer: None }
    }

    #[must_use]
    pub fn new(observer: NativeQaRpcObserverHandle) -> Self {
        Self {
            observer: Some(observer),
        }
    }

    pub fn record(&self, receipt: NativeQaRpcReceipt) {
        if let Some(observer) = &self.observer {
            observer.record(receipt);
        }
    }
}

#[must_use]
pub fn observer_slot(observer: Option<NativeQaRpcObserverHandle>) -> NativeQaRpcObserverSlot {
    observer.map_or_else(NativeQaRpcObserverSlot::empty, NativeQaRpcObserverSlot::new)
}

#[must_use]
pub fn project_id_for(
    operation: NativeQaRpcOperation,
    request: &crate::gateway::protocol::JsonRpcRequest,
    response: &crate::gateway::protocol::JsonRpcResponse,
) -> Option<String> {
    match operation {
        NativeQaRpcOperation::Select => request
            .params
            .as_ref()
            .and_then(|params| params.get("project_id"))
            .and_then(|project_id| project_id.as_str())
            .map(str::to_owned),
        NativeQaRpcOperation::Discovery => response
            .result
            .as_ref()
            .and_then(|result| result.get("project_context"))
            .and_then(|context| context.get("project_id"))
            .and_then(|project_id| project_id.as_str())
            .map(str::to_owned),
        NativeQaRpcOperation::Connect => None,
    }
}

#[must_use]
pub fn operation_for(method: &str) -> Option<NativeQaRpcOperation> {
    match method {
        "panel.project.select" => Some(NativeQaRpcOperation::Select),
        "panel.capabilities.snapshot" => Some(NativeQaRpcOperation::Discovery),
        _ => None,
    }
}

#[must_use]
pub fn result_for(
    operation: NativeQaRpcOperation,
    response: &crate::gateway::protocol::JsonRpcResponse,
) -> NativeQaRpcResult {
    if response.is_success() {
        return match operation {
            NativeQaRpcOperation::Discovery => NativeQaRpcResult::SnapshotOk,
            NativeQaRpcOperation::Connect | NativeQaRpcOperation::Select => {
                NativeQaRpcResult::Authorized
            }
        };
    }
    match operation {
        NativeQaRpcOperation::Discovery => NativeQaRpcResult::SnapshotError,
        NativeQaRpcOperation::Connect | NativeQaRpcOperation::Select => NativeQaRpcResult::Refused,
    }
}

pub fn observe_rpc(
    slot: &NativeQaRpcObserverSlot,
    caller: &crate::gateway::server::connection::dispatch::CallerSnapshot,
    request: &crate::gateway::protocol::JsonRpcRequest,
    response: &crate::gateway::protocol::JsonRpcResponse,
) {
    let (Some(user), Some(device)) = (caller.verified_user.clone(), caller.device_id.clone())
    else {
        return;
    };
    let Some(operation) = operation_for(&request.method) else {
        return;
    };
    slot.record(NativeQaRpcReceipt {
        operation,
        user: Some(user),
        device: Some(device),
        project_id: project_id_for(operation, request, response),
        result: result_for(operation, response),
    });
}

pub fn observe_connect(
    slot: &NativeQaRpcObserverSlot,
    user: Option<String>,
    device: Option<String>,
    authorized: bool,
) {
    if !authorized {
        return;
    }
    let (Some(user), Some(device)) = (user, device) else {
        return;
    };
    slot.record(NativeQaRpcReceipt {
        operation: NativeQaRpcOperation::Connect,
        user: Some(user),
        device: Some(device),
        project_id: None,
        result: NativeQaRpcResult::Authorized,
    });
}

#[cfg(test)]
mod tests {
    use super::{operation_for, NativeQaRpcOperation};

    #[test]
    fn only_panel_context_methods_are_observable() {
        assert_eq!(
            operation_for("panel.project.select"),
            Some(NativeQaRpcOperation::Select)
        );
        assert_eq!(
            operation_for("panel.capabilities.snapshot"),
            Some(NativeQaRpcOperation::Discovery)
        );
        assert_eq!(operation_for("gateway.identity.get"), None);
    }
}
