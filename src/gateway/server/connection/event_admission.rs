//! The one event-admission decision every connection kind shares.
//!
//! The global event bus is process-wide; this function decides, for one
//! connection and one bus frame, whether the frame is dropped, sent (and in
//! which exact wire form), or whether the connection must close. Legacy `/ws`,
//! Desktop IPC and Web `/v1/ws` all call it, so there is one filter, one
//! ownership rule and one projection — never a per-transport copy.
//!
//! Moved verbatim out of the legacy `/ws` event arm; only control flow changed
//! (`continue` → [`EventVerdict::Skip`], socket writes → returned verdicts).

use tokio::sync::broadcast::{self, error::RecvError};
use tracing::{debug, info, warn};

use super::{
    device_revoked_id, device_revoked_should_close, event_wire_form, extract_topic_and_data,
    is_token_rotated_frame, overflow_warning_frame, rotated_should_close_remote,
    ui_display_admits_connection, v1_event_admitted, wall_admits, ConnectionContext,
};

/// What a connection does with one bus frame.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EventVerdict {
    /// Not for this connection.
    Skip,
    /// Send these exact wire bytes (projection and wrapping already applied).
    Send(String),
    /// Close the connection; the reason is the WebSocket close reason
    /// (`token_rotated` / `device_revoked`, close code 4001 on `/ws`).
    Close(&'static str),
}

