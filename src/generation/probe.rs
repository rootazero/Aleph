//! Lightweight generation-provider connectivity probe — single source of truth.
//!
//! Mirrors [`crate::providers::probe`] for chat providers: a single shared
//! "can this provider answer?" check so the gateway's `generation_providers.test`
//! RPC performs a REAL network round-trip instead of blindly reporting success.
//!
//! It validates reachability + credential acceptance WITHOUT generating any
//! (paid) media: it GETs a cheap endpoint (`{base}/v1/models` for
//! OpenAI-compatible bases) with the provider's correct auth header and
//! inspects the HTTP status. Connection errors mean unreachable; `401`/`403`
//! mean the credentials were rejected; a `2xx` means the endpoint answered
//! and did not reject the key.
//!
//! **Only a `2xx` verifies a credential.** Every other status leaves the
//! credential UNTESTED: the handler writes `verified = true` straight from
//! this outcome (`src/gateway/handlers/generation_providers/handlers.rs:737`),
//! so a status the probe cannot read as "the key was accepted" must not turn
//! into a green badge. A `404` means "this endpoint does not expose
//! `/v1/models`" (or the URL is wrong), a `405` means "this endpoint does not
//! accept a GET" — the `openai_compat` presets store a full POST endpoint in
//! `base_url`, so a GET against them answers `405` — and a `5xx` means the
//! upstream failed. None of them prove the key works, so none of them
//! verify it.
//!
//! Lives in the generation domain (not the gateway) so the dependency
//! direction stays `gateway → generation` (P1) — gateway handlers never own
//! probe semantics.

use crate::config::types::generation::presets::get_preset_by_type;
use crate::generation::providers::canonical_provider_type;
use crate::generation::providers::url_normalize::{resolve_base_url, ResolvedUrl};
use std::time::Duration;

/// Result of one generation-provider connectivity probe. The gateway maps this
/// onto its wire-stable `TestConnectionResult`.
#[derive(Debug, Clone)]
pub struct GenerationProbeOutcome {
    pub success: bool,
    pub message: String,
}

/// Connectivity-probe timeout. Generation APIs can be slow under load, but a
/// `GET /models` (or auth rejection) returns quickly; 10s is generous headroom.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The HTTP auth header a provider type expects. Most generation providers are
/// OpenAI-compatible (`Authorization: Bearer …`); the rest use their own
/// scheme. Sending the correct scheme avoids a false "auth failed" verdict for
/// non-Bearer providers (a valid key must not be rejected just because we sent
/// the wrong header).
///
/// The provider type is canonicalized first, so this table names one entry
/// per provider instead of repeating every alias of it.
fn auth_header_for(provider_type: &str, api_key: &str) -> (&'static str, String) {
    let canonical = canonical_provider_type(provider_type);
    match canonical.as_ref() {
        "deepgram_stt" | "deepgram_tts" => ("Authorization", format!("Token {api_key}")),
        "azure_speech" => ("Ocp-Apim-Subscription-Key", api_key.to_string()),
        "bfl" => ("x-key", api_key.to_string()),
        "cartesia" => ("X-API-Key", api_key.to_string()),
        "google" | "google_veo" => ("x-goog-api-key", api_key.to_string()),
        "elevenlabs" => ("xi-api-key", api_key.to_string()),
        "fal" => ("Authorization", format!("Key {api_key}")),
        "volcengine_tts" => ("Authorization", format!("Bearer;{api_key}")),
        // OpenAI-compatible default: openai / openai_tts / openai_whisper /
        // openai_compat / stability / replicate / suno / midjourney /
        // minimax_tts / …
        _ => ("Authorization", format!("Bearer {api_key}")),
    }
}

/// The built-in endpoint of a provider type, when the provider itself carries
/// one.
///
/// The presets are not the only place a default endpoint lives: the Google
/// providers default to the Gemini API host even though their preset carries
/// no `base_url` at all. The probe has to ask the provider module for it --
/// keeping a second copy of the URL here would be exactly the drift this
/// audit is about.
fn default_endpoint_for(canonical: &str) -> Option<&'static str> {
    match canonical {
        "google" => Some(crate::generation::providers::google_imagen::DEFAULT_ENDPOINT),
        "google_veo" => Some(crate::generation::providers::google_veo::DEFAULT_ENDPOINT),
        _ => None,
    }
}

