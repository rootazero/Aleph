//! Permission-based event filtering for WebSocket clients.
//!
//! `EventScopeGuard` prevents sensitive events (pairing, exec approval, etc.)
//! from reaching clients that lack the required permissions. Events whose topic
//! does not match any rule are considered *unguarded* and pass through freely.

/// Guards sensitive event topics behind permission checks.
///
/// Each rule is a `(topic_prefix, required_permissions)` pair. When a topic
/// matches a prefix (via `starts_with` or exact equality), the client must hold
/// **at least one** of the listed permissions. Topics that match no rule are
/// allowed unconditionally.
pub struct EventScopeGuard {
    rules: Vec<(String, Vec<String>)>,
}

impl EventScopeGuard {
    /// Create a guard with custom rules.
    #[must_use]
    pub const fn new(rules: Vec<(String, Vec<String>)>) -> Self {
        Self { rules }
    }

    /// Default rules for the Aleph Gateway.
    ///
    /// | Prefix | Required (any of) |
    /// |--------|-------------------|
    /// | `pairing.` | admin, pairing |
    /// | `guest.` | admin, guest.manager |
    /// | `config.changed` | admin, config.viewer |
    /// | `node.` | admin |
    /// | `pty.` | admin |
    /// | `runtime.` | admin |
    ///
    /// `pty.` is the delivery-side half of the `pty.` RPC gate, added for the
    /// same reason `node.` was and one round later. `pty.screen` (formerly
    /// `pty.output`, which carried the base64 of every byte the child process
    /// wrote) carries bounded per-frame diffs of the operator's rendered
    /// terminal — the screen content, not raw bytes, but exactly as sensitive
    /// — and `pty.exit` its status; the RPC face has been in
    /// [`ADMIN_PREFIXES`](crate::gateway::method_admin) all along, with the
    /// written reason that a PTY is a raw shell mediated by neither the command
    /// policy nor the exec tier. Gating only the RPC left the screen content
    /// itself on an unguarded topic — `session_identity_of` had no `pty.*` arm
    /// either at the time, so the frames fell to `_ => Global` and reached
    /// every connection. Requires `admin` alone, for the same reason `node.`
    /// does.
    ///
    /// `session_identity_of` now DOES have a `pty.*` arm
    /// ([`crate::gateway::event_visibility::SessionIdentity::ByPtySession`]),
    /// narrowing per-session ownership within the operators this rule admits
    /// — the same two-layer shape `approval.` uses (role here, ownership one
    /// filter term further down). This rule stays: ownership alone would
    /// still let a member who somehow held a permission scope subscribe raw
    /// shell output, and role alone would still cross-wire two operators'
    /// terminals — see `handlers::pty::require_owned`'s doc.
    ///
    /// `runtime.` is the delivery-side half of the `runtime.` RPC gate
    /// (`runtime.agents.list`'s prefix in `method_admin.rs`). Its one topic,
    /// `runtime.agents.changed`, carries NO session id or other payload
    /// (`json!({})` — clients re-fetch via the gated RPC), so unlike `pty.`
    /// it needs no `session_identity_of` arm to narrow per-session ownership
    /// within the operators this rule admits: role alone is the whole
    /// answer, because there is no per-row content on the wire to leak.
    ///
    /// `node.` is the delivery-side half of the `environments.` RPC gate
    /// (`method_admin.rs`). `node.connected` / `node.disconnected` carry the
    /// same node ids and names `environments.list` enumerates, so gating only
    /// the RPC would leave a member who hand-subscribes `node.**` reading the
    /// fleet live — the Panel's cluster page does exactly that subscribe. It
    /// requires `admin` alone: no fine-grained sibling permission is minted
    /// for it, because nothing would ever grant one ([`scope_for_role`] hands
    /// out `"*"` or nothing, and inventing a permission name with no producer
    /// is the zero-consumer abstraction R10 refuses).
    ///
    /// Cluster NODES are unaffected: a node connection never subscribes to a
    /// topic (its whole inbound surface is reverse-RPC over its own outbound
    /// channel, which does not pass through this guard).
    #[must_use]
    pub fn default_rules() -> Self {
        Self {
            rules: vec![
                // ⚠️ Every required-permission vector below is `["admin"]`, and
                // that is not a coincidence to be tidied later — it is the
                // whole model. [`scope_for_role`] is the ONLY producer of a
                // connection's permission vector and it hands out `["*"]` or
                // `[]`, so `is_superuser_scope` decides every verdict here and
                // the `permissions.iter().any(...)` clause below is unreachable
                // in production.
                //
                // Until 2026-08-09 five of these rules named fine-grained
                // siblings — `pairing`, `guest.manager`, `exec.approver`,
                // `config.viewer` — that nothing could ever grant, alongside
                // `"admin"`, which `scope_for_role` also never mints. The
                // ruling written for `node.` above ("inventing a permission
                // name with no producer is the zero-consumer abstraction R10
                // refuses") applied to all of them; it just had not been
                // finished. The names are gone.
                //
                // What they cost was not runtime: it was a reader concluding
                // that Aleph has per-user permission state and designing a
                // narrow grant against `exec.approver`. It does not. The
                // `users` table is `(user_id, display_name, role, status,
                // created_at)` — the role enum plus the project roster is the
                // entire authorization model, and the per-resource grant table
                // is a recorded non-goal (see FEATURE_LOCATOR §5.22).
                //
                // `"admin"` stays as the marker of INTENT — "this prefix is
                // operator-only" — and is pinned as the only admissible name by
                // `no_rule_names_a_permission_nothing_can_grant`.
                ("pairing.".to_string(), vec!["admin".to_string()]),
                ("guest.".to_string(), vec!["admin".to_string()]),
                // `approval.` deliberately has NO rule here since 2026-08-08.
                // This table keys on the topic PREFIX, and the approval family
                // carries two different kinds of frame under one prefix: a
                // tool-gate approval names the blocked session, a cluster-node
                // approval names none. Role is the wrong question for either: a
                // member must receive the card for their OWN parked tool call,
                // because `Auto` — the default tier — parks every non-idempotent
                // call, and the member is the principal now allowed to release
                // it (`method_admin`'s `exec.` carve-out). Ownership answers it
                // instead, one filter further down the same chain:
                // `event_visibility::session_identity_of` maps an empty
                // `session_key` to `OperatorOnly` and a real one to
                // `BySessionKey`/`BySessionKeyOrAdmin`, so a member gets their
                // own and an operator still gets everyone's — which is what a
                // rule here used to buy, unchanged. Do not re-add a rule here —
                // it would re-close the member half without the fleet half
                // noticing.
                //
                // `surface.approval` has no rule here either, since the banner
                // started carrying the session key it is derived from. It used
                // to be the exception on the grounds that it "carries no
                // session to scope by" — which was true, and was a property of
                // `r5_router::approval_for` dropping the field, not of the
                // banner. The consequence was the K shape one level up: the
                // person whose own call is parked received the decision card
                // and never the interrupt that exists to fetch them to it.
                ("config.changed".to_string(), vec!["admin".to_string()]),
                ("node.".to_string(), vec!["admin".to_string()]),
                ("pty.".to_string(), vec!["admin".to_string()]),
                ("runtime.".to_string(), vec!["admin".to_string()]),
            ],
        }
    }

