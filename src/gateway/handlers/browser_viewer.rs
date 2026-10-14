//! Authenticated issuer RPC adapters. All resource/grant policy lives in core.

use std::sync::Arc;

use super::HandlerRegistry;
pub use crate::browser::live::issuer::BrowserViewerIssuerState;
use crate::gateway::caller_identity::current_caller_verified_user;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, PERMISSION_DENIED,
};
use aleph_protocol::browser_live::{
    DecideResourceRequest, IssueViewerGrantRequest, ListResourcesRequest, RevokeViewerGrantRequest,
};

/// Register a snapshot of server-owned state. The backend setter re-registers
/// after installing state, preventing an early None registration from lingering.
pub(crate) fn register(
    registry: &mut HandlerRegistry,
    state: Option<Arc<BrowserViewerIssuerState>>,
) {
    let issue_state = state.clone();
    registry.register("browser.viewer.grant.issue", move |req| {
        let state = issue_state.clone();
        async move { handle_issue(req, state).await }
    });
    let revoke_state = state.clone();
    registry.register("browser.viewer.grant.revoke", move |req| {
        let state = revoke_state.clone();
        async move { handle_revoke(req, state).await }
    });
    let decide_state = state.clone();
    registry.register("browser.viewer.resource.decide", move |req| {
        let state = decide_state.clone();
        async move { handle_decide(req, state).await }
    });
    registry.register("browser.viewer.resource.list", move |req| {
        let state = state.clone();
        async move { handle_list(req, state).await }
    });
}

/// Read-only snapshot of the verified caller's own live resources, so a client
/// that (re)connected after a `browser.live.resource` frame was published can
/// seed its view from the server registry. Same verified-user gate and generic
/// denial as the mutating viewer methods; params must be absent or `{}`.
pub async fn handle_list(
    request: JsonRpcRequest,
    state: Option<Arc<BrowserViewerIssuerState>>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    let Some(user) = current_caller_verified_user() else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(state) = state else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let params = request
        .params
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    // serde accepts an empty sequence for a field-less struct; the wire shape
    // is an object (or absent), so anything else is refused explicitly.
    if !params.is_object() || serde_json::from_value::<ListResourcesRequest>(params).is_err() {
        return JsonRpcResponse::error(id, INVALID_PARAMS, "invalid params");
    }
    match state.list(&user) {
        Ok(response) => JsonRpcResponse::success(
            id,
            serde_json::to_value(response).expect("wire response serializes"),
        ),
        Err(_) => JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"),
    }
}

pub async fn handle_issue(
    request: JsonRpcRequest,
    state: Option<Arc<BrowserViewerIssuerState>>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    // Handshake-VERIFIED user only: a stamped/implicit-owner user without a
    // verified binding (loopback, shared token, legacy device) is denied.
    let Some(user) = current_caller_verified_user() else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(state) = state else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(params) = request
        .params
        .and_then(|p| serde_json::from_value::<IssueViewerGrantRequest>(p).ok())
    else {
        // Never echo params/serde errors: unknown fields could contain secrets.
        return JsonRpcResponse::error(id, INVALID_PARAMS, "invalid params");
    };
    match state.issue(&user, params) {
        Ok(response) => JsonRpcResponse::success(
            id,
            serde_json::to_value(response).expect("wire response serializes"),
        ),
        Err(_) => JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"),
    }
}

pub async fn handle_revoke(
    request: JsonRpcRequest,
    state: Option<Arc<BrowserViewerIssuerState>>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    // Handshake-VERIFIED user only: a stamped/implicit-owner user without a
    // verified binding (loopback, shared token, legacy device) is denied.
    let Some(user) = current_caller_verified_user() else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(state) = state else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(params) = request
        .params
        .and_then(|p| serde_json::from_value::<RevokeViewerGrantRequest>(p).ok())
    else {
        return JsonRpcResponse::error(id, INVALID_PARAMS, "invalid params");
    };
    match state.revoke(&user, &params.grant_id) {
        Ok(response) => JsonRpcResponse::success(
            id,
            serde_json::to_value(response).expect("wire response serializes"),
        ),
        Err(_) => JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"),
    }
}

