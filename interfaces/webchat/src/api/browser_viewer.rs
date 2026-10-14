//! Typed Panel calls for the browser viewer resource and Frames-only grant.
//!
//! The Panel supplies only an opaque resource id received from the Gateway's
//! browser-live event stream. It never supplies a target, URL, profile or
//! permission chosen by UI state. The Gateway authenticates the caller and
//! derives the binding and expiry.

use crate::context::DashboardState;
use aleph_protocol::browser_live::{
    DecideResourceRequest, IssueViewerGrantRequest, IssueViewerGrantResponse, ListResourcesRequest,
    ListResourcesResponse, PreviewResource, ResourceDecision, RevokeViewerGrantRequest,
    RevokeViewerGrantResponse, ViewerPermission,
};
use leptos::prelude::{Get, GetUntracked};
use serde_json::Value;

/// Exact RPC method name served by the gateway.
pub const BROWSER_VIEWER_DECIDE_METHOD: &str = "browser.viewer.resource.decide";
/// Exact Frames-only grant issuer method.
pub const BROWSER_VIEWER_ISSUE_METHOD: &str = "browser.viewer.grant.issue";
/// Exact grant revocation method.
pub const BROWSER_VIEWER_REVOKE_METHOD: &str = "browser.viewer.grant.revoke";
/// Exact read-only list of the verified caller's live resources.
pub const BROWSER_VIEWER_LIST_METHOD: &str = "browser.viewer.resource.list";

const INVALID_ISSUE_RESPONSE: &str = "Invalid browser viewer grant response";
const INVALID_REVOKE_RESPONSE: &str = "Invalid browser viewer revoke response";
const INVALID_LIST_RESPONSE: &str = "Invalid browser viewer resource list";
const REVOKE_NOT_CONFIRMED: &str = "Browser viewer grant was not revoked";

const NOT_PAIRED: &str = "Browser viewer requires a paired desktop connection";

/// The paired desktop identity a viewer call is made under: the
/// Gateway-derived user plus the socket generation it was observed on. Used
/// only to decide whether Panel UI state is still current; it is never sent
/// over RPC (the Gateway re-derives the user itself) and holds no token.
#[derive(Clone, PartialEq, Eq)]
pub struct ViewerCaller {
    user_id: String,
    connection_epoch: u64,
    gateway_url: String,
}

impl std::fmt::Debug for ViewerCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerCaller")
            .field("connection_epoch", &self.connection_epoch)
            .finish_non_exhaustive()
    }
}

impl ViewerCaller {
    /// Single derivation of "who may call": a non-blank server-derived user on
    /// a live connection. Anything else (including a purely local browser
    /// identity) is `None`, and `None` is never a valid caller.
    #[must_use]
    pub fn derive_from(user: Option<&str>, connected: bool, epoch: u64) -> Option<Self> {
        let user = user.filter(|u| !u.trim().is_empty())?;
        connected.then(|| Self {
            user_id: user.to_string(),
            connection_epoch: epoch,
            gateway_url: String::new(),
        })
    }

    /// Untracked snapshot; safe after the owning scope was disposed (`None`).
    #[must_use]
    pub fn current(state: &DashboardState) -> Option<Self> {
        let user = state.protected_user.try_get_untracked()?;
        let connected = state.is_connected.try_get_untracked()?;
        let epoch = state.connection_epoch.try_get_untracked()?;
        let gateway = state.gateway_url.try_get_untracked()?;
        Self::derive_from(user.as_deref(), connected, epoch)?.at_gateway(gateway)
    }

    /// Tracked snapshot for views / effects; `None` once disposed.
    #[must_use]
    pub fn tracked(state: &DashboardState) -> Option<Self> {
        let user = state.protected_user.try_get()?;
        let connected = state.is_connected.try_get()?;
        let epoch = state.connection_epoch.try_get()?;
        let gateway = state.gateway_url.try_get()?;
        Self::derive_from(user.as_deref(), connected, epoch)?.at_gateway(gateway)
    }

    fn at_gateway(mut self, gateway: String) -> Option<Self> {
        if gateway.is_empty() {
            return None;
        }
        self.gateway_url = gateway;
        Some(self)
    }

    /// `true` only when both sides are `Some` and identical (`None == None`
    /// is not valid).
    #[must_use]
    pub fn still_valid(held: Option<&Self>, now: Option<&Self>) -> bool {
        matches!((held, now), (Some(a), Some(b)) if a == b)
    }
}

/// Refuse to send unless `caller` is still the current paired caller.
fn ensure_current(caller: &ViewerCaller, state: &DashboardState) -> Result<(), String> {
    if ViewerCaller::still_valid(Some(caller), ViewerCaller::current(state).as_ref()) {
        Ok(())
    } else {
        Err(NOT_PAIRED.to_string())
    }
}

