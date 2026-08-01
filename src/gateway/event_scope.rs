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
    fn test_exec_approval_requires_permission() {
        let guard = EventScopeGuard::default_rules();

        // No permissions — denied.
        assert!(!guard.can_receive("exec.approval.pending", &[]));

        // "exec.approver" — allowed.
        assert!(guard.can_receive("exec.approval.pending", &["exec.approver".to_string()]));

        // "admin" — allowed.
        assert!(guard.can_receive("exec.approval.result", &["admin".to_string()]));
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
}
