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
    /// | `surface.approval` | admin, exec.approver |
    #[must_use]
    pub fn default_rules() -> Self {
        Self {
            rules: vec![
                (
                    "pairing.".to_string(),
                    vec!["admin".to_string(), "pairing".to_string()],
                ),
                (
                    "guest.".to_string(),
                    vec!["admin".to_string(), "guest.manager".to_string()],
                ),
                (
                    "approval.".to_string(),
                    vec!["admin".to_string(), "exec.approver".to_string()],
                ),
                (
                    "surface.approval".to_string(),
                    vec!["admin".to_string(), "exec.approver".to_string()],
                ),
                (
                    "config.changed".to_string(),
                    vec!["admin".to_string(), "config.viewer".to_string()],
                ),
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
                return permissions.iter().any(|p| p == "*" || required.contains(p));
            }
        }
        // No rule matched — unguarded, allow for all.
        true
    }
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
/// untouched, while the admin-guarded topics stop reaching him: `approval.*`
/// (exec approval cards, **including the command text being approved**),
/// `surface.approval`, `config.changed`, `pairing.*` and `guest.*`. Members used
/// to be stamped `"*"`, which short-circuits every rule — the login wall admits
/// them, so they were live connections receiving an admin's approval traffic.
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

        // "admin" — allowed.
        assert!(guard.can_receive("pairing.requested", &["admin".to_string()]));

        // "pairing" — allowed.
        assert!(guard.can_receive("pairing.approved", &["pairing".to_string()]));
    }

    #[test]
    fn test_approval_events_require_permission() {
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
        assert!(!guard.can_receive("approval.requested", &[]));

        // Irrelevant permission — denied.
        assert!(!guard.can_receive("approval.requested", &["viewer".to_string()]));

        // "exec.approver" — allowed.
        assert!(guard.can_receive("approval.requested", &["exec.approver".to_string()]));

        // "admin" — allowed.
        assert!(guard.can_receive("approval.resolved", &["admin".to_string()]));
        assert!(guard.can_receive("approval.expired", &["admin".to_string()]));
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

    #[test]
    fn chat_tier_excluded_from_approval_events() {
        let g = EventScopeGuard::default_rules();
        let chat = vec!["chat".to_string(), "read".to_string()];
        assert!(
            !g.can_receive("approval.requested", &chat),
            "chat tier must NOT see approval requests"
        );
        assert!(!g.can_receive("approval.resolved", &chat));
        assert!(
            g.can_receive("agent.run.started", &chat),
            "unguarded topics still flow"
        );
    }

    #[test]
    fn surface_approval_is_operator_gated() {
        let g = EventScopeGuard::default_rules();

        // chat / guest tier and no-perms must NOT receive approval banners.
        let chat = vec!["chat".to_string(), "read".to_string()];
        assert!(
            !g.can_receive("surface.approval", &chat),
            "chat tier must NOT see surface.approval banners"
        );
        assert!(!g.can_receive("surface.approval", &[]));
        assert!(!g.can_receive("surface.approval", &["viewer".to_string()]));

        // operator [*] / exec.approver / admin must.
        assert!(g.can_receive("surface.approval", &["*".to_string()]));
        assert!(g.can_receive("surface.approval", &["exec.approver".to_string()]));
        assert!(g.can_receive("surface.approval", &["admin".to_string()]));

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

        for topic in [
            // Exec approval cards carry the command text being approved.
            "approval.requested",
            "approval.resolved",
            "approval.expired",
            "surface.approval",
            "config.changed",
            "pairing.requested",
            "pairing.approved",
            "guest.joined",
        ] {
            assert!(
                !g.can_receive(topic, &member),
                "a member must NOT receive the admin-guarded topic {topic}"
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
