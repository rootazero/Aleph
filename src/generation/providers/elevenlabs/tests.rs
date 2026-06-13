#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert_eq!(provider.api_key, "xi-test-key");
        assert_eq!(provider.endpoint, constants::DEFAULT_ENDPOINT);
        assert_eq!(provider.model, constants::DEFAULT_MODEL);
        assert_eq!(provider.default_voice_id, constants::DEFAULT_VOICE_ID);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = ElevenLabsProvider::new(
            "xi-test-key",
            Some("https://custom.elevenlabs.io".to_string()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(provider.endpoint, "https://custom.elevenlabs.io");
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = ElevenLabsProvider::new(
            "xi-test-key",
            None,
            Some("eleven_multilingual_v2".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(provider.model, "eleven_multilingual_v2");
    }

    #[test]
    fn test_new_with_custom_voice_name() {
        let provider =
            ElevenLabsProvider::new("xi-test-key", None, None, Some("josh".to_string())).unwrap();

        assert_eq!(provider.default_voice_id, "TxGEqnHWrfWFTfGW9XjX");
    }

    #[test]
    fn test_new_with_voice_id_directly() {
        let voice_id = "CustomVoiceId12345678";
        let provider =
            ElevenLabsProvider::new("xi-test-key", None, None, Some(voice_id.to_string())).unwrap();

        assert_eq!(provider.default_voice_id, voice_id);
    }

    #[test]
    fn test_new_with_unknown_voice_fails() {
        let result =
            ElevenLabsProvider::new("xi-test-key", None, None, Some("unknown-voice".to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_new_empty_api_key_fails() {
        let result = ElevenLabsProvider::new("", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_whitespace_api_key_fails() {
        let result = ElevenLabsProvider::new("   ", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_unknown_model_passes_through() {
        // Unknown models are accepted (forward-compat); the provider only logs
        // a warning rather than rejecting, so newer ElevenLabs models work
        // without a code change.
        let provider = ElevenLabsProvider::new(
            "xi-test-key",
            None,
            Some("eleven_v99_future".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(provider.model, "eleven_v99_future");
    }

    // === Voice resolution tests ===

    #[test]
    fn test_resolve_voice_id_by_name() {
        let result = ElevenLabsProvider::resolve_voice_id("rachel").unwrap();
        assert_eq!(result, "21m00Tcm4TlvDq8ikWAM");

        let result = ElevenLabsProvider::resolve_voice_id("josh").unwrap();
        assert_eq!(result, "TxGEqnHWrfWFTfGW9XjX");
    }

    #[test]
    fn test_resolve_voice_id_case_insensitive() {
        let lower = ElevenLabsProvider::resolve_voice_id("rachel").unwrap();
        let upper = ElevenLabsProvider::resolve_voice_id("RACHEL").unwrap();
        let mixed = ElevenLabsProvider::resolve_voice_id("RaChEl").unwrap();

        assert_eq!(lower, upper);
        assert_eq!(upper, mixed);
        assert_eq!(lower, "21m00Tcm4TlvDq8ikWAM");
    }

    #[test]
    fn test_resolve_voice_id_passthrough() {
        // Long alphanumeric strings should be passed through as-is
        let voice_id = "21m00Tcm4TlvDq8ikWAM";
        let result = ElevenLabsProvider::resolve_voice_id(voice_id).unwrap();
        assert_eq!(result, voice_id);

        // Custom voice ID
        let custom_id = "CustomVoiceId12345678";
        let result = ElevenLabsProvider::resolve_voice_id(custom_id).unwrap();
        assert_eq!(result, custom_id);
    }

    #[test]
    fn test_resolve_voice_id_unknown_fails() {
        let result = ElevenLabsProvider::resolve_voice_id("unknown");
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.name(), "elevenlabs");
    }

    #[test]
    fn test_color() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.color(), "#00c7b7");
    }

    #[test]
    fn test_default_model() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.default_model(), Some("eleven_monolingual_v1"));

        let custom_provider = ElevenLabsProvider::new(
            "xi-test-key",
            None,
            Some("eleven_turbo_v2".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(custom_provider.default_model(), Some("eleven_turbo_v2"));
    }

    #[test]
    fn test_supported_types() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Speech));
    }

    #[test]
    fn test_supports_speech() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_does_not_support_image() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    // === URL building tests ===

    #[test]
    fn test_tts_url() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let url = provider.tts_url("21m00Tcm4TlvDq8ikWAM");
        assert_eq!(
            url,
            "https://api.elevenlabs.io/v1/text-to-speech/21m00Tcm4TlvDq8ikWAM"
        );

        let custom_provider = ElevenLabsProvider::new(
            "xi-test-key",
            Some("https://custom.api.io".to_string()),
            None,
            None,
        )
        .unwrap();
        let url = custom_provider.tts_url("voice123");
        assert_eq!(url, "https://custom.api.io/v1/text-to-speech/voice123");
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world");

        let body = provider.build_request_body(&request);

        assert_eq!(body.text, "Hello world");
        assert_eq!(body.model_id, "eleven_monolingual_v1");
        assert_eq!(body.voice_settings.stability, 0.5);
        assert_eq!(body.voice_settings.similarity_boost, 0.75);
        assert!(body.voice_settings.style.is_none());
        assert!(body.voice_settings.use_speaker_boost.is_none());
        assert!(body.voice_settings.speed.is_none());
    }

    #[test]
    fn test_build_request_body_speed_clamped_from_params() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        // 1.5 is above ElevenLabs' 1.2 ceiling and must clamp.
        let fast = GenerationRequest::speech("Hi")
            .with_params(GenerationParams::builder().speed(1.5).build());
        assert_eq!(
            provider.build_request_body(&fast).voice_settings.speed,
            Some(1.2)
        );
        // 0.3 is below the 0.7 floor and must clamp.
        let slow = GenerationRequest::speech("Hi")
            .with_params(GenerationParams::builder().speed(0.3).build());
        assert_eq!(
            provider.build_request_body(&slow).voice_settings.speed,
            Some(0.7)
        );
    }

    #[test]
    fn test_build_request_body_voice_settings_from_extra() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hi").with_params(
            GenerationParams::builder()
                .extra("stability", serde_json::json!(0.9))
                .extra("similarity_boost", serde_json::json!(0.4))
                .extra("style", serde_json::json!(0.6))
                .extra("use_speaker_boost", serde_json::json!(true))
                .build(),
        );

        let body = provider.build_request_body(&request);
        assert_eq!(body.voice_settings.stability, 0.9);
        assert_eq!(body.voice_settings.similarity_boost, 0.4);
        assert_eq!(body.voice_settings.style, Some(0.6));
        assert_eq!(body.voice_settings.use_speaker_boost, Some(true));
    }

    #[test]
    fn test_build_request_body_with_model() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world")
            .with_params(GenerationParams::builder().model("eleven_turbo_v2").build());

        let body = provider.build_request_body(&request);

        assert_eq!(body.model_id, "eleven_turbo_v2");
    }

    // === Content type tests ===

    #[test]
    fn test_content_type_for_format() {
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("mp3_44100_128")),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("mp3_44100_192")),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("pcm_16000")),
            "audio/pcm"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("pcm_44100")),
            "audio/pcm"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(None),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("unknown")),
            "audio/mpeg"
        );
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_quota_exceeded() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            "Payment required",
        );

        assert!(matches!(error, GenerationError::QuotaExceededError { .. }));
    }

    #[test]
    fn test_parse_error_response_validation() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "Validation error",
        );

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        );

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization() {
        let request = types::TtsRequest {
            text: "Hello world".to_string(),
            model_id: "eleven_monolingual_v1".to_string(),
            voice_settings: types::VoiceSettings {
                stability: 0.5,
                similarity_boost: 0.75,
                style: None,
                use_speaker_boost: None,
                speed: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"model_id\":\"eleven_monolingual_v1\""));
        assert!(json.contains("\"stability\":0.5"));
        assert!(json.contains("\"similarity_boost\":0.75"));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"style\""));
        assert!(!json.contains("\"use_speaker_boost\""));
        assert!(!json.contains("\"speed\""));
    }

    #[test]
    fn test_request_serialization_with_optional_fields() {
        let request = types::TtsRequest {
            text: "Hello world".to_string(),
            model_id: "eleven_multilingual_v2".to_string(),
            voice_settings: types::VoiceSettings {
                stability: 0.6,
                similarity_boost: 0.8,
                style: Some(0.3),
                use_speaker_boost: Some(true),
                speed: Some(1.1),
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"style\":0.3"));
        assert!(json.contains("\"use_speaker_boost\":true"));
        assert!(json.contains("\"speed\":1.1"));
    }

    // === Constants tests ===

    #[test]
    fn test_voices_list() {
        assert_eq!(constants::VOICES.len(), 9);

        // Verify all expected voices are present
        let voice_names: Vec<&str> = constants::VOICES.iter().map(|(name, _)| *name).collect();
        assert!(voice_names.contains(&"rachel"));
        assert!(voice_names.contains(&"domi"));
        assert!(voice_names.contains(&"bella"));
        assert!(voice_names.contains(&"antoni"));
        assert!(voice_names.contains(&"elli"));
        assert!(voice_names.contains(&"josh"));
        assert!(voice_names.contains(&"arnold"));
        assert!(voice_names.contains(&"adam"));
        assert!(voice_names.contains(&"sam"));
    }

    #[test]
    fn test_models_list() {
        assert_eq!(constants::MODELS.len(), 8);
        assert!(constants::MODELS.contains(&"eleven_monolingual_v1"));
        assert!(constants::MODELS.contains(&"eleven_multilingual_v2"));
        assert!(constants::MODELS.contains(&"eleven_flash_v2_5"));
        assert!(constants::MODELS.contains(&"eleven_turbo_v2_5"));
        assert!(constants::MODELS.contains(&"eleven_turbo_v2"));
    }

    #[test]
    fn test_output_formats_list() {
        assert_eq!(constants::OUTPUT_FORMATS.len(), 6);
        assert!(constants::OUTPUT_FORMATS.contains(&"mp3_44100_128"));
        assert!(constants::OUTPUT_FORMATS.contains(&"mp3_44100_192"));
        assert!(constants::OUTPUT_FORMATS.contains(&"pcm_16000"));
        assert!(constants::OUTPUT_FORMATS.contains(&"pcm_22050"));
        assert!(constants::OUTPUT_FORMATS.contains(&"pcm_24000"));
        assert!(constants::OUTPUT_FORMATS.contains(&"pcm_44100"));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ElevenLabsProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(ElevenLabsProvider::new("xi-test", None, None, None).unwrap());

        assert_eq!(provider.name(), "elevenlabs");
        assert!(provider.supports(GenerationType::Speech));
    }
}