/// Resolve the cheap endpoint to GET for a connectivity probe.
///
/// Falls back to the preset's default base URL when the caller did not supply
/// one (e.g. testing a saved provider that relies on the preset default), and
/// then to the provider's own built-in endpoint when the preset carries none.
/// Returns `None` when no base URL can be determined at all (e.g. an
/// `openai_compat` provider with no `base_url` configured) — the caller turns
/// that into a clear error rather than a misleading "success".
///
/// The provider type is canonicalized before the preset lookup. The presets
/// are keyed by the canonical type, so a provider configured as `"tts"`,
/// `"dalle"`, `"mj"` or any other alias has to be resolved before the lookup
/// can find it — without that step the lookup missed every alias and the
/// probe reported "未配置 base_url" for providers that work.
fn probe_url(base_url: Option<&str>, provider_type: &str) -> Option<String> {
    let raw = base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let canonical = canonical_provider_type(provider_type);
            get_preset_by_type(canonical.as_ref())
                .and_then(|p| p.base_url)
                .or_else(|| {
                    default_endpoint_for(canonical.as_ref())
                })
        })?;

    match resolve_base_url(raw) {
        ResolvedUrl::Standard(base) => Some(format!("{base}/v1/models")),
        ResolvedUrl::Custom(url) => Some(url),
    }
}

/// Map an HTTP status (or a transport error) onto a probe outcome. Pure so it
/// is exhaustively host-testable without a live network.
///
/// **Only a `2xx` is a success.** The verdict is written straight into the
/// provider's `verified` flag, and the Panel turns that flag into a green
/// badge and an enabled "set as default" button. A status that does not mean
/// "the endpoint answered and accepted the key" therefore has to stay a
/// failure -- even when the endpoint is perfectly reachable, because
/// "reachable" is not "verified".
///
/// The provider type is canonicalized first, so the rules below name one
/// provider each instead of repeating every alias of it.
fn interpret(
    provider_type: &str,
    status: Option<u16>,
    transport_err: Option<&str>,
) -> GenerationProbeOutcome {
    let canonical = canonical_provider_type(provider_type);
    // Providers with no `/v1/models` endpoint: a 404 here is an artifact of
    // the probe URL, not evidence the credentials work.
    //
    // The list names CANONICAL provider types, because the provider type is
    // canonicalized above. It used to name a hand-picked subset of the
    // aliases, and the aliases it missed turned the 404 rule off exactly
    // where it was needed: `"mj"` (the Midjourney alias) and
    // `"google"`/`"imagen"`/`"google_imagen"` all answered a 404 with
    // "连接成功" and got `verified = true` written into the config.
    const NO_MODELS_ENDPOINT: &[&str] = &[
        "google",
        "google_veo",
        "elevenlabs",
        "bfl",
        "suno",
        "midjourney",
        "fal",
    ];
    if let Some(e) = transport_err {
        return GenerationProbeOutcome {
            success: false,
            message: format!("无法连接到提供商: {e}"),
        };
    }
    let Some(code) = status else {
        return GenerationProbeOutcome {
            success: false,
            message: "连接测试失败：未收到响应".to_string(),
        };
    };
    if code == 401 || code == 403 {
        return GenerationProbeOutcome {
            success: false,
            message: format!("认证失败：API 密钥被拒绝 (HTTP {code})"),
        };
    }
    if (200..300).contains(&code) {
        return GenerationProbeOutcome {
            success: true,
            message: format!("连接成功 (HTTP {code})"),
        };
    }
    if code == 404 {
        if NO_MODELS_ENDPOINT.contains(&canonical.as_ref()) {
            return GenerationProbeOutcome {
                success: false,
                message: "端点不提供 /v1/models，无法用轻量请求验证；请通过一次真实生成测试凭据"
                    .to_string(),
            };
        }
        return GenerationProbeOutcome {
            success: false,
            message: "端点未找到 (HTTP 404)：请检查 base_url 是否指向正确的地址".to_string(),
        };
    }
    if code == 405 || code == 501 {
        return GenerationProbeOutcome {
            success: false,
            message: format!(
                "端点不接受轻量 GET 请求 (HTTP {code})，无法用轻量请求验证凭据；请通过一次真实生成测试凭据"
            ),
        };
    }
    GenerationProbeOutcome {
        success: false,
        message: format!("连接测试未通过 (HTTP {code})：凭据未被验证"),
    }
}

