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
    AxElement, QueryByRoleParams, QueryFocusedParams, QueryListResult, QueryResult,
    QueryTreeParams, METHOD_QUERY_BY_ROLE, METHOD_QUERY_FOCUSED, METHOD_QUERY_TREE,
};

/// `AccessibilityCapability` implementation backed by the Swift helper.
pub struct BridgeAccessibility {
    bridge: Arc<SwiftBridge>,
}

impl BridgeAccessibility {
    /// Build a new `BridgeAccessibility` that issues RPC calls via `bridge`.
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

fn bridge_err(method: &str, err: impl std::fmt::Display) -> DesktopError {
    DesktopError::BridgeFailed(format!("{method} RPC: {err}"))
}

#[async_trait]
impl AccessibilityCapability for BridgeAccessibility {
    async fn query_focused(&self) -> Result<Option<AxElement>> {
        debug!("Proxying ax.query_focused to Swift helper");
        let r: QueryResult = self
            .bridge
            .call(METHOD_QUERY_FOCUSED, QueryFocusedParams {})
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_FOCUSED, e))?;
        Ok(r.element)
    }

    async fn query_tree(&self, params: QueryTreeParams) -> Result<Option<AxElement>> {
        debug!(
            pid = ?params.pid,
            max_depth = params.max_depth,
            "Proxying ax.query_tree to Swift helper"
        );
        let r: QueryResult = self
            .bridge
            .call(METHOD_QUERY_TREE, params)
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_TREE, e))?;
        Ok(r.element)
    }

    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<Vec<AxElement>> {
        debug!(
            role = %params.role,
            pid = ?params.pid,
            "Proxying ax.query_by_role to Swift helper"
        );
        let r: QueryListResult = self
            .bridge
            .call(METHOD_QUERY_BY_ROLE, params)
            .await
            .map_err(|e| bridge_err(METHOD_QUERY_BY_ROLE, e))?;
        Ok(r.elements)
    }
}