/// Thin adapter: the caller is the authenticated RPC user (never a run scope
/// or connection id); all policy lives in `BrowserViewerIssuerState::decide`.
/// A missing state/lifecycle is the same generic denial as any other failure.
pub async fn handle_decide(
    request: JsonRpcRequest,
    state: Option<Arc<BrowserViewerIssuerState>>,
) -> JsonRpcResponse {
    let id = request.id.clone();
    // Handshake-VERIFIED user only: a stamped/implicit-owner user without a
    // verified binding (loopback, shared token, legacy device) is denied.
    let Some(user) = current_caller_verified_user() else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(state) = state else {
        return JsonRpcResponse::error(id, PERMISSION_DENIED, "denied");
    };
    let Some(params) = request
        .params
        .and_then(|p| serde_json::from_value::<DecideResourceRequest>(p).ok())
    else {
        return JsonRpcResponse::error(id, INVALID_PARAMS, "invalid params");
    };
    match state.decide(&user, params) {
        Ok(event) => JsonRpcResponse::success(
            id,
            serde_json::to_value(event).expect("wire response serializes"),
        ),
        Err(_) => JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::live::issuer::tests::{fixture, USER};
    use crate::gateway::caller_identity::with_verified_caller_identity;
    use serde_json::{json, Value};
    use std::time::Duration;

    async fn rpc(
        registry: &HandlerRegistry,
        user: Option<&str>,
        role: &str,
        method: &str,
        params: Value,
    ) -> JsonRpcResponse {
        let req = JsonRpcRequest::with_id(method, Some(params), json!(1));
        // Verified == stamped user: the handshake proved this principal.
        with_verified_caller_identity(
            Some(role.into()),
            user.map(str::to_string),
            false,
            Some("connection-one".into()),
            user.map(str::to_string),
            registry.handle(&req),
        )
        .await
    }

    /// Stamped user and role present, but NO handshake-verified user
    /// (loopback implicit owner / shared token / legacy device).
    async fn rpc_unverified(
        registry: &HandlerRegistry,
        stamped: Option<&str>,
        verified: Option<&str>,
        role: &str,
        method: &str,
        params: Value,
    ) -> JsonRpcResponse {
        let req = JsonRpcRequest::with_id(method, Some(params), json!(1));
        with_verified_caller_identity(
            Some(role.into()),
            stamped.map(str::to_string),
            true,
            Some("connection-one".into()),
            verified.map(str::to_string),
            registry.handle(&req),
        )
        .await
    }

    #[tokio::test]
    async fn credentialless_implicit_owner_denies_every_viewer_method() {
        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state.clone()));
        let issue = json!({"resource_id":f.resource.resource_id,"permissions":["frames"]});
        let revoke = json!({"grant_id":"unknown"});
        let decide =
            json!({"resource_id":f.resource.resource_id,"expected_revision":1,"decision":"safe"});
        // Stamped user == owner of the resource, role operator, but never verified.
        for role in ["operator", "member"] {
            for verified in [None, Some(""), Some(" "), Some("someone-else")] {
                for (method, params) in [
                    ("browser.viewer.grant.issue", issue.clone()),
                    ("browser.viewer.grant.revoke", revoke.clone()),
                    (DECIDE, decide.clone()),
                    (LIST, json!({})),
                ] {
                    let response =
                        rpc_unverified(&registry, Some(USER), verified, role, method, params).await;
                    assert_denied(&response);
                }
            }
        }
    }

    #[tokio::test]
    async fn verified_member_issues_own_resource_and_foreign_verified_is_denied() {
        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state.clone()));
        let params = json!({"resource_id":f.resource.resource_id,"permissions":["frames"]});
        // Verified == stamped == owner: allowed.
        let ok = rpc_unverified(
            &registry,
            Some(USER),
            Some(USER),
            "member",
            "browser.viewer.grant.issue",
            params.clone(),
        )
        .await;
        assert!(ok.error.is_none(), "{:?}", ok.error);
        // Verified foreign user (stamped == verified) cannot touch the owner's resource.
        let foreign = rpc_unverified(
            &registry,
            Some("other"),
            Some("other"),
            "member",
            "browser.viewer.grant.issue",
            params,
        )
        .await;
        assert_denied(&foreign);
    }

    fn assert_denied(response: &JsonRpcResponse) {
        assert!(response.result.is_none());
        let error = response.error.as_ref().unwrap();
        assert_eq!(error.code, PERMISSION_DENIED);
        assert_eq!(error.message, "denied");
        assert!(error.data.is_none());
    }

    #[tokio::test]
    async fn issuer_registration_missing_state_and_connection_only_deny() {
        let mut registry = HandlerRegistry::new();
        register(&mut registry, None);
        for method in ["browser.viewer.grant.issue", "browser.viewer.grant.revoke"] {
            assert_denied(&rpc(&registry, Some(USER), "member", method, json!({})).await);
        }
        let f = fixture(Duration::from_secs(600));
        register(&mut registry, Some(f.state));
        for user in [None, Some(""), Some(" ")] {
            assert_denied(
                &rpc(
                    &registry,
                    user,
                    "operator",
                    "browser.viewer.grant.issue",
                    json!({"resource_id":f.resource.resource_id,"permissions":["frames"]}),
                )
                .await,
            );
            assert_denied(
                &rpc(
                    &registry,
                    user,
                    "operator",
                    "browser.viewer.grant.revoke",
                    json!({"grant_id":"unknown"}),
                )
                .await,
            );
        }
    }

    #[tokio::test]
    async fn issuer_rpc_owned_issue_and_revoke_enforce_identity_without_oracle() {
        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state.clone()));
        let params = json!({"resource_id":f.resource.resource_id,"permissions":["frames","human_input","navigate"]});
        let foreign = rpc(
            &registry,
            Some("other"),
            "operator",
            "browser.viewer.grant.issue",
            params.clone(),
        )
        .await;
        let unknown = rpc(
            &registry,
            Some("other"),
            "operator",
            "browser.viewer.grant.issue",
            json!({"resource_id":"unknown","permissions":["frames"]}),
        )
        .await;
        assert_denied(&foreign);
        assert_eq!(
            serde_json::to_value(&foreign).unwrap(),
            serde_json::to_value(&unknown).unwrap()
        );
        let response = rpc(
            &registry,
            Some(USER),
            "member",
            "browser.viewer.grant.issue",
            params,
        )
        .await;
        assert!(response.error.is_none(), "{:?}", response.error);
        let issued: aleph_protocol::browser_live::IssueViewerGrantResponse =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(
            f.grants.grant_info(&issued.grant_id).unwrap().permissions,
            vec![aleph_protocol::browser_live::ViewerPermission::Frames]
        );
        let revoke = json!({"grant_id":issued.grant_id});
        let foreign = rpc(
            &registry,
            Some("other"),
            "operator",
            "browser.viewer.grant.revoke",
            revoke.clone(),
        )
        .await;
        let unknown = rpc(
            &registry,
            Some("other"),
            "operator",
            "browser.viewer.grant.revoke",
            json!({"grant_id":"unknown"}),
        )
        .await;
        assert_denied(&foreign);
        assert_eq!(
            serde_json::to_value(&foreign).unwrap(),
            serde_json::to_value(&unknown).unwrap()
        );
        let response = rpc(
            &registry,
            Some(USER),
            "member",
            "browser.viewer.grant.revoke",
            revoke.clone(),
        )
        .await;
        assert_eq!(response.result, Some(json!({"revoked":true})));
        assert_denied(
            &rpc(
                &registry,
                Some(USER),
                "member",
                "browser.viewer.grant.revoke",
                revoke,
            )
            .await,
        );
    }

    #[tokio::test]
    async fn issuer_rpc_rejects_authority_fields_and_noncanonical_permissions() {
        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state));
        for field in [
            "issuer", "binding", "origin", "target", "profile", "session", "url",
        ] {
            let mut params = json!({"resource_id":f.resource.resource_id,"permissions":["frames"]});
            params[field] = json!("secret-attacker-value");
            let response = rpc(
                &registry,
                Some(USER),
                "member",
                "browser.viewer.grant.issue",
                params,
            )
            .await;
            assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
            assert!(!serde_json::to_string(&response)
                .unwrap()
                .contains("secret-attacker-value"));
        }
        for permissions in [json!(["frames", "frames"]), json!(["navigate", "frames"])] {
            let response = rpc(
                &registry,
                Some(USER),
                "member",
                "browser.viewer.grant.issue",
                json!({"resource_id":f.resource.resource_id,"permissions":permissions}),
            )
            .await;
            assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
        }
    }

    // Task20: `browser.viewer.resource.decide` (user decision on a Pending revision).
    const DECIDE: &str = "browser.viewer.resource.decide";

    #[tokio::test]
    async fn decide_rpc_missing_state_and_anonymous_callers_deny_generically() {
        let mut registry = HandlerRegistry::new();
        register(&mut registry, None);
        let params = json!({"resource_id":"r","expected_revision":1,"decision":"safe"});
        assert_denied(&rpc(&registry, Some(USER), "member", DECIDE, params.clone()).await);
        let f = fixture(Duration::from_secs(600));
        register(&mut registry, Some(f.state));
        for user in [None, Some(""), Some(" ")] {
            assert_denied(&rpc(&registry, user, "operator", DECIDE, params.clone()).await);
        }
    }

    #[tokio::test]
    async fn decide_rpc_rejects_authority_fields_and_noncanonical_values_without_echo() {
        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state));
        let id = f.resource.resource_id.clone();
        for field in [
            "issuer",
            "binding",
            "origin",
            "target",
            "profile",
            "session",
            "url",
            "user",
            "user_id",
            "principal",
            "expires_at_ms",
            "expiry",
            "redaction",
            "connection_id",
        ] {
            let mut params = json!({"resource_id":id,"expected_revision":1,"decision":"safe"});
            params[field] = json!("secret-attacker-value");
            let response = rpc(&registry, Some(USER), "member", DECIDE, params).await;
            assert_eq!(
                response.error.as_ref().unwrap().code,
                INVALID_PARAMS,
                "{field}"
            );
            assert!(!serde_json::to_string(&response)
                .unwrap()
                .contains("secret-attacker-value"));
        }
        for decision in [
            json!("redacted"),
            json!("pending"),
            json!("Safe"),
            json!(null),
            json!(1),
        ] {
            let params = json!({"resource_id":id,"expected_revision":1,"decision":decision});
            let response = rpc(&registry, Some(USER), "member", DECIDE, params).await;
            assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
        }
        for revision in [json!(-1), json!("1"), json!(1.5)] {
            let params = json!({"resource_id":id,"expected_revision":revision,"decision":"safe"});
            let response = rpc(&registry, Some(USER), "member", DECIDE, params).await;
            assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
        }
        let missing = json!({"resource_id":id,"decision":"safe"});
        let response = rpc(&registry, Some(USER), "member", DECIDE, missing).await;
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn decide_rpc_without_lifecycle_denies_owner_and_leaves_resource_untouched() {
        // `fixture()` state is built by `new()` with no lifecycle: a decision
        // must deny (never a reported-success no-op) and must not mutate.
        let f = fixture(Duration::from_secs(600));
        let mut pending = f.draft.clone();
        pending.binding.target.target_id = "pending-target".into();
        pending.redaction = aleph_protocol::browser_live::RedactionStatus::Pending;
        let resource = f.resources.publish(pending).unwrap();
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state));
        let params = json!({
            "resource_id": resource.resource_id,
            "expected_revision": resource.revision,
            "decision": "safe"
        });
        let owner = rpc(&registry, Some(USER), "member", DECIDE, params).await;
        let unknown = rpc(
            &registry,
            Some(USER),
            "member",
            DECIDE,
            json!({"resource_id":"unknown","expected_revision":1,"decision":"safe"}),
        )
        .await;
        assert_denied(&owner);
        assert_eq!(
            serde_json::to_value(&owner).unwrap(),
            serde_json::to_value(&unknown).unwrap()
        );
        assert_eq!(
            f.resources.get_active(&resource.resource_id),
            Some(resource)
        );
    }

    #[tokio::test]
    async fn decide_rpc_with_lifecycle_applies_owner_decision_and_denies_non_owner() {
        use crate::browser::live::issuer::tests::ORIGIN;
        use crate::browser::live::lifecycle::BrowserLiveLifecycle;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::events::GatewayEventFrame;
        use aleph_protocol::browser_live::{BusEvent, ClosedReason, RedactionStatus};
        use tokio::sync::broadcast::error::TryRecvError;

        let f = fixture(Duration::from_secs(600));
        let bus = Arc::new(GatewayEventBus::default());
        let mut rx = bus.subscribe_typed();
        let lifecycle =
            BrowserLiveLifecycle::new(f.resources.clone(), f.grants.clone()).with_event_bus(bus);
        let state = BrowserViewerIssuerState::new_with_lifecycle(
            lifecycle,
            ORIGIN,
            Duration::from_secs(300),
        )
        .unwrap();
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(state));

        let mut safe_draft = f.draft.clone();
        safe_draft.binding.target.target_id = "pending-safe".into();
        safe_draft.redaction = RedactionStatus::Pending;
        let pending = f.resources.publish(safe_draft).unwrap();
        let params = |r: &aleph_protocol::browser_live::PreviewResource, d: &str| {
            json!({
                "resource_id": r.resource_id,
                "expected_revision": r.revision,
                "decision": d
            })
        };

        // Non-owner: generic denial, no mutation, no event.
        let denied = rpc(
            &registry,
            Some("other"),
            "operator",
            DECIDE,
            params(&pending, "safe"),
        )
        .await;
        assert_denied(&denied);
        assert_eq!(
            f.resources.get_active(&pending.resource_id),
            Some(pending.clone())
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        // Owner Safe: revision + 1, redaction Safe, everything else unchanged.
        let response = rpc(
            &registry,
            Some(USER),
            "member",
            DECIDE,
            params(&pending, "safe"),
        )
        .await;
        assert!(response.error.is_none(), "{:?}", response.error);
        let BusEvent::Resource { resource } =
            serde_json::from_value(response.result.unwrap()).unwrap()
        else {
            panic!("Safe decision must return a resource event");
        };
        assert_eq!(resource.revision, pending.revision + 1);
        assert_eq!(resource.redaction, RedactionStatus::Safe);
        let mut expected = pending.clone();
        expected.revision = resource.revision;
        expected.redaction = RedactionStatus::Safe;
        assert_eq!(resource, expected, "expiry and metadata must not change");
        assert_eq!(
            f.resources.get_active(&pending.resource_id),
            Some(resource.clone())
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(GatewayEventFrame::BrowserLiveResource { resource: announced })
                if announced == resource
        ));

        // Owner Refused: closed with Revoked, resource no longer active.
        let mut refuse_draft = f.draft.clone();
        refuse_draft.binding.target.target_id = "pending-refuse".into();
        refuse_draft.redaction = RedactionStatus::Pending;
        let to_refuse = f.resources.publish(refuse_draft).unwrap();
        let response = rpc(
            &registry,
            Some(USER),
            "member",
            DECIDE,
            params(&to_refuse, "refused"),
        )
        .await;
        assert!(response.error.is_none(), "{:?}", response.error);
        let BusEvent::Closed { closed } = serde_json::from_value(response.result.unwrap()).unwrap()
        else {
            panic!("Refused decision must return a closed event");
        };
        assert_eq!(closed.resource_id, to_refuse.resource_id);
        assert_eq!(closed.reason, ClosedReason::Revoked);
        assert_eq!(f.resources.get_active(&to_refuse.resource_id), None);
    }

    const LIST: &str = "browser.viewer.resource.list";

    /// The list RPC is the verified owner's exact registry view: same verified
    /// gate and generic denial as the mutating methods, no state -> denied,
    /// any client-supplied filter -> invalid params (never a filtered list),
    /// absent params == `{}`, and a foreign verified caller sees nothing.
    #[tokio::test]
    async fn list_rpc_returns_the_verified_owners_exact_resources_only() {
        use aleph_protocol::browser_live::ListResourcesResponse;

        let mut empty = HandlerRegistry::new();
        register(&mut empty, None);
        assert_denied(&rpc(&empty, Some(USER), "member", LIST, json!({})).await);
        assert_denied(&rpc(&empty, None, "member", LIST, json!({})).await);

        let f = fixture(Duration::from_secs(600));
        let mut registry = HandlerRegistry::new();
        register(&mut registry, Some(f.state.clone()));
        assert_denied(&rpc(&registry, None, "member", LIST, json!({})).await);
        assert_denied(&rpc(&registry, Some(USER), "guest", LIST, json!({})).await);

        let ok = rpc(&registry, Some(USER), "member", LIST, json!({})).await;
        assert!(ok.error.is_none(), "{:?}", ok.error);
        let listed: ListResourcesResponse = serde_json::from_value(ok.result.unwrap()).unwrap();
        assert_eq!(listed.resources, vec![f.resource.clone()]);

        let req = JsonRpcRequest::with_id(LIST, None, json!(1));
        let absent = with_verified_caller_identity(
            Some("member".into()),
            Some(USER.into()),
            false,
            Some("connection-one".into()),
            Some(USER.into()),
            registry.handle(&req),
        )
        .await;
        let absent: ListResourcesResponse = serde_json::from_value(absent.result.unwrap()).unwrap();
        assert_eq!(absent.resources, vec![f.resource.clone()]);

        for filter in [
            json!({"user_id": USER}),
            json!({"target_id": "t"}),
            json!([]),
        ] {
            let bad = rpc(&registry, Some(USER), "member", LIST, filter).await;
            assert!(bad.result.is_none());
            let error = bad.error.unwrap();
            assert_eq!(error.code, INVALID_PARAMS);
            assert_eq!(error.message, "invalid params");
        }

        let foreign = rpc(&registry, Some("other"), "member", LIST, json!({})).await;
        let foreign: ListResourcesResponse =
            serde_json::from_value(foreign.result.unwrap()).unwrap();
        assert!(foreign.resources.is_empty());
    }
}
