//! Outbound delivery to a named, addressable surface.
//!
//! A `DeliverySurface` is NOT a `MessagingChannel`: it only delivers outbound
//! interactions and names its identity. It never parses inbound text — that
//! line is the whole point of Approach A (see the Phase 0/1 spec). Phase 1
//! carries only R5 notifications; approval callback delivery joins in Phase 2.

use crate::sync_primitives::Arc;

use serde_json::Value;

use super::SurfaceKind;

/// A core-decided notification ready to render. The "is this worth
/// interrupting the user" policy has already been applied upstream (the R5
/// router); a surface only marshals it onto its transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceNotification {
    pub title: String,
    pub body: String,
    /// Originating topic, for diagnostics only.
    pub source_topic: String,
}

/// A core-decided approval banner ready to render. Like `SurfaceNotification`
/// the interrupt-worthiness is already decided upstream (the R5 router). The
/// payload is intentionally sparse — detail lives in the Panel card; the
/// banner only carries `approval_id` (for diagnostics/correlation) + static
/// title/body.
///
/// `session_key` is the one non-sparse field, and it is not for rendering: it
/// is what lets the forward-filter answer *whose* banner this is. Without it
/// the frame could only be role-gated, which is why the banner was the last of
/// the four approval frames still broadcast to operators alone — a member whose
/// own tool call is parked got the card and no banner. It mirrors
/// `ApprovalRequested::session_key` exactly, including the empty string a
/// cluster-node approval carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceApproval {
    pub approval_id: String,
    pub session_key: String,
    pub title: String,
    pub body: String,
}

/// An outbound interaction the core routes to a delivery surface.
#[derive(Debug, Clone)]
pub enum OutboundInteraction {
    Notify(SurfaceNotification),
    ApprovalRequest(SurfaceApproval),
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("failed to publish delivery frame: {0}")]
    Publish(String),
}

/// A named, addressable outbound surface (desktop shell, browser panel, …).
pub trait DeliverySurface: Send + Sync {
    /// This surface's identity. Used by the forward-filter's `audience` gate.
    fn kind(&self) -> SurfaceKind;
    /// Deliver one outbound interaction. Implementations marshal onto their
    /// transport (e.g. publish a `SurfaceNotify` frame); they do NOT render.
    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError>;
}

/// The set of surfaces the core can address. Phase 1 registers exactly one
/// (desktop); the Vec is the extension point for future surfaces.
pub type SurfaceRegistry = Vec<Arc<dyn DeliverySurface>>;

/// Forward-filter gate: may a connection of `channel_kind` receive an event
/// whose payload is `event_data`?
///
/// Events without an `audience` array are unrestricted — that is EVERY legacy
/// event, so this is byte-for-byte zero-regression. An addressed event reaches
/// only connections whose declared kind is listed; a connection that declared
/// no kind is excluded from any addressed event (fail-closed).
#[must_use]
pub fn audience_allows(event_data: Option<&Value>, channel_kind: Option<SurfaceKind>) -> bool {
    let Some(audience) = event_data
        .and_then(|d| d.get("audience"))
        .and_then(|a| a.as_array())
    else {
        return true;
    };
    match channel_kind {
        Some(kind) => audience
            .iter()
            .filter_map(|v| v.as_str())
            .any(|k| k == kind.as_str()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_audience_field_is_unrestricted() {
        // Every legacy event lacks `audience` → always forwarded.
        let data = json!({ "run_id": "r1" });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(audience_allows(Some(&data), None));
        assert!(audience_allows(None, Some(SurfaceKind::Desktop)));
    }

    #[test]
    fn addressed_event_reaches_only_listed_kind() {
        let data = json!({ "audience": ["desktop"], "title": "x" });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Desktop)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Cli)));
    }

    #[test]
    fn addressed_event_excludes_undeclared_connection() {
        let data = json!({ "audience": ["desktop"] });
        assert!(!audience_allows(Some(&data), None));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Unknown)));
    }

    #[test]
    fn multi_kind_audience_matches_any() {
        let data = json!({ "audience": ["desktop", "browser"] });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Desktop)));
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Cli)));
    }
}
