//! Routing result types for conversation-aware routing.
//!
//! This module defines the result types returned by ConversationAwareRouter,
//! which include both the routing decision and conversation context.

use crate::router::RoutingMatch;
use crate::semantic::MatchResult;

/// Result of conversation-aware routing
///
/// Contains the routing decision along with conversation context
/// that should be included in the AI prompt.
#[derive(Debug, Clone, Default)]
pub struct RoutingResult {
    /// Core routing match (command/keyword rules)
    pub routing_match: RoutingMatch,
    /// Conversation history context (formatted for AI prompt)
    pub conversation_context: Option<String>,
    /// Whether this is a continuation of an existing conversation
    pub is_continuation: bool,
    /// Current turn number in the conversation
    pub turn_number: u32,
    /// Semantic match result (if semantic matching is enabled)
    pub semantic_match: Option<MatchResult>,
}

impl RoutingResult {
    /// Create an empty routing result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if any routing rule was matched
    pub fn has_match(&self) -> bool {
        self.routing_match.has_match()
    }

    /// Get the provider name from the routing match
    pub fn provider_name(&self) -> Option<&str> {
        self.routing_match.provider_name()
    }

    /// Get the cleaned input (command prefix stripped)
    pub fn cleaned_input(&self) -> Option<&str> {
        self.routing_match.cleaned_input()
    }

    /// Assemble the final system prompt
    ///
    /// This combines:
    /// 1. Rule-based prompts from command/keyword matching
    /// 2. Conversation history context
    ///
    /// # Returns
    /// The assembled system prompt, or None if no prompts are available.
    pub fn assemble_prompt(&self) -> Option<String> {
        let rule_prompt = self.routing_match.assemble_prompt();

        match (&rule_prompt, &self.conversation_context) {
            (Some(rp), Some(cc)) => {
                // Combine rule prompt with conversation context
                Some(format!("{}\n\n{}", rp, cc))
            }
            (Some(rp), None) => Some(rp.clone()),
            (None, Some(cc)) => Some(cc.clone()),
            (None, None) => None,
        }
    }

    /// Get only the rule-based system prompt (without conversation context)
    pub fn rule_prompt(&self) -> Option<String> {
        self.routing_match.assemble_prompt()
    }

    /// Get capabilities from the matched rule
    pub fn get_capabilities(&self) -> Vec<crate::payload::Capability> {
        self.routing_match.get_capabilities()
    }

    /// Check if this result includes conversation context
    pub fn has_conversation_context(&self) -> bool {
        self.conversation_context.is_some() && self.is_continuation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_result() {
        let result = RoutingResult::empty();
        assert!(!result.has_match());
        assert!(!result.is_continuation);
        assert_eq!(result.turn_number, 0);
    }

    #[test]
    fn test_has_conversation_context() {
        let mut result = RoutingResult::empty();

        // No context
        assert!(!result.has_conversation_context());

        // Context but not continuation
        result.conversation_context = Some("Previous: Hi".to_string());
        result.is_continuation = false;
        assert!(!result.has_conversation_context());

        // Context and continuation
        result.is_continuation = true;
        assert!(result.has_conversation_context());
    }
}
