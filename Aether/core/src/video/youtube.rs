//! YouTube transcript extraction
//!
//! Extracts transcripts from YouTube videos by:
//! 1. Parsing video page HTML to find player response
//! 2. Extracting caption track URLs
//! 3. Fetching and parsing transcript XML

use crate::config::VideoConfig;
use crate::error::{AetherError, Result};
use crate::video::transcript::{TranscriptSegment, VideoTranscript};
use regex::Regex;
use std::sync::LazyLock;
use std::time::Duration;
use tracing::{debug, info};

/// Regex pattern for matching YouTube URLs and extracting video IDs
static YOUTUBE_URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/|youtube\.com/v/)([a-zA-Z0-9_-]{11})"
    ).expect("Invalid YouTube URL regex")
});

/// Regex pattern for detecting YouTube URLs in text (looser matching)
static YOUTUBE_DETECT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:https?://)?(?:www\.)?(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/)[a-zA-Z0-9_-]{11}"
    ).expect("Invalid YouTube detect regex")
});

/// Extract YouTube URL from user input text
///
/// Returns the first YouTube URL found in the input, or None if no URL is found.
pub fn extract_youtube_url(input: &str) -> Option<String> {
    YOUTUBE_DETECT_REGEX
        .find(input)
        .map(|m| m.as_str().to_string())
}

/// Check if the input contains a YouTube URL
pub fn contains_youtube_url(input: &str) -> bool {
    YOUTUBE_DETECT_REGEX.is_match(input)
}

/// YouTube transcript extractor
pub struct YouTubeExtractor {
    client: reqwest::Client,
    config: VideoConfig,
}

