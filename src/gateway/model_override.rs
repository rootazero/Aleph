//! Per-call model override for chat/agent runs.
//!
//! When the user picks a model in the chat-window picker, the panel attaches
//! a [`ModelOverride`] to the next `chat.send`. The gateway threads it
//! through `AgentRunParams → RunRequest → run_loop` where it short-circuits
//! `ProviderRegistry::resolve_with_fallback`.
//!
//! ### Why qualified vs raw
//!
//! Openclaw's picker emits `openai/gpt-5` (qualified) or `gpt-5` (raw); the
//! Rust port turns that into a typed enum so the compiler — not a runtime
//! `if let` — enforces the discriminator. The qualified variant pins both
//! provider and model so the resolver can't accidentally route a "qualified"
//! request through the fallback chain. The raw variant lets the resolver
//! pick a provider via its existing model-name heuristics.
//!
//! ### Sticky vs per-turn
//!
//! Both variants are **per-turn** overrides. The panel persists the user's
//! latest pick to the session row (out-of-scope for this module); when a
//! send arrives without an explicit override, the gateway falls back to the
//! sticky session preference and finally to the agent's configured default.

use serde::{Deserialize, Serialize};

/// User-selected model for a single chat/agent turn.
///
/// Wire format mirrors openclaw `ChatModelOverride`:
/// ```jsonc
/// // qualified — pin both
/// { "kind": "qualified", "provider": "openai", "model": "gpt-5" }
/// // raw — resolver figures out the provider
/// { "kind": "raw",        "model": "gpt-5" }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ModelOverride {
    /// Pin both provider and model — skips the fallback chain. Used when the
    /// panel sends the explicit `provider/model` pair from the catalog.
    Qualified {
        /// Provider id (preset name, e.g. `"openai"`, `"deepseek"`).
        provider: String,
        /// Model id (e.g. `"gpt-5"`).
        model: String,
    },
    /// Send only the model name; the resolver picks the provider via the
    /// existing prefix heuristic (`gpt-*` → openai, `claude-*` → anthropic, …).
    /// Falls through to the agent's default provider if no rule matches.
    Raw {
        /// Model id (e.g. `"claude-sonnet-4"`).
        model: String,
    },
}

impl ModelOverride {
    /// Model id the user asked for (both variants carry one).
    pub fn model(&self) -> &str {
        match self {
            Self::Qualified { model, .. } => model,
            Self::Raw { model } => model,
        }
    }

    /// Provider name when explicitly pinned, `None` for raw.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Qualified { provider, .. } => Some(provider),
            Self::Raw { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_qualified() {
        let v = serde_json::json!({
            "kind": "qualified",
            "provider": "openai",
            "model": "gpt-5",
        });
        let mo: ModelOverride = serde_json::from_value(v).unwrap();
        assert_eq!(mo.model(), "gpt-5");
        assert_eq!(mo.provider(), Some("openai"));
    }

    #[test]
    fn deserialize_raw() {
        let v = serde_json::json!({ "kind": "raw", "model": "claude-sonnet-4" });
        let mo: ModelOverride = serde_json::from_value(v).unwrap();
        assert_eq!(mo.model(), "claude-sonnet-4");
        assert_eq!(mo.provider(), None);
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let v = serde_json::json!({ "kind": "bogus", "model": "x" });
        assert!(serde_json::from_value::<ModelOverride>(v).is_err());
    }

    #[test]
    fn missing_model_is_rejected() {
        let v = serde_json::json!({ "kind": "raw" });
        assert!(serde_json::from_value::<ModelOverride>(v).is_err());
    }

    #[test]
    fn qualified_round_trips() {
        let mo = ModelOverride::Qualified {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
        };
        let s = serde_json::to_string(&mo).unwrap();
        let back: ModelOverride = serde_json::from_str(&s).unwrap();
        assert_eq!(mo, back);
    }
}