    /// Check whether a client with the given permissions may receive an event
    /// published on `topic`.
    ///
    /// - If the topic matches a rule prefix, the client must hold at least one
    ///   of that rule's required permissions.
    /// - If no rule matches, the event is unguarded and allowed for everyone.
    #[must_use]
    pub fn can_receive(&self, topic: &str, permissions: &[String]) -> bool {
        for (prefix, required) in &self.rules {
            if topic.starts_with(prefix) || topic == prefix {
                // A device holding the `"*"` wildcard (operator / local daemon) is a
                // superuser and satisfies every scope rule. Otherwise it needs at
                // least one of the topic's required permissions.
                return is_superuser_scope(permissions)
                    || permissions.iter().any(|p| required.contains(p));
            }
        }
        // No rule matched — unguarded, allow for all.
        true
    }
}

/// Whether a connection's permission set is the superuser scope — the `"*"`
/// wildcard [`scope_for_role`] hands an operator (and the local daemon).
///
/// One derivation, two readers: [`EventScopeGuard::can_receive`]'s wildcard
/// arm, and the delivery loop's admin input to
/// [`crate::gateway::event_visibility::EventVisibilityIndex::event_admits`]
/// (the `BySessionKeyOrAdmin` arm that keeps an operator receiving a member's
/// approval card now that `approval.*` is scoped by ownership rather than by
/// role). Two spellings of "is this caller an admin" is exactly the drift the
/// approval plane cannot afford.
#[must_use]
pub fn is_superuser_scope(permissions: &[String]) -> bool {
    permissions.iter().any(|p| p == "*")
}

