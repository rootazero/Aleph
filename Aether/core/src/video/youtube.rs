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

/// Get yt-dlp path in Aether config directory
fn get_ytdlp_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(".config").join("aether").join("yt-dlp")
}

/// Find yt-dlp executable path, auto-installing if needed
fn which_ytdlp() -> Option<std::path::PathBuf> {
    // First check Aether's config directory
    let config_path = get_ytdlp_config_path();
    if config_path.exists() {
        return Some(config_path);
    }

    // Check common system paths as fallback
    let paths = [
        "/opt/homebrew/bin/yt-dlp",
        "/usr/local/bin/yt-dlp",
        "/usr/bin/yt-dlp",
    ];

    for path in paths {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try PATH
    std::process::Command::new("which")
        .arg("yt-dlp")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(std::path::PathBuf::from(path));
                }
            }
            None
        })
}

/// Auto-install yt-dlp to ~/.config/yt-dlp using curl
fn install_ytdlp() -> Result<std::path::PathBuf> {
    use std::process::Command;
    use std::fs;

    let config_path = get_ytdlp_config_path();

    // Ensure ~/.config directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AetherError::video(format!("Failed to create config directory: {}", e))
        })?;
    }

    info!("Installing yt-dlp to {:?}", config_path);

    // Download yt-dlp using curl (built-in on macOS)
    let download_url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";
    let output = Command::new("curl")
        .args([
            "-L",                                    // Follow redirects
            "--insecure",                            // Skip SSL verification (for environments with SSL issues)
            "-o", config_path.to_str().unwrap_or(""),
            download_url,
        ])
        .output()
        .map_err(|e| AetherError::video(format!("Failed to run curl: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AetherError::video(format!("Failed to download yt-dlp: {}", stderr)));
    }

    // Make it executable
    let chmod_output = Command::new("chmod")
        .args(["a+rx", config_path.to_str().unwrap_or("")])
        .output()
        .map_err(|e| AetherError::video(format!("Failed to chmod yt-dlp: {}", e)))?;

    if !chmod_output.status.success() {
        return Err(AetherError::video("Failed to make yt-dlp executable"));
    }

    info!("yt-dlp installed successfully");
    Ok(config_path)
}

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
        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE};

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"));
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7"));
        // YouTube consent cookie to bypass consent page
        headers.insert(COOKIE, HeaderValue::from_static("CONSENT=YES+cb; SOCS=CAESEwgDEgk2MjcxNDkzNDgaAmVuIAEaBgiA_sCjBg"));

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
        // Try multiple formats - json3 is often more accessible than default XML
        let transcript_data = self.fetch_caption(&caption_url, &video_id).await?;
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
            .header(REFERER, HeaderValue::from_static("https://www.youtube.com/"))
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
                    debug!(format = "json3", len = text.len(), "Caption fetched successfully");
                    return Ok(text);
                }
            }
        }

        debug!("json3 format failed, trying default XML format");

        // Fall back to default XML format
        let response = self
            .client
            .get(base_url)
            .header(REFERER, HeaderValue::from_static("https://www.youtube.com/"))
            .send()
            .await
            .map_err(|e| AetherError::video(format!("Failed to fetch caption: {}", e)))?;

        if response.status().is_success() {
            let text = response.text().await.map_err(|e| {
                AetherError::video(format!("Failed to read caption response: {}", e))
            })?;

            if !text.is_empty() {
                debug!(format = "xml", len = text.len(), "Caption fetched successfully");
                return Ok(text);
            }
        }

        debug!("Direct HTTP fetch failed, trying yt-dlp as fallback");

        // Fall back to yt-dlp if available
        Self::fetch_caption_via_ytdlp(video_id, &self.config.preferred_language).await
    }

    /// Fetch caption using yt-dlp command-line tool as fallback
    ///
    /// yt-dlp has sophisticated anti-bot bypass mechanisms that often work
    /// when direct HTTP requests fail. Will auto-install yt-dlp if not found.
    ///
    /// Tries preferred language first, falls back to English, then any available language.
    async fn fetch_caption_via_ytdlp(video_id: &str, preferred_lang: &str) -> Result<String> {
        use std::process::Command;
        use std::fs;

        // Check if yt-dlp is available, auto-install if not
        let ytdlp = match which_ytdlp() {
            Some(path) => path,
            None => {
                info!("yt-dlp not found, attempting auto-install...");
                install_ytdlp()?
            }
        };
        let temp_dir = std::env::temp_dir();
        let output_template = temp_dir.join(format!("aether_sub_{}", video_id));
        let url = format!("https://www.youtube.com/watch?v={}", video_id);

        debug!(video_id = %video_id, lang = %preferred_lang, "Fetching caption via yt-dlp");

        // Build language priority list: preferred language, then English as fallback
        // Use comma-separated list for --sub-langs to try multiple languages
        let lang_list = if preferred_lang == "en" {
            "en".to_string()
        } else {
            format!("{},en", preferred_lang)
        };

        // Run yt-dlp to download subtitles with language fallback
        let output = Command::new(&ytdlp)
            .args([
                "--no-check-certificates",  // Bypass SSL issues
                "--write-auto-sub",         // Download auto-generated subtitles
                "--sub-langs", &lang_list,  // Try multiple languages (preferred, then en)
                "--sub-format", "vtt",
                "--skip-download",          // Don't download video
                "-o", output_template.to_str().unwrap_or("/tmp/aether_sub"),
                &url,
            ])
            .output()
            .map_err(|e| AetherError::video(format!("Failed to run yt-dlp: {}", e)))?;

        // Note: yt-dlp may exit successfully even if no subtitles found (just logs a warning)
        // So we need to check for actual subtitle files instead of just exit status
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(stderr = %stderr, "yt-dlp failed");
            return Err(AetherError::video_with_suggestion(
                "yt-dlp failed to download subtitles",
                "The video may not have captions available, or there may be network issues.",
            ));
        }

        // Find the downloaded subtitle file, trying in priority order
        let vtt_path = temp_dir.join(format!("aether_sub_{}.{}.vtt", video_id, preferred_lang));
        let en_vtt_path = temp_dir.join(format!("aether_sub_{}.en.vtt", video_id));

        let subtitle_path = if vtt_path.exists() {
            debug!(lang = %preferred_lang, "Found preferred language subtitle");
            vtt_path
        } else if en_vtt_path.exists() {
            debug!("Preferred language not available, using English fallback");
            en_vtt_path
        } else {
            // Try to find any .vtt file that matches
            let pattern = format!("aether_sub_{}.", video_id);
            let mut found_path = None;
            if let Ok(entries) = fs::read_dir(&temp_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(&pattern) && name.ends_with(".vtt") {
                        debug!(file = %name, "Found alternative subtitle file");
                        found_path = Some(entry.path());
                        break;
                    }
                }
            }

            found_path.ok_or_else(|| {
                // Log the stdout/stderr for debugging
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug!(stdout = %stdout, stderr = %stderr, "No subtitle files found after yt-dlp");
                AetherError::video_with_suggestion(
                    "No subtitles available for this video",
                    "The video may not have captions (auto-generated or manual) in any supported language.",
                )
            })?
        };

        // Read and convert VTT to our format
        let vtt_content = fs::read_to_string(&subtitle_path)
            .map_err(|e| AetherError::video(format!("Failed to read subtitle file: {}", e)))?;

        // Clean up temp file
        let _ = fs::remove_file(&subtitle_path);

        debug!(len = vtt_content.len(), "Caption fetched via yt-dlp successfully");

        // Return VTT content - will be parsed by parse_transcript_vtt
        Ok(vtt_content)
    }

    /// Fetch a page with error handling
    async fn fetch_page(&self, url: &str) -> Result<String> {
        debug!(url = %url, "Fetching page");

        let response = self.client.get(url).send().await.map_err(|e| {
            AetherError::video(format!("Failed to fetch page: {}", e))
        })?;

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

        let text = response.text().await.map_err(|e| {
            AetherError::video(format!("Failed to read response: {}", e))
        })?;

        // Double check for empty body
        if text.is_empty() {
            return Err(AetherError::video_with_suggestion(
                "YouTube returned empty response body",
                "This may be due to rate limiting. Try again later.",
            ));
        }

        Ok(text)
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
    ///
    /// This uses a more robust approach that handles all escape sequences correctly,
    /// including Unicode escapes (\uXXXX) and escaped quotes (\").
    fn extract_json_object(s: &str) -> Option<String> {
        let bytes = s.as_bytes();
        if bytes.is_empty() || bytes[0] != b'{' {
            return None;
        }

        let mut depth = 0;
        let mut in_string = false;
        let mut i = 0;

        while i < bytes.len() {
            let c = bytes[i];

            if in_string {
                if c == b'\\' && i + 1 < bytes.len() {
                    // Skip the next character (handles \", \\, \n, \uXXXX, etc.)
                    i += 2;
                    continue;
                } else if c == b'"' {
                    in_string = false;
                }
            } else {
                match c {
                    b'"' => in_string = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(s[..=i].to_string());
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
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

    /// Parse transcript data (XML, JSON3, or VTT format)
    fn parse_transcript_data(data: &str) -> Result<Vec<TranscriptSegment>> {
        let trimmed = data.trim();

        // YouTube transcripts come in XML format
        if trimmed.starts_with("<?xml") || data.contains("<transcript>") {
            Self::parse_transcript_xml(data)
        } else if trimmed.starts_with('{') {
            Self::parse_transcript_json(data)
        } else if trimmed.starts_with("WEBVTT") {
            Self::parse_transcript_vtt(data)
        } else {
            Err(AetherError::video("Unknown transcript format"))
        }
    }

    /// Parse WebVTT format transcript (from yt-dlp)
    fn parse_transcript_vtt(vtt: &str) -> Result<Vec<TranscriptSegment>> {
        let mut segments = Vec::new();
        let mut current_text = String::new();
        let mut current_start = 0.0;
        let mut current_end = 0.0;

        // VTT timestamp regex: 00:00:00.000 --> 00:00:00.000
        let timestamp_regex = Regex::new(
            r"(\d{2}):(\d{2}):(\d{2})\.(\d{3})\s*-->\s*(\d{2}):(\d{2}):(\d{2})\.(\d{3})"
        ).map_err(|e| AetherError::video(format!("Invalid VTT regex: {}", e)))?;

        // Tag removal regex for VTT formatting tags
        let tag_regex = Regex::new(r"<[^>]+>")
            .map_err(|e| AetherError::video(format!("Invalid tag regex: {}", e)))?;

        let lines: Vec<&str> = vtt.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            // Skip WEBVTT header and metadata
            if line.starts_with("WEBVTT") || line.starts_with("Kind:") || line.starts_with("Language:") || line.is_empty() {
                i += 1;
                continue;
            }

            // Check for timestamp line
            if let Some(caps) = timestamp_regex.captures(line) {
                // If we have accumulated text, save the previous segment
                if !current_text.is_empty() {
                    let text = current_text.trim().to_string();
                    if !text.is_empty() {
                        segments.push(TranscriptSegment::new(
                            current_start,
                            current_end - current_start,
                            text,
                        ));
                    }
                    current_text.clear();
                }

                // Parse timestamps
                current_start = Self::parse_vtt_timestamp(&caps, 1);
                current_end = Self::parse_vtt_timestamp(&caps, 5);

                i += 1;

                // Collect text lines until empty line or next timestamp
                while i < lines.len() {
                    let text_line = lines[i].trim();
                    if text_line.is_empty() || timestamp_regex.is_match(text_line) {
                        break;
                    }

                    // Remove VTT tags like <c> and timing info
                    let clean_text = tag_regex.replace_all(text_line, "").to_string();
                    let clean_text = clean_text.trim();

                    if !clean_text.is_empty() {
                        if !current_text.is_empty() {
                            current_text.push(' ');
                        }
                        current_text.push_str(clean_text);
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
        }

        // Don't forget the last segment
        if !current_text.is_empty() {
            let text = current_text.trim().to_string();
            if !text.is_empty() {
                segments.push(TranscriptSegment::new(
                    current_start,
                    current_end - current_start,
                    text,
                ));
            }
        }

        // Deduplicate consecutive segments with same or similar text
        // YouTube VTT often has duplicates due to styling
        let mut deduped: Vec<TranscriptSegment> = Vec::new();
        for seg in segments {
            if let Some(last) = deduped.last() {
                // Skip if text is same or very similar to last segment
                if last.text != seg.text {
                    deduped.push(seg);
                }
            } else {
                deduped.push(seg);
            }
        }

        if deduped.is_empty() {
            return Err(AetherError::video("No transcript segments found in VTT"));
        }

        Ok(deduped)
    }

    /// Parse VTT timestamp components to seconds
    fn parse_vtt_timestamp(caps: &regex::Captures, start_group: usize) -> f64 {
        let hours: f64 = caps.get(start_group).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
        let minutes: f64 = caps.get(start_group + 1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
        let seconds: f64 = caps.get(start_group + 2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
        let millis: f64 = caps.get(start_group + 3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);

        hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0
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
                println!("✅ Success!");
                println!("Title: {}", transcript.title);
                println!("Language: {}", transcript.language);
                println!("Segments: {}", transcript.segments.len());
                println!("First 200 chars: {}", &transcript.format_for_context()[..200.min(transcript.format_for_context().len())]);
            }
            Err(e) => {
                println!("❌ Failed: {:?}", e);
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
        let (caption_url, lang) = YouTubeExtractor::find_caption_url(&player_response, "en").unwrap();
        println!("4. Caption URL found for language: {}", lang);
        println!("   URL preview: {}...", &caption_url[..100.min(caption_url.len())]);

        // Fetch caption using the new method
        println!("\n5. Fetching caption...");
        match extractor.fetch_caption(&caption_url, video_id).await {
            Ok(body) => {
                println!("   Body length: {} bytes", body.len());
                println!("   Body preview: {}", &body[..500.min(body.len())]);
            }
            Err(e) => {
                println!("   Failed: {:?}", e);
            }
        }
    }
}
