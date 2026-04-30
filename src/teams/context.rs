//! Inbox-aware context injection for agents.
//!
//! Provides [`InboxContext`] — a lightweight summary of an agent's unread
//! messages across all teams — and the [`InboxContextProvider`] trait that
//! the [`ContextInjector`](crate::agents::swarm::context_injector::ContextInjector)
//! uses to append inbox awareness to the agent's system prompt.

use async_trait::async_trait;

use crate::sync_primitives::Arc;
use crate::teams::messages::Inbox;
use crate::teams::sessions::SessionStore;
use crate::teams::store::TeamStore;

// ---------------------------------------------------------------------------
// InboxContext
// ---------------------------------------------------------------------------

/// Lightweight summary of an agent's unread inbox state.
#[derive(Debug, Clone, Default)]
pub struct InboxContext {
    /// Number of unread messages where the agent is a direct recipient (To).
    pub unread_to: u32,
    /// Number of unread informational messages (Cc).
    pub unread_cc: u32,
    /// One-line summary of urgent items (empty when nothing urgent).
    pub urgent_summary: String,
    /// IDs of active collaborative sessions the agent participates in.
    pub active_sessions: Vec<String>,
}

impl InboxContext {
    /// Format the inbox state as a short text block suitable for injection
    /// into an agent's system prompt. Returns `None` when the inbox is empty
    /// and there are no active sessions.
    pub fn to_injection_text(&self) -> Option<String> {
        if self.unread_to == 0 && self.unread_cc == 0 && self.active_sessions.is_empty() {
            return None;
        }

        let mut parts = Vec::new();
        if self.unread_to > 0 {
            parts.push(format!(
                "{} unread messages requiring your action",
                self.unread_to
            ));
        }
        if self.unread_cc > 0 {
            parts.push(format!("{} informational messages (cc)", self.unread_cc));
        }
        if !self.urgent_summary.is_empty() {
            parts.push(self.urgent_summary.clone());
        }

        let mut text = format!(
            "[Team Inbox] {}\nUse inbox_read to view details.",
            parts.join("; ")
        );

        if !self.active_sessions.is_empty() {
            text.push_str(&format!(
                "\n[Active Sessions] {} session(s): {}. Use session_read to view.",
                self.active_sessions.len(),
                self.active_sessions.join(", ")
            ));
        }

        Some(text)
    }
}

// ---------------------------------------------------------------------------
// InboxContextProvider trait
// ---------------------------------------------------------------------------

/// Provides inbox context for an agent, aggregated across all teams.
#[async_trait]
pub trait InboxContextProvider: Send + Sync {
    /// Return a summary of the agent's unread inbox state.
    async fn get_inbox_context(&self, agent_id: &str) -> InboxContext;
}

// ---------------------------------------------------------------------------
// TeamInboxContextProvider
// ---------------------------------------------------------------------------

/// Concrete provider that queries [`Inbox`] + [`TeamStore`] + [`SessionStore`]
/// to build an aggregated [`InboxContext`] across every team the agent belongs to.
pub struct TeamInboxContextProvider {
    inbox: Arc<Inbox>,
    team_store: Arc<dyn TeamStore>,
    session_store: Option<Arc<dyn SessionStore>>,
}

impl TeamInboxContextProvider {
    pub fn new(inbox: Arc<Inbox>, team_store: Arc<dyn TeamStore>) -> Self {
        Self {
            inbox,
            team_store,
            session_store: None,
        }
    }

    /// Set an optional session store for active-session awareness.
    pub fn with_session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }
}

#[async_trait]
impl InboxContextProvider for TeamInboxContextProvider {
    async fn get_inbox_context(&self, agent_id: &str) -> InboxContext {
        // Find every team the agent belongs to, then sum unread counts.
        let teams = match self.team_store.get_agent_teams(agent_id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "Failed to get agent teams for inbox context");
                return InboxContext::default();
            }
        };

        let mut total_to: u32 = 0;
        let mut total_cc: u32 = 0;
        let mut active_sessions = Vec::new();

        for team in &teams {
            match self.inbox.get_unread_counts(agent_id, &team.id).await {
                Ok((to, cc)) => {
                    total_to = total_to.saturating_add(to);
                    total_cc = total_cc.saturating_add(cc);
                }
                Err(e) => {
                    tracing::debug!(agent_id, team_id = %team.id, error = %e, "Failed to get unread counts");
                }
            }

            // Collect active sessions for this team
            if let Some(ref ss) = self.session_store {
                match ss.list_active_sessions(&team.id).await {
                    Ok(sessions) => {
                        for s in sessions {
                            if s.participants.contains(&agent_id.to_string()) {
                                active_sessions.push(s.id);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(team_id = %team.id, error = %e, "Failed to list active sessions");
                    }
                }
            }
        }

        InboxContext {
            unread_to: total_to,
            unread_cc: total_cc,
            urgent_summary: String::new(),
            active_sessions,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_inbox_produces_no_text() {
        let ctx = InboxContext::default();
        assert!(ctx.to_injection_text().is_none());
    }

    #[test]
    fn test_to_only() {
        let ctx = InboxContext {
            unread_to: 3,
            unread_cc: 0,
            urgent_summary: String::new(),
            active_sessions: vec![],
        };
        let text = ctx.to_injection_text().unwrap();
        assert!(text.contains("3 unread messages requiring your action"));
        assert!(text.contains("[Team Inbox]"));
        assert!(text.contains("inbox_read"));
        assert!(!text.contains("informational"));
    }

    #[test]
    fn test_cc_only() {
        let ctx = InboxContext {
            unread_to: 0,
            unread_cc: 5,
            urgent_summary: String::new(),
            active_sessions: vec![],
        };
        let text = ctx.to_injection_text().unwrap();
        assert!(text.contains("5 informational messages (cc)"));
    }

    #[test]
    fn test_both_with_urgent() {
        let ctx = InboxContext {
            unread_to: 2,
            unread_cc: 1,
            urgent_summary: "Review request from agent-lead".into(),
            active_sessions: vec![],
        };
        let text = ctx.to_injection_text().unwrap();
        assert!(text.contains("2 unread messages"));
        assert!(text.contains("1 informational"));
        assert!(text.contains("Review request from agent-lead"));
    }
}