/// The event scope stamped onto a connection holding the resolved wire `role`.
///
/// **Single authority for the role → event-scope mapping.** Both writers call
/// it: the `connect` handshake (`server::handler`) and the live role re-stamp
/// (`handlers::users::restamp_live_connections`). Written twice, the two halves
/// drift — a demoted admin would keep the wildcard on his open tab until he
/// happened to reconnect, which is the exact indefinite window the re-stamp
/// exists to close on the `caller_role` axis.
///
/// - `"operator"` ⇒ the `"*"` wildcard. Loopback and every credential-authorized
///   admin keep byte-identical scope to before this function existed.
/// - anything else (`"member"`, `"guest"`) ⇒ no scopes.
///
/// An empty scope is **not** a blackout. [`EventScopeGuard::can_receive`] is
/// *default-allow*: only the prefixes named in
/// [`default_rules`](EventScopeGuard::default_rules) are guarded, and every
/// other topic — chat, session, `agent.run.*`, streaming deltas — passes for any
/// connection regardless of permissions. So a member's daily surfaces are
/// untouched, while the admin-guarded topics stop reaching him:
/// `config.changed`, `pairing.*`, `guest.*`,
/// `node.*` (cluster fleet topology — the live half of the admin-gated
/// `environments.list`) and `pty.*` (the operator's raw terminal bytes — the
/// live half of the admin-gated `pty.` family). Members used to be stamped
/// `"*"`, which short-circuits every rule — the login wall admits them, so they
/// were live connections receiving an admin's approval traffic.
///
/// The three `approval.*` frames were on that list until 2026-08-08 and are
/// deliberately no longer: they are gated per PAYLOAD in
/// [`event_visibility`](crate::gateway::event_visibility), because the family
/// carries both session-scoped and fleet-scoped frames under one prefix and a
/// prefix table can only answer for both at once. Answering "operator" for both
/// is what left a member unable to see the gate blocking their own run.
/// `surface.approval` — the R5 banner leg — followed them off this table once
/// it started carrying the session key it is derived from; it is the same
/// question about the same approval, and leaving it here would have kept the
/// interrupt away from precisely the person expected to answer it.
#[must_use]
pub fn scope_for_role(role: &str) -> Vec<String> {
    if role == "operator" {
        vec!["*".to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_visibility::{session_identity_of, SessionIdentity};
    use crate::gateway::source_census;

    #[test]
    fn test_unguarded_event_allowed_for_all() {
        let guard = EventScopeGuard::default_rules();

        // Random topics that match no rule should always be allowed.
        assert!(guard.can_receive("agent.started", &[]));
        assert!(guard.can_receive("chat.message", &["viewer".to_string()]));
        assert!(guard.can_receive("session.created", &["admin".to_string()]));
        assert!(guard.can_receive("random.topic.xyz", &[]));
    }

    #[test]
    fn test_pairing_event_requires_permission() {
        let guard = EventScopeGuard::default_rules();

        // No permissions — denied.
        assert!(!guard.can_receive("pairing.requested", &[]));

        // Irrelevant permission — denied.
        assert!(!guard.can_receive("pairing.requested", &["viewer".to_string()]));

        // "admin" — allowed. It is the rule's marker of intent; the value a
        // real connection carries is `"*"` (see `scope_for_role`).
        assert!(guard.can_receive("pairing.requested", &["admin".to_string()]));

        // The operator's actual permission vector.
        assert!(guard.can_receive("pairing.approved", &["*".to_string()]));

        // …and the fine-grained sibling this rule used to name is gone: nothing
        // could ever mint it, and leaving it here invited a narrow grant to be
        // designed against a permission system that does not exist.
        assert!(!guard.can_receive("pairing.approved", &["pairing".to_string()]));
    }

    /// Aleph has no per-user permission state: `users` is `(user_id,
    /// display_name, role, status, created_at)`, and [`scope_for_role`] — the
    /// only producer of a connection's permission vector — returns `["*"]` or
    /// `[]`. A rule naming anything else describes a grant system that does not
    /// exist, and the cost is a reader designing against it.
    #[test]
    fn no_rule_names_a_permission_nothing_can_grant() {
        let guard = EventScopeGuard::default_rules();
        for (prefix, required) in &guard.rules {
            for name in required {
                assert_eq!(
                    name, "admin",
                    "rule `{prefix}` requires `{name}`, which no producer can ever mint. \
                     `scope_for_role` hands out `\"*\"` or nothing; `\"admin\"` is the \
                     marker of intent. If a real grant axis is ever added, add its \
                     producer FIRST and then widen this test."
                );
            }
        }
    }

    /// The approval family's gate MOVED on 2026-08-08; it was not removed.
    ///
    /// This table keys on the topic prefix, and the family carries two kinds of
    /// frame under one prefix — a tool-gate approval that names the blocked
    /// session, and a cluster-node approval that names none. A prefix rule can
    /// only answer for both at once, and answering "operator" for both is what
    /// made a member's own gate invisible to them.
    ///
    /// So the assertion here is the inverse of what it used to be — this guard
    /// no longer decides approvals — **and it is paired with the assertion that
    /// somebody else now does**, so "moved" cannot silently decay into
    /// "deleted". The real decision is pinned in `event_visibility.rs`
    /// (`an_approval_is_scoped_by_its_session_and_fleet_approvals_are_operator_only`).
    #[test]
    fn the_approval_family_is_gated_by_payload_not_by_this_prefix_table() {
        let guard = EventScopeGuard::default_rules();

        // Topic names must be the ones a producer actually publishes —
        // `events::frame` emits `approval.requested` / `approval.resolved` /
        // `approval.expired`, which the `"approval."` prefix rule covers. This
        // test previously asserted against `exec.approval.pending` /
        // `exec.approval.result`, which no producer emits (the similar-looking
        // `exec.approval.resolve` and `exec.approvals.pending` are RPC method
        // names, not event topics). Those strings start with `exec.`, match no
        // rule, and are therefore unguarded — so the test failed the moment the
        // lib-test binary could build again. Guarding a fictional topic proves
        // nothing; assert the real ones.
        //
        // 2026-08-08: THIS guard no longer decides the raw `approval.*`
        // topics — ownership does, one filter further along the same chain
        // (`event_visibility`'s `BySessionKeyOrAdmin`). A member has to receive
        // the card for their OWN parked tool call, and role cannot express
        // "their own". So these three are unguarded HERE by design, and the
        // assertion that matters moved to
        // `event_visibility::tests::an_approval_frame_reaches_its_owner_and_every_admin_and_nobody_else`.
        for topic in [
            "approval.requested",
            "approval.resolved",
            "approval.expired",
        ] {
            assert!(
                guard.can_receive(topic, &[]),
                "{topic} must pass THIS term — the per-session decision it \
                 needs is made where the payload is visible"
            );
        }

        // …and the term that does decide is really wired: a fleet approval
        // (empty session key) is operator-only, a session approval is not.
        assert_eq!(
            session_identity_of(
                "approval.requested",
                Some(&serde_json::json!({"session_key": ""}))
            ),
            SessionIdentity::OperatorOnly,
            "the gate moved to event_visibility — if this fails, approvals are \
             gated NOWHERE, because the prefix rule above is gone"
        );

        // The banner leg followed the other three off this table once it began
        // carrying a session key. Same assertion, same file, opposite sign:
        // this guard must be silent about it, and `event_visibility` must not.
        assert!(
            guard.can_receive("surface.approval", &[]),
            "the prefix rule must be GONE — a member holds no admin permission \
             and would never see the banner for their own parked call"
        );
        assert_eq!(
            session_identity_of(
                "surface.approval",
                Some(&serde_json::json!({"session_key": "agent:main:s1"}))
            ),
            SessionIdentity::BySessionKeyOrAdmin("agent:main:s1".to_string()),
            "…and the ownership gate must have picked it up — if this fails, \
             the banner is gated NOWHERE"
        );
    }

    #[test]
    fn test_admin_has_access_to_all_guarded_events() {
        let guard = EventScopeGuard::default_rules();
        let admin = vec!["admin".to_string()];

        assert!(guard.can_receive("pairing.requested", &admin));
        assert!(guard.can_receive("guest.joined", &admin));
        assert!(guard.can_receive("exec.approval.pending", &admin));
        assert!(guard.can_receive("config.changed", &admin));
    }

    #[test]
    fn wildcard_permission_satisfies_guarded_topics() {
        let g = EventScopeGuard::default_rules();
        let star = vec!["*".to_string()];
        assert!(
            g.can_receive("approval.requested", &star),
            "operator [*] must receive approval events"
        );
        assert!(
            g.can_receive("pairing.requested", &star),
            "operator [*] must receive pairing events"
        );
        assert!(g.can_receive("config.changed", &star));
    }

    /// A chat-tier channel is still shut out of the approval plane — but by the
    /// filter that can actually see WHOSE approval it is.
    ///
    /// This test used to assert the exclusion here, and moving the raw topics
    /// off this guard would have quietly deleted it. It is not deleted: a
    /// chat-tier connection carries no `caller_user`, so
    /// `BySessionKeyOrAdmin` resolves nothing for it and denies — the same
    /// answer, from the filter that also gets a member's own card right.
    /// Pinned end-to-end in `event_visibility`'s approval test.
    #[test]
    fn chat_tier_excluded_from_the_banner_and_the_admin_families() {
        let g = EventScopeGuard::default_rules();
        let chat = vec!["chat".to_string(), "read".to_string()];
        assert!(
            !is_superuser_scope(&chat),
            "chat tier must not satisfy the admin arm that lets an operator \
             read another user's approval card"
        );
        assert!(!g.can_receive("config.changed", &chat));
        assert!(!g.can_receive("pty.screen", &chat));
        assert!(
            g.can_receive("agent.run.started", &chat),
            "unguarded topics still flow"
        );
    }

    /// Rewritten, not deleted: the banner is still shut out of a chat-tier
    /// connection, but by the filter that can see WHOSE banner it is.
    #[test]
    fn surface_approval_is_scoped_by_owner_not_by_role() {
        let g = EventScopeGuard::default_rules();
        let chat = vec!["chat".to_string(), "read".to_string()];

        // A chat-tier connection carries no `caller_user`, so
        // `BySessionKeyOrAdmin` resolves nothing for it, and it does not
        // satisfy the admin arm either — the same exclusion, one filter down.
        assert!(
            !is_superuser_scope(&chat),
            "chat tier must not satisfy the admin arm of the banner's owner check"
        );
        assert_eq!(
            session_identity_of("surface.approval", Some(&serde_json::json!({}))),
            SessionIdentity::OperatorOnly,
            "a banner with no session key is a FLEET approval and stays the \
             operator's — fail closed, exactly as its three siblings do"
        );

        // surface.notify stays unguarded (R5 to any desktop, not approval).
        assert!(
            g.can_receive("surface.notify", &chat),
            "surface.notify must stay unguarded"
        );
    }

    /// The operator half of the role → scope mapping is a zero-change
    /// guarantee: loopback and every credential-authorized admin are stamped
    /// the same `["*"]` they were stamped before `scope_for_role` existed.
    #[test]
    fn operator_scope_is_the_unchanged_wildcard() {
        assert_eq!(scope_for_role("operator"), vec!["*".to_string()]);

        let g = EventScopeGuard::default_rules();
        let op = scope_for_role("operator");
        for topic in [
            "approval.requested",
            "approval.resolved",
            "surface.approval",
            "config.changed",
            "pairing.requested",
            "guest.joined",
            "agent.run.started",
            "session.created",
        ] {
            assert!(
                g.can_receive(topic, &op),
                "operator must still receive {topic}"
            );
        }
    }

    /// A member is a *logged-in* principal — the login wall admits him and he
    /// holds a live socket — so his scope is what decides whether an admin's
    /// approval cards land on his screen. He used to be stamped `"*"`, which
    /// short-circuits every rule in `can_receive`.
    #[test]
    fn member_scope_excludes_admin_guarded_topics() {
        let g = EventScopeGuard::default_rules();
        let member = scope_for_role("member");
        assert!(member.is_empty(), "a member holds no event scopes");

        // The R5 approval BANNER followed the three `approval.*` frames off
        // this table (2026-08-09) once it started carrying the session key it
        // is derived from — "this leg carries no session" was a property of
        // `r5_router::approval_for` dropping the field, not of the banner. The
        // protection did not disappear, it moved: asserted here so removing
        // the topic above cannot silently mean removing the guarantee.
        assert_eq!(
            session_identity_of(
                "surface.approval",
                Some(&serde_json::json!({"session_key": "agent:main:s1"}))
            ),
            SessionIdentity::BySessionKeyOrAdmin("agent:main:s1".to_string()),
            "the banner must be owner-scoped now — a member gets their own and \
             no one else's, and `member` holds no scope satisfying the admin arm"
        );
        assert!(!is_superuser_scope(&member));

        for topic in [
            "config.changed",
            "pairing.requested",
            "pairing.approved",
            "guest.joined",
            // Cluster fleet topology: node ids + names, the live half of the
            // admin-gated `environments.list`.
            "node.connected",
            "node.disconnected",
            // The operator's rendered terminal content, the live half of the
            // admin-gated `pty.*` RPC family.
            "pty.screen",
            "pty.exit",
        ] {
            assert!(
                !g.can_receive(topic, &member),
                "a member must NOT receive the admin-guarded topic {topic}"
            );
        }
    }

    /// SOURCE-level pin: every `node.*` topic the center actually publishes
    /// must be refused to a member.
    ///
    /// The fleet events are raw `TopicEvent`s with no `GatewayEventFrame`
    /// variant behind them, so nothing in this crate breaks if the `node.`
    /// rule is deleted — the topics simply fall through `can_receive`'s
    /// default-allow tail and every logged-in member starts receiving live
    /// cluster topology again, silently. Reading the producer's source text is
    /// the only thing that is not blind to that, and it is deliberately NOT a
    /// suffix whitelist: a `node.evicted` added tomorrow is covered by the
    /// prefix rule and by this scan without anyone editing a list here.
    #[test]
    fn every_node_topic_the_center_publishes_is_refused_to_a_member() {
        // Production half only: handler.rs's own test module publishes topics
        // that no client ever sees.
        let production = source_census::production_prefix(include_str!("server/handler.rs"));
        let topics = source_census::topic_event_literals(&production);
        assert!(
            !topics.is_empty(),
            "the scanner matched no `TopicEvent::new(\"…\"` call in \
             server/handler.rs — the call shape changed and this pin has \
             quietly become vacuous"
        );

        let g = EventScopeGuard::default_rules();
        let member = scope_for_role("member");
        let operator = scope_for_role("operator");
        let mut node_topics = 0usize;
        for topic in &topics {
            if !topic.starts_with("node.") {
                continue;
            }
            node_topics += 1;
            assert!(
                !g.can_receive(topic, &member),
                "server/handler.rs publishes `{topic}`, which a member can \
                 still receive — cluster topology is admin-only on both faces"
            );
            assert!(
                g.can_receive(topic, &operator),
                "an operator must still receive `{topic}` — the Panel's fleet \
                 list is driven by it"
            );
        }
        assert!(
            node_topics >= 2,
            "only {node_topics} `node.*` topic(s) scraped; handler.rs publishes \
             node.connected and node.disconnected, so the scanner is missing some"
        );
    }

    /// The fleet has two faces and they must agree. `environments.list` (RPC)
    /// and `node.*` (events) enumerate the same node ids and names, so gating
    /// one and not the other is a hole with a gate standing next to it — which
    /// is exactly the state this pin was written to end.
    #[test]
    fn the_fleet_is_gated_on_both_its_rpc_and_its_event_face() {
        assert!(
            crate::gateway::method_admin::method_requires_admin("environments.list"),
            "the fleet's RPC face must be admin-gated"
        );
        let g = EventScopeGuard::default_rules();
        assert!(
            !g.can_receive("node.connected", &scope_for_role("member")),
            "the fleet's event face must be admin-gated too — otherwise the \
             RPC gate just moves the disclosure onto the event bus"
        );
    }

    /// The twin of the fleet pin, for the surface with the highest-value
    /// payload in the repo: a PTY is a raw shell, mediated by neither the
    /// command policy nor the exec tier — which is the reason `method_admin`
    /// itself gives for gating `pty.`. `pty.screen` is a bounded per-frame diff
    /// of the operator's rendered terminal. Gating one face and not the other
    /// does not reduce the disclosure, it relocates it onto the event bus, and
    /// the event bus is the quieter of the two (a withheld frame raises no
    /// error, an unwanted one raises no alarm).
    #[test]
    fn the_terminal_is_gated_on_both_its_rpc_and_its_event_face() {
        assert!(
            crate::gateway::method_admin::method_requires_admin("pty.spawn"),
            "the terminal's RPC face must be admin-gated"
        );
        let g = EventScopeGuard::default_rules();
        for topic in ["pty.screen", "pty.exit"] {
            assert!(
                !g.can_receive(topic, &scope_for_role("member")),
                "{topic} carries the operator's live terminal content and \
                 must be admin-gated on the event face too"
            );
        }
        // The operator still receives — the half that fails silently.
        assert!(g.can_receive("pty.screen", &scope_for_role("operator")));
    }

    /// The agent panel's twin of the same pin: `runtime.agents.list` (RPC)
    /// and `runtime.agents.changed` (event) are two faces of the same PTY
    /// session data `pty.*` already gates. Gating the RPC and leaving the
    /// event unguarded would not reduce the disclosure, it would relocate it
    /// — a member could not list the table but could still watch it change.
    #[test]
    fn the_agent_panel_is_gated_on_both_its_rpc_and_its_event_face() {
        assert!(
            crate::gateway::method_admin::method_requires_admin("runtime.agents.list"),
            "the agent panel's RPC face must be admin-gated"
        );
        let g = EventScopeGuard::default_rules();
        // Via the protocol constant, not a re-typed literal (fix round 1,
        // review Minor 6) — a rename in the protocol crate must redden this
        // gate test, not silently keep testing a string nothing publishes.
        let topic = aleph_protocol::runtime::RUNTIME_AGENTS_CHANGED_TOPIC;
        assert!(
            !g.can_receive(topic, &scope_for_role("member")),
            "{topic} must be admin-gated on the event face too — \
             gating only the RPC just relocates the disclosure onto the event bus"
        );
        // The operator still receives — the half that fails silently.
        assert!(g.can_receive(topic, &scope_for_role("operator")));
    }

    /// Prefix hygiene, mirroring `method_admin`'s
    /// `prefix_match_requires_the_trailing_dot`: the guarded prefix is
    /// `node.`, so a topic that merely starts with those four letters stays
    /// unguarded. Pinned because widening it would black out an unrelated
    /// family for every member with no error anywhere.
    #[test]
    fn the_node_rule_matches_only_the_dotted_prefix() {
        let g = EventScopeGuard::default_rules();
        let member = scope_for_role("member");
        for topic in ["nodes.listed", "nodeapp.started", "node"] {
            assert!(
                g.can_receive(topic, &member),
                "{topic} matches no rule and must stay unguarded"
            );
        }
    }

    /// The other half of the same fix: narrowing a member's scope must not
    /// black out his daily surfaces. `can_receive` is default-allow, so every
    /// topic that matches no rule still flows on an empty scope.
    #[test]
    fn member_still_receives_ordinary_session_and_chat_topics() {
        let g = EventScopeGuard::default_rules();
        let member = scope_for_role("member");

        for topic in [
            "agent.started",
            "agent.run.started",
            "chat.message",
            "session.created",
            "surface.notify",
        ] {
            assert!(
                g.can_receive(topic, &member),
                "unguarded topic {topic} must still reach a member"
            );
        }
    }

    /// Guest (walled) resolves through the same arm as member — no scopes.
    /// Pinned so a future carve-out for one cannot silently widen the other.
    #[test]
    fn guest_and_unknown_roles_hold_no_scope() {
        assert!(scope_for_role("guest").is_empty());
        assert!(scope_for_role("").is_empty());
        assert!(
            scope_for_role("admin").is_empty(),
            "the wire word is `operator`, not the store's `admin`"
        );
    }
}
