//! Process-global per-session model preference.
//!
//! The "A layer" of AI dynamic routing (R8): the `select_model` tool lets the
//! main-loop LLM pick the model for the rest of a conversation. Model binding
//! happens per-run at `pick_llm` time — once, before the Think→Act loop starts
//! — so a mid-run pick takes effect at the next *run* (the next user message),
//! not at the next loop iteration. This handle is that cross-run channel.
//!
//! Mirrors [`route_handle`](super::route_handle): a single process-global,
//! lock-guarded map read at run construction (`harness_bridge`) and written by
//! the tool. In-memory by design — a per-conversation model preference is soft
//! UX state, not durable config; it resets on restart, which is the intended
//! "fresh session" behaviour. Keyed by the canonical `SessionKey` string so the
//! writer (tool, from `TURN_CONTEXT`) and reader (bridge, from its run key)
//! agree.

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A session's LLM-chosen model, optionally pinned to a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelPref {
    /// Provider id to pin (e.g. `"openai"`). `None` = let the bridge fall back
    /// to the live default provider (the model is still stamped onto it).
    pub provider: Option<String>,
    /// Model id to stamp onto every request for this session.
    pub model: String,
}

static SESSION_MODELS: OnceLock<RwLock<HashMap<String, SessionModelPref>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, SessionModelPref>> {
    SESSION_MODELS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record the model preference for `session_key`, overwriting any prior pick.
pub fn set_session_model(session_key: &str, provider: Option<String>, model: String) {
    map().write().unwrap_or_else(|e| e.into_inner()).insert(
        session_key.to_string(),
        SessionModelPref { provider, model },
    );
}

/// Read the model preference for `session_key`, if one was set this run.
#[must_use]
pub fn get_session_model(session_key: &str) -> Option<SessionModelPref> {
    map()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_key)
        .cloned()
}

/// Drop a session's preference (revert to the agent/flow default).
pub fn clear_session_model(session_key: &str) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_key);
}

/// The provider keys a `select_model(provider=…)` pin can actually resolve to.
static PINNABLE_PROVIDERS: OnceLock<std::collections::BTreeSet<String>> = OnceLock::new();

/// Publish the provider keys the run builder will resolve a pin against.
///
/// Registered by the production boot path from the *same map* `harness_bridge`
/// later looks the pin up in, so the tool's validation and the runtime's
/// resolution cannot disagree. Without it, `select_model(provider="openai")` on
/// an Anthropic-only deployment returned `ok: true`, then silently substituted
/// the default chain at run time — the user got an answer from a different
/// vendor than the one the tool confirmed, and the mis-attributed
/// `(provider, model)` pair was written into the routing-experience store the
/// model later reads back as "verified routing experience".
///
/// First call wins (one chain assembly per boot), matching
/// [`route_observe`](super::route_observe)'s global.
pub fn set_pinnable_providers(names: impl IntoIterator<Item = String>) {
    let _ = PINNABLE_PROVIDERS.set(names.into_iter().collect());
}

/// Whether `provider` can be pinned, plus the valid set for the error message.
///
/// `None` means "no set was published" (tests, pre-boot) — callers must treat
/// that as *unvalidated*, never as "nothing is pinnable".
#[must_use]
pub fn pinnable_providers() -> Option<&'static std::collections::BTreeSet<String>> {
    PINNABLE_PROVIDERS.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear_roundtrip() {
        let key = "test:session:roundtrip";
        assert_eq!(get_session_model(key), None);
        set_session_model(key, Some("openai".to_string()), "gpt-5".to_string());
        assert_eq!(
            get_session_model(key),
            Some(SessionModelPref {
                provider: Some("openai".to_string()),
                model: "gpt-5".to_string(),
            })
        );
        // Overwrite with a provider-less pick.
        set_session_model(key, None, "claude-opus-4".to_string());
        let got = get_session_model(key).unwrap();
        assert_eq!(got.model, "claude-opus-4");
        assert_eq!(got.provider, None);
        clear_session_model(key);
        assert_eq!(get_session_model(key), None);
    }

    #[test]
    fn distinct_sessions_isolated() {
        set_session_model("test:session:a", None, "model-a".to_string());
        set_session_model("test:session:b", None, "model-b".to_string());
        assert_eq!(
            get_session_model("test:session:a").unwrap().model,
            "model-a"
        );
        assert_eq!(
            get_session_model("test:session:b").unwrap().model,
            "model-b"
        );
    }
}
