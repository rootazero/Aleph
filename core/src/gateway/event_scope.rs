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
    pub fn new(rules: Vec<(String, Vec<String>)>) -> Self {
        Self { rules }
    }

    /// Default rules for the Aleph Gateway.
    ///
    /// | Prefix | Required (any of) |
    /// |--------|-------------------|
    /// | `pairing.` | admin, pairing |
    /// | `poe.sign.` | admin, poe.approver |
    /// | `guest.` | admin, guest.manager |
    /// | `exec.approval.` | admin, exec.approver |
    /// | `config.changed` | admin, config.viewer |
    pub fn default_rules() -> Self {
        Self {
            rules: vec![
                (
                    "pairing.".to_string(),
                    vec!["admin".to_string(), "pairing".to_string()],
                ),
                (
                    "poe.sign.".to_string(),
                    vec!["admin".to_string(), "poe.approver".to_string()],
                ),
                (
                    "guest.".to_string(),
                    vec!["admin".to_string(), "guest.manager".to_string()],
                ),
                (
                    "exec.approval.".to_string(),
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
    pub fn can_receive(&self, topic: &str, permissions: &[String]) -> bool {
        for (prefix, required) in &self.rules {
            if topic.starts_with(prefix) || topic == prefix {
                // Topic is guarded — client needs at least one required perm.
                return permissions.iter().any(|p| required.contains(p));
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
        assert!(guard.can_receive("poe.sign.request", &admin));
        assert!(guard.can_receive("guest.joined", &admin));
        assert!(guard.can_receive("exec.approval.pending", &admin));
        assert!(guard.can_receive("config.changed", &admin));
    }
}
