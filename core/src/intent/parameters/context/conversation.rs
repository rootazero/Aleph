//! ConversationContext - Multi-turn conversation context

use std::collections::HashMap;

use super::PendingParam;

/// Multi-turn conversation context
///
/// Tracks the state of an ongoing conversation including pending parameters,
/// recent intents, and turn count for context-aware routing decisions.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    /// Pending parameters from previous turn
    pub pending_params: HashMap<String, PendingParam>,

    /// Recent intents in this session (most recent first)
    pub recent_intents: Vec<String>,

    /// Number of turns in current session
    pub turn_count: u32,
}

impl ConversationContext {
    /// Create a new empty conversation context
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an intent
    pub fn record_intent(&mut self, intent_type: impl Into<String>) {
        self.recent_intents.insert(0, intent_type.into());
        // Keep only last 10 intents
        if self.recent_intents.len() > 10 {
            self.recent_intents.truncate(10);
        }
    }

    /// Add a pending parameter
    pub fn add_pending_param(&mut self, param: PendingParam) {
        self.pending_params.insert(param.name.clone(), param);
    }

    /// Clear pending parameters
    pub fn clear_pending_params(&mut self) {
        self.pending_params.clear();
    }

    /// Remove a specific pending parameter
    pub fn resolve_pending_param(&mut self, param_name: &str) -> Option<PendingParam> {
        self.pending_params.remove(param_name)
    }

    /// Get the most recent intent
    pub fn last_intent(&self) -> Option<&str> {
        self.recent_intents.first().map(|s| s.as_str())
    }

    /// Check if a specific intent was used recently
    pub fn has_recent_intent(&self, intent: &str, within_turns: usize) -> bool {
        self.recent_intents
            .iter()
            .take(within_turns)
            .any(|i| i == intent)
    }

    /// Increment turn count
    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
    }

    /// Get pending parameter for a specific intent
    pub fn get_pending_for_intent(&self, intent: &str) -> Option<&PendingParam> {
        self.pending_params
            .values()
            .find(|p| p.required_for == intent)
    }

    /// Check if there are any non-expired pending params
    pub fn has_active_pending_params(&self) -> bool {
        self.pending_params.values().any(|p| !p.is_expired())
    }
}
