//! macOS `AccessibilityCapability` implementation.
//!
//! Thin RPC forwarder over the long-lived `SwiftBridge`.  All heavy AX work
//! (tree walking, permission checks, value coercion) happens in the Swift
//! helper — the Rust side only marshals JSON in and out.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop::{DesktopError, Result, SwiftBridge};
use aleph_protocol::desktop_bridge::methods::ax::{
    clamp_max_nodes, AxActionResult, AxElement, PerformActionParams, QueryByRoleParams,
    QueryFocusedParams, QueryListResult, QueryResult, QueryTreeParams, SetValueParams,
    METHOD_PERFORM_ACTION, METHOD_QUERY_BY_ROLE, METHOD_QUERY_FOCUSED, METHOD_QUERY_TREE,
    METHOD_SET_VALUE,
};

/// `AccessibilityCapability` implementation backed by the Swift helper.
pub struct BridgeAccessibility {
    bridge: Arc<SwiftBridge>,
}

impl BridgeAccessibility {
    /// Build a new `BridgeAccessibility` that issues RPC calls via `bridge`.
    pub const fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

fn bridge_err(method: &str, err: impl std::fmt::Display) -> DesktopError {
    DesktopError::BridgeFailed(format!("{method} RPC: {err}"))
}

#[async_trait]
impl AccessibilityCapability for BridgeAccessibility {
    /// The helper resolves `pid` to that application's own AX element and reads
    /// its `AXFocusedUIElement`, so the answer is the target app's focus even
    /// when the app is in the background — the case the targeted input rail is
    /// built around. The pid filter below is belt-and-braces for an older helper
    /// that ignores the field and answers system-wide: the trait's contract is
    /// "this element belongs to that process, or nothing does".
    async fn query_focused(&self, params: QueryFocusedParams) -> Result<Option<AxElement>> {
        debug!(pid = ?params.pid, "Proxying ax.query_focused to Swift helper");
        let want_pid = params.pid;
        let r: QueryResult = self
            .bridge
            .call(METHOD_QUERY_FOCUSED, params)
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_FOCUSED, e))?;
        Ok(r.element.filter(|el| want_pid.is_none_or(|p| el.pid == p)))
    }

    async fn query_tree(&self, mut params: QueryTreeParams) -> Result<QueryResult> {
        params.max_nodes = clamp_max_nodes(params.max_nodes);
        debug!(
            pid = ?params.pid,
            max_depth = params.max_depth,
            max_nodes = params.max_nodes,
            "Proxying ax.query_tree to Swift helper"
        );
        self.bridge
            .call(METHOD_QUERY_TREE, params)
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_TREE, e))
    }

    async fn query_by_role(&self, mut params: QueryByRoleParams) -> Result<QueryListResult> {
        params.max_nodes = clamp_max_nodes(params.max_nodes);
        debug!(
            role = %params.role,
            pid = ?params.pid,
            max_nodes = params.max_nodes,
            "Proxying ax.query_by_role to Swift helper"
        );
        self.bridge
            .call(METHOD_QUERY_BY_ROLE, params)
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_BY_ROLE, e))
    }

    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        debug!("Proxying ax.set_value to Swift helper");
        self.bridge
            .call(METHOD_SET_VALUE, params)
            .await
            .map_err(|e| bridge_err(METHOD_SET_VALUE, e))
    }

    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        debug!("Proxying ax.perform_action to Swift helper");
        self.bridge
            .call(METHOD_PERFORM_ACTION, params)
            .await
            .map_err(|e| bridge_err(METHOD_PERFORM_ACTION, e))
    }
}
