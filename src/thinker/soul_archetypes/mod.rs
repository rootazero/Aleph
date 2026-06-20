//! Soul archetypes — built-in persona bases composed into each agent's SOUL.md.
//!
//! Three-layer model: Base (universal) + Archetype (1 of 4) + Personalization
//! (per-agent, authored by the creation interview). Templates are embedded at
//! compile time so the precise wording is never paraphrased.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Universal operating-identity layer shared by every agent.
pub const SOUL_BASE: &str = include_str!("templates/base.md");

/// Built-in persona archetype selected at agent creation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SoulArchetype {
    /// Analysis / research / decisions — rigorous, argues, tags claims.
    Expert,
    /// Support / journaling / presence — warm, listens.
    Companion,
    /// General getting-things-done — fast, answer-first.
    #[default]
    Assistant,
    /// Coding / building / automation — action-biased, surgical, verifies.
    Maker,
}

impl SoulArchetype {
    /// Verbatim archetype template (embedded at compile time).
    #[must_use]
    pub fn template(self) -> &'static str {
        match self {
            Self::Expert => include_str!("templates/expert.md"),
            Self::Companion => include_str!("templates/companion.md"),
            Self::Assistant => include_str!("templates/assistant.md"),
            Self::Maker => include_str!("templates/maker.md"),
        }
    }

    /// One-line catalog blurb used by the creation interview protocol.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Expert => {
                "analysis, research, decisions — rigorous, argues the counter-case, tags claims and confidence"
            }
            Self::Companion => {
                "support, journaling, presence — warm, listens, does not rush to fix"
            }
            Self::Assistant => "general getting-things-done — fast, answer-first, low-friction",
            Self::Maker => {
                "writing code, building, automation — action-biased, surgical, plans then verifies"
            }
        }
    }
}

/// Compose a full SOUL.md from Base + Archetype + optional personalization.
#[must_use]
pub fn compose_soul(
    archetype: SoulArchetype,
    agent_name: &str,
    personalization: Option<&str>,
) -> String {
    let mut out = format!(
        "_You are {agent_name}._\n\n{base}\n\n---\n\n{archetype}",
        base = SOUL_BASE.trim(),
        archetype = archetype.template().trim(),
    );
    if let Some(p) = personalization {
        let p = p.trim();
        if !p.is_empty() {
            out.push_str("\n\n---\n\n## This Agent\n\n");
            out.push_str(p);
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archetype_serde_is_lowercase_and_defaults_assistant() {
        assert_eq!(
            serde_json::to_string(&SoulArchetype::Expert).unwrap(),
            "\"expert\""
        );
        let m: SoulArchetype = serde_json::from_str("\"maker\"").unwrap();
        assert_eq!(m, SoulArchetype::Maker);
        assert_eq!(SoulArchetype::default(), SoulArchetype::Assistant);
    }

    #[test]
    fn templates_are_nonempty_and_distinct() {
        let all = [
            SoulArchetype::Expert,
            SoulArchetype::Companion,
            SoulArchetype::Assistant,
            SoulArchetype::Maker,
        ];
        for a in all {
            assert!(!a.template().trim().is_empty());
            assert!(!a.summary().trim().is_empty());
        }
        assert_ne!(
            SoulArchetype::Expert.template(),
            SoulArchetype::Maker.template()
        );
    }

    #[test]
    fn compose_without_personalization_has_base_and_archetype_only() {
        let soul = compose_soul(SoulArchetype::Expert, "Quant", None);
        assert!(soul.contains("_You are Quant._"));
        assert!(soul.contains("Never fabricate facts, citations")); // base honesty floor
        assert!(soul.contains("Accuracy beats approval.")); // expert marker
        assert!(!soul.contains("## This Agent"));
    }

    #[test]
    fn compose_with_personalization_appends_section() {
        let soul = compose_soul(
            SoulArchetype::Assistant,
            "Helper",
            Some("Focus: inbox triage. Hard boundary: never auto-send."),
        );
        assert!(soul.contains("Lead with the answer or the action.")); // assistant marker
        assert!(soul.contains("## This Agent"));
        assert!(soul.contains("Focus: inbox triage. Hard boundary: never auto-send."));
    }

    #[test]
    fn compose_treats_blank_personalization_as_none() {
        let soul = compose_soul(SoulArchetype::Maker, "Builder", Some("   \n  "));
        assert!(soul.contains("Bias to action, surgical edits, verified results.")); // maker marker
        assert!(!soul.contains("## This Agent"));
    }
}
