//! Shared agent visual identity for chat surfaces (bubbles, sidebar clusters,
//! @-mention palette). Resolves an agent_id to a display name, an avatar glyph
//! (emoji or monogram fallback), and a stable color hashed from the id.
//!
//! Pure functions only — host-testable, no Leptos signals or DOM.

use crate::api::agents::AgentSummary;
use std::collections::HashMap;

/// 6-slot palette shared with the roster rail. Slot chosen by id hash so a
/// given agent keeps its color regardless of roster order.
const PALETTE: [&str; 6] = ["#7c9cff", "#4ec9b0", "#e0a458", "#c586c0", "#4fc1ff", "#d16969"];

/// Stable color for an agent, hashed from its id (FNV-1a 32-bit). Deterministic
/// across sessions, independent of roster membership/order.
#[must_use]
pub fn agent_color_for_id(agent_id: &str) -> &'static str {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in agent_id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

/// Telegram-style grouping: show the avatar + name header only when this
/// message starts a new run of the same agent. `prev` is the agent_id of the
/// previously rendered message (None for the first, or a non-team message).
#[must_use]
pub fn should_show_attribution(prev: Option<&str>, this: Option<&str>) -> bool {
    match this {
        None => false,            // own / single-agent message: never a team header
        Some(id) => prev != Some(id),
    }
}

/// Resolved visual identity for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentityView {
    pub name: String,
    /// Avatar glyph: the agent's emoji, or a 1-char monogram fallback.
    pub avatar: String,
    pub color: &'static str,
}

/// Resolve identity from an id→summary map (built from `agents.list`). Falls
/// back gracefully: name→id, emoji→monogram(first char of name/id), color always.
#[must_use]
pub fn agent_identity(agent_id: &str, agents: &HashMap<String, AgentSummary>) -> AgentIdentityView {
    let summary = agents.get(agent_id);
    let name = summary
        .and_then(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| agent_id.to_string());
    let avatar = summary
        .and_then(|s| s.emoji.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| monogram(&name));
    AgentIdentityView { name, avatar, color: agent_color_for_id(agent_id) }
}

/// First character of `source`, uppercased, as a monogram avatar. Empty → "?".
fn monogram(source: &str) -> String {
    source
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(id: &str, name: Option<&str>, emoji: Option<&str>) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: name.map(String::from),
            emoji: emoji.map(String::from),
            description: None,
            model: None,
            is_default: false,
        }
    }

    #[test]
    fn color_is_stable_per_id_and_in_palette() {
        assert_eq!(agent_color_for_id("risk_analyst"), agent_color_for_id("risk_analyst"));
        assert!(PALETTE.contains(&agent_color_for_id("anything")));
    }

    #[test]
    fn resolves_name_and_emoji_when_present() {
        let mut m = HashMap::new();
        m.insert("a".to_string(), sum("a", Some("风险分析师"), Some("🛡️")));
        let id = agent_identity("a", &m);
        assert_eq!(id.name, "风险分析师");
        assert_eq!(id.avatar, "🛡️");
    }

    #[test]
    fn falls_back_to_id_and_monogram_for_unknown_agent() {
        let m = HashMap::new();
        let id = agent_identity("growth_analyst", &m);
        assert_eq!(id.name, "growth_analyst");
        assert_eq!(id.avatar, "G");
    }

    #[test]
    fn monogram_uses_name_first_char_when_no_emoji() {
        let mut m = HashMap::new();
        m.insert("x".to_string(), sum("x", Some("alice"), None));
        assert_eq!(agent_identity("x", &m).avatar, "A");
    }

    #[test]
    fn attribution_shows_on_agent_change_and_hides_on_repeat() {
        assert!(should_show_attribution(None, Some("a")));
        assert!(should_show_attribution(Some("b"), Some("a")));
        assert!(!should_show_attribution(Some("a"), Some("a")));
        assert!(!should_show_attribution(Some("a"), None));
    }
}
