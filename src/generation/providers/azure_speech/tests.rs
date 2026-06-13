//! Unit tests for the Azure Speech provider. Network integration is exercised
//! manually with a real subscription key.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = AzureSpeechProvider::new("   ", Some("eastus".into()), None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn rejects_missing_region() {
    let err = AzureSpeechProvider::new("k", None, None).unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[test]
fn rejects_empty_region() {
    let err = AzureSpeechProvider::new("k", Some("   ".into()), None).unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[test]
fn region_name_expands_to_full_endpoint() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    assert_eq!(
        p.endpoint,
        "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
    );
}

#[test]
fn full_url_is_preserved_with_path_appended() {
    let p = AzureSpeechProvider::new(
        "k",
        Some("https://eastus.tts.speech.microsoft.com".into()),
        None,
    )
    .unwrap();
    assert!(p.endpoint.ends_with("/cognitiveservices/v1"));
}

#[test]
fn full_url_with_path_is_preserved_verbatim() {
    let p = AzureSpeechProvider::new(
        "k",
        Some("https://my-proxy.example.com/cognitiveservices/v1".into()),
        None,
    )
    .unwrap();
    assert_eq!(
        p.endpoint,
        "https://my-proxy.example.com/cognitiveservices/v1"
    );
}

#[test]
fn default_voice_is_jenny_us() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    assert_eq!(p.default_model(), Some("en-US-JennyNeural"));
}

#[test]
fn supports_speech_only() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    assert!(p.supports(GenerationType::Speech));
    assert!(!p.supports(GenerationType::Transcription));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    let request = GenerationRequest::image("a cat");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_empty_input() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    let request = GenerationRequest::speech("");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[tokio::test]
async fn rejects_oversized_input() {
    let text = "a".repeat(MAX_INPUT_CHARS + 1);
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    let request = GenerationRequest::speech(text);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[tokio::test]
async fn rejects_unknown_format() {
    let p = AzureSpeechProvider::new("k", Some("eastus".into()), None).unwrap();
    let params = GenerationParams::builder().format("flac").build();
    let request = GenerationRequest::speech("hi").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedFormatError { .. }
    ));
}

#[test]
fn locale_extracted_from_voice_name() {
    assert_eq!(locale_from_voice("en-US-JennyNeural"), "en-US");
    assert_eq!(locale_from_voice("zh-CN-XiaoxiaoNeural"), "zh-CN");
    assert_eq!(locale_from_voice("badname"), "en-US"); // fallback
}

#[test]
fn build_ssml_wraps_plain_text() {
    let ssml = build_ssml("Hello & welcome", "en-US-JennyNeural", None);
    assert!(ssml.contains("xml:lang=\"en-US\""));
    assert!(ssml.contains("name=\"en-US-JennyNeural\""));
    // Ampersand should be escaped to satisfy XML parser.
    assert!(ssml.contains("Hello &amp; welcome"));
    assert!(!ssml.contains("Hello & welcome"));
    // No speed → no prosody wrapper.
    assert!(!ssml.contains("<prosody"));
}

#[test]
fn build_ssml_passes_through_raw_ssml() {
    let raw = r#"<speak version="1.0"><voice name="en-US-AriaNeural"><prosody rate="+10%">Hi</prosody></voice></speak>"#;
    let ssml = build_ssml(raw, "en-US-JennyNeural", Some(1.5));
    // Raw SSML is preserved verbatim even when a speed is supplied.
    assert_eq!(ssml, raw);
}

#[test]
fn build_ssml_wraps_speed_in_prosody() {
    let ssml = build_ssml("Hi", "en-US-JennyNeural", Some(1.5));
    assert!(ssml.contains(r#"<prosody rate="1.5">Hi</prosody>"#));
}

#[test]
fn build_ssml_clamps_out_of_range_speed() {
    // Above the 2.0 ceiling clamps to 2; below the 0.5 floor clamps to 0.5.
    assert!(build_ssml("Hi", "en-US-JennyNeural", Some(9.0)).contains(r#"rate="2""#));
    assert!(build_ssml("Hi", "en-US-JennyNeural", Some(0.1)).contains(r#"rate="0.5""#));
}

#[test]
fn map_output_format_defaults_to_mp3() {
    assert_eq!(map_output_format(None), DEFAULT_OUTPUT_FORMAT);
    assert_eq!(map_output_format(Some("mp3")), DEFAULT_OUTPUT_FORMAT);
    assert_eq!(map_output_format(Some("wav")), "riff-24khz-16bit-mono-pcm");
    assert_eq!(map_output_format(Some("opus")), "ogg-24khz-16bit-mono-opus");
}

#[test]
fn xml_escape_covers_all_five_entities() {
    assert_eq!(
        xml_escape(r#"<a href="x">A&B's</a>"#),
        "&lt;a href=&quot;x&quot;&gt;A&amp;B&apos;s&lt;/a&gt;"
    );
}

#[test]
fn static_voice_list_includes_multiple_locales() {
    let voices = AzureSpeechProvider::static_voice_list();
    assert!(voices.iter().any(|v| v.id.starts_with("en-US-")));
    assert!(voices.iter().any(|v| v.id.starts_with("zh-CN-")));
    assert!(voices.iter().any(|v| v.id.starts_with("ja-JP-")));
}
