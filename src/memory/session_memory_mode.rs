//! Per-session memory mode — whether this conversation gets memory injected.
//!
//! The fifth session knob, alongside [`ExecTier`](crate::config::types::policies::ExecTier)
//! (autonomy), [`SessionMode`](crate::config::types::policies::SessionMode)
//! (tool surface), [`ThinkLevel`](crate::agents::thinking::ThinkLevel)
//! (reasoning depth) and the model pin. Same carrier
//! (`identity_meta.custom`), same request > session > global precedence, a
//! different orthogonal axis: **what the model is told it already knows.**
//!
//! # Why a per-session dial and not just `[memory] enabled`
//!
//! `[memory] enabled` is install-wide. The question users actually have is
//! per-conversation: *this* thread is a clean-room review, or a demo, or a
//! transcript someone else will read — do not fold my curated memory, my note
//! index and my recall hits into it. Turning the install-wide switch off to get
//! that costs every other conversation its memory, so nobody does; they open a
//! new agent instead, which splits their memory partition permanently.
//!
//! codex reached the same conclusion from the other direction — its threads
//! carry a `memory_mode` (`enabled` / `disabled` / `polluted`) and its TUI
//! settings page writes it per thread. Aleph does not port the third state:
//! `polluted` marks a thread whose context came from an external MCP source and
//! is therefore unsafe to *learn from*, which is a write-side concern Aleph
//! answers elsewhere (`memory_trace`, the ingest governance gate). This knob is
//! read-side only, and says so.
//!
//! # What it does and does not gate
//!
//! `Off` suppresses the three **injected** envelopes — curated memory, the wiki
//! orientation index, and per-query hybrid recall — at the one place they
//! converge (`harness_bridge::prompt_build`). It does **not** disable the
//! memory *tools*: `memory_search`, `remember`, `note_*` stay callable, because
//! silently removing a tool the model can see is how a model ends up insisting
//! it saved something it did not. The distinction is "not injected" versus "not
//! available", and only the first is a presentation choice.
//!
//! It also does not gate writing. A conversation with injection off still
//! records what it learns; muting the read side of a session must not quietly
//! also mute the write side, or a user who wanted a clean prompt would find a
//! hole in their history months later.

use serde::{Deserialize, Serialize};

/// `identity_meta.custom` key holding a session's memory-mode override.
///
/// Fifth twin of `EXEC_TIER_SESSION_KEY` / `MODE_SESSION_KEY` /
/// `THINK_LEVEL_SESSION_KEY` / `MODEL_PIN_SESSION_KEY`.
pub const MEMORY_MODE_SESSION_KEY: &str = "memory_mode";

/// Whether memory envelopes are injected into this conversation's prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// Inject curated memory, the orientation index and per-query recall —
    /// the behaviour every release to date has shipped.
    On,
    /// Inject none of them. Memory tools stay callable and writes still land.
    Off,
}

impl MemoryMode {
    /// Parse from its serialized id. Accepts the two ids plus the spellings a
    /// human types at a prompt, because this is reached from a slash command
    /// as well as from a pill.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "on" | "enabled" | "true" => Some(Self::On),
            "off" | "disabled" | "false" => Some(Self::Off),
            _ => None,
        }
    }

    /// Serialized id, as stored and as reported to clients.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// Whether memory envelopes should be built for this turn.
    #[must_use]
    pub const fn injects(self) -> bool {
        matches!(self, Self::On)
    }
}

/// Resolve the mode for one turn: requested > stored > global.
///
/// `global` is `[memory] enabled`, read live so a config change reaches the
/// next turn. Split out as a pure function — mirroring
/// `turn_mode::resolve_session_mode` — so the precedence is pinned by tests
/// that need no engine.
#[must_use]
pub fn resolve_memory_mode(
    global_enabled: bool,
    requested: Option<MemoryMode>,
    stored: Option<MemoryMode>,
) -> MemoryMode {
    requested.or(stored).unwrap_or(if global_enabled {
        MemoryMode::On
    } else {
        MemoryMode::Off
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for mode in [MemoryMode::On, MemoryMode::Off] {
            assert_eq!(MemoryMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(MemoryMode::from_id("nonsense"), None);
        assert_eq!(MemoryMode::from_id(""), None);
    }

    #[test]
    fn human_spellings_parse() {
        // Reached from `/memory off` as well as from a pill, and "disabled" is
        // what codex calls it — refusing the word a user just read elsewhere is
        // a refusal with no upside.
        assert_eq!(MemoryMode::from_id(" OFF "), Some(MemoryMode::Off));
        assert_eq!(MemoryMode::from_id("disabled"), Some(MemoryMode::Off));
        assert_eq!(MemoryMode::from_id("Enabled"), Some(MemoryMode::On));
    }

    #[test]
    fn precedence_is_requested_then_stored_then_global() {
        assert_eq!(resolve_memory_mode(true, None, None), MemoryMode::On);
        assert_eq!(resolve_memory_mode(false, None, None), MemoryMode::Off);
        // A stored choice outlives the turn that carried it — that is the
        // whole point: reopening the terminal must not re-enable injection on
        // a thread the user muted.
        assert_eq!(
            resolve_memory_mode(true, None, Some(MemoryMode::Off)),
            MemoryMode::Off
        );
        // …and it can go the other way: a session may opt IN on an install
        // whose global switch is off.
        assert_eq!(
            resolve_memory_mode(false, None, Some(MemoryMode::On)),
            MemoryMode::On
        );
        // The request wins over both (the pill / slash command switching an
        // existing conversation).
        assert_eq!(
            resolve_memory_mode(true, Some(MemoryMode::Off), Some(MemoryMode::On)),
            MemoryMode::Off
        );
    }

    #[test]
    fn only_on_injects() {
        assert!(MemoryMode::On.injects());
        assert!(!MemoryMode::Off.injects());
    }
}