/// Decide what connection `conn_id` does with one bus frame.
///
/// `session_device_id` is the device the connection authenticated with, used
/// by the per-device revocation kick. Identity, role, permissions and channel
/// kind are read from the shared `connections` row, so a live restamp is seen
/// here in the same instant the request path sees it.
pub(super) async fn admit_event(
    ctx: &ConnectionContext,
    conn_id: &str,
    session_device_id: Option<&str>,
    event_json: String,
) -> EventVerdict {
    // Token rotation kick: close remote (token-authorized)
    // sessions so they re-authenticate; never forward this
    // frame to clients verbatim, and never close loopback.
    if rotated_should_close_remote(&event_json, ctx.client_is_local) {
        info!("token rotated — closing remote session {}", conn_id);
        return EventVerdict::Close("token_rotated");
    }
    // Loopback receives token_rotated: swallow silently, do not forward.
    if is_token_rotated_frame(&event_json) {
        return EventVerdict::Skip;
    }
    // Per-device revocation kick: close only the sessions bound
    // to the revoked device. `gateway.devices.revoke` already
    // downgraded this connection to the login wall synchronously
    // (`invalidate_device_sessions`), so anything pipelined ahead
    // of this frame is refused rather than served; this closes the
    // socket so the client stops holding a dead session open.
    if device_revoked_should_close(&event_json, session_device_id) {
        info!("device revoked — closing session {}", conn_id);
        return EventVerdict::Close("device_revoked");
    }
    // Everyone else: the revocation names another device (or none
    // of ours). Never forwarded — it carries a device id and no
    // client renders it.
    if device_revoked_id(&event_json).is_some() {
        return EventVerdict::Skip;
    }
    // Parse ONCE. This value is what the filter chain reads
    // and what the wire-form step below rewrites/wraps —
    // it used to be parsed a second time purely to decide
    // whether to wrap, which is what paid for the payload
    // projection now folded in here.
    let parsed = serde_json::from_str::<serde_json::Value>(&event_json).ok();
    // Try to extract topic from event for filtering
    let (should_forward, projected_payload) = if let Some(event_obj) = parsed.as_ref() {
        let (topic, event_data) = extract_topic_and_data(event_obj);
        if !crate::gateway::pty::manager::frame_admits(topic, event_data) {
            return EventVerdict::Skip;
        }
        // The event bus is process-wide; enforce UI ownership at
        // this transport boundary before the generic subscriber
        // receives the payload. Panel surface-id filtering is only
        // a second line of defense.
        if !ui_display_admits_connection(topic, event_data, conn_id)
            || (topic == "capabilities.ui.display"
                && !crate::gateway::capability_ui::display_mount_admits(event_data, conn_id))
        {
            return EventVerdict::Skip;
        }

        // The login wall + permission-based scope guard check +
        // surface audience + caller identity for the owner-scoped
        // event filter (P1, spec §5.4). All four read the same
        // ConnectionState under one lock — extend the tuple, don't
        // take the lock twice.
        let (wall_ok, scope_allowed, channel_kind, event_caller_user, event_caller_role) = {
            let conns = ctx.connections.read().await;
            match conns.get(conn_id) {
                Some(s) => (
                    // 0th term: the login wall, the same predicate
                    // and the same `caller_role` field the request
                    // arm evaluates at the top of the dispatch loop
                    // — so a live demotion through
                    // `restamp_live_connections` closes the event
                    // plane in the same instant it closes the RPC
                    // plane. The method argument is `""`: there is
                    // no `connect` exemption to grant here, because
                    // an event is never the frame that authorizes a
                    // connection.
                    wall_admits(Some(s.caller_role.as_str()), ""),
                    ctx.event_scope_guard.can_receive(topic, &s.permissions),
                    s.channel_kind,
                    s.caller_user.clone(),
                    // The 5th term reads the role too: a
                    // fleet-scoped frame has no owner to
                    // compare against, and the topic-prefix
                    // term above cannot tell it apart from
                    // its session-scoped siblings.
                    s.caller_role.clone(),
                ),
                None => (false, false, None, None, String::new()),
            }
        };

        // The owner-scoped filter needs the frame's OWN fields
        // (run_id/session_key), which for stream.* notifications
        // live directly under `.params` — `event_data` above only
        // resolves for the TopicEvent `.data` and the
        // double-wrapped `TopicEvent::to_notification()`
        // `.params.data` shapes (both handled by
        // `extract_topic_and_data`), not the bare stream-form
        // `.params` (no `GatewayEventFrame` stream.* variant
        // nests a second `.data` inside it), so fall back to the
        // raw `.params` object for that one remaining case.
        let visibility_payload = event_data.or_else(|| event_obj.get("params"));

        // `note_frame` runs unconditionally, before the filter —
        // every connection's loop keeps the shared, process-wide
        // index warm (first writer wins) regardless of whether
        // THIS connection ends up receiving the frame.
        ctx.event_visibility
            .note_frame(topic, visibility_payload)
            .await;

        let mut admits = wall_ok
            && scope_allowed
            && crate::gateway::surface::delivery::audience_allows(event_data, channel_kind)
            && ctx
                .subscription_manager
                .should_receive(conn_id, topic, event_data)
                .await
            && match ctx.session_store.as_ref() {
                Some(store) => {
                    ctx.event_visibility
                        .event_admits_for(
                            topic,
                            visibility_payload,
                            event_caller_user.as_deref(),
                            Some(event_caller_role.as_str()),
                            store,
                            ctx.team_store.as_ref(),
                        )
                        .await
                }
                // No store wired (probe/legacy wiring): skip the
                // 4th term entirely — zero-change guarantee, see
                // `GatewaySharedState::session_store`.
                None => !matches!(
                    topic,
                    "capability.event" | "capability.event.status" | "capabilities.ui.display"
                ),
            };
        if admits
            && matches!(
                topic,
                "capability.event" | "capability.event.status" | "capabilities.ui.display"
            )
        {
            // Session I/O cannot bless an identity restamped while
            // it was in flight. Selection and authority stay separate.
            admits = ctx
                .connections
                .read()
                .await
                .get(conn_id)
                .is_some_and(|state| {
                    !state.first_message
                        && state.caller_user == event_caller_user
                        && state.caller_role == event_caller_role
                        && wall_admits(Some(&state.caller_role), "")
                        && ctx.event_scope_guard.can_receive(topic, &state.permissions)
                        && crate::gateway::surface::delivery::audience_allows(
                            event_data,
                            state.channel_kind,
                        )
                })
                && (topic != "capabilities.ui.display"
                    || crate::gateway::capability_ui::display_mount_admits(event_data, conn_id));
        }

        // 5th term, and the only one that is not pass/fail:
        // one frame (`stream.running_set_changed`) carries a
        // set spanning every user, so it is admitted whole and
        // its ARRAY is narrowed for this connection instead.
        // `None` for every other topic — see
        // `EventVisibilityIndex::project_for`, including why a
        // narrowed-to-empty frame must still be SENT.
        let projected = match (admits, ctx.session_store.as_ref()) {
            (true, Some(store)) => {
                ctx.event_visibility
                    .project_for(
                        topic,
                        visibility_payload,
                        event_caller_user.as_deref(),
                        store,
                    )
                    .await
            }
            _ => None,
        };
        (admits, projected)
    } else {
        // Can't parse event, forward by default
        (true, None)
    };

    if !should_forward {
        return EventVerdict::Skip;
    }
    // Subscription/visibility checks await; revalidate at transport admission.
    if parsed.as_ref().is_some_and(|event| {
        let (topic, data) = extract_topic_and_data(event);
        !crate::gateway::pty::manager::frame_admits(topic, data)
    }) {
        return EventVerdict::Skip;
    }
    debug!("Forwarding event to {}", conn_id);
    // Apply the projection (if any) and wrap the bare
    // TopicEvent form — see `event_wire_form`.
    EventVerdict::Send(match parsed {
        Some(event_obj) => event_wire_form(event_obj, projected_payload, event_json),
        None => event_json,
    })
}