pub struct BrowserViewerApi;

fn decide_params(
    resource_id: &str,
    expected_revision: u64,
    decision: ResourceDecision,
) -> Result<Value, String> {
    serde_json::to_value(DecideResourceRequest {
        resource_id: resource_id.to_string(),
        expected_revision,
        decision,
    })
    .map_err(|_| "Failed to encode browser viewer decision".to_string())
}

fn issue_params(resource_id: &str) -> Result<Value, String> {
    serde_json::to_value(IssueViewerGrantRequest {
        resource_id: resource_id.to_string(),
        permissions: vec![ViewerPermission::Frames],
    })
    .map_err(|_| "Failed to encode browser viewer grant request".to_string())
}

fn revoke_params(grant_id: &str) -> Result<Value, String> {
    serde_json::to_value(RevokeViewerGrantRequest {
        grant_id: grant_id.to_string(),
    })
    .map_err(|_| "Failed to encode browser viewer revoke request".to_string())
}

fn list_params() -> Result<Value, String> {
    serde_json::to_value(ListResourcesRequest {})
        .map_err(|_| "Failed to encode browser viewer list request".to_string())
}

/// Decode the list result strictly: an unknown or malformed shape is an error,
/// never an empty list (an empty list is a real answer: "nothing is live").
fn decode_list(result: Value) -> Result<Vec<PreviewResource>, String> {
    serde_json::from_value::<ListResourcesResponse>(result)
        .map(|r| r.resources)
        .map_err(|_| INVALID_LIST_RESPONSE.to_string())
}

impl BrowserViewerApi {
    /// The Gateway registry's exact current view of this caller's live,
    /// unexpired resources. Used to seed the browser workspace after a
    /// (re)connect, because `browser.live.resource` frames published before
    /// this socket existed are never replayed. Gated on the paired caller
    /// like every other viewer call; the Gateway re-derives the user itself.
    pub async fn list_resources(
        state: &DashboardState,
        caller: &ViewerCaller,
    ) -> Result<Vec<PreviewResource>, String> {
        let params = list_params()?;
        ensure_current(caller, state)?;
        decode_list(state.rpc_call(BROWSER_VIEWER_LIST_METHOD, params).await?)
    }

    /// Ask the gateway to record `decision` for exactly
    /// `resource_id` @ `expected_revision`. `Ok(())` means only "the gateway
    /// accepted the call"; the new state arrives over the event bridge.
    pub async fn decide(
        state: &DashboardState,
        caller: &ViewerCaller,
        resource_id: &str,
        expected_revision: u64,
        decision: ResourceDecision,
    ) -> Result<(), String> {
        let params = decide_params(resource_id, expected_revision, decision)?;
        ensure_current(caller, state)?;
        state
            .rpc_call(BROWSER_VIEWER_DECIDE_METHOD, params)
            .await
            .map(|_| ())
    }

    /// Issue the narrowest viewer grant for an exact Gateway-published
    /// resource. The returned URL is opaque to this API and must be handed to
    /// the launch seam unchanged after the caller validates its envelope.
    pub async fn issue_frames_grant(
        state: &DashboardState,
        caller: &ViewerCaller,
        resource_id: &str,
    ) -> Result<IssueViewerGrantResponse, String> {
        let params = issue_params(resource_id)?;
        ensure_current(caller, state)?;
        let result = state.rpc_call(BROWSER_VIEWER_ISSUE_METHOD, params).await?;
        serde_json::from_value(result).map_err(|_| INVALID_ISSUE_RESPONSE.to_string())
    }

