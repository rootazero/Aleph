//! Main YouTube transcript extractor implementation

use crate::config::VideoConfig;
use crate::error::{AetherError, Result};
use crate::video::transcript::VideoTranscript;
use std::time::Duration;
use tracing::{debug, info};

use super::caption::fetch_caption_via_ytdlp;
use super::parser::{extract_json_object, parse_transcript_data};
use super::url::parse_video_id;

/// YouTube transcript extractor
pub struct YouTubeExtractor {
    client: reqwest::Client,
    config: VideoConfig,
}

impl YouTubeExtractor {
    /// Create a new YouTube extractor with the given configuration
    pub fn new(config: VideoConfig) -> Self {
        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE};

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"));
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7"),
        );
        // YouTube consent cookie to bypass consent page
        headers.insert(
            COOKIE,
            HeaderValue::from_static(
                "CONSENT=YES+cb; SOCS=CAESEwgDEgk2MjcxNDkzNDgaAmVuIAEaBgiA_sCjBg",
            ),
        );

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
            .default_headers(headers)
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
        let video_id = parse_video_id(url)?;
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
        // Try multiple formats - json3 is often more accessible than default XML
        let transcript_data = self.fetch_caption(&caption_url, &video_id).await?;
        let segments = parse_transcript_data(&transcript_data)?;

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

    /// Fetch caption data with multiple format attempts
    ///
    /// YouTube's timedtext API can be finicky about requests. This method
    /// tries multiple approaches to get the transcript data.
    async fn fetch_caption(&self, base_url: &str, video_id: &str) -> Result<String> {
        use reqwest::header::{HeaderValue, REFERER};

        // Try json3 format first (often more accessible)
        let json3_url = if base_url.contains("&fmt=") {
            base_url.replace("&fmt=srv3", "&fmt=json3")
        } else {
            format!("{}&fmt=json3", base_url)
        };

        debug!(url = %json3_url, "Trying json3 format");

        // Build request with Referer header pointing to YouTube
        let response = self
            .client
            .get(&json3_url)
            .header(
                REFERER,
                HeaderValue::from_static("https://www.youtube.com/"),
            )
            .send()
            .await
            .map_err(|e| AetherError::video(format!("Failed to fetch caption: {}", e)))?;

        if response.status().is_success() {
            let content_length = response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok());

            if content_length != Some(0) {
                let text = response.text().await.map_err(|e| {
                    AetherError::video(format!("Failed to read caption response: {}", e))
                })?;

                if !text.is_empty() {
                    debug!(
                        format = "json3",
                        len = text.len(),
                        "Caption fetched successfully"
                    );
                    return Ok(text);
                }
            }
        }

        debug!("json3 format failed, trying default XML format");

        // Fall back to default XML format
        let response = self
            .client
            .get(base_url)
            .header(
                REFERER,
                HeaderValue::from_static("https://www.youtube.com/"),
            )
            .send()
            .await
            .map_err(|e| AetherError::video(format!("Failed to fetch caption: {}", e)))?;

        if response.status().is_success() {
            let text = response.text().await.map_err(|e| {
                AetherError::video(format!("Failed to read caption response: {}", e))
            })?;

            if !text.is_empty() {
                debug!(
                    format = "xml",
                    len = text.len(),
                    "Caption fetched successfully"
                );
                return Ok(text);
            }
        }

        debug!("Direct HTTP fetch failed, trying yt-dlp as fallback");

        // Fall back to yt-dlp if available
        fetch_caption_via_ytdlp(video_id, &self.config.preferred_language).await
    }

    /// Fetch a page with error handling
    pub async fn fetch_page(&self, url: &str) -> Result<String> {
        debug!(url = %url, "Fetching page");

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| AetherError::video(format!("Failed to fetch page: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(AetherError::video(format!("HTTP error: {}", status)));
        }

        // Check content length header for early empty response detection
        let content_length = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());

        if content_length == Some(0) {
            debug!("YouTube returned empty response (Content-Length: 0) - anti-bot protection");
            return Err(AetherError::video_with_suggestion(
                "YouTube returned empty response",
                "This may be due to rate limiting or anti-bot protection. Try again in a few minutes.",
            ));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AetherError::video(format!("Failed to read response: {}", e)))?;

        // Double check for empty body
        if text.is_empty() {
            return Err(AetherError::video_with_suggestion(
                "YouTube returned empty response body",
                "This may be due to rate limiting. Try again later.",
            ));
        }

        Ok(text)
    }

    /// Extract ytInitialPlayerResponse from page HTML
    pub fn extract_player_response(html: &str) -> Result<serde_json::Value> {
        // Try multiple patterns as YouTube may change their page structure
        let patterns = [
            "var ytInitialPlayerResponse = ",
            "ytInitialPlayerResponse = ",
        ];

        for pattern in patterns {
            if let Some(start_idx) = html.find(pattern) {
                let json_start = start_idx + pattern.len();

                // Find the end of the JSON object by counting braces
                if let Some(json_value) = extract_json_object(&html[json_start..]) {
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

    /// Extract video title from player response
    pub fn extract_title(player_response: &serde_json::Value) -> String {
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
    pub fn find_caption_url(
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

        // Helper to check if language code matches (handles variants like zh-Hans, zh-Hant)
        let lang_matches = |code: &str, target: &str| -> bool {
            let code_lower = code.to_lowercase();
            let target_lower = target.to_lowercase();
            code_lower.starts_with(&target_lower)
                || (target_lower == "zh"
                    && (code_lower.starts_with("zh-hans")
                        || code_lower.starts_with("zh-hant")
                        || code_lower.starts_with("zh-cn")
                        || code_lower.starts_with("zh-tw")))
        };

        // Try to find preferred language first
        let track = caption_tracks
            .iter()
            .find(|t| {
                t.get("languageCode")
                    .and_then(|l| l.as_str())
                    .map(|l| lang_matches(l, preferred_lang))
                    .unwrap_or(false)
            })
            .or_else(|| {
                // Fall back to English if available
                caption_tracks.iter().find(|t| {
                    t.get("languageCode")
                        .and_then(|l| l.as_str())
                        .map(|l| lang_matches(l, "en"))
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
}
