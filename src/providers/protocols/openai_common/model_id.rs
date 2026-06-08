//! Shared OpenAI model-id normalization.
//!
//! Both the Chat Completions and Responses protocols accept the same family of
//! OpenAI model ids and hit the same aggregator quirks (no-dash typos, leaked
//! `openai/` vendor-routing prefixes). Keeping the canonicalization in one place
//! means the two adapters can never drift — previously only Chat normalized,
//! while Responses passed the raw id straight through.
//!
//! **Endpoint awareness (hermes-parity).** The `openai/` vendor prefix means
//! opposite things on different hosts: on the first-party OpenAI API
//! (`api.openai.com`, `*.openai.azure.com`) the wire id is bare (`gpt-4o`) and
//! a leaked `openai/` must be stripped; but on an OpenAI-*protocol* aggregator
//! (OpenRouter etc.) the `vendor/model` slug — `openai/gpt-4o` — is the
//! REQUIRED routing key and stripping it mis-routes the request. So the strip
//! is gated on the endpoint host (see [`normalize_openai_model_id`]). The
//! no-dash typo repairs are universally safe and always applied.

use std::borrow::Cow;

/// Forgive common OpenAI model-id typos that production users hit, and strip a
/// leaked `openai/` vendor-routing prefix **only when the endpoint is the
/// first-party OpenAI API**.
///
/// `base_url` is the provider's configured endpoint (`None` ⇒ the default
/// first-party OpenAI host). The `openai/` prefix is stripped iff the host is
/// first-party (`api.openai.com`, Azure OpenAI); on aggregators that speak the
/// OpenAI protocol but require `vendor/model` slugs (OpenRouter, …) the prefix
/// is preserved so the request routes to the right upstream model.
///
/// Covers the no-dash variants returned by some routing aggregators
/// (`gpt4o` → `gpt-4o`, `gpt4omini` → `gpt-4o-mini`, `o3mini` → `o3-mini`).
/// Anything already canonical passes through unchanged as `Cow::Borrowed`
/// (zero-copy).
pub fn normalize_openai_model_id<'a>(model_id: &'a str, base_url: Option<&str>) -> Cow<'a, str> {
    let trimmed = model_id.trim();
    // Strip the `openai/` vendor-routing prefix only on first-party OpenAI
    // hosts; on aggregators the slug is the routing key and must survive.
    let core = if is_first_party_openai(base_url) {
        trimmed.strip_prefix("openai/").unwrap_or(trimmed)
    } else {
        trimmed
    };
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

/// Whether `base_url` points at a first-party OpenAI endpoint (where the wire
/// model id is bare and a leaked `openai/` prefix should be stripped).
///
/// `None`/empty ⇒ the default OpenAI host ⇒ first-party. Reuses the
/// scheme-tolerant host parse from the endpoint catalog (no duplication); an
/// unparseable URL conservatively counts as first-party so the historical
/// strip behaviour is preserved when the host can't be determined. Matches
/// `api.openai.com` and Azure's `*.openai.azure.com` via the `openai` host
/// token — aggregator hosts (`openrouter.ai`, …) do not contain it.
fn is_first_party_openai(base_url: Option<&str>) -> bool {
    let raw = match base_url.map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return true,
    };
    match crate::providers::model_catalog::endpoint::extract_host(raw) {
        Some(host) => host.contains("openai"),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Typo repairs are endpoint-independent — assert on both host classes.
    #[test]
    fn rewrites_no_dash_variants() {
        for base in [
            None,
            Some("https://api.openai.com/v1"),
            Some("https://openrouter.ai/api"),
        ] {
            assert_eq!(normalize_openai_model_id("gpt4o", base), "gpt-4o");
            assert_eq!(normalize_openai_model_id("gpt4omini", base), "gpt-4o-mini");
            assert_eq!(normalize_openai_model_id("o3mini", base), "o3-mini");
        }
    }

    #[test]
    fn strips_vendor_prefix_on_first_party_host() {
        // None ⇒ default first-party host ⇒ strip (historical behaviour).
        assert_eq!(normalize_openai_model_id("openai/gpt-5", None), "gpt-5");
        assert_eq!(
            normalize_openai_model_id("openai/gpt-5", Some("https://api.openai.com/v1")),
            "gpt-5"
        );
        // Azure OpenAI is also first-party.
        assert_eq!(
            normalize_openai_model_id("openai/gpt-4o", Some("https://my-rsc.openai.azure.com")),
            "gpt-4o"
        );
    }

    #[test]
    fn preserves_vendor_prefix_on_aggregator_host() {
        // OpenRouter (and any OpenAI-protocol aggregator) requires the
        // `vendor/model` slug — stripping it would mis-route the request.
        assert_eq!(
            normalize_openai_model_id("openai/gpt-4o", Some("https://openrouter.ai/api")),
            "openai/gpt-4o"
        );
        // Non-OpenAI vendor slugs were never stripped, and still aren't.
        assert_eq!(
            normalize_openai_model_id(
                "anthropic/claude-sonnet-4",
                Some("https://openrouter.ai/api")
            ),
            "anthropic/claude-sonnet-4"
        );
    }

    #[test]
    fn passes_through_canonical_borrowed() {
        let out = normalize_openai_model_id("gpt-4o", None);
        assert_eq!(out, "gpt-4o");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn passes_through_unknown() {
        assert_eq!(
            normalize_openai_model_id("some-future-model", None),
            "some-future-model"
        );
    }
}