    /// Revoke a grant that was issued but could not be opened or became stale.
    /// Deliberately not gated on a local pairing snapshot: cleanup must still
    /// reach the Gateway (which verifies the user itself) after a local flag
    /// flipped.
    /// The response must explicitly confirm revocation; an unconfirmed result
    /// is treated as a failure and never silently claimed as cleanup.
    pub async fn revoke_grant(state: &DashboardState, grant_id: &str) -> Result<(), String> {
        let result = state
            .rpc_call(BROWSER_VIEWER_REVOKE_METHOD, revoke_params(grant_id)?)
            .await?;
        let response: RevokeViewerGrantResponse =
            serde_json::from_value(result).map_err(|_| INVALID_REVOKE_RESPONSE.to_string())?;
        if response.revoked {
            Ok(())
        } else {
            Err(REVOKE_NOT_CONFIRMED.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_user_and_epoch_on_another_gateway_is_stale() {
        let base = ViewerCaller::derive_from(Some("user-42"), true, 7).unwrap();
        let first = base
            .clone()
            .at_gateway("wss://first.example/ws".into())
            .unwrap();
        let second = base.at_gateway("wss://second.example/ws".into()).unwrap();
        assert!(!ViewerCaller::still_valid(Some(&first), Some(&second)));
    }
    use serde_json::json;

    #[test]
    fn caller_requires_nonblank_user_and_a_live_connection() {
        assert!(ViewerCaller::derive_from(Some("alice"), true, 1).is_some());
        assert!(ViewerCaller::derive_from(None, true, 1).is_none());
        assert!(ViewerCaller::derive_from(Some("  "), true, 1).is_none());
        assert!(ViewerCaller::derive_from(Some("alice"), false, 1).is_none());
    }

    #[test]
    fn caller_validity_breaks_on_user_epoch_or_disconnect_and_none_is_never_valid() {
        let held = ViewerCaller::derive_from(Some("alice"), true, 1);
        assert!(ViewerCaller::still_valid(
            held.as_ref(),
            ViewerCaller::derive_from(Some("alice"), true, 1).as_ref()
        ));
        for now in [
            ViewerCaller::derive_from(Some("bob"), true, 1),
            ViewerCaller::derive_from(Some("alice"), true, 2),
            ViewerCaller::derive_from(Some("alice"), false, 1),
            None,
        ] {
            assert!(!ViewerCaller::still_valid(held.as_ref(), now.as_ref()));
        }
        assert!(!ViewerCaller::still_valid(None, None));
        assert!(!format!("{held:?}").contains("alice"));
    }

    #[test]
    fn method_names_are_the_exact_gateway_names() {
        assert_eq!(
            BROWSER_VIEWER_DECIDE_METHOD,
            "browser.viewer.resource.decide"
        );
        assert_eq!(BROWSER_VIEWER_ISSUE_METHOD, "browser.viewer.grant.issue");
        assert_eq!(BROWSER_VIEWER_REVOKE_METHOD, "browser.viewer.grant.revoke");
        assert_eq!(BROWSER_VIEWER_LIST_METHOD, "browser.viewer.resource.list");
    }

    #[test]
    fn list_sends_empty_params_and_decodes_strictly() {
        assert_eq!(list_params().unwrap(), json!({}));
        assert_eq!(decode_list(json!({"resources": []})).unwrap(), Vec::new());
        for bad in [
            json!(null),
            json!([]),
            json!({}),
            json!({"resources": [{"resource_id": "r"}]}),
            json!({"resources": [], "extra": 1}),
        ] {
            assert_eq!(decode_list(bad).unwrap_err(), INVALID_LIST_RESPONSE);
        }
    }

    #[test]
    fn decision_params_carry_exactly_the_three_protocol_fields() {
        assert_eq!(
            decide_params("resource-5", 3, ResourceDecision::Safe).unwrap(),
            json!({
                "resource_id": "resource-5",
                "expected_revision": 3,
                "decision": "safe",
            })
        );
        assert_eq!(
            decide_params("resource-5", 3, ResourceDecision::Refused).unwrap()["decision"],
            "refused"
        );
    }

    #[test]
    fn issue_params_can_request_frames_only_for_the_opaque_resource() {
        assert_eq!(
            issue_params("resource-5").unwrap(),
            json!({"resource_id": "resource-5", "permissions": ["frames"]})
        );
        let object = issue_params("resource-5").unwrap();
        assert_eq!(object.as_object().unwrap().len(), 2);
    }

    #[test]
    fn revoke_params_carry_only_the_opaque_grant_id() {
        assert_eq!(
            revoke_params("grant-5").unwrap(),
            json!({"grant_id": "grant-5"})
        );
    }

    #[test]
    fn response_decoding_is_strict_and_debug_redacts_the_launch_url() {
        let response: IssueViewerGrantResponse = serde_json::from_value(json!({
            "launch_url": "https://gw.example/viewer/browser#grant_id=g&nonce=secret",
            "grant_id": "g",
            "expires_at_ms": 42,
        }))
        .unwrap();
        assert!(!format!("{response:?}").contains("secret"));
        assert!(serde_json::from_value::<IssueViewerGrantResponse>(json!({
            "launch_url": "https://gw.example/viewer/browser#grant_id=g&nonce=secret",
            "grant_id": "g",
            "expires_at_ms": 42,
            "extra": true,
        }))
        .is_err());
        assert!(serde_json::from_value::<RevokeViewerGrantResponse>(json!({
            "revoked": true,
            "extra": true,
        }))
        .is_err());
    }
}