/// Probe a generation provider with a lightweight authenticated `GET` and
/// classify the result. Never generates media, so it costs nothing.
pub async fn probe_generation_provider(
    provider_type: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> GenerationProbeOutcome {
    let normalized = provider_type.trim().to_ascii_lowercase();
    let pt = normalized.as_str();

    let Some(url) = probe_url(base_url, pt) else {
        return GenerationProbeOutcome {
            success: false,
            message: "未配置 base_url，无法测试连接".to_string(),
        };
    };

    let (header_name, header_value) = auth_header_for(pt, api_key);

    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return interpret(pt, None, Some(&e.to_string())),
    };

    match client
        .get(&url)
        .header(header_name, header_value)
        .send()
        .await
    {
        Ok(resp) => interpret(pt, Some(resp.status().as_u16()), None),
        Err(e) => interpret(pt, None, Some(&e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_family_uses_bearer() {
        for pt in [
            "openai",
            "openai_image",
            "dalle",
            "openai_compat",
            "stability",
            "replicate",
        ] {
            let (name, value) = auth_header_for(pt, "k");
            assert_eq!(name, "Authorization", "{pt}");
            assert_eq!(value, "Bearer k", "{pt}");
        }
    }

    #[test]
    fn unknown_provider_type_falls_back_to_bearer() {
        let (name, value) = auth_header_for("some_new_provider", "k");
        assert_eq!(name, "Authorization");
        assert_eq!(value, "Bearer k");
    }

    #[test]
    fn non_bearer_providers_use_their_own_scheme() {
        assert_eq!(
            auth_header_for("deepgram", "k"),
            ("Authorization", "Token k".to_string())
        );
        assert_eq!(
            auth_header_for("azure_speech", "k"),
            ("Ocp-Apim-Subscription-Key", "k".to_string())
        );
        assert_eq!(auth_header_for("bfl", "k"), ("x-key", "k".to_string()));
        assert_eq!(
            auth_header_for("cartesia", "k"),
            ("X-API-Key", "k".to_string())
        );
        assert_eq!(
            auth_header_for("google", "k"),
            ("x-goog-api-key", "k".to_string())
        );
        assert_eq!(
            auth_header_for("google_veo", "k"),
            ("x-goog-api-key", "k".to_string())
        );
        assert_eq!(
            auth_header_for("elevenlabs", "k"),
            ("xi-api-key", "k".to_string())
        );
        assert_eq!(
            auth_header_for("fal", "k"),
            ("Authorization", "Key k".to_string())
        );
        assert_eq!(
            auth_header_for("volcengine_tts", "k"),
            ("Authorization", "Bearer;k".to_string())
        );
    }

    #[test]
    fn probe_url_standard_base_appends_models() {
        assert_eq!(
            probe_url(Some("https://api.openai.com/v1"), "openai"),
            Some("https://api.openai.com/v1/models".to_string())
        );
        assert_eq!(
            probe_url(Some("https://api.example.com"), "openai_compat"),
            Some("https://api.example.com/v1/models".to_string())
        );
    }

    #[test]
    fn probe_url_custom_full_url_used_as_is() {
        assert_eq!(
            probe_url(
                Some("https://api.example.com/custom/endpoint"),
                "openai_compat"
            ),
            Some("https://api.example.com/custom/endpoint".to_string())
        );
    }

    #[test]
    fn probe_url_blank_base_falls_back_to_preset() {
        // openai has a preset base URL; blank inline base_url must fall back.
        let url = probe_url(Some("   "), "openai");
        assert!(url.is_some(), "expected preset fallback for blank base_url");
    }

    #[test]
    fn probe_url_none_for_unknown_type_without_base() {
        assert_eq!(probe_url(None, "definitely_not_a_real_provider_type"), None);
    }

    /// An alias must reach the SAME preset as the canonical type it stands
    /// for.
    ///
    /// This is the regression the alias table fixed. The probe looked presets
    /// up by an exact match on the configured `provider_type`, and the
    /// presets are keyed by the canonical type, so every alias -- "tts",
    /// "dalle", "mj", "veo" -- found no preset and the connectivity test
    /// answered "未配置 base_url，无法测试连接" for a provider that works.
    #[test]
    fn probe_url_resolves_an_alias_to_its_canonical_preset() {
        for (alias, canonical) in [
            ("dalle", "openai"),
            ("tts", "openai_tts"),
            ("whisper", "openai_whisper"),
            ("deepgram", "deepgram_stt"),
            ("bfl_flux", "bfl"),
            ("flux", "bfl"),
            ("sdxl", "stability"),
            ("imagen", "google"),
            ("google_imagen", "google"),
            ("veo", "google_veo"),
            ("mj", "midjourney"),
        ] {
            assert_eq!(
                probe_url(None, alias),
                probe_url(None, canonical),
                "alias '{alias}' must reach the same preset as '{canonical}'"
            );
        }

        // …and the aliases whose canonical preset carries a base URL must
        // find it, instead of answering "no base_url".
        for alias in ["dalle", "tts", "whisper", "deepgram", "bfl_flux", "flux", "sdxl"] {
            assert!(
                probe_url(None, alias).is_some(),
                "alias '{alias}' has no preset to fall back on"
            );
        }
    }

    /// The 404 rule must hold for the ALIASES as well.
    ///
    /// The list of no-models providers used to name a hand-picked subset of
    /// the spellings, and the spellings it missed turned the rule off exactly
    /// where it was needed: "mj" and "google"/"imagen"/"google_imagen"
    /// answered a 404 with "连接成功", and the handler wrote
    /// `verified = true` into the config on that verdict.
    #[test]
    fn interpret_404_fails_for_no_models_provider_aliases() {
        for pt in [
            "mj",
            "google",
            "imagen",
            "google_imagen",
            "bfl_flux",
            "flux",
            "veo",
        ] {
            assert!(
                !interpret(pt, Some(404), None).success,
                "provider '{pt}' must not be verified on a 404"
            );
        }
    }

    #[test]
    fn interpret_transport_error_fails() {
        let o = interpret("openai_image", None, Some("dns error"));
        assert!(!o.success);
        assert!(o.message.contains("无法连接"));
    }

    #[test]
    fn interpret_auth_rejection_fails() {
        assert!(!interpret("openai_image", Some(401), None).success);
        assert!(!interpret("openai_image", Some(403), None).success);
    }

    /// A `2xx` is the ONLY verdict that verifies a credential.
    ///
    /// The handler writes this verdict straight into the provider's
    /// `verified` flag, so a status that cannot mean "the key was accepted"
    /// must not turn into a green badge. The old rule -- "any status that is
    /// not 401/403 means the server answered" -- certified a wrong URL (404),
    /// a POST-only endpoint probed with GET (405) and a failing upstream
    /// (500) as "连接成功".
    #[test]
    fn interpret_only_2xx_verifies() {
        for code in [200, 201, 204, 299] {
            assert!(
                interpret("openai_image", Some(code), None).success,
                "HTTP {code} must verify"
            );
        }
        for code in [400, 404, 405, 409, 418, 429, 500, 502, 503] {
            assert!(
                !interpret("openai_image", Some(code), None).success,
                "HTTP {code} must NOT verify"
            );
        }
    }

    /// A `405` is the answer every `openai_compat` preset gives.
    ///
    /// Those presets store the full POST endpoint in `base_url`
    /// (`https://api.ideogram.ai/openai/v1/images/generations`), and the
    /// probe GETs it -- so the answer is "method not allowed", not "the key
    /// was rejected". Reporting that as success stamped `verified = true`
    /// on a provider whose key had never been exercised.
    #[test]
    fn interpret_method_not_allowed_is_not_a_verification() {
        let outcome = interpret("openai_compat", Some(405), None);
        assert!(!outcome.success);
        assert!(outcome.message.contains("轻量"));
    }

    /// Regression: providers without a /v1/models endpoint must NOT be
    /// reported as verified on a 404 — the handler writes `verified = true`
    /// into the config from this outcome.
    #[test]
    fn interpret_404_fails_for_no_models_providers() {
        for pt in ["google_veo", "elevenlabs", "bfl", "suno", "midjourney"] {
            assert!(
                !interpret(pt, Some(404), None).success,
                "provider '{pt}' must not be verified on a 404"
            );
        }
        // …and a 404 is NOT a verification for providers that do have
        // /v1/models either: the URL is simply wrong.
        assert!(!interpret("openai_image", Some(404), None).success);
    }

    /// The Google providers carry their default endpoint in the provider
    /// module, not in their preset -- the probe has to ask the module.
    ///
    /// Without the fallback the probe answered "未配置 base_url，无法测试
    /// 连接" for a provider that was configured perfectly, and the Panel
    /// never let the operator mark it as the default.
    #[test]
    fn probe_url_falls_back_to_the_provider_default_endpoint() {
        let url = probe_url(None, "google").expect("google has a built-in endpoint");
        assert!(
            url.starts_with("https://generativelanguage.googleapis.com"),
            "unexpected google probe url: {url}"
        );
        let veo = probe_url(None, "veo").expect("google_veo has a built-in endpoint");
        assert_eq!(url, veo);
    }
}
