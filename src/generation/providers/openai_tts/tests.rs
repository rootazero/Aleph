#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();

        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(
            provider.endpoint,
            format!("{}/v1/audio/speech", DEFAULT_ENDPOINT)
        );
        assert_eq!(provider.model, DEFAULT_MODEL);
        assert_eq!(provider.default_voice, DEFAULT_VOICE);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://custom.openai.com".to_string()),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            provider.endpoint,
            "https://custom.openai.com/v1/audio/speech"
        );
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("tts-1-hd".to_string()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(provider.model, "tts-1-hd");
    }

    #[test]
    fn test_new_with_custom_voice() {
        let provider =
            OpenAiTtsProvider::new("sk-test-key", None, None, Some("nova".to_string()), None)
                .unwrap();

        assert_eq!(provider.default_voice, "nova");
    }

    #[test]
    fn test_new_empty_api_key_fails() {
        let result = OpenAiTtsProvider::new("", None, None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_whitespace_api_key_fails() {
        let result = OpenAiTtsProvider::new("   ", None, None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_unknown_voice_succeeds() {
        // Unknown voices are allowed (with warning) for third-party/newer voices
        let result = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            None,
            Some("future-voice".to_string()),
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_unknown_model_succeeds() {
        // Unknown models are allowed (with warning) for newer API versions
        let result = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("gpt-4o-mini-tts".to_string()),
            None,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_speech_url() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        assert_eq!(
            provider.speech_url(),
            "https://api.openai.com/v1/audio/speech"
        );

        let custom_provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://api.example.com".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            custom_provider.speech_url(),
            "https://api.example.com/v1/audio/speech"
        );
    }

    #[test]
    fn test_url_normalization_with_v1_suffix() {
        // User provides URL with /v1 suffix - should NOT produce duplicate /v1
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://ai.t8star.cn/v1".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(provider.endpoint, "https://ai.t8star.cn/v1/audio/speech");
        assert_eq!(
            provider.speech_url(),
            "https://ai.t8star.cn/v1/audio/speech"
        );
    }

    #[test]
    fn test_url_normalization_with_trailing_slash() {
        let _provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://api.example.com/".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
    }

    #[test]
    fn test_url_normalization_with_v1_and_trailing_slash() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://api.example.com/v1/".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(provider.endpoint, "https://api.example.com/v1/audio/speech");
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        assert_eq!(provider.name(), "openai-tts");
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Speech));
    }

    #[test]
    fn test_supports() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        assert_eq!(provider.default_model(), Some("tts-1"));

        let custom_provider = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("tts-1-hd".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(custom_provider.default_model(), Some("tts-1-hd"));
    }

    // === Voice validation tests ===

    #[test]
    fn test_validate_voice_valid() {
        assert!(AVAILABLE_VOICES.contains(&"alloy"));
        assert!(AVAILABLE_VOICES.contains(&"echo"));
        assert!(AVAILABLE_VOICES.contains(&"fable"));
        assert!(AVAILABLE_VOICES.contains(&"onyx"));
        assert!(AVAILABLE_VOICES.contains(&"nova"));
        assert!(AVAILABLE_VOICES.contains(&"shimmer"));
    }

    #[test]
    fn test_validate_voice_invalid() {
        assert!(!AVAILABLE_VOICES.contains(&"invalid"));
        assert!(!AVAILABLE_VOICES.contains(&""));
        assert!(!AVAILABLE_VOICES.contains(&"ALLOY")); // Case sensitive
    }

    // === Content type tests ===

    #[test]
    fn test_content_type_for_format() {
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("mp3")),
            "audio/mpeg"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("opus")),
            "audio/opus"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("aac")),
            "audio/aac"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("flac")),
            "audio/flac"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(None),
            "audio/mpeg"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("unknown")),
            "audio/mpeg"
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "tts-1");
        assert_eq!(body.input, "Hello world");
        assert_eq!(body.voice, "alloy");
        assert!(body.response_format.is_none());
        assert!(body.speed.is_none());
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world").with_params(
            GenerationParams::builder()
                .model("tts-1-hd")
                .voice("nova")
                .format("opus")
                .speed(1.5)
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "tts-1-hd");
        assert_eq!(body.input, "Hello world");
        assert_eq!(body.voice, "nova");
        assert_eq!(body.response_format, Some("opus".to_string()));
        assert_eq!(body.speed, Some(1.5));
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::UNAUTHORIZED, "Unauthorized");

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let error = provider.parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_bad_request_empty_input() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let error = provider.parse_error_response(
            reqwest::StatusCode::BAD_REQUEST,
            "input is required and cannot be empty",
        );

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None, None).unwrap();
        let error = provider.parse_error_response(
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
    fn test_request_serialization_minimal() {
        let request = types::TtsRequest {
            model: "tts-1".to_string(),
            input: "Hello world".to_string(),
            voice: "alloy".to_string(),
            response_format: None,
            speed: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"tts-1\""));
        assert!(json.contains("\"input\":\"Hello world\""));
        assert!(json.contains("\"voice\":\"alloy\""));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"response_format\""));
        assert!(!json.contains("\"speed\""));
    }

    #[test]
    fn test_request_serialization_full() {
        let request = types::TtsRequest {
            model: "tts-1-hd".to_string(),
            input: "Hello world".to_string(),
            voice: "nova".to_string(),
            response_format: Some("opus".to_string()),
            speed: Some(1.5),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"tts-1-hd\""));
        assert!(json.contains("\"voice\":\"nova\""));
        assert!(json.contains("\"response_format\":\"opus\""));
        assert!(json.contains("\"speed\":1.5"));
    }

    // === Constants tests ===

    #[test]
    fn test_available_voices() {
        assert_eq!(AVAILABLE_VOICES.len(), 6);
        assert!(AVAILABLE_VOICES.contains(&"alloy"));
        assert!(AVAILABLE_VOICES.contains(&"echo"));
        assert!(AVAILABLE_VOICES.contains(&"fable"));
        assert!(AVAILABLE_VOICES.contains(&"onyx"));
        assert!(AVAILABLE_VOICES.contains(&"nova"));
        assert!(AVAILABLE_VOICES.contains(&"shimmer"));
    }

    #[test]
    fn test_available_models() {
        assert_eq!(AVAILABLE_MODELS.len(), 2);
        assert!(AVAILABLE_MODELS.contains(&"tts-1"));
        assert!(AVAILABLE_MODELS.contains(&"tts-1-hd"));
    }

    #[test]
    fn test_available_formats() {
        assert_eq!(AVAILABLE_FORMATS.len(), 4);
        assert!(AVAILABLE_FORMATS.contains(&"mp3"));
        assert!(AVAILABLE_FORMATS.contains(&"opus"));
        assert!(AVAILABLE_FORMATS.contains(&"aac"));
        assert!(AVAILABLE_FORMATS.contains(&"flac"));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAiTtsProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(OpenAiTtsProvider::new("sk-test", None, None, None, None).unwrap());

        assert_eq!(provider.name(), "openai-tts");
        assert!(provider.supports(GenerationType::Speech));
    }
}