/// What a v1 endpoint does with one received bus frame.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum V1Event {
    /// Write this frame and keep serving.
    Send(String),
    /// Write this warning, then end the connection: frames were lost and the
    /// client must resynchronize from a snapshot.
    Lost(String),
    /// End the connection without writing (revocation kick, bus closed).
    Close(&'static str),
}

/// The live event plane of ONE v1 connection (Desktop IPC or Web `/v1/ws`)
/// that negotiated `live_events`.
///
/// It subscribes to the bus directly — one hop, so every lost frame surfaces
/// as `Lagged` on this receiver; legacy `/ws`'s bus→buffer forwarder needs a
/// ping-tick watchdog for losses on its first hop, this does not. Every frame
/// goes through the shared [`admit_event`], then the v1 topic list, then the
/// endpoint's frame limit.
///
/// Use: select on `receiver.recv()` (cancel-safe) and pass the result to
/// [`V1EventFeed::admit`] in the branch body, never inside the select — the
/// admission awaits, and a cancelled admission would drop a frame silently.
pub(super) struct V1EventFeed {
    pub(super) receiver: broadcast::Receiver<String>,
    conn_id: String,
    device_id: Option<String>,
    max_frame_bytes: usize,
    overflow: u64,
}

impl V1EventFeed {
    pub(super) fn subscribe(
        ctx: &ConnectionContext,
        conn_id: String,
        device_id: Option<String>,
        max_frame_bytes: usize,
    ) -> Self {
        Self {
            receiver: ctx.event_bus.subscribe(),
            conn_id,
            device_id,
            max_frame_bytes,
            overflow: 0,
        }
    }

    /// Decide one received frame. `None`: nothing to write for it.
    pub(super) async fn admit(
        &mut self,
        ctx: &ConnectionContext,
        received: Result<String, RecvError>,
    ) -> Option<V1Event> {
        let event_json = match received {
            Ok(event_json) => event_json,
            Err(RecvError::Lagged(dropped)) => {
                self.overflow += dropped;
                warn!(conn_id = %self.conn_id, dropped, "v1 event receiver lagged; closing");
                return Some(V1Event::Lost(overflow_warning_frame(
                    dropped,
                    self.overflow,
                )));
            }
            Err(RecvError::Closed) => return Some(V1Event::Close("event bus closed")),
        };
        let verdict = admit_event(ctx, &self.conn_id, self.device_id.as_deref(), event_json).await;
        let wire = match verdict {
            EventVerdict::Skip => return None,
            EventVerdict::Close(reason) => return Some(V1Event::Close(reason)),
            EventVerdict::Send(wire) => wire,
        };
        if !v1_event_admitted(&wire) {
            return None;
        }
        // A frame the client cannot accept is one lost frame on a live
        // connection: say so (the client marks its view stale and refetches)
        // and keep serving. Same shape bounds the receiver enforces.
        if wire.len() > self.max_frame_bytes
            || aleph_client::local_ipc::parse_payload(&wire).is_err()
        {
            self.overflow += 1;
            return Some(V1Event::Send(overflow_warning_frame(1, self.overflow)));
        }
        Some(V1Event::Send(wire))
    }
}

