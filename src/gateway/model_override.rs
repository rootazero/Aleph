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
//! ### Sticky vs per-turn — and what "sticky" does NOT mean
//!
//! Both variants are **per-turn** overrides.
//!
//! ⚠️ This doc used to add "the panel persists the user's latest pick to the
//! session row (out-of-scope for this module)". **No such persistence exists.**
//! `sessions.model` records which model last *served* a run
//! (`handlers/agent/helpers.rs`, from the run outcome) — it is not the user's
//! choice and nothing reads it back during resolution. What resolution actually
//! consults, in order, is: this per-turn override → the sticky value in
//! `providers::session_model_handle` (a **process-global `OnceLock<RwLock<
//! HashMap>>`**, written only by the `select_model` tool) → the agent default.
//!
//! So the model is still the one session dial that does not survive a restart
//! **as a preference**. `exec_tier` and `session_mode` persist in
//! `SessionMetadata.identity_meta.custom` and are restored through
//! `SessionListRow` into the Panel pills; the model picker is a Leptos signal
//! that resets on reload (`chat/state.rs` says so itself), and the server-side
//! sticky map dies with the process. Picking `openai/gpt-5.6`, sending three
//! messages and reloading still reverts to the agent default.
//!
//! ⚠️ One narrow exception now exists, and it is not persistence: a run that
//! was **interrupted** by a crash records the (provider, model) it was bound to
//! on its `RunStarted` marker (`session::events::RunEnvelopeSnapshot`), and
//! `resume_coordinator` replays that pair through this type's
//! `model_override` slot when it re-triggers the run. That restores the
//! *crashed run*, not the user's preference: it is per-run by construction (it
//! never writes back to the session row, which is exactly why this carrier was
//! chosen), it is validated against the catalog first — a retired id resumes on
//! its successor with the model told why — and the next message after the
//! recovered run resolves through the ordinary chain again.
//!
//! Making the *preference* persist still means giving it the same carrier the
//! other two dials use — a `turn_model.rs` twin of `turn_mode.rs` resolving
//! request > session > global with stamp-on-carry, which would also retire that
//! process-global map (P6). Not done here; this note exists so the next reader
//! is not misled by a documented chain that was never built.

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
    #[must_use]
    pub fn model(&self) -> &str {
        match self {
            Self::Qualified { model, .. } => model,
            Self::Raw { model } => model,
        }
    }

    /// Provider name when explicitly pinned, `None` for raw.
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Qualified { provider, .. } => Some(provider),
            Self::Raw { .. } => None,
        }
    }

    /// Build a voice-mode override from the `[voice]` config strings.
    ///
    /// * empty `model` → `None` (no override; run uses the global default).
    /// * `model` set, `provider` empty → [`Self::Raw`] (resolver picks the
    ///   provider by model-name heuristic).
    /// * both set → [`Self::Qualified`] (pin both, skip fallback).
    ///
    /// Pure for unit testing — takes plain `&str` so the config layer never
    /// depends on the gateway.
    #[must_use]
    pub fn from_voice(provider: &str, model: &str) -> Option<Self> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let provider = provider.trim();
        if provider.is_empty() {
            Some(Self::Raw {
                model: model.to_string(),
            })
        } else {
            Some(Self::Qualified {
                provider: provider.to_string(),
                model: model.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod voice_override_tests {
    use super::ModelOverride;

    #[test]
    fn empty_model_is_none() {
        assert_eq!(ModelOverride::from_voice("siliconflow", ""), None);
        assert_eq!(ModelOverride::from_voice("", "   "), None);
    }

    #[test]
    fn model_only_is_raw() {
        assert_eq!(
            ModelOverride::from_voice("", "deepseek-ai/DeepSeek-V3"),
            Some(ModelOverride::Raw {
                model: "deepseek-ai/DeepSeek-V3".to_string()
            })
        );
    }

    #[test]
    fn both_set_is_qualified() {
        assert_eq!(
            ModelOverride::from_voice("siliconflow", "deepseek-ai/DeepSeek-V3"),
            Some(ModelOverride::Qualified {
                provider: "siliconflow".to_string(),
                model: "deepseek-ai/DeepSeek-V3".to_string(),
            })
        );
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(
            ModelOverride::from_voice("  siliconflow  ", "  m  "),
            Some(ModelOverride::Qualified {
                provider: "siliconflow".to_string(),
                model: "m".to_string(),
            })
        );
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
