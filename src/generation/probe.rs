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
//! mean the credentials were rejected; any other response means the server is
//! reachable and did not reject the key.
//!
//! Lives in the generation domain (not the gateway) so the dependency
//! direction stays `gateway → generation` (P1) — gateway handlers never own
//! probe semantics.

use crate::config::types::generation::presets::get_preset_by_type;
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
/// the wrong header). Provider-type strings mirror `providers/factory.rs`.
fn auth_header_for(provider_type: &str, api_key: &str) -> (&'static str, String) {
    match provider_type {
        "deepgram_stt" | "deepgram" | "deepgram_tts" => {
            ("Authorization", format!("Token {api_key}"))
        }
        "azure_speech" | "azure_tts" => ("Ocp-Apim-Subscription-Key", api_key.to_string()),
        "bfl" | "bfl_flux" | "flux" => ("x-key", api_key.to_string()),
        "cartesia" => ("X-API-Key", api_key.to_string()),
        "google" | "google_imagen" | "imagen" | "google_veo" | "veo" => {
            ("x-goog-api-key", api_key.to_string())
        }
        "elevenlabs" => ("xi-api-key", api_key.to_string()),
        "fal" => ("Authorization", format!("Key {api_key}")),
        "volcengine_tts" => ("Authorization", format!("Bearer;{api_key}")),
        // OpenAI-compatible default: openai / openai_image / dalle / openai_tts
        // / tts / openai_whisper / whisper / openai_compat / stability / sdxl /
        // replicate / suno / midjourney / minimax_tts / …
        _ => ("Authorization", format!("Bearer {api_key}")),
    }
}

/// Resolve the cheap endpoint to GET for a connectivity probe.
///
/// Falls back to the preset's default base URL when the caller did not supply
/// one (e.g. testing a saved provider that relies on the preset default).
/// Returns `None` when no base URL can be determined at all (e.g. an
/// `openai_compat` provider with no `base_url` configured) — the caller turns
/// that into a clear error rather than a misleading "success".
fn probe_url(base_url: Option<&str>, provider_type: &str) -> Option<String> {
    let raw = base_url
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            get_preset_by_type(provider_type).and_then(|p| p.base_url.map(str::to_string))
        })?;

    match resolve_base_url(&raw) {
        ResolvedUrl::Standard(base) => Some(format!("{base}/v1/models")),
        ResolvedUrl::Custom(url) => Some(url),
    }
}

/// Map an HTTP status (or a transport error) onto a probe outcome. Pure so it
/// is exhaustively host-testable without a live network.
///
/// **404 is not success for no-models providers.** `google_veo`,
/// `elevenlabs`, `bfl`, `suno` and `midjourney` do not expose `GET /v1/models`
/// at all, so the generic "any non-401/403 status means success" rule
/// classified their 404 as "连接成功" and the handler wrote `verified = true`
/// into the config. The UI badge then showed "verified" for a credential that
/// has never been exercised against a real endpoint. For those provider types
/// a 404 means "we probed the wrong endpoint", not "credentials work".
fn interpret(
    provider_type: &str,
    status: Option<u16>,
    transport_err: Option<&str>,
) -> GenerationProbeOutcome {
    // Providers with no `/v1/models` endpoint: a 404 here is an artifact of
    // the probe URL, not evidence the credentials work.
    const NO_MODELS_ENDPOINT: &[&str] =
        &["google_veo", "veo", "elevenlabs", "bfl", "bfl_flux", "flux", "suno", "midjourney"];
    if matches!(status, Some(404)) && NO_MODELS_ENDPOINT.contains(&provider_type) {
        return GenerationProbeOutcome {
            success: false,
            message: "端点不提供 /v1/models，无法用轻量请求验证；请通过一次真实生成测试凭据"
                .to_string(),
        };
    }
    if let Some(e) = transport_err {
        return GenerationProbeOutcome {
            success: false,
            message: format!("无法连接到提供商: {e}"),
        };
    }
    match status {
        Some(401 | 403) => GenerationProbeOutcome {
            success: false,
            message: format!("认证失败：API 密钥被拒绝 (HTTP {})", status.unwrap_or(401)),
        },
        Some(s) => GenerationProbeOutcome {
            success: true,
            message: format!("连接成功 (HTTP {s})"),
        },
        None => GenerationProbeOutcome {
            success: false,
            message: "连接测试失败：未收到响应".to_string(),
        },
    }
}

/// Probe a generation provider with a lightweight authenticated `GET` and
/// classify the result. Never generates media, so it costs nothing.
pub async fn probe_generation_provider(
    provider_type: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> GenerationProbeOutcome {
    let Some(url) = probe_url(base_url, provider_type) else {
        return GenerationProbeOutcome {
            success: false,
            message: "未配置 base_url，无法测试连接".to_string(),
        };
    };

    let (header_name, header_value) = auth_header_for(provider_type, api_key);

    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return interpret(provider_type, None, Some(&e.to_string())),
    };

    match client
        .get(&url)
        .header(header_name, header_value)
        .send()
        .await
    {
        Ok(resp) => interpret(provider_type, Some(resp.status().as_u16()), None),
        Err(e) => interpret(provider_type, None, Some(&e.to_string())),
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

    #[test]
    fn interpret_reachable_succeeds() {
        // 200 (models listed), 404 (no /models but reachable), 405, 400 all mean
        // the server answered and did not reject the credentials.
        assert!(interpret("openai_image", Some(200), None).success);
        assert!(interpret("openai_image", Some(404), None).success);
        assert!(interpret("openai_image", Some(405), None).success);
        assert!(interpret("openai_image", Some(400), None).success);
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
        // …but a 404 is still fine for providers that DO have /v1/models.
        assert!(interpret("openai_image", Some(404), None).success);
    }
}
