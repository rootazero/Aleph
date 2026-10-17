//! Per-model decision: which Chat-API token-limit field name to send.
//!
//! `OpenAI` reasoning model families (`o1-`, `o3-`, `o4-`, `gpt-5`) reject
//! `max_tokens` with HTTP 400; they require `max_completion_tokens` instead.
//! All other models (gpt-4o, gpt-4-turbo, gpt-3.5-turbo, third-party compat
//! backends, ...) continue to use `max_tokens`.
//!
//! The Responses protocol is unaffected — it uses `max_output_tokens`.
//!
//! Operators can override the heuristic via
//! [`crate::config::types::provider::ModelCompat::uses_max_completion_tokens`].
//! An explicit `Some(true)` forces `max_completion_tokens` regardless of
//! model name (e.g. a vendor-emulating OpenAI reasoner that does not match
//! our prefix list); `Some(false)` forces `max_tokens` on a model that
//! would otherwise trip the heuristic (e.g. a compatibility proxy that
//! returns 400 on `max_completion_tokens`). `None` falls back to the
//! built-in prefix match.

use crate::config::types::provider::ModelCompat;

/// Returns true if the model requires `max_completion_tokens` instead of `max_tokens`.
#[must_use]
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim();
    m.starts_with("o1-") || m.starts_with("o3-") || m.starts_with("o4-") || m.starts_with("gpt-5")
}

/// Like [`uses_max_completion_tokens`], but consults a [`ModelCompat`]
/// override first. `Some(v)` from the compat block wins unconditionally;
/// `None` (or no compat at all) falls through to the name-prefix heuristic.
///
/// Use this from request builders that already have a resolved
/// [`ModelCompat`] in hand — it keeps the override lookup single-sourced
/// with the existing [`PayloadPolicy`] compat wiring rather than re-running
/// `model_compat_for` at the call site.
///
/// [`PayloadPolicy`]: crate::providers::protocols::openai_common::provider_policy::PayloadPolicy
#[must_use]
pub fn uses_max_completion_tokens_for(model: &str, compat: Option<&ModelCompat>) -> bool {
    if let Some(v) = compat.and_then(|c| c.uses_max_completion_tokens) {
        return v;
    }
    uses_max_completion_tokens(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_chat_models_use_max_tokens() {
        assert!(!uses_max_completion_tokens("gpt-4o"));
        assert!(!uses_max_completion_tokens("gpt-4-turbo"));
        assert!(!uses_max_completion_tokens("gpt-3.5-turbo"));
        assert!(!uses_max_completion_tokens(""));
    }

    #[test]
    fn o1_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o1-mini"));
        assert!(uses_max_completion_tokens("o1-preview"));
    }

    #[test]
    fn o3_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o3-mini"));
        assert!(uses_max_completion_tokens("o3-pro"));
    }

    #[test]
    fn o4_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o4-mini"));
    }

    #[test]
    fn gpt5_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("gpt-5"));
        assert!(uses_max_completion_tokens("gpt-5.4"));
        assert!(uses_max_completion_tokens("gpt-5-codex"));
    }

    #[test]
    fn trims_whitespace_before_match() {
        assert!(uses_max_completion_tokens("  o3-mini  "));
        assert!(!uses_max_completion_tokens("  gpt-4o  "));
    }

    fn compat_with_uses_max_completion_tokens(v: Option<bool>) -> ModelCompat {
        ModelCompat {
            uses_max_completion_tokens: v,
            ..ModelCompat::default()
        }
    }

    #[test]
    fn uses_max_completion_tokens_for_none_falls_back_to_heuristic() {
        let compat = compat_with_uses_max_completion_tokens(None);
        assert!(uses_max_completion_tokens_for("o3-mini", Some(&compat)));
        assert!(!uses_max_completion_tokens_for("gpt-4o", Some(&compat)));
        assert!(uses_max_completion_tokens_for("o3-mini", None));
        assert!(!uses_max_completion_tokens_for("gpt-4o", None));
    }

    #[test]
    fn uses_max_completion_tokens_for_some_true_forces_max_completion_tokens() {
        let compat = compat_with_uses_max_completion_tokens(Some(true));
        // Legacy name with explicit override forces max_completion_tokens
        assert!(uses_max_completion_tokens_for("gpt-4o", Some(&compat)));
        // Reasoning name stays true (override is a no-op here but proves the
        // helper still consults the compat block)
        assert!(uses_max_completion_tokens_for("o3-mini", Some(&compat)));
    }

    #[test]
    fn uses_max_completion_tokens_for_some_false_forces_max_tokens() {
        let compat = compat_with_uses_max_completion_tokens(Some(false));
        // Reasoning name with explicit override forces max_tokens
        assert!(!uses_max_completion_tokens_for("o3-mini", Some(&compat)));
        // Legacy name stays false (override is a no-op here but proves the
        // helper still consults the compat block)
        assert!(!uses_max_completion_tokens_for("gpt-4o", Some(&compat)));
    }
}
