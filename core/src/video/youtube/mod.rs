//! YouTube transcript extraction
//!
//! Extracts transcripts from YouTube videos by:
//! 1. Parsing video page HTML to find player response
//! 2. Extracting caption track URLs
//! 3. Fetching and parsing transcript XML
//!
//! # Module Structure
//!
//! - `url`: URL parsing and detection utilities
//! - `parser`: Transcript format parsing (XML, JSON3, VTT)
//! - `caption`: Caption fetching with yt-dlp fallback
//! - `extractor`: Main YouTubeExtractor implementation

mod caption;
mod extractor;
mod parser;
mod url;

// Re-exports for backward compatibility
pub use extractor::YouTubeExtractor;
pub use url::extract_youtube_url;

#[cfg(test)]
mod tests {
    use super::*;
    use parser::{decode_html_entities, extract_json_object, parse_transcript_xml};
    use super::url::{contains_youtube_url, parse_video_id};

    #[test]
    fn test_parse_video_id_standard_url() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let id = parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_short_url() {
        let url = "https://youtu.be/dQw4w9WgXcQ";
        let id = parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_embed_url() {
        let url = "https://youtube.com/embed/dQw4w9WgXcQ";
        let id = parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_with_timestamp() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=152s";
        let id = parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_invalid_url() {
        let url = "https://example.com/video";
        let result = parse_video_id(url);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_youtube_url_from_text() {
        let input = "Please analyze this video: https://youtube.com/watch?v=abc12345678 thanks!";
        let url = extract_youtube_url(input);
        assert_eq!(
            url,
            Some("https://youtube.com/watch?v=abc12345678".to_string())
        );
    }

    #[test]
    fn test_extract_youtube_url_short_format() {
        let input = "Check out https://youtu.be/abc12345678";
        let url = extract_youtube_url(input);
        assert_eq!(url, Some("https://youtu.be/abc12345678".to_string()));
    }

    #[test]
    fn test_extract_youtube_url_no_match() {
        let input = "No video URL here";
        let url = extract_youtube_url(input);
        assert!(url.is_none());
    }

    #[test]
    fn test_contains_youtube_url() {
        assert!(contains_youtube_url(
            "https://youtube.com/watch?v=abc12345678"
        ));
        assert!(contains_youtube_url(
            "Check https://youtu.be/abc12345678 out"
        ));
        assert!(!contains_youtube_url("No URL here"));
    }

    #[test]
    fn test_decode_html_entities() {
        assert_eq!(
            decode_html_entities("Hello &amp; World"),
            "Hello & World"
        );
        assert_eq!(
            decode_html_entities("&lt;tag&gt;"),
            "<tag>"
        );
        assert_eq!(
            decode_html_entities("It&#39;s great"),
            "It's great"
        );
    }

    #[test]
    fn test_parse_transcript_xml() {
        let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
<transcript>
<text start="0.0" dur="2.5">Hello everyone</text>
<text start="2.5" dur="3.0">Welcome to the video</text>
</transcript>"#;

        let segments = parse_transcript_xml(xml).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello everyone");
        assert!((segments[0].start_seconds - 0.0).abs() < 0.001);
        assert!((segments[0].duration_seconds - 2.5).abs() < 0.001);
        assert_eq!(segments[1].text, "Welcome to the video");
    }

    #[test]
    fn test_parse_transcript_xml_with_entities() {
        let xml = r#"<transcript>
<text start="0.0" dur="1.0">Hello &amp; goodbye</text>
</transcript>"#;

        let segments = parse_transcript_xml(xml).unwrap();
        assert_eq!(segments[0].text, "Hello & goodbye");
    }

    #[test]
    fn test_extract_json_object() {
        let input = r#"{"key": "value"};var x = 1;"#;
        let json = extract_json_object(input);
        assert_eq!(json, Some(r#"{"key": "value"}"#.to_string()));
    }

    #[test]
    fn test_extract_json_object_nested() {
        let input = r#"{"outer": {"inner": "value"}};"#;
        let json = extract_json_object(input);
        assert_eq!(json, Some(r#"{"outer": {"inner": "value"}}"#.to_string()));
    }

    #[test]
    fn test_extract_title() {
        let response = serde_json::json!({
            "videoDetails": {
                "title": "My Video Title"
            }
        });
        let title = YouTubeExtractor::extract_title(&response);
        assert_eq!(title, "My Video Title");
    }

    #[test]
    fn test_extract_title_missing() {
        let response = serde_json::json!({});
        let title = YouTubeExtractor::extract_title(&response);
        assert_eq!(title, "Unknown Video");
    }

    #[test]
    fn test_find_caption_url_preferred_language() {
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "ja", "baseUrl": "https://example.com/ja"},
                        {"languageCode": "en", "baseUrl": "https://example.com/en"},
                        {"languageCode": "zh", "baseUrl": "https://example.com/zh"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "en").unwrap();
        assert_eq!(url, "https://example.com/en");
        assert_eq!(lang, "en");
    }

    #[test]
    fn test_find_caption_url_fallback_to_english() {
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "ja", "baseUrl": "https://example.com/ja"},
                        {"languageCode": "en", "baseUrl": "https://example.com/en"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "zh").unwrap();
        assert_eq!(url, "https://example.com/en");
        assert_eq!(lang, "en");
    }

    #[test]
    fn test_find_caption_url_fallback_to_first() {
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "ja", "baseUrl": "https://example.com/ja"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "en").unwrap();
        assert_eq!(url, "https://example.com/ja");
        assert_eq!(lang, "ja");
    }

    #[test]
    fn test_find_caption_url_no_captions() {
        let response = serde_json::json!({});
        let result = YouTubeExtractor::find_caption_url(&response, "en");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_caption_url_chinese_variants() {
        // Test that "zh" matches "zh-Hans" (Simplified Chinese)
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "en", "baseUrl": "https://example.com/en"},
                        {"languageCode": "zh-Hans", "baseUrl": "https://example.com/zh-hans"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "zh").unwrap();
        assert_eq!(url, "https://example.com/zh-hans");
        assert_eq!(lang, "zh-Hans");

        // Test that "zh" matches "zh-Hant" (Traditional Chinese)
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "en", "baseUrl": "https://example.com/en"},
                        {"languageCode": "zh-Hant", "baseUrl": "https://example.com/zh-hant"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "zh").unwrap();
        assert_eq!(url, "https://example.com/zh-hant");
        assert_eq!(lang, "zh-Hant");

        // Test that "zh" matches "zh-CN"
        let response = serde_json::json!({
            "captions": {
                "playerCaptionsTracklistRenderer": {
                    "captionTracks": [
                        {"languageCode": "zh-CN", "baseUrl": "https://example.com/zh-cn"}
                    ]
                }
            }
        });

        let (url, lang) = YouTubeExtractor::find_caption_url(&response, "zh").unwrap();
        assert_eq!(url, "https://example.com/zh-cn");
        assert_eq!(lang, "zh-CN");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::config::VideoConfig;

    #[tokio::test]
    #[ignore] // Run with: cargo test test_real_youtube_extraction -- --ignored --nocapture
    async fn test_real_youtube_extraction() {
        let config = VideoConfig::default();
        let extractor = YouTubeExtractor::new(config);

        // Famous test video with captions
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

        println!("Testing URL: {}", url);

        match extractor.extract_transcript(url).await {
            Ok(transcript) => {
                println!("Success!");
                println!("Title: {}", transcript.title);
                println!("Language: {}", transcript.language);
                println!("Segments: {}", transcript.segments.len());
                let context = transcript.format_for_context();
                let preview: String = context.chars().take(200).collect();
                println!("First 200 chars: {}", preview);
            }
            Err(e) => {
                println!("Failed: {:?}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_debug_youtube_extraction() {
        let config = VideoConfig::default();
        let extractor = YouTubeExtractor::new(config);

        let video_id = "dQw4w9WgXcQ";
        let url = format!("https://www.youtube.com/watch?v={}", video_id);

        println!("1. Fetching video page: {}", url);
        let html = extractor.fetch_page(&url).await.unwrap();
        println!("   HTML length: {} bytes", html.len());

        // Extract player response
        let player_response = YouTubeExtractor::extract_player_response(&html).unwrap();
        println!("2. Parsed player response successfully");

        // Get title
        let title = YouTubeExtractor::extract_title(&player_response);
        println!("3. Title: {}", title);

        // Find caption URL
        let (caption_url, lang) =
            YouTubeExtractor::find_caption_url(&player_response, "en").unwrap();
        println!("4. Caption URL found for language: {}", lang);
        let url_preview: String = caption_url.chars().take(100).collect();
        println!("   URL preview: {}...", url_preview);
    }
}
