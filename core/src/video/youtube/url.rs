//! YouTube URL parsing and detection utilities

use crate::error::{AlephError, Result};
use regex::Regex;
use std::sync::LazyLock;

/// Regex pattern for matching YouTube URLs and extracting video IDs
pub static YOUTUBE_URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/|youtube\.com/v/)([a-zA-Z0-9_-]{11})"
    ).expect("Invalid YouTube URL regex")
});

/// Regex pattern for detecting YouTube URLs in text (looser matching)
pub static YOUTUBE_DETECT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
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

/// Parse video ID from a YouTube URL
pub fn parse_video_id(url: &str) -> Result<String> {
    YOUTUBE_URL_REGEX
        .captures(url)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| AlephError::video(format!("Invalid YouTube URL: {}", url)))
}
