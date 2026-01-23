//! InputFeatures - Pre-extracted input features for fast matching

use regex::Regex;

/// Pre-extracted input features for fast matching
///
/// These features are extracted once and can be used by multiple matchers
/// without re-parsing the input string.
#[derive(Debug, Clone, Default)]
pub struct InputFeatures {
    /// Extracted URLs from input
    pub urls: Vec<String>,

    /// Whether input contains a question mark (? or ？)
    pub has_question_mark: bool,

    /// Word count (split by whitespace)
    pub word_count: usize,

    /// Total character count
    pub char_count: usize,

    /// Whether input contains CJK (Chinese/Japanese/Korean) characters
    pub has_cjk: bool,
}

impl InputFeatures {
    /// Extract features from input text
    ///
    /// This performs a single pass over the input to extract all relevant features.
    pub fn extract(input: &str) -> Self {
        let urls = Self::extract_urls(input);
        let has_cjk = input.chars().any(|c| {
            // CJK Unified Ideographs (Chinese)
            matches!(c as u32, 0x4E00..=0x9FFF)
            // CJK Extension A
            || matches!(c as u32, 0x3400..=0x4DBF)
            // Hiragana (Japanese)
            || matches!(c as u32, 0x3040..=0x309F)
            // Katakana (Japanese)
            || matches!(c as u32, 0x30A0..=0x30FF)
            // Hangul Syllables (Korean)
            || matches!(c as u32, 0xAC00..=0xD7AF)
        });

        Self {
            urls,
            has_question_mark: input.contains('?') || input.contains('？'),
            word_count: input.split_whitespace().count(),
            char_count: input.chars().count(),
            has_cjk,
        }
    }

    /// Extract URLs from text using regex
    fn extract_urls(input: &str) -> Vec<String> {
        // Match URLs starting with http:// or https://
        // Excludes common trailing punctuation and brackets
        let url_pattern = Regex::new(r"https?://[^\s<>\[\]{}|\\^`\x00-\x1f]+").ok();

        url_pattern
            .map(|re| {
                re.find_iter(input)
                    .map(|m| m.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if input contains YouTube URLs
    pub fn has_youtube_url(&self) -> bool {
        self.urls
            .iter()
            .any(|url| url.contains("youtube.com") || url.contains("youtu.be"))
    }

    /// Get YouTube URLs if present
    pub fn get_youtube_urls(&self) -> Vec<&str> {
        self.urls
            .iter()
            .filter(|url| url.contains("youtube.com") || url.contains("youtu.be"))
            .map(|s| s.as_str())
            .collect()
    }
}
