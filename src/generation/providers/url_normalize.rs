//! Shared URL normalization for generation providers.
//!
//! Standard URLs (domain-only or domain+/v1) get auto-completed with
//! the appropriate endpoint path. Custom full URLs are used as-is.

use crate::generation::GenerationType;

/// Resolved URL — either a standard base that derives endpoints,
/// or a custom full URL used as-is.
#[derive(Debug, Clone)]
pub enum ResolvedUrl {
    /// Standard OpenAI-compatible base URL.
    /// All operation endpoints derived automatically.
    Standard(String),
    /// Custom full URL. Used as-is for primary operation only.
    Custom(String),
}

impl ResolvedUrl {
    /// Get the primary endpoint URL for the given generation type.
    pub fn primary_endpoint(&self, gen_type: GenerationType) -> String {
        match self {
            ResolvedUrl::Custom(url) => url.clone(),
            ResolvedUrl::Standard(base) => {
                let suffix = match gen_type {
                    GenerationType::Image => "/v1/images/generations",
                    GenerationType::Video => "/v1/videos/generations",
                    GenerationType::Speech => "/v1/audio/speech",
                    GenerationType::Audio => "/v1/audio/generations",
                    GenerationType::Transcription => "/v1/audio/transcriptions",
                };
                format!("{}{}", base, suffix)
            }
        }
    }

    /// Get the secondary endpoint URL (edit for image, STT for speech).
    /// Returns None for custom URLs or types without secondary endpoints.
    pub fn secondary_endpoint(&self, gen_type: GenerationType) -> Option<String> {
        match self {
            ResolvedUrl::Custom(_) => None,
            ResolvedUrl::Standard(base) => {
                let suffix = match gen_type {
                    GenerationType::Image => Some("/v1/images/edits"),
                    _ => None,
                };
                suffix.map(|s| format!("{}{}", base, s))
            }
        }
    }
}

/// Resolve a user-configured URL into a ResolvedUrl.
///
/// Rules:
/// - Domain-only (no path after scheme) → Standard (auto-complete)
/// - Domain + /v1 → Standard (auto-complete)
/// - Anything else → Custom (use as-is)
pub fn resolve_base_url(url: &str) -> ResolvedUrl {
    let trimmed = url.trim_end_matches('/');
    if needs_auto_complete(trimmed) {
        let base = trimmed.trim_end_matches("/v1").trim_end_matches('/');
        ResolvedUrl::Standard(base.to_string())
    } else {
        ResolvedUrl::Custom(trimmed.to_string())
    }
}

/// Check if a URL is a standard base that needs endpoint path auto-completion.
///
/// Standard: domain-only (no `/` in path) or domain + `/v1`.
/// Everything else is treated as a custom full URL.
fn needs_auto_complete(url: &str) -> bool {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // No slash at all = pure domain, or ends with /v1 = standard base
    !after_scheme.contains('/') || after_scheme.ends_with("/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_only_is_standard() {
        let r = resolve_base_url("https://api.example.com");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_domain_with_v1_is_standard() {
        let r = resolve_base_url("https://api.example.com/v1");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_domain_with_v1_trailing_slash() {
        let r = resolve_base_url("https://api.example.com/v1/");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_full_path_is_custom() {
        let r = resolve_base_url("https://api.example.com/v2/videos/generations");
        assert!(
            matches!(r, ResolvedUrl::Custom(ref u) if u == "https://api.example.com/v2/videos/generations")
        );
    }

    #[test]
    fn test_custom_path_is_custom() {
        let r = resolve_base_url("https://api.example.com/custom/tts");
        assert!(
            matches!(r, ResolvedUrl::Custom(ref u) if u == "https://api.example.com/custom/tts")
        );
    }

    #[test]
    fn test_primary_endpoint_image() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(
            r.primary_endpoint(GenerationType::Image),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_primary_endpoint_speech() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(
            r.primary_endpoint(GenerationType::Speech),
            "https://api.example.com/v1/audio/speech"
        );
    }

    #[test]
    fn test_primary_endpoint_video() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(
            r.primary_endpoint(GenerationType::Video),
            "https://api.example.com/v1/videos/generations"
        );
    }

    #[test]
    fn test_primary_endpoint_audio() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(
            r.primary_endpoint(GenerationType::Audio),
            "https://api.example.com/v1/audio/generations"
        );
    }

    #[test]
    fn test_secondary_endpoint_image_edit() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(
            r.secondary_endpoint(GenerationType::Image),
            Some("https://api.example.com/v1/images/edits".into())
        );
    }

    #[test]
    fn test_primary_endpoint_transcription() {
        let r = resolve_base_url("https://api.openai.com");
        assert_eq!(
            r.primary_endpoint(GenerationType::Transcription),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn test_secondary_endpoint_video_none() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Video), None);
    }

    #[test]
    fn test_custom_url_primary() {
        let r = ResolvedUrl::Custom("https://custom.api.com/my/endpoint".into());
        assert_eq!(
            r.primary_endpoint(GenerationType::Speech),
            "https://custom.api.com/my/endpoint"
        );
    }

    #[test]
    fn test_custom_url_no_secondary() {
        let r = ResolvedUrl::Custom("https://custom.api.com/my/endpoint".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Image), None);
    }
}
