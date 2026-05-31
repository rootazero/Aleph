//! Shared OpenAI model-id normalization.
//!
//! Both the Chat Completions and Responses protocols accept the same family of
//! OpenAI model ids and hit the same aggregator quirks (no-dash typos, leaked
//! `openai/` vendor-routing prefixes). Keeping the canonicalization in one place
//! means the two adapters can never drift — previously only Chat normalized,
//! while Responses passed the raw id straight through.

use std::borrow::Cow;

/// Forgive common OpenAI model-id typos that production users hit, and strip the
/// `openai/` vendor-routing prefix some aggregators leak.
///
/// Covers the no-dash variants returned by some routing aggregators
/// (`gpt4o` → `gpt-4o`, `gpt4omini` → `gpt-4o-mini`, `o3mini` → `o3-mini`).
/// Anything already canonical passes through unchanged as `Cow::Borrowed`
/// (zero-copy).
pub fn normalize_openai_model_id(model_id: &str) -> Cow<'_, str> {
    let trimmed = model_id.trim();
    // Strip vendor-routing prefix sometimes used by aggregators.
    let core = trimmed.strip_prefix("openai/").unwrap_or(trimmed);
    let lower = core.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "gpt4o" => Some("gpt-4o"),
        "gpt4omini" | "gpt-4omini" | "gpt4o-mini" => Some("gpt-4o-mini"),
        "gpt4turbo" => Some("gpt-4-turbo"),
        "o1mini" => Some("o1-mini"),
        "o3mini" => Some("o3-mini"),
        "o4mini" => Some("o4-mini"),
        _ => None,
    };
    match canonical {
        Some(c) => Cow::Owned(c.to_string()),
        None if core.len() != trimmed.len() => Cow::Owned(core.to_string()),
        None => Cow::Borrowed(model_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_no_dash_variants() {
        assert_eq!(normalize_openai_model_id("gpt4o"), "gpt-4o");
        assert_eq!(normalize_openai_model_id("gpt4omini"), "gpt-4o-mini");
        assert_eq!(normalize_openai_model_id("o3mini"), "o3-mini");
    }

    #[test]
    fn strips_vendor_prefix() {
        assert_eq!(normalize_openai_model_id("openai/gpt-5"), "gpt-5");
    }

    #[test]
    fn passes_through_canonical_borrowed() {
        let out = normalize_openai_model_id("gpt-4o");
        assert_eq!(out, "gpt-4o");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn passes_through_unknown() {
        assert_eq!(
            normalize_openai_model_id("some-future-model"),
            "some-future-model"
        );
    }
}
