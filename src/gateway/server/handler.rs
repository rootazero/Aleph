//! WebSocket connection lifecycle entry point.
//!
//! Five phases: upgrade → auth → dispatch → forward → cleanup.
//! Phase implementations live in the [`connection`] submodule; this file
//! keeps the legacy `crate::gateway::server::handler::*` API stable by
//! re-exporting the items external callers expect (see callers in
//! `server/mod.rs`, `artifact_route.rs`, `canvas_asset_route.rs`,
//! `metrics_endpoint.rs`).
//!
//! The test module at the bottom of this file stays whole — it exercises
//! the helpers re-exported from `connection::*` via `use super::*;` and
//! pins the production-half source in `connection/forward.rs` with an
//! `include_str!` guard (see [`tests::the_delivery_loop_parses_each_event_once_and_projects_it`]).

// Imports needed by the test module below — these were originally at the
// top of `handler.rs` and are now in `connection/mod.rs`. Re-importing
// them here so `use super::*;` in the test module keeps resolving.
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use crate::gateway::middleware::MiddlewareChain;
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use crate::gateway::protocol::JsonRpcResponse;
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use crate::gateway::rate_limiter::RateLimiter;
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use super::per_client_buffer::PerClientBuffer;
#[allow(unused_imports)] // tests-only; legacy re-exports for handler::* callers
use tokio::sync::broadcast;

// Single re-export block — pulls every helper / orchestrator symbol the
// legacy `handler::name` callers (server/mod.rs, artifact_route.rs,
// canvas_asset_route.rs, metrics_endpoint.rs) and the test module's
// `use super::*;` expect. The actual definitions live in
// `super::connection::*`; see `connection/mod.rs`.
#[allow(unused_imports)] // re-exports consumed by `mod tests` via `use super::*;`
pub use super::connection::{
    connect_verdict, device_revoked_id, device_revoked_should_close, dispatch_with_caller_context,
    event_wire_form, extract_topic_and_data, forward_bus_to_client, is_token_rotated_frame,
    node_connect_claim, overflow_warning_frame, parse_trusted_ips, process_request,
    refuse_insecure_remote, resolve_stamped_identity, rotated_should_close_remote, wall_admits,
    ws_upgrade_handler, DEVICE_REVOKED_TOPIC, TOKEN_ROTATED_TOPIC,
};

#[cfg(test)]
mod token_rotation_tests {
    use super::{is_token_rotated_frame, rotated_should_close_remote, TOKEN_ROTATED_TOPIC};
    use crate::gateway::events::GatewayEventFrame;

    /// The exact wire string `GatewayEvents::publish_frame` emits for the
    /// rotation event: the TopicEvent wrapper `{"topic": ..., "data": <frame>}`.
    /// Built from the real `topic_name()` + serde serialization so the test
    /// catches drift in either the topic name or the frame wrapping — the
    /// original tests fed the bare inner frame and so masked the wire-format bug.
    fn rotated_wire_frame() -> String {
        let frame = GatewayEventFrame::TokenRotated;
        serde_json::json!({
            "topic": frame.topic_name(),
            "data": serde_json::to_value(&frame).unwrap(),
        })
        .to_string()
    }

    #[test]
    fn topic_constant_matches_frame_topic_name() {
        // Drift guard: the interceptor's literal must equal the frame's topic,
        // or the kick silently breaks again the next time the topic is renamed.
        assert_eq!(
            GatewayEventFrame::TokenRotated.topic_name(),
            TOKEN_ROTATED_TOPIC
        );
    }

    #[test]
    fn detects_real_publish_frame_wire_form() {
        // Regression for the wire-format bug: the wrapped TopicEvent form the
        // forward loop actually receives must be recognized.
        assert!(is_token_rotated_frame(&rotated_wire_frame()));
    }

    #[test]
    fn remote_session_closes_on_token_rotated() {
        assert!(rotated_should_close_remote(&rotated_wire_frame(), false));
    }

    #[test]
    fn loopback_session_ignores_token_rotated() {
        assert!(!rotated_should_close_remote(&rotated_wire_frame(), true));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!rotated_should_close_remote(
            r#"{"topic":"acp.sessions.changed"}"#,
            false
        ));
        assert!(!rotated_should_close_remote(
            r#"{"topic":"alerts.system"}"#,
            false
        ));
        // The bare inner serde-tagged frame is NOT the wire form and must not
        // match — only the wrapped TopicEvent form reaches the interceptor.
        assert!(!rotated_should_close_remote(
            r#"{"type":"token_rotated"}"#,
            false
        ));
    }
}

#[cfg(test)]
mod device_revocation_tests {
    use super::{device_revoked_id, device_revoked_should_close, DEVICE_REVOKED_TOPIC};
    use crate::gateway::events::GatewayEventFrame;

    /// The exact wire string `publish_frame` emits — the wrapped TopicEvent form
    /// `{"topic": …, "data": <frame>}`, built from the real `topic_name()` and
    /// serde output. Same discipline as the rotation tests: feeding the bare
    /// inner frame is what once let a dud predicate stay green.
    fn revoked_wire_frame(device_id: &str) -> String {
        let frame = GatewayEventFrame::DeviceRevoked {
            device_id: device_id.to_string(),
        };
        serde_json::json!({
            "topic": frame.topic_name(),
            "data": serde_json::to_value(&frame).unwrap(),
        })
        .to_string()
    }

    #[test]
    fn topic_constant_matches_frame_topic_name() {
        assert_eq!(
            GatewayEventFrame::DeviceRevoked {
                device_id: "x".into()
            }
            .topic_name(),
            DEVICE_REVOKED_TOPIC
        );
    }