/// The next received bus frame of a connection's feed, or never when the
/// connection did not negotiate `live_events`. Cancel-safe: it only awaits
/// `broadcast::Receiver::recv`.
pub(super) async fn next_received(feed: &mut Option<V1EventFeed>) -> Result<String, RecvError> {
    match feed {
        Some(feed) => feed.receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Characterization of the legacy `/ws` event arm, pinned against the shared
/// function it was moved into. Each case is a behavior the arm had before the
/// move; a change here is a change for every connection kind at once.
#[cfg(test)]
mod tests {
    use super::{admit_event, EventVerdict};
    use crate::gateway::handlers::HandlerRegistry;
    use crate::gateway::lane::ChannelClass;
    use crate::gateway::middleware::MiddlewareChain;
    use crate::gateway::rate_limiter::RateLimiter;
    use crate::gateway::router::SessionKey;
    use crate::gateway::server::connection::{ConnectionContext, ConnectionState};
    use crate::gateway::server::GatewayServer;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    fn member(user: &str) -> ConnectionState {
        let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), false);
        state.first_message = false;
        state.caller_role = "member".into();
        state.caller_user = Some(user.into());
        state.verified_user = Some(user.into());
        state.device_id = Some(format!("device-{user}"));
        state
    }

    struct Fixture {
        ctx: ConnectionContext,
        store: Arc<dyn SessionStore>,
        _root: TempDir,
    }

    /// Two members, `alice` and `bob`, each on a connection named after them.
    fn fixture(client_is_local: bool) -> Fixture {
        let root = TempDir::new().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: root.path().join("sessions"),
                ..Default::default()
            })
            .unwrap(),
        );
        let gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        let connections = Arc::new(RwLock::new(HashMap::from([
            ("alice".to_owned(), member("alice")),
            ("bob".to_owned(), member("bob")),
        ])));
        let limiter = Arc::new(RateLimiter::new(Default::default()));
        let ctx = ConnectionContext {
            middleware_chain: MiddlewareChain::new(Arc::new(HandlerRegistry::new()), limiter),
            event_bus: gateway.event_bus.clone(),
            connections,
            subscription_manager: gateway.subscription_manager.clone(),
            presence: gateway.presence.clone(),
            state_versions: gateway.state_versions.clone(),
            rate_limiter: gateway.rate_limiter.clone(),
            lane_manager: gateway.lane_manager.clone(),
            idempotency_guard: gateway.idempotency_guard.clone(),
            event_scope_guard: gateway.event_scope_guard.clone(),
            channel_class: ChannelClass::Desktop,
            ping_interval_secs: 15,
            idle_timeout_secs: 60,
            require_idempotency_key: true,
            security_store: None,
            device_token_mgr: None,
            client_ip: "127.0.0.1".parse().unwrap(),
            client_is_local,
            node_registry: gateway.node_registry.clone(),
            exec_approval_manager: None,
            audit_log: None,
            session_store: Some(store.clone()),
            team_store: None,
            event_visibility: gateway.event_visibility.clone(),
        };
        Fixture {
            ctx,
            store,
            _root: root,
        }
    }

    async fn admit(fixture: &Fixture, conn_id: &str, frame: &Value) -> EventVerdict {
        let device = format!("device-{conn_id}");
        admit_event(&fixture.ctx, conn_id, Some(&device), frame.to_string()).await
    }

    fn stream(method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    }

    #[tokio::test]
    async fn token_rotation_closes_remote_and_is_swallowed_on_loopback() {
        let frame = json!({"topic": "gateway.token.rotated", "data": {"type": "token_rotated"}});
        let remote = fixture(false);
        assert_eq!(
            admit(&remote, "alice", &frame).await,
            EventVerdict::Close("token_rotated")
        );
        let loopback = fixture(true);
        assert_eq!(admit(&loopback, "alice", &frame).await, EventVerdict::Skip);
    }

    #[tokio::test]
    async fn device_revocation_closes_only_the_named_device() {
        let fixture = fixture(false);
        let frame = json!({
            "topic": "gateway.device.revoked",
            "data": {"type": "device_revoked", "device_id": "device-alice"},
        });
        assert_eq!(
            admit(&fixture, "alice", &frame).await,
            EventVerdict::Close("device_revoked")
        );
        assert_eq!(admit(&fixture, "bob", &frame).await, EventVerdict::Skip);
    }

    #[tokio::test]
    async fn a_connection_without_a_row_or_behind_the_login_wall_receives_nothing() {
        let fixture = fixture(false);
        let frame = json!({"topic": "characterization.global", "data": {}});
        assert_eq!(admit(&fixture, "ghost", &frame).await, EventVerdict::Skip);
        fixture
            .ctx
            .connections
            .write()
            .await
            .get_mut("bob")
            .unwrap()
            .caller_role = "guest".into();
        assert_eq!(admit(&fixture, "bob", &frame).await, EventVerdict::Skip);
        assert!(matches!(
            admit(&fixture, "alice", &frame).await,
            EventVerdict::Send(_)
        ));
    }

    #[tokio::test]
    async fn a_bare_topic_event_is_wrapped_as_an_event_notification() {
        let fixture = fixture(false);
        let frame = json!({"topic": "characterization.global", "data": {"n": 1}});
        let EventVerdict::Send(wire) = admit(&fixture, "alice", &frame).await else {
            panic!("a global topic reaches a member");
        };
        let wire: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(wire["jsonrpc"], "2.0");
        assert_eq!(wire["method"], "event");
        assert_eq!(wire["params"], frame);
    }

    #[tokio::test]
    async fn an_unparsable_frame_is_forwarded_verbatim() {
        let fixture = fixture(false);
        let verdict = admit_event(&fixture.ctx, "alice", None, "not json".into()).await;
        assert_eq!(verdict, EventVerdict::Send("not json".into()));
    }

    /// v1 narrows by the topic of the frame AS SENT: the bare stream form and
    /// the wrapped `event` form are both read through `extract_topic_and_data`,
    /// and anything off the list — or unnameable — stays off the v1 wire.
    #[test]
    fn v1_topics_narrow_by_the_wire_frames_own_topic() {
        use crate::gateway::server::connection::{overflow_warning_frame, v1_event_admitted};
        let chunk = stream("stream.response_chunk", json!({"run_id": "r"}));
        assert!(v1_event_admitted(&chunk.to_string()));
        assert!(v1_event_admitted(&overflow_warning_frame(1, 1)));
        let reasoning = stream("stream.reasoning", json!({"run_id": "r"}));
        assert!(!v1_event_admitted(&reasoning.to_string()));
        let wrapped = json!({"jsonrpc": "2.0", "method": "event", "params": {"topic": "session.created", "data": {}}});
        assert!(!v1_event_admitted(&wrapped.to_string()));
        assert!(!v1_event_admitted("not json"));
    }

    #[tokio::test]
    async fn stream_frames_reach_the_session_owner_byte_for_byte_and_nobody_else() {
        let fixture = fixture(false);
        let key = SessionKey::main("s3-characterization");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("alice")),
            fixture.store.get_or_create(&key),
        )
        .await
        .unwrap();
        let accepted = stream(
            "stream.run_accepted",
            json!({
                "type": "run_accepted",
                "run_id": "r-1",
                "session_key": key.to_key_string(),
                "accepted_at": "t",
            }),
        );
        let chunk = stream(
            "stream.response_chunk",
            json!({"type": "response_chunk", "run_id": "r-1", "seq": 1, "delta": "hi", "content": "hi"}),
        );
        for frame in [&accepted, &chunk] {
            assert_eq!(
                admit(&fixture, "alice", frame).await,
                EventVerdict::Send(frame.to_string())
            );
            assert_eq!(admit(&fixture, "bob", frame).await, EventVerdict::Skip);
        }
        let unseeded = stream(
            "stream.response_chunk",
            json!({"type": "response_chunk", "run_id": "r-unknown", "seq": 1, "delta": "x"}),
        );
        assert_eq!(
            admit(&fixture, "alice", &unseeded).await,
            EventVerdict::Skip
        );
    }
}
