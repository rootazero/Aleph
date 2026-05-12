//! Per-model decision: which Chat-API token-limit field name to send.
//!
//! OpenAI reasoning model families (`o1-`, `o3-`, `o4-`, `gpt-5`) reject
//! `max_tokens` with HTTP 400; they require `max_completion_tokens` instead.
//! All other models (gpt-4o, gpt-4-turbo, gpt-3.5-turbo, third-party compat
//! backends, ...) continue to use `max_tokens`.
//!
//! The Responses protocol is unaffected — it uses `max_output_tokens`.

/// Returns true if the model requires `max_completion_tokens` instead of `max_tokens`.
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim();
    m.starts_with("o1-")
        || m.starts_with("o3-")
        || m.starts_with("o4-")
        || m.starts_with("gpt-5")
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
}