    #[test]
    fn reads_device_id_from_the_real_publish_frame_wire_form() {
        assert_eq!(
            device_revoked_id(&revoked_wire_frame("device-7")).as_deref(),
            Some("device-7")
        );
        // Bare inner frame is not the wire form.
        assert!(device_revoked_id(r#"{"type":"device_revoked","device_id":"device-7"}"#).is_none());
    }

    #[test]
    fn closes_only_the_named_device() {
        let frame = revoked_wire_frame("device-7");
        assert!(device_revoked_should_close(&frame, Some("device-7")));
        // A different paired device keeps its session.
        assert!(!device_revoked_should_close(&frame, Some("device-8")));
    }

    #[test]
    fn never_closes_an_unbound_session() {
        // Loopback, legacy shared-token, and still-walled connections carry no
        // device_id. A per-device revoke must never collaterally kick the
        // operator's own local App — that is `gateway.token.rotate`'s job.
        assert!(!device_revoked_should_close(
            &revoked_wire_frame("device-7"),
            None
        ));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!device_revoked_should_close(
            r#"{"topic":"gateway.token.rotated","data":{}}"#,
            Some("device-7")
        ));
        assert!(!device_revoked_should_close(
            r#"{"topic":"acp.sessions.changed"}"#,
            Some("device-7")
        ));
        assert!(!device_revoked_should_close("not json", Some("device-7")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── Connect-time identity stamping (resolve_stamped_identity) ─────────
    // These pin the branch logic extracted verbatim from the connect
    // handshake's ConnectionState-stamping site (server::handler still calls
    // this exact function). The surrounding WS glue — lock acquisition, the
    // `state.caller_role = ..` / `state.caller_user = ..` assignment, and
    // `handle_connection`'s dispatch loop itself — has no injectable seam
    // (it operates on a live `axum::extract::ws::WebSocket`) and is not
    // covered here; see the Task 2 fix report for what that would require.

    fn store_with_device_user(
        device_id: &str,
        user_id: &str,
        role: crate::gateway::security::store::UserRole,
    ) -> crate::gateway::security::store::SecurityStore {
        use crate::gateway::security::store::{DeviceUpsertData, SecurityStore};
        let store = SecurityStore::in_memory().unwrap();
        store.create_user(user_id, "Test User", role).unwrap();
        store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "Test Device",
                device_type: Some("panel"),
                public_key: &[1u8; 32],
                fingerprint: device_id,
                role: "operator",
                scopes: &[],
                user_id: None,
            })
            .unwrap();
        store.set_device_user(device_id, user_id).unwrap();
        store
    }

    #[test]
    fn unauthorized_stays_walled_even_with_a_store_present() {
        // A store being available never overrides an unauthorized verdict.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (user, role) = resolve_stamped_identity(false, false, Some("dev-r"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    #[test]
    fn no_store_and_no_device_falls_back_to_owner_when_authorized() {
        // probe/test server with no security store, and a connection that is
        // not device-bound (loopback / legacy shared token): the pre-P0 shape,
        // LAN-trust degrade preserved.
        let (user, role) = resolve_stamped_identity(true, false, None, None);
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
        assert_eq!(role, "operator");
    }

    #[test]
    fn no_store_but_device_bound_fails_closed_to_guest() {
        // The other half of the same arm: a device id WAS presented but there
        // is no store to resolve its binding against. "Could not look it up"
        // must not read as "unbound, therefore owner" — the device id is
        // remote-controlled input, so fail closed exactly like the store-`Err`
        // arm inside `resolve_connection_identity`.
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-x"), None);
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    // ── The connect response's echoed verdict (connect_verdict) ──────────
    // Composed with `resolve_stamped_identity` so each case runs the real
    // chain the handshake runs: credential verdict + store → resolved role →
    // wire triple.

    #[test]
    fn connect_response_reports_member_authority_to_a_member() {
        // The regression: the response used to be computed from the
        // credential verdict alone, so a member was told `role: "operator"`
        // and rendered an operator UI whose every admin surface then failed.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (_, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(connect_verdict(true, role), ("member", true, false));
    }

    #[test]
    fn connect_response_walls_a_deactivated_users_valid_device() {
        // Valid credential, no principal. Reported with the EXISTING walled
        // shape — no new vocabulary, no new close reason.
        use crate::gateway::security::store::UserStatus;
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(
            connect_verdict(true, role),
            ("guest", false, true),
            "a credential that grants nothing must not claim operator authority"
        );
    }

    #[test]
    fn connect_response_is_unchanged_for_operator_and_walled_connections() {
        // Zero-change guarantee: every pre-P0 shape produces exactly the
        // triple the old credential-only overlay produced.
        // Loopback / legacy shared token (no store, no device).
        let (_, lo_role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(connect_verdict(true, lo_role), ("operator", true, false));
        // Remote, admin-bound device.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (_, adm_role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(connect_verdict(true, adm_role), ("operator", true, false));
        // Rejected credential.
        let (_, guest_role) = resolve_stamped_identity(false, false, None, Some(&store));
        assert_eq!(connect_verdict(false, guest_role), ("guest", false, true));
    }

    // ── The event scope stamped alongside that verdict ────────────────────
    // The stamping itself has no seam (it edits ConnectionState inside the
    // live WS loop), but both its inputs are pure, so the composition the
    // handshake actually evaluates is testable: resolved role → scope.

    #[test]
    fn connect_stamps_a_member_out_of_the_admin_event_scope() {
        // The finding: a member holds authority (the login wall admits him),
        // and the stamping used to key on that, handing him `"*"` — which
        // short-circuits every EventScopeGuard rule. So a member's socket was
        // delivered exec approval cards including the command text.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (_, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        let scope = crate::gateway::event_scope::scope_for_role(role);
        assert!(scope.is_empty(), "a member must not be stamped `*`");

        let guard = crate::gateway::event_scope::EventScopeGuard::default_rules();
        // The raw approval CARDS are no longer this guard's business (they are
        // owner-scoped per frame since 2026-08-08, so a member gets their own
        // and no one else's). What a member must still not hold is the
        // superuser scope, which is what would hand them everybody's.
        // ...and that same predicate is now what decides the R5 BANNER too: it
        // left this table on 2026-08-09 once it began carrying the session key
        // it is derived from, so `can_receive("surface.approval", …)` no longer
        // answers anything. `is_superuser_scope` above IS the admin arm of the
        // banner's owner check — the assertion did not go away, it merged into
        // the line before this comment.
        assert!(!crate::gateway::event_scope::is_superuser_scope(&scope));
        assert!(guard.can_receive("surface.approval", &scope));
        assert!(!guard.can_receive("config.changed", &scope));
        assert!(!guard.can_receive("pairing.requested", &scope));
        assert!(!guard.can_receive("pty.screen", &scope));
        // `approval.requested` deliberately passes THIS table since 2026-08-08
        // — a member must be able to answer the gate blocking their own run.
        // The per-session decision is made in `event_visibility`, pinned there.
        assert!(guard.can_receive("approval.requested", &scope));
        // ...while his daily surfaces are untouched (default-allow guard).
        assert!(guard.can_receive("agent.run.started", &scope));
        assert!(guard.can_receive("chat.message", &scope));
    }

    #[test]
    fn connect_stamps_operator_and_walled_scopes_unchanged() {
        // Zero-change guarantee on the scope axis, mirroring
        // `connect_response_is_unchanged_for_operator_and_walled_connections`.
        let star = vec!["*".to_string()];
        // Loopback / legacy shared token.
        let (_, lo_role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(crate::gateway::event_scope::scope_for_role(lo_role), star);
        // Remote, admin-bound device.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (_, adm_role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(crate::gateway::event_scope::scope_for_role(adm_role), star);
        // Rejected credential ⇒ walled, no scope (as before).
        let (_, guest_role) = resolve_stamped_identity(false, false, None, Some(&store));
        assert!(crate::gateway::event_scope::scope_for_role(guest_role).is_empty());
    }

    #[test]
    fn scope_keyed_on_role_matches_holds_authority_except_for_members() {
        // The stamping switched from `holds_authority` to the resolved role.
        // That is safe because `resolve_stamped_identity` returns "guest"
        // whenever `!authorized`, so `holds_authority == (role != "guest")`
        // for every shape — the ONE divergence is the member, which is the
        // fix. Pinned here so a future change to either function that breaks
        // the equivalence is loud rather than a silent scope widening.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let admin_store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let cases = [
            // (authorized, loopback, device, store)
            (true, true, None, None),
            (true, false, None, None),
            (false, false, None, None),
            (true, false, Some("dev-x"), None),
            (true, false, Some("dev-a"), Some(&store)),
            (false, false, Some("dev-a"), Some(&store)),
            (true, false, Some("dev-r"), Some(&admin_store)),
        ];
        for (authorized, loopback, device, st) in cases {
            let (_, role) = resolve_stamped_identity(authorized, loopback, device, st);
            let (_, holds_authority, _) = connect_verdict(authorized, role);
            let scope = crate::gateway::event_scope::scope_for_role(role);
            if role == "member" {
                assert!(
                    holds_authority && scope.is_empty(),
                    "a member holds authority yet must hold no event scope"
                );
            } else {
                assert_eq!(
                    holds_authority,
                    !scope.is_empty(),
                    "non-member scope must still track holds_authority \
                     (authorized={authorized}, loopback={loopback}, role={role})"
                );
            }
        }
    }

    // ── The login wall's own predicate (wall_admits) ──────────────────────
    // These drive the ACTUAL wall expression the dispatch loop evaluates.
    // The `resolve_stamped_identity` tests above prove a member connection is
    // *stamped* "member"; only these prove the wall then lets it through —
    // the distinction is not academic, it is precisely how "member is refused
    // every method and then flood-kicked as an abuser" stayed green.

    #[test]
    fn wall_admits_member_on_a_daily_method() {
        assert!(
            wall_admits(Some("member"), "chat.send"),
            "a member connection must clear the guest wall; the admin/member \
             split is method_admin.rs's job, deeper in"
        );
        assert!(wall_admits(Some("member"), "sessions.list"));
        assert!(wall_admits(Some("member"), "connect"));
    }

    /// The event arm evaluates the wall with `method: ""` — there is no
    /// `connect` exemption to grant on a frame nobody asked for. Both halves
    /// matter and the POSITIVE one is load-bearing: gating the delivery plane
    /// fails *silently* (a withheld frame produces no error to any client), so
    /// a wrong role assumption would dark a real surface with no symptom. The
    /// two authorized roles must still receive.
    #[test]
    fn the_event_arm_wall_refuses_a_guest_and_still_serves_both_authorized_roles() {
        // The state a socket carries before it has sent anything at all:
        // `ConnectionState::new` stamps `caller_role: "guest"`.
        assert!(
            !wall_admits(Some("guest"), ""),
            "an unauthorized socket must receive no event frame; `pty.screen` \
             is Global-classified and carries the operator's live terminal \
             content"
        );
        // …and the same is true for a role word nobody stamps.
        assert!(!wall_admits(Some("bogus"), ""));
        assert!(!wall_admits(None, ""));

        // The half that would go silently dark if the predicate were wrong.
        // A cluster node resolves through `resolve_connection_identity`'s
        // unbound-device arm to `("u-owner", "operator")`, so nodes are on
        // this side of the wall too.
        assert!(
            wall_admits(Some("operator"), ""),
            "operator connections — Panel, CLI and cluster nodes alike — must \
             still receive events"
        );
        assert!(
            wall_admits(Some("member"), ""),
            "a member's own stream.* frames must still arrive; this wall is \
             the GUEST wall, and the per-user filter is event_visibility's job"
        );
    }

    #[test]
    fn wall_admits_operator_on_everything() {
        assert!(wall_admits(Some("operator"), "chat.send"));
        assert!(wall_admits(Some("operator"), "connect"));
        assert!(wall_admits(Some("operator"), "config.patch"));
    }

    #[test]
    fn wall_refuses_guest_except_connect() {
        assert!(
            !wall_admits(Some("guest"), "chat.send"),
            "the wall must stay the guest wall"
        );
        assert!(
            wall_admits(Some("guest"), "connect"),
            "connect is the only way to authorize, so it is always admitted"
        );
    }

    #[test]
    fn wall_fails_closed_on_absent_or_unknown_roles() {
        // Pre-handshake / vanished connection state, and any role string the
        // wall does not know, are refused everything but `connect`.
        assert!(!wall_admits(None, "chat.send"));
        assert!(wall_admits(None, "connect"));
        assert!(!wall_admits(Some("admin"), "chat.send")); // wire word is "operator"
        assert!(!wall_admits(Some(""), "chat.send"));
    }

    #[test]
    fn admin_user_device_stamps_operator_and_user_id() {
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(user.as_deref(), Some("u-root"));
        assert_eq!(role, "operator");
    }

    #[test]
    fn member_user_device_stamps_member_and_user_id() {
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(user.as_deref(), Some("u-alice"));
        assert_eq!(role, "member");
    }

    #[test]
    fn deactivated_user_device_is_walled_at_connect_time() {
        // The key behavior this task pins: a device bound to a deactivated
        // user is walled at connect time even though its token was valid
        // (authorized == true).
        use crate::gateway::security::store::UserStatus;
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        store
            .update_user("u-root", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    #[test]
    fn loopback_stamps_owner_operator_regardless_of_device() {
        let (user, role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
        assert_eq!(role, "operator");
    }

    // ── LAN-trust node-shape detection + cluster registration ────────────

    #[test]
    fn node_connect_claim_detects_commands_or_tags_shape() {
        // The node client always sends both `commands` and `tags`.
        let full = serde_json::json!({
            "device_name": "build-box",
            "commands": [],
            "tags": ["linux"]
        });
        let claim = node_connect_claim(Some(&full)).expect("node shape");
        assert_eq!(claim.device_name, "build-box");
        assert!(
            claim.presented_id.is_none(),
            "first boot presents no id — admit_node mints one"
        );
        // Either key alone is enough.
        let tags_only = serde_json::json!({"device_name": "t", "tags": []});
        assert!(node_connect_claim(Some(&tags_only)).is_some());
        let commands_only = serde_json::json!({"device_name": "c2", "commands": []});
        assert!(node_connect_claim(Some(&commands_only)).is_some());
        // A reconnecting node presents its persisted id.
        let with_id = serde_json::json!({
            "device_id": "node-7", "device_name": "x", "commands": [], "tags": []
        });
        assert_eq!(
            node_connect_claim(Some(&with_id)).unwrap().presented_id,
            Some("node-7".to_string())
        );
        // An empty device_id is treated as absent (first boot), not as an id.
        let empty_id = serde_json::json!({
            "device_id": "", "device_name": "x", "commands": [], "tags": []
        });
        assert!(node_connect_claim(Some(&empty_id))
            .unwrap()
            .presented_id
            .is_none());
    }

    #[test]
    fn ordinary_connect_params_are_not_node_shaped() {
        // Panel / CLI connects (no commands/tags) must not register as nodes.
        let panel = serde_json::json!({
            "device_name": "Web Panel",
            "channel_kind": "browser",
            "token": "legacy:sig"
        });
        assert!(node_connect_claim(Some(&panel)).is_none());
        let bare = serde_json::json!({});
        assert!(node_connect_claim(Some(&bare)).is_none());
        assert!(node_connect_claim(None).is_none());
    }

    #[test]
    fn node_shape_connect_registers_and_disconnect_deregisters() {
        let registry = crate::cluster::NodeRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let channel = crate::cluster::ReverseRpcChannel::new(tx);
        let params = serde_json::json!({
            "device_id": "node-7",
            "device_name": "build-box",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["linux"]
        });
        // Same decision + registration sequence the dispatch loop runs on a
        // successful connect (admission resolved to the presented id).
        let claim = node_connect_claim(Some(&params)).expect("node shape");
        let node_id = claim.presented_id.expect("presented id");
        assert!(crate::cluster::maybe_register_node(
            &registry,
            Some("node"),
            &node_id,
            "conn-1",
            Some(&params),
            &channel,
        ));
        // Online + resolvable, as environments.list / node_invoke require.
        assert_eq!(
            registry.node_identity_by_conn("conn-1"),
            Some(("node-7".to_string(), "build-box".to_string()))
        );
        let envs = registry.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-7");
        assert_eq!(envs[0].commands[0].name, "bash");
        // The existing disconnect path deregisters by conn_id.
        assert!(registry.deregister("conn-1"));
        assert!(registry.node_identity_by_conn("conn-1").is_none());
        assert!(registry.list_environments().is_empty());
    }

    #[test]
    fn overflow_warning_frame_has_reconnect_advice() {
        let frame = overflow_warning_frame(7, 42);
        let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["method"].as_str(), Some("event"));
        assert_eq!(v["params"]["topic"].as_str(), Some("connection.warning"));
        assert_eq!(
            v["params"]["data"]["reason"].as_str(),
            Some("events_overflow")
        );
        assert_eq!(v["params"]["data"]["dropped"].as_u64(), Some(7));
        assert_eq!(v["params"]["data"]["total_overflow"].as_u64(), Some(42));
        assert_eq!(v["params"]["data"]["advice"].as_str(), Some("reconnect"));
    }

    #[tokio::test]
    async fn forwarder_survives_lag_and_forwards_post_lag_events() {
        // Regression: a transient broadcast `Lagged` must NOT kill the forwarder.
        // Overflow a small bus, then enqueue a post-lag event and close the bus.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(4);
        let (buffer, mut client_rx) = PerClientBuffer::with_capacity(256);
        let metrics = buffer.metrics().clone();

        for i in 0..10 {
            let _ = bus_tx.send(format!("e{i}"));
        }
        let _ = bus_tx.send("after".to_string());
        drop(bus_tx); // forwarder returns on `Closed` once retained events drain

        // Must terminate (not hang): proves `Lagged` is handled, not fatal.
        forward_bus_to_client(bus_rx, buffer).await;

        // The global-hop drop was accounted on the shared overflow metric.
        assert!(metrics.overflow() >= 1, "lag must be counted as overflow");

        // The most recent event (sent AFTER the lag-inducing burst) survived and
        // was forwarded — the OLD `while let Ok` loop would have broken before it.
        let mut last = None;
        while let Ok(s) = client_rx.try_recv() {
            last = Some(s);
        }
        assert_eq!(last.as_deref(), Some("after"));
    }

    #[tokio::test]
    async fn forwarder_forwards_all_events_without_lag() {
        // Healthy path: every event reaches the client buffer in order.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(64);
        let (buffer, mut client_rx) = PerClientBuffer::with_capacity(256);
        let metrics = buffer.metrics().clone();

        for i in 0..5 {
            let _ = bus_tx.send(format!("m{i}"));
        }
        drop(bus_tx);
        forward_bus_to_client(bus_rx, buffer).await;

        assert_eq!(metrics.overflow(), 0, "no overflow on the healthy path");
        for i in 0..5 {
            assert_eq!(
                client_rx.try_recv().ok().as_deref(),
                Some(format!("m{i}").as_str())
            );
        }
    }

    #[tokio::test]
    async fn forwarder_terminates_when_client_receiver_dropped() {
        // Regression (task-leak): when the connection closes, its sole per-client
        // receiver drops, so the forwarder must reap itself instead of leaking for
        // the process lifetime. The global bus is kept OPEN for the whole await, so
        // termination can ONLY come from the send-failure path — never
        // `RecvError::Closed`. A hang here means the leak (one stranded task holding
        // a live global-bus receiver per WS disconnect) is back.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(16);
        let (buffer, client_rx) = PerClientBuffer::with_capacity(256);

        drop(client_rx); // connection closed: sole per-client receiver gone
        let _ = bus_tx.send("orphan".to_string()); // wakes forwarder → send fails

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            forward_bus_to_client(bus_rx, buffer),
        )
        .await
        .expect("forwarder must terminate once its per-client receiver is dropped");

        // bus_tx is still alive here → proves termination was NOT via bus closure.
        drop(bus_tx);
    }

    // ── Trusted-proxy IP parsing (F5) ─────────────────────────────────────

    #[test]
    fn parses_trusted_ips_dropping_garbage() {
        let parsed = super::parse_trusted_ips(&[
            "127.0.0.1".to_string(),
            "::1".to_string(),
            "not-an-ip".to_string(),
        ]);
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&"127.0.0.1".parse().unwrap()));
        assert!(parsed.contains(&"::1".parse().unwrap()));
    }

    #[test]
    fn insecure_remote_gate_truth_table() {
        const LOCAL: bool = true;
        const REMOTE: bool = false;

        // A genuinely local client is always allowed, secure or not.
        assert!(!super::refuse_insecure_remote(LOCAL, false, false));
        assert!(!super::refuse_insecure_remote(LOCAL, false, true));

        // Remote + insecure + not allowed ⇒ refuse.
        assert!(super::refuse_insecure_remote(REMOTE, false, false));
        // Remote + secure ⇒ allow.
        assert!(!super::refuse_insecure_remote(REMOTE, true, false));
        // Remote + insecure + explicitly allowed ⇒ allow.
        assert!(!super::refuse_insecure_remote(REMOTE, false, true));
    }

    /// The gate must key on the trusted-proxy-aware `local` bit, not on
    /// `ip.is_loopback()`. A same-host proxy that forwards without
    /// `X-Forwarded-For` resolves `ip` back to its own loopback address; if
    /// the gate read that, an internet client on a plaintext leg would be
    /// admitted as "loopback" and then go on to collect every other loopback
    /// privilege in this file.
    #[test]
    fn the_insecure_transport_gate_reads_the_local_bit_not_the_resolved_ip() {
        use crate::gateway::trusted_proxy::resolve_client;
        use axum::http::HeaderMap;
        use std::net::IpAddr;

        let proxy: IpAddr = "127.0.0.1".parse().unwrap();
        let resolved = resolve_client(proxy, &HeaderMap::new(), true, &[proxy]);
        assert!(
            resolved.ip.is_loopback(),
            "precondition: with no XFF the resolved IP really is loopback — \
             that is what made reading it wrong"
        );
        assert!(super::refuse_insecure_remote(resolved.local, false, false));
    }

    // ── P1 scope attribution around dispatch (dispatch_with_caller_context) ──
    // `dispatch_with_caller_context` is the single function BOTH dispatch
    // stations call (`do_lane_dispatch`'s closure and the idempotency
    // `Proceed` arm — see the call sites above). Exercising it once proves
    // both stations by construction: neither wraps `process_request` any
    // other way, so there is no second code path to drift out of sync. This
    // mirrors how `resolve_stamped_identity`/`connect_verdict` are tested
    // above rather than the live WS loop itself — that loop has no
    // injectable seam (it operates on a real `axum::extract::ws::WebSocket`).

    #[tokio::test]
    async fn both_dispatch_stations_seed_scope() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        // A probe method that reports what `scope::current_scope()` sees
        // from inside `process_request`'s dispatch.
        let mut registry = HandlerRegistry::new();
        registry.register("probe.scope", |req| async move {
            let owner = crate::scope::current_scope().map(|attr| attr.owner_user_id);
            JsonRpcResponse::success(req.id, serde_json::json!({ "owner_user_id": owner }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.scope","params":{}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("member".to_string()),
            Some("u-alice".to_string()),
            false,
            None,
        )
        .await;
        assert!(
            resp.contains("\"owner_user_id\":\"u-alice\""),
            "scope must be observable inside process_request's dispatch: {resp}"
        );
    }

    /// `cron.create` reached the way a Panel or CLI caller reaches it — a real
    /// dispatch, not a hand-seeded scope — must leave the caller's identity on
    /// the row that lands in the store.
    ///
    /// The effect asserted is the PERSISTED job, re-read from a freshly loaded
    /// `CronStore`, not the RPC's own response: the response used to be a
    /// perfectly successful `{"job": …}` while both columns were NULL.
    #[tokio::test]
    async fn cron_create_through_dispatch_persists_the_caller_as_owner() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;
        use crate::tasks::cron::store::CronStore;
        use crate::tasks::cron::{CronConfig, CronService};

        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.db");
        let service = CronService::new(CronConfig {
            db_path: db_path.to_string_lossy().to_string(),
            ..CronConfig::default()
        })
        .unwrap();
        let cron = Arc::new(tokio::sync::Mutex::new(service));

        let mut registry = HandlerRegistry::new();
        let handler_cron = cron.clone();
        registry.register("cron.create", move |req| {
            let cron = handler_cron.clone();
            async move { crate::gateway::handlers::cron::handle_create(req, cron).await }
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"cron.create","params":{
            "name":"nightly-digest","agent_id":"main","prompt":"digest",
            "schedule_kind":{"kind":"every","every_ms":60000}}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("operator".to_string()),
            Some("u-x".to_string()),
            true,
            None,
        )
        .await;
        assert!(
            !resp.contains("\"error\""),
            "precondition: the create itself must succeed: {resp}"
        );

        let store = CronStore::load(db_path).unwrap();
        let job = store
            .jobs()
            .iter()
            .find(|j| j.name == "nightly-digest")
            .expect("the job the RPC reported creating must be on disk");
        assert_eq!(
            job.owner_user_id.as_deref(),
            Some("u-x"),
            "a job created over the wire belongs to the caller who created it"
        );
        assert_eq!(
            job.scope_id.as_deref(),
            Some(
                crate::scope::ScopeId::Personal("u-x".to_string())
                    .render()
                    .as_str()
            ),
            "the scope column must be the rendered personal boundary, not just \
             a repeat of the owner id"
        );
    }

    #[tokio::test]
    async fn dispatch_with_caller_context_leaves_scope_unset_for_no_caller_user() {
        // Loopback / legacy shared-token connections resolve to `caller_user:
        // None` — must not seed a scope attribution (no owner to attribute to).
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        let mut registry = HandlerRegistry::new();
        registry.register("probe.scope", |req| async move {
            let owner = crate::scope::current_scope().map(|attr| attr.owner_user_id);
            JsonRpcResponse::success(req.id, serde_json::json!({ "owner_user_id": owner }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.scope","params":{}}"#;

        let resp =
            dispatch_with_caller_context(text, &mc, Some("operator".to_string()), None, true, None)
                .await;
        assert!(
            resp.contains("\"owner_user_id\":null"),
            "no caller_user must mean no scope attribution: {resp}"
        );
    }

    /// `pty.resize` refuses to guess a connection identity — it reads
    /// `CALLER_CONN_ID`, which only `dispatch_with_caller_context` scopes.
    /// This proves that scope actually carries the real connection id from
    /// the dispatch boundary down into `process_request`'s handler dispatch,
    /// the same way `both_dispatch_stations_seed_scope` proves it for
    /// `CALLER_USER`.
    #[tokio::test]
    async fn dispatch_with_caller_context_seeds_conn_id() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        let mut registry = HandlerRegistry::new();
        registry.register("probe.conn_id", |req| async move {
            let conn_id = crate::gateway::caller_identity::current_caller_conn_id();
            JsonRpcResponse::success(req.id, serde_json::json!({ "conn_id": conn_id }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.conn_id","params":{}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("operator".to_string()),
            None,
            true,
            Some("127.0.0.1:9999".to_string()),
        )
        .await;
        assert!(
            resp.contains("\"conn_id\":\"127.0.0.1:9999\""),
            "conn id must be observable inside process_request's dispatch: {resp}"
        );
    }

    // ── extract_topic_and_data — wire-envelope tests (P1 fix round 1) ─────
    // The event_visibility.rs suite hand-builds post-extraction `data` and
    // never runs the REAL envelope through the REAL extraction. These tests
    // feed literal production wire JSON — generated via the actual publish
    // path wherever practical — through `extract_topic_and_data` itself, so
    // a future producer/wrapper-shape mismatch (like the "event"-wrapped
    // double-nesting fix round 1 found) shows up here, not just in a
    // classification unit test that never saw the real bytes.

    use crate::gateway::events::frame::GatewayEventFrame;

    /// The bare `TopicEvent` form — real producer: `publish_frame` on a
    /// non-stream `GatewayEventFrame` variant.
    #[test]
    fn extract_topic_and_data_handles_the_real_bare_topic_event_wire_form() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let frame = GatewayEventFrame::SessionLifecycleChanged {
            session_key: "agent:main:main".to_string(),
            old_state: None,
            new_state: "active".to_string(),
            reason: None,
        };
        bus.publish_frame(&frame).unwrap();
        let wire = rx
            .try_recv()
            .expect("publish_frame must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(topic, "session.lifecycle.changed");
        assert_eq!(
            data.and_then(|d| d.get("session_key"))
                .and_then(|v| v.as_str()),
            Some("agent:main:main")
        );
    }

    /// The `stream.*` JSON-RPC notification form — real producer:
    /// `publish_frame` on a streaming `GatewayEventFrame` variant. `data`
    /// stays `None` here by design (no stream.* frame nests a second `.data`
    /// inside `.params`) — the WS loop's `visibility_payload` fallback
    /// (`event_data.or_else(|| event_obj.get("params"))`) is what reaches
    /// into `.params` for this shape; exercised end-to-end below.
    #[test]
    fn extract_topic_and_data_handles_the_real_stream_wire_form() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let frame = GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: "agent:main:main".to_string(),
            accepted_at: "t".to_string(),
        };
        bus.publish_frame(&frame).unwrap();
        let wire = rx
            .try_recv()
            .expect("publish_frame must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(topic, "stream.run_accepted");
        assert_eq!(
            data, None,
            "no stream.* frame nests a second .data in .params"
        );
        // The frame's own fields live directly under .params — same value
        // the WS loop's visibility_payload fallback reaches for.
        assert_eq!(
            event_obj
                .get("params")
                .and_then(|p| p.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("r1")
        );
    }

    /// The double-wrapped `TopicEvent::to_notification()` form — real
    /// producer: `subagent_tree_relay.rs`'s exact construction
    /// (`TopicEvent::new(topic, data).to_notification()`, published as a raw
    /// string via `GatewayEventBus::publish`, bypassing `publish_frame`
    /// entirely). Before fix round 1, `extract_topic_and_data`'s
    /// predecessor read `topic` as the literal string `"event"` here —
    /// this pins the fix.
    #[test]
    fn extract_topic_and_data_unwraps_the_real_double_nested_event_envelope() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let tree_event = serde_json::json!({
            "kind": "settled",
            "node_id": "n1",
            "root_session": "agent:main:main",
            "lifecycle": "completed",
            "duration_ms": 100,
            "iterations": 1,
            "tool_calls_made": 1,
            "total_tokens": 10,
        });
        let notification = TopicEvent::new("run.subagent_tree", tree_event).to_notification();
        let json = serde_json::to_string(&notification).unwrap();
        bus.publish(json);
        let wire = rx.try_recv().expect("publish must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        // Prove the envelope really is double-nested (method == "event", no
        // top-level "topic") — otherwise this test would pass for the wrong
        // reason.
        assert_eq!(
            event_obj.get("method").and_then(|m| m.as_str()),
            Some("event")
        );
        assert!(event_obj.get("topic").is_none());

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(
            topic, "run.subagent_tree",
            "must unwrap to the REAL topic, not the literal \"event\" wrapper method"
        );
        assert_eq!(
            data.and_then(|d| d.get("root_session"))
                .and_then(|v| v.as_str()),
            Some("agent:main:main")
        );
    }

    fn visibility_test_store() -> (
        crate::gateway::session_store::file_backend::FileSessionStore,
        tempfile::TempDir,
    ) {
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        let temp = tempfile::TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        (store, temp)
    }

    /// End-to-end: real `publish_frame` wire bytes → `extract_topic_and_data`
    /// → the SAME `visibility_payload` fallback the WS loop computes →
    /// `EventVisibilityIndex::note_frame`/`event_admits`. Proves the owner
    /// scoping this task adds actually receives a resolvable `run_id`/
    /// `session_key` from a REAL `RunAccepted`→`AgentTrace` run, not just a
    /// hand-built payload shaped to look like one.
    #[tokio::test]
    async fn owner_scoping_round_trips_through_the_real_publish_path() {
        use crate::gateway::event_visibility::EventVisibilityIndex;
        use crate::gateway::router::SessionKey;
        use crate::gateway::session_store::SessionStore;

        let (store, _temp) = visibility_test_store();
        let key = SessionKey::main("main");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let index = EventVisibilityIndex::new();

        // Seed: real RunAccepted, real wire bytes, real extraction.
        let accepted = GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: key.to_key_string(),
            accepted_at: "t".to_string(),
        };
        bus.publish_frame(&accepted).unwrap();
        let wire = rx.try_recv().unwrap();
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();
        let (topic, event_data) = extract_topic_and_data(&event_obj);
        let visibility_payload = event_data.or_else(|| event_obj.get("params"));
        index.note_frame(topic, visibility_payload).await;

        // A later same-run frame, resolved purely through the seed above —
        // real wire bytes, real extraction, same fallback the loop uses.
        let trace = GatewayEventFrame::AgentTrace {
            run_id: "r1".to_string(),
            seq: 1,
            event: aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 },
        };
        bus.publish_frame(&trace).unwrap();
        let wire2 = rx.try_recv().unwrap();
        let event_obj2: serde_json::Value = serde_json::from_str(&wire2).unwrap();
        let (topic2, event_data2) = extract_topic_and_data(&event_obj2);
        let visibility_payload2 = event_data2.or_else(|| event_obj2.get("params"));

        assert!(
            index
                .event_admits(
                    topic2,
                    visibility_payload2,
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    topic2,
                    visibility_payload2,
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await
        );
    }

    /// The running-set projection end to end, through the SAME four steps the
    /// delivery loop runs — real `publish_frame` bytes → one parse →
    /// `extract_topic_and_data` → the loop's `visibility_payload` fallback →
    /// `project_for` → [`event_wire_form`] — and asserted on the BYTES that
    /// would be written to the socket, not on the projection's return value.
    ///
    /// The frame stays admitted (`Global`); what changes is what it says.
    #[tokio::test]
    async fn the_running_set_frame_reaches_the_wire_narrowed_to_this_connection() {
        use crate::gateway::event_visibility::EventVisibilityIndex;
        use crate::gateway::router::SessionKey;
        use crate::gateway::session_store::SessionStore;

        let (store, _temp) = visibility_test_store();
        let alice_key = SessionKey::main("wire-alice");
        let bob_key = SessionKey::main("wire-bob");
        for (key, owner) in [(&alice_key, "alice"), (&bob_key, "bob")] {
            crate::scope::with_scope(
                Some(crate::scope::ScopeAttribution::personal(owner)),
                store.get_or_create(key),
            )
            .await
            .unwrap();
        }
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_frame(&GatewayEventFrame::RunningSetChanged {
            seq: 12,
            running: vec![alice_key.to_key_string(), bob_key.to_key_string()],
        })
        .unwrap();
        let event_json = rx.try_recv().unwrap();
        assert!(
            event_json.contains(&bob_key.to_key_string()),
            "the frame as PUBLISHED carries every user's key — otherwise this \
             test would pass without any projection at all"
        );

        let parsed: serde_json::Value = serde_json::from_str(&event_json).unwrap();
        let (topic, event_data) = extract_topic_and_data(&parsed);
        let visibility_payload = event_data.or_else(|| parsed.get("params"));

        let index = EventVisibilityIndex::new();
        assert!(
            index
                .event_admits(
                    topic,
                    visibility_payload,
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "the frame itself stays Global — suppressing it would latch alice's \
             red dot on the seq guard"
        );
        let projected = index
            .project_for(topic, visibility_payload, Some("alice"), &store)
            .await;

        let wire = event_wire_form(parsed, projected, event_json.clone());
        assert_ne!(wire, event_json, "the bytes on the wire must have changed");
        let sent: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            sent["method"], "stream.running_set_changed",
            "still the same notification the Panel dispatches on"
        );
        assert_eq!(
            sent["params"]["running"],
            serde_json::json!([alice_key.to_key_string()]),
            "alice is told about her own session and nobody else's"
        );
        assert_eq!(
            sent["params"]["seq"], 12,
            "the client's ordering guard must survive the rewrite verbatim"
        );
    }

    /// The other half of [`event_wire_form`]'s contract, and the reason the
    /// projection is affordable: a frame with nothing to project is forwarded
    /// as the ORIGINAL bytes — no re-serialization, byte-identical — while the
    /// bare `TopicEvent` form is still wrapped exactly as before.
    #[test]
    fn an_unprojected_frame_is_forwarded_as_its_original_bytes() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_frame(&GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: "agent:main:main".to_string(),
            accepted_at: "t".to_string(),
        })
        .unwrap();
        let stream_json = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stream_json).unwrap();
        assert_eq!(
            event_wire_form(parsed, None, stream_json.clone()),
            stream_json,
            "a stream-form frame nobody projected must be forwarded verbatim"
        );

        bus.publish_frame(&GatewayEventFrame::SessionLifecycleChanged {
            session_key: "agent:main:main".to_string(),
            old_state: None,
            new_state: "active".to_string(),
            reason: None,
        })
        .unwrap();
        let topic_json = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&topic_json).unwrap();
        let wrapped: serde_json::Value =
            serde_json::from_str(&event_wire_form(parsed, None, topic_json)).unwrap();
        assert_eq!(
            wrapped["method"], "event",
            "the bare TopicEvent form is still wrapped for the Panel"
        );
        assert_eq!(wrapped["params"]["topic"], "session.lifecycle.changed");
    }

    /// Source-level pin for the delivery loop, which has no unit-testable seam
    /// of its own — it is one `tokio::select!` arm inside the socket task, so
    /// every function it calls can be green while the loop calls none of them.
    /// Two facts about it are invisible from everywhere else:
    ///
    /// 1. the event JSON is parsed exactly ONCE per frame (it was parsed a
    ///    second time for years, purely to decide whether to wrap a
    ///    `TopicEvent` — deleting that is what pays for the projection), and
    /// 2. `EventVisibilityIndex::project_for` is actually CALLED there.
    ///
    /// Only the PRODUCTION half of the file is inspected (everything above the
    /// first test module) and the needles are assembled at runtime, so neither
    /// the tests above nor this one can count as a match.
    ///
    /// NOTE: After splitting `handler.rs` into the `connection/` submodule,
    /// the production-half source lives in `connection/forward.rs` — the
    /// parse needle and `project_for(` call site both moved there with the
    /// forward loop. Reading `forward.rs` is equivalent to reading the old
    /// production half because forward.rs deliberately contains no
    /// `#[cfg(test)]` boundary.
    #[test]
    fn the_delivery_loop_parses_each_event_once_and_projects_it() {
        let src = include_str!("connection/forward.rs");
        let production = src
            .split(&format!("#[cfg{}]", "(test)"))
            .next()
            .expect("the file has a production half");

        let parse_needle = format!(
            "serde_json::from_str::<serde_json::Value>(&{}_json)",
            "event"
        );
        assert_eq!(
            production.matches(&parse_needle).count(),
            1,
            "the delivery loop must parse each event exactly once; a second \
             `{parse_needle}` means the double parse is back"
        );

        let project_needle = format!("{}_for(", "project");
        assert_eq!(
            production.matches(&project_needle).count(),
            1,
            "the payload projection must be wired into the delivery loop — \
             `project_for` is fully tested in `event_visibility`, and with no \
             call site here that proves nothing"
        );
    }
}