impl YouTubeExtractor {
    /// Create a new YouTube extractor with the given configuration
    pub fn new(config: VideoConfig) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(15))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    /// Extract transcript from a YouTube URL
    ///
    /// # Arguments
    /// * `url` - A YouTube video URL
    ///
    /// # Returns
    /// * `Ok(VideoTranscript)` - The extracted transcript
    /// * `Err(AetherError)` - If extraction fails
    pub async fn extract_transcript(&self, url: &str) -> Result<VideoTranscript> {
        // 1. Parse video ID from URL
        let video_id = Self::parse_video_id(url)?;
        info!(video_id = %video_id, "Extracting YouTube transcript");

        // 2. Fetch video page
        let page_url = format!("https://www.youtube.com/watch?v={}", video_id);
        let html = self.fetch_page(&page_url).await?;

        // 3. Extract player response JSON
        let player_response = Self::extract_player_response(&html)?;

        // 4. Extract video title
        let title = Self::extract_title(&player_response);

        // 5. Find caption track URL
        let (caption_url, actual_language) =
            Self::find_caption_url(&player_response, &self.config.preferred_language)?;

        // 6. Fetch and parse transcript
        let transcript_data = self.fetch_page(&caption_url).await?;
        let segments = Self::parse_transcript_data(&transcript_data)?;

        info!(
            video_id = %video_id,
            title = %title,
            segments = segments.len(),
            language = %actual_language,
            "Successfully extracted transcript"
        );

        // 7. Create transcript and truncate if needed
        let mut transcript =
            VideoTranscript::new(video_id.to_string(), title, actual_language, segments);

        if self.config.max_transcript_length > 0 {
            transcript.truncate_to_chars(self.config.max_transcript_length);
            if transcript.was_truncated {
                debug!(
                    max_chars = self.config.max_transcript_length,
                    "Transcript was truncated"
                );
            }
        }

        Ok(transcript)
    }

    /// Fetch a page with error handling
    async fn fetch_page(&self, url: &str) -> Result<String> {
        debug!(url = %url, "Fetching page");

        let response = self.client.get(url).send().await.map_err(|e| {
            AetherError::video(format!("Failed to fetch page: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(AetherError::video(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        response.text().await.map_err(|e| {
            AetherError::video(format!("Failed to read response: {}", e))
        })
    }

    /// Parse video ID from a YouTube URL
    pub fn parse_video_id(url: &str) -> Result<String> {
        YOUTUBE_URL_REGEX
            .captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| AetherError::video(format!("Invalid YouTube URL: {}", url)))
    }

    /// Extract ytInitialPlayerResponse from page HTML
    fn extract_player_response(html: &str) -> Result<serde_json::Value> {
        // Try multiple patterns as YouTube may change their page structure
        let patterns = [
            "var ytInitialPlayerResponse = ",
            "ytInitialPlayerResponse = ",
        ];

        for pattern in patterns {
            if let Some(start_idx) = html.find(pattern) {
                let json_start = start_idx + pattern.len();

                // Find the end of the JSON object by counting braces
                if let Some(json_value) = Self::extract_json_object(&html[json_start..]) {
                    match serde_json::from_str(&json_value) {
                        Ok(value) => return Ok(value),
                        Err(e) => {
                            debug!(error = %e, "Failed to parse player response JSON");
                            continue;
                        }
                    }
                }
            }
        }

        Err(AetherError::video(
            "Could not find or parse player response in page",
        ))
    }

    /// Extract a JSON object from the beginning of a string
    fn extract_json_object(s: &str) -> Option<String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.is_empty() || chars[0] != '{' {
            return None;
        }

        let mut depth = 0;
        let mut in_string = false;
        let mut escape_next = false;

        for (i, &c) in chars.iter().enumerate() {
            if escape_next {
                escape_next = false;
                continue;
            }

            match c {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '{' if !in_string => depth += 1,
                '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(chars[..=i].iter().collect());
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// Extract video title from player response
    fn extract_title(player_response: &serde_json::Value) -> String {
        player_response
            .get("videoDetails")
            .and_then(|vd| vd.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown Video")
            .to_string()
    }

    /// Find caption track URL from player response
    ///
    /// Returns (caption_url, language_code)
    fn find_caption_url(
        player_response: &serde_json::Value,
        preferred_lang: &str,
    ) -> Result<(String, String)> {
        let caption_tracks = player_response
            .get("captions")
            .and_then(|c| c.get("playerCaptionsTracklistRenderer"))
            .and_then(|r| r.get("captionTracks"))
            .and_then(|t| t.as_array())
            .ok_or_else(|| AetherError::video("No captions available for this video"))?;

        if caption_tracks.is_empty() {
            return Err(AetherError::video("No captions available for this video"));
        }

        // Try to find preferred language first
        let track = caption_tracks
            .iter()
            .find(|t| {
                t.get("languageCode")
                    .and_then(|l| l.as_str())
                    .map(|l| l.starts_with(preferred_lang))
                    .unwrap_or(false)
            })
            .or_else(|| {
                // Fall back to English if available
                caption_tracks.iter().find(|t| {
                    t.get("languageCode")
                        .and_then(|l| l.as_str())
                        .map(|l| l.starts_with("en"))
                        .unwrap_or(false)
                })
            })
            .or_else(|| caption_tracks.first())
            .ok_or_else(|| AetherError::video("No suitable caption track found"))?;

        let url = track
            .get("baseUrl")
            .and_then(|u| u.as_str())
            .ok_or_else(|| AetherError::video("Caption URL not found in track"))?;

        let language = track
            .get("languageCode")
            .and_then(|l| l.as_str())
            .unwrap_or("unknown")
            .to_string();

        Ok((url.to_string(), language))
    }

    /// Parse transcript data (XML or JSON3 format)
    fn parse_transcript_data(data: &str) -> Result<Vec<TranscriptSegment>> {
        // YouTube transcripts come in XML format
        if data.trim().starts_with("<?xml") || data.contains("<transcript>") {
            Self::parse_transcript_xml(data)
        } else if data.trim().starts_with('{') {
            Self::parse_transcript_json(data)
        } else {
            Err(AetherError::video("Unknown transcript format"))
        }
    }

    /// Parse YouTube transcript XML format
    fn parse_transcript_xml(xml: &str) -> Result<Vec<TranscriptSegment>> {
        let mut segments = Vec::new();

        // Simple XML parsing for YouTube transcript format:
        // <text start="0.0" dur="2.5">Hello everyone</text>
        let text_regex = Regex::new(
            r#"<text\s+start="([^"]+)"\s+dur="([^"]+)"[^>]*>([^<]*)</text>"#
        ).map_err(|e| AetherError::video(format!("Invalid regex: {}", e)))?;

        for caps in text_regex.captures_iter(xml) {
            let start: f64 = caps.get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0.0);

            let dur: f64 = caps.get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0.0);

            let text = caps.get(3)
                .map(|m| Self::decode_html_entities(m.as_str()))
                .unwrap_or_default();

            if !text.is_empty() {
                segments.push(TranscriptSegment::new(start, dur, text));
            }
        }

        if segments.is_empty() {
            return Err(AetherError::video("No transcript segments found in XML"));
        }

        Ok(segments)
    }

    /// Parse YouTube transcript JSON3 format (alternative format)
    fn parse_transcript_json(json: &str) -> Result<Vec<TranscriptSegment>> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| AetherError::video(format!("Failed to parse transcript JSON: {}", e)))?;

        let events = value
            .get("events")
            .and_then(|e| e.as_array())
            .ok_or_else(|| AetherError::video("No events in transcript JSON"))?;

        let mut segments = Vec::new();

        for event in events {
            // Skip events without segments (like style events)
            let segs = match event.get("segs").and_then(|s| s.as_array()) {
                Some(s) => s,
                None => continue,
            };

            let start_ms = event
                .get("tStartMs")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);

            let dur_ms = event
                .get("dDurationMs")
                .and_then(|d| d.as_i64())
                .unwrap_or(0);

            let text: String = segs
                .iter()
                .filter_map(|seg| seg.get("utf8").and_then(|u| u.as_str()))
                .collect::<Vec<_>>()
                .join("");

            let text = text.trim().to_string();
            if !text.is_empty() && text != "\n" {
                segments.push(TranscriptSegment::new(
                    start_ms as f64 / 1000.0,
                    dur_ms as f64 / 1000.0,
                    text,
                ));
            }
        }

        if segments.is_empty() {
            return Err(AetherError::video("No transcript segments found in JSON"));
        }

        Ok(segments)
    }

    /// Decode HTML entities in transcript text
    fn decode_html_entities(text: &str) -> String {
        text.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("&#x27;", "'")
            .replace("&nbsp;", " ")
            .replace("\n", " ")
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_video_id_standard_url() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let id = YouTubeExtractor::parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_short_url() {
        let url = "https://youtu.be/dQw4w9WgXcQ";
        let id = YouTubeExtractor::parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_embed_url() {
        let url = "https://youtube.com/embed/dQw4w9WgXcQ";
        let id = YouTubeExtractor::parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_with_timestamp() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=152s";
        let id = YouTubeExtractor::parse_video_id(url).unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
    }

    #[test]
    fn test_parse_video_id_invalid_url() {
        let url = "https://example.com/video";
        let result = YouTubeExtractor::parse_video_id(url);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_youtube_url_from_text() {
        let input = "Please analyze this video: https://youtube.com/watch?v=abc12345678 thanks!";
        let url = extract_youtube_url(input);
        assert_eq!(url, Some("https://youtube.com/watch?v=abc12345678".to_string()));
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
        assert!(contains_youtube_url("https://youtube.com/watch?v=abc12345678"));
        assert!(contains_youtube_url("Check https://youtu.be/abc12345678 out"));
        assert!(!contains_youtube_url("No URL here"));
    }

    #[test]
    fn test_decode_html_entities() {
        assert_eq!(
            YouTubeExtractor::decode_html_entities("Hello &amp; World"),
            "Hello & World"
        );
        assert_eq!(
            YouTubeExtractor::decode_html_entities("&lt;tag&gt;"),
            "<tag>"
        );
        assert_eq!(
            YouTubeExtractor::decode_html_entities("It&#39;s great"),
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

        let segments = YouTubeExtractor::parse_transcript_xml(xml).unwrap();
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

        let segments = YouTubeExtractor::parse_transcript_xml(xml).unwrap();
        assert_eq!(segments[0].text, "Hello & goodbye");
    }

    #[test]
    fn test_extract_json_object() {
        let input = r#"{"key": "value"};var x = 1;"#;
        let json = YouTubeExtractor::extract_json_object(input);
        assert_eq!(json, Some(r#"{"key": "value"}"#.to_string()));
    }

    #[test]
    fn test_extract_json_object_nested() {
        let input = r#"{"outer": {"inner": "value"}};"#;
        let json = YouTubeExtractor::extract_json_object(input);
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
}
