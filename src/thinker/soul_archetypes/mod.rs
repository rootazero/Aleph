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
    /// Every archetype, in creation-interview presentation order (the default,
    /// [`SoulArchetype::Assistant`], sits third — recommended when the purpose
    /// is unclear). Single source for enumeration: the tests derive from this,
    /// so adding a variant can never leave a hand-kept
    /// `[Expert, Companion, …]` array silently behind.
    pub const ALL: [SoulArchetype; 4] = [
        SoulArchetype::Expert,
        SoulArchetype::Maker,
        SoulArchetype::Assistant,
        SoulArchetype::Companion,
    ];

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

    /// Lowercase wire/storage identifier (matches the serde representation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expert => "expert",
            Self::Companion => "companion",
            Self::Assistant => "assistant",
            Self::Maker => "maker",
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

    /// Suggested `IDENTITY.md` **Role** seed. A starting point derived from the
    /// chosen archetype (the user edits it), not a hard label.
    #[must_use]
    pub fn role_hint(self) -> &'static str {
        match self {
            Self::Expert => "advisor / analyst",
            Self::Companion => "companion / listener",
            Self::Assistant => "assistant",
            Self::Maker => "builder / engineer",
        }
    }

    /// Suggested `IDENTITY.md` **Vibe** seed derived from the archetype.
    #[must_use]
    pub fn vibe_hint(self) -> &'static str {
        match self {
            Self::Expert => "sharp, rigorous, argues the counter-case",
            Self::Companion => "warm, present, unhurried",
            Self::Assistant => "fast, direct, low-friction",
            Self::Maker => "action-biased, surgical, verifies",
        }
    }

    /// Suggested signature **Emoji** seed derived from the archetype.
    #[must_use]
    pub fn emoji_hint(self) -> &'static str {
        match self {
            Self::Expert => "\u{1f3af}",    // 🎯
            Self::Companion => "\u{1f331}", // 🌱
            Self::Assistant => "\u{26a1}",  // ⚡
            Self::Maker => "\u{1f527}",     // 🔧
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
        for a in SoulArchetype::ALL {
            assert!(!a.template().trim().is_empty());
            assert!(!a.summary().trim().is_empty());
            assert!(!a.role_hint().trim().is_empty());
            assert!(!a.vibe_hint().trim().is_empty());
            assert!(!a.emoji_hint().trim().is_empty());
        }
        assert_ne!(
            SoulArchetype::Expert.template(),
            SoulArchetype::Maker.template()
        );
    }

    #[test]
    fn all_covers_every_variant_exactly_once() {
        // Round-trip through the wire ids: ALL must enumerate all 4 distinct
        // archetypes (guards against a variant added without extending ALL).
        let mut ids: Vec<&str> = SoulArchetype::ALL.iter().map(|a| a.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, ["assistant", "companion", "expert", "maker"]);
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
