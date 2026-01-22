//! MatchingContext - Complete context for intent matching decisions
//!
//! Provides comprehensive context for intelligent intent routing:
//! - `InputFeatures`: Pre-extracted input features for fast matching
//! - `PendingParam`: Pending parameter waiting for user input
//! - `ConversationContext`: Multi-turn dialogue context
//! - `AppContext`: Current application context
//! - `TimeContext`: Temporal information for time-based rules
//! - `MatchingContext`: Complete matching context combining all above
//!
//! # Example
//!
//! ```rust
//! use aethecore::intent::parameters::{MatchingContext, ConversationContext, AppContext, TimeContext};
//!
//! let context = MatchingContext::builder()
//!     .raw_input("What's the weather?")
//!     .conversation(ConversationContext::default())
//!     .app(AppContext::new("com.apple.Notes", "Notes"))
//!     .time(TimeContext::now())
//!     .build();
//!
//! assert!(context.is_question());
//! ```

use chrono::{Datelike, Local, Timelike};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

// =============================================================================
// InputFeatures
// =============================================================================

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

// =============================================================================
// PendingParam
// =============================================================================

/// Pending parameter waiting for user input
///
/// When an intent is detected but requires additional parameters,
/// a PendingParam is created to track what's needed and when it was requested.
#[derive(Debug, Clone)]
pub struct PendingParam {
    /// Parameter name (e.g., "location", "url")
    pub name: String,

    /// Intent type this parameter is required for
    pub required_for: String,

    /// The prompt text shown to user
    pub prompt: String,

    /// When this pending param was created
    pub created_at: Instant,
}

impl PendingParam {
    /// Create a new pending parameter
    pub fn new(
        name: impl Into<String>,
        required_for: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            required_for: required_for.into(),
            prompt: prompt.into(),
            created_at: Instant::now(),
        }
    }

    /// Check if this pending param has expired (default: 5 minutes)
    pub fn is_expired(&self) -> bool {
        self.is_expired_after_secs(300) // 5 minutes
    }

    /// Check if expired after given seconds
    pub fn is_expired_after_secs(&self, seconds: u64) -> bool {
        self.created_at.elapsed().as_secs() > seconds
    }
}

// =============================================================================
// ConversationContext
// =============================================================================

/// Multi-turn conversation context
///
/// Tracks the state of an ongoing conversation including pending parameters,
/// recent intents, and turn count for context-aware routing decisions.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    /// Pending parameters from previous turn
    pub pending_params: HashMap<String, PendingParam>,

    /// Recent intents in this session (most recent first)
    pub recent_intents: Vec<String>,

    /// Number of turns in current session
    pub turn_count: u32,
}

impl ConversationContext {
    /// Create a new empty conversation context
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an intent
    pub fn record_intent(&mut self, intent_type: impl Into<String>) {
        self.recent_intents.insert(0, intent_type.into());
        // Keep only last 10 intents
        if self.recent_intents.len() > 10 {
            self.recent_intents.truncate(10);
        }
    }

    /// Add a pending parameter
    pub fn add_pending_param(&mut self, param: PendingParam) {
        self.pending_params.insert(param.name.clone(), param);
    }

    /// Clear pending parameters
    pub fn clear_pending_params(&mut self) {
        self.pending_params.clear();
    }

    /// Remove a specific pending parameter
    pub fn resolve_pending_param(&mut self, param_name: &str) -> Option<PendingParam> {
        self.pending_params.remove(param_name)
    }

    /// Get the most recent intent
    pub fn last_intent(&self) -> Option<&str> {
        self.recent_intents.first().map(|s| s.as_str())
    }

    /// Check if a specific intent was used recently
    pub fn has_recent_intent(&self, intent: &str, within_turns: usize) -> bool {
        self.recent_intents
            .iter()
            .take(within_turns)
            .any(|i| i == intent)
    }

    /// Increment turn count
    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
    }

    /// Get pending parameter for a specific intent
    pub fn get_pending_for_intent(&self, intent: &str) -> Option<&PendingParam> {
        self.pending_params
            .values()
            .find(|p| p.required_for == intent)
    }

    /// Check if there are any non-expired pending params
    pub fn has_active_pending_params(&self) -> bool {
        self.pending_params.values().any(|p| !p.is_expired())
    }
}

// =============================================================================
// AppContext
// =============================================================================

/// Current application context
///
/// Provides information about the application the user is currently using,
/// which can influence intent routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub bundle_id: String,

    /// Application name (e.g., "Notes")
    pub app_name: String,

    /// Window title (if available)
    pub window_title: Option<String>,
}

impl AppContext {
    /// Create a new app context
    pub fn new(bundle_id: impl Into<String>, app_name: impl Into<String>) -> Self {
        Self {
            bundle_id: bundle_id.into(),
            app_name: app_name.into(),
            window_title: None,
        }
    }

    /// Create an unknown app context
    pub fn unknown() -> Self {
        Self::new("unknown", "Unknown")
    }

    /// Set window title
    pub fn with_window_title(mut self, title: impl Into<String>) -> Self {
        self.window_title = Some(title.into());
        self
    }

    /// Check if the app matches a bundle ID pattern
    ///
    /// Supports wildcards: `com.apple.*` matches `com.apple.Notes`
    pub fn matches_bundle(&self, pattern: &str) -> bool {
        if let Some(prefix) = pattern.strip_suffix(".*") {
            self.bundle_id.starts_with(prefix)
        } else {
            self.bundle_id == pattern
        }
    }

    /// Check if this is a code editor
    pub fn is_code_editor(&self) -> bool {
        const CODE_EDITORS: &[&str] = &[
            "com.microsoft.VSCode",
            "com.apple.dt.Xcode",
            "com.sublimetext",
            "com.jetbrains",
            "dev.zed.Zed",
            "com.github.atom",
            "io.cursor",
        ];

        CODE_EDITORS
            .iter()
            .any(|editor| self.matches_bundle(editor))
    }

    /// Check if this is a browser
    pub fn is_browser(&self) -> bool {
        const BROWSERS: &[&str] = &[
            "com.apple.Safari",
            "com.google.Chrome",
            "org.mozilla.firefox",
            "com.brave.Browser",
            "com.microsoft.edgemac",
            "company.thebrowser.Browser",
        ];

        BROWSERS.iter().any(|browser| self.matches_bundle(browser))
    }

    /// Check if this is a terminal
    pub fn is_terminal(&self) -> bool {
        const TERMINALS: &[&str] = &[
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "io.alacritty",
            "com.github.wez.wezterm",
            "dev.warp.Warp-Stable",
        ];

        TERMINALS.iter().any(|term| self.matches_bundle(term))
    }
}

impl Default for AppContext {
    fn default() -> Self {
        Self::unknown()
    }
}

// =============================================================================
// TimeContext
// =============================================================================

/// Temporal information for time-based routing rules
///
/// Provides current time information that can be used to make
/// context-aware routing decisions based on time of day or day of week.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeContext {
    /// Hour of day (0-23)
    pub hour: u32,

    /// Minute (0-59)
    pub minute: u32,

    /// Day of week (0 = Sunday, 6 = Saturday)
    pub weekday: u32,

    /// Is weekend (Saturday or Sunday)
    pub is_weekend: bool,
}

impl TimeContext {
    /// Create time context for current time
    pub fn now() -> Self {
        let now = Local::now();

        Self {
            hour: now.hour(),
            minute: now.minute(),
            weekday: now.weekday().num_days_from_sunday(),
            is_weekend: matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
        }
    }

    /// Create time context for a specific hour/minute (for testing)
    pub fn at(hour: u32, minute: u32) -> Self {
        let now = Local::now();
        Self {
            hour,
            minute,
            weekday: now.weekday().num_days_from_sunday(),
            is_weekend: matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
        }
    }

    /// Check if current time is within a time range
    ///
    /// # Arguments
    ///
    /// * `start_hour` - Start hour (0-23)
    /// * `end_hour` - End hour (0-23), can be less than start for overnight ranges
    pub fn is_within_hours(&self, start_hour: u32, end_hour: u32) -> bool {
        if start_hour <= end_hour {
            // Normal range: e.g., 9-17
            self.hour >= start_hour && self.hour < end_hour
        } else {
            // Overnight range: e.g., 22-6
            self.hour >= start_hour || self.hour < end_hour
        }
    }

    /// Check if it's business hours (9am-6pm weekdays)
    pub fn is_business_hours(&self) -> bool {
        !self.is_weekend && self.is_within_hours(9, 18)
    }

    /// Check if it's morning (6am-12pm)
    pub fn is_morning(&self) -> bool {
        self.is_within_hours(6, 12)
    }

    /// Check if it's afternoon (12pm-6pm)
    pub fn is_afternoon(&self) -> bool {
        self.is_within_hours(12, 18)
    }

    /// Check if it's evening (6pm-10pm)
    pub fn is_evening(&self) -> bool {
        self.is_within_hours(18, 22)
    }

    /// Check if it's night (10pm-6am)
    pub fn is_night(&self) -> bool {
        self.is_within_hours(22, 6)
    }
}

impl Default for TimeContext {
    fn default() -> Self {
        Self::now()
    }
}

// =============================================================================
// MatchingContext
// =============================================================================

/// Complete context for intent matching decisions
///
/// Combines all context types into a single structure that can be passed
/// to intent matchers for comprehensive context-aware routing.
#[derive(Debug, Clone)]
pub struct MatchingContext {
    /// User input (with any command prefix)
    pub raw_input: String,

    /// Cleaned input (command prefix stripped if applicable)
    pub cleaned_input: Option<String>,

    /// Conversation context (multi-turn)
    pub conversation: ConversationContext,

    /// Application context
    pub app: AppContext,

    /// Time context
    pub time: TimeContext,

    /// Pre-extracted input features
    pub features: InputFeatures,
}

impl MatchingContext {
    /// Create a new MatchingContext builder
    pub fn builder() -> MatchingContextBuilder {
        MatchingContextBuilder::default()
    }

    /// Create a simple context for testing
    ///
    /// Uses default values for conversation, app, and time context.
    pub fn simple(input: impl Into<String>) -> Self {
        let raw_input = input.into();
        let features = InputFeatures::extract(&raw_input);

        Self {
            raw_input,
            cleaned_input: None,
            conversation: ConversationContext::default(),
            app: AppContext::unknown(),
            time: TimeContext::now(),
            features,
        }
    }

    /// Get the effective input (cleaned if available, otherwise raw)
    pub fn effective_input(&self) -> &str {
        self.cleaned_input.as_deref().unwrap_or(&self.raw_input)
    }

    /// Check if input contains URLs
    pub fn has_urls(&self) -> bool {
        !self.features.urls.is_empty()
    }

    /// Check if input is a question
    pub fn is_question(&self) -> bool {
        self.features.has_question_mark
    }

    /// Check if conversation has pending parameters
    pub fn has_pending_params(&self) -> bool {
        !self.conversation.pending_params.is_empty()
    }

    /// Get pending parameter for a specific intent
    pub fn get_pending_param(&self, intent: &str) -> Option<&PendingParam> {
        self.conversation.get_pending_for_intent(intent)
    }

    /// Check if input contains CJK characters
    pub fn has_cjk(&self) -> bool {
        self.features.has_cjk
    }

    /// Get word count of effective input
    pub fn word_count(&self) -> usize {
        self.features.word_count
    }

    /// Get character count of effective input
    pub fn char_count(&self) -> usize {
        self.features.char_count
    }
}

// =============================================================================
// MatchingContextBuilder
// =============================================================================

/// Builder for MatchingContext
#[derive(Debug, Default)]
pub struct MatchingContextBuilder {
    raw_input: String,
    cleaned_input: Option<String>,
    conversation: Option<ConversationContext>,
    app: Option<AppContext>,
    time: Option<TimeContext>,
    features: Option<InputFeatures>,
}

impl MatchingContextBuilder {
    /// Set raw input
    pub fn raw_input(mut self, input: impl Into<String>) -> Self {
        self.raw_input = input.into();
        self
    }

    /// Set cleaned input
    pub fn cleaned_input(mut self, input: impl Into<String>) -> Self {
        self.cleaned_input = Some(input.into());
        self
    }

    /// Set conversation context
    pub fn conversation(mut self, ctx: ConversationContext) -> Self {
        self.conversation = Some(ctx);
        self
    }

    /// Set application context
    pub fn app(mut self, ctx: AppContext) -> Self {
        self.app = Some(ctx);
        self
    }

    /// Set time context
    pub fn time(mut self, ctx: TimeContext) -> Self {
        self.time = Some(ctx);
        self
    }

    /// Set pre-extracted features
    pub fn features(mut self, features: InputFeatures) -> Self {
        self.features = Some(features);
        self
    }

    /// Build the MatchingContext
    pub fn build(self) -> MatchingContext {
        let features = self
            .features
            .unwrap_or_else(|| InputFeatures::extract(&self.raw_input));

        MatchingContext {
            raw_input: self.raw_input,
            cleaned_input: self.cleaned_input,
            conversation: self.conversation.unwrap_or_default(),
            app: self.app.unwrap_or_else(AppContext::unknown),
            time: self.time.unwrap_or_else(TimeContext::now),
            features,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // InputFeatures Tests
    // =========================================================================

    #[test]
    fn test_input_features_extract_basic() {
        let features = InputFeatures::extract("Hello world!");

        assert!(!features.has_question_mark);
        assert_eq!(features.word_count, 2);
        assert_eq!(features.char_count, 12);
        assert!(!features.has_cjk);
        assert!(features.urls.is_empty());
    }

    #[test]
    fn test_input_features_question_mark() {
        let features_en = InputFeatures::extract("What's the weather?");
        assert!(features_en.has_question_mark);

        let features_zh = InputFeatures::extract("今天天气怎么样？");
        assert!(features_zh.has_question_mark);

        let features_no = InputFeatures::extract("Tell me about the weather");
        assert!(!features_no.has_question_mark);
    }

    #[test]
    fn test_input_features_cjk_detection() {
        // Chinese
        let features_zh = InputFeatures::extract("今天北京天气怎么样？");
        assert!(features_zh.has_cjk);

        // Japanese Hiragana
        let features_ja = InputFeatures::extract("こんにちは");
        assert!(features_ja.has_cjk);

        // Japanese Katakana
        let features_ka = InputFeatures::extract("コンピューター");
        assert!(features_ka.has_cjk);

        // Korean
        let features_ko = InputFeatures::extract("안녕하세요");
        assert!(features_ko.has_cjk);

        // English only
        let features_en = InputFeatures::extract("Hello world");
        assert!(!features_en.has_cjk);
    }

    #[test]
    fn test_input_features_url_extraction() {
        let features =
            InputFeatures::extract("Visit https://example.com and https://google.com/search");

        assert_eq!(features.urls.len(), 2);
        assert!(features.urls.contains(&"https://example.com".to_string()));
        assert!(features
            .urls
            .contains(&"https://google.com/search".to_string()));
    }

    #[test]
    fn test_input_features_youtube_url() {
        let features =
            InputFeatures::extract("Check out this video: https://www.youtube.com/watch?v=abc123");

        assert!(features.has_youtube_url());
        let youtube_urls = features.get_youtube_urls();
        assert_eq!(youtube_urls.len(), 1);
        assert!(youtube_urls[0].contains("youtube.com"));
    }

    #[test]
    fn test_input_features_mixed_content() {
        let features = InputFeatures::extract("请查看 https://example.com 这个网站？");

        assert!(features.has_question_mark);
        assert!(features.has_cjk);
        assert_eq!(features.urls.len(), 1);
    }

    // =========================================================================
    // PendingParam Tests
    // =========================================================================

    #[test]
    fn test_pending_param_new() {
        let param = PendingParam::new("location", "weather", "Please provide a location:");

        assert_eq!(param.name, "location");
        assert_eq!(param.required_for, "weather");
        assert_eq!(param.prompt, "Please provide a location:");
        assert!(!param.is_expired());
    }

    #[test]
    fn test_pending_param_expiry() {
        let param = PendingParam::new("test", "test_intent", "test prompt");

        // Newly created param should not be expired
        assert!(!param.is_expired());
        assert!(!param.is_expired_after_secs(60));
        assert!(!param.is_expired_after_secs(0)); // 0 means immediately expired only if elapsed > 0
    }

    // =========================================================================
    // ConversationContext Tests
    // =========================================================================

    #[test]
    fn test_conversation_context_default() {
        let ctx = ConversationContext::default();

        assert!(ctx.pending_params.is_empty());
        assert!(ctx.recent_intents.is_empty());
        assert_eq!(ctx.turn_count, 0);
    }

    #[test]
    fn test_conversation_context_record_intent() {
        let mut ctx = ConversationContext::new();

        ctx.record_intent("weather");
        ctx.record_intent("translation");

        assert_eq!(ctx.last_intent(), Some("translation"));
        assert!(ctx.has_recent_intent("weather", 5));
        assert!(ctx.has_recent_intent("translation", 5));
        assert!(!ctx.has_recent_intent("code", 5));
    }

    #[test]
    fn test_conversation_context_intent_truncation() {
        let mut ctx = ConversationContext::new();

        // Add 15 intents
        for i in 0..15 {
            ctx.record_intent(format!("intent_{}", i));
        }

        // Should only keep 10
        assert_eq!(ctx.recent_intents.len(), 10);
        // Most recent should be intent_14
        assert_eq!(ctx.last_intent(), Some("intent_14"));
        // intent_0 through intent_4 should be gone
        assert!(!ctx.has_recent_intent("intent_0", 10));
    }

    #[test]
    fn test_conversation_context_pending_params() {
        let mut ctx = ConversationContext::new();

        let param = PendingParam::new("location", "weather", "Where?");
        ctx.add_pending_param(param);

        assert!(ctx.has_active_pending_params());
        assert!(ctx.get_pending_for_intent("weather").is_some());
        assert!(ctx.get_pending_for_intent("translation").is_none());

        // Resolve the param
        let resolved = ctx.resolve_pending_param("location");
        assert!(resolved.is_some());
        assert!(!ctx.has_active_pending_params());
    }

    #[test]
    fn test_conversation_context_increment_turn() {
        let mut ctx = ConversationContext::new();

        assert_eq!(ctx.turn_count, 0);
        ctx.increment_turn();
        assert_eq!(ctx.turn_count, 1);
        ctx.increment_turn();
        assert_eq!(ctx.turn_count, 2);
    }

    // =========================================================================
    // AppContext Tests
    // =========================================================================

    #[test]
    fn test_app_context_new() {
        let ctx = AppContext::new("com.apple.Notes", "Notes");

        assert_eq!(ctx.bundle_id, "com.apple.Notes");
        assert_eq!(ctx.app_name, "Notes");
        assert!(ctx.window_title.is_none());
    }

    #[test]
    fn test_app_context_unknown() {
        let ctx = AppContext::unknown();

        assert_eq!(ctx.bundle_id, "unknown");
        assert_eq!(ctx.app_name, "Unknown");
    }

    #[test]
    fn test_app_context_with_window_title() {
        let ctx = AppContext::new("com.apple.Notes", "Notes").with_window_title("My Shopping List");

        assert_eq!(ctx.window_title, Some("My Shopping List".to_string()));
    }

    #[test]
    fn test_app_context_matches_bundle() {
        let ctx = AppContext::new("com.apple.Notes", "Notes");

        assert!(ctx.matches_bundle("com.apple.Notes"));
        assert!(ctx.matches_bundle("com.apple.*"));
        assert!(!ctx.matches_bundle("com.google.*"));
        assert!(!ctx.matches_bundle("com.apple.Safari"));
    }

    #[test]
    fn test_app_context_is_code_editor() {
        let vscode = AppContext::new("com.microsoft.VSCode", "Visual Studio Code");
        assert!(vscode.is_code_editor());
        assert!(!vscode.is_browser());
        assert!(!vscode.is_terminal());

        let xcode = AppContext::new("com.apple.dt.Xcode", "Xcode");
        assert!(xcode.is_code_editor());

        let notes = AppContext::new("com.apple.Notes", "Notes");
        assert!(!notes.is_code_editor());
    }

    #[test]
    fn test_app_context_is_browser() {
        let safari = AppContext::new("com.apple.Safari", "Safari");
        assert!(safari.is_browser());
        assert!(!safari.is_code_editor());

        let chrome = AppContext::new("com.google.Chrome", "Google Chrome");
        assert!(chrome.is_browser());
    }

    #[test]
    fn test_app_context_is_terminal() {
        let terminal = AppContext::new("com.apple.Terminal", "Terminal");
        assert!(terminal.is_terminal());
        assert!(!terminal.is_code_editor());
        assert!(!terminal.is_browser());

        let iterm = AppContext::new("com.googlecode.iterm2", "iTerm2");
        assert!(iterm.is_terminal());
    }

    // =========================================================================
    // TimeContext Tests
    // =========================================================================

    #[test]
    fn test_time_context_now() {
        let time = TimeContext::now();

        // Basic sanity checks
        assert!(time.hour < 24);
        assert!(time.minute < 60);
        assert!(time.weekday < 7);
    }

    #[test]
    fn test_time_context_at() {
        let time = TimeContext::at(14, 30);

        assert_eq!(time.hour, 14);
        assert_eq!(time.minute, 30);
    }

    #[test]
    fn test_time_context_is_within_hours() {
        // Test normal range (9-17)
        let morning = TimeContext::at(10, 0);
        assert!(morning.is_within_hours(9, 17));

        let early = TimeContext::at(8, 0);
        assert!(!early.is_within_hours(9, 17));

        // Test overnight range (22-6)
        let late_night = TimeContext::at(23, 0);
        assert!(late_night.is_within_hours(22, 6));

        let early_morning = TimeContext::at(3, 0);
        assert!(early_morning.is_within_hours(22, 6));

        let afternoon = TimeContext::at(15, 0);
        assert!(!afternoon.is_within_hours(22, 6));
    }

    #[test]
    fn test_time_context_time_periods() {
        let morning = TimeContext::at(8, 0);
        assert!(morning.is_morning());
        assert!(!morning.is_afternoon());
        assert!(!morning.is_evening());
        assert!(!morning.is_night());

        let afternoon = TimeContext::at(14, 0);
        assert!(!afternoon.is_morning());
        assert!(afternoon.is_afternoon());

        let evening = TimeContext::at(20, 0);
        assert!(evening.is_evening());

        let night = TimeContext::at(23, 0);
        assert!(night.is_night());

        let late_night = TimeContext::at(3, 0);
        assert!(late_night.is_night());
    }

    // =========================================================================
    // MatchingContext Tests
    // =========================================================================

    #[test]
    fn test_matching_context_simple() {
        let ctx = MatchingContext::simple("What's the weather?");

        assert_eq!(ctx.raw_input, "What's the weather?");
        assert!(ctx.is_question());
        assert!(!ctx.has_pending_params());
        assert!(!ctx.has_urls());
    }

    #[test]
    fn test_matching_context_builder() {
        let ctx = MatchingContext::builder()
            .raw_input("What's the weather in Beijing?")
            .app(AppContext::new("com.apple.Notes", "Notes"))
            .build();

        assert_eq!(ctx.app.bundle_id, "com.apple.Notes");
        assert!(ctx.is_question());
    }

    #[test]
    fn test_matching_context_effective_input() {
        // Without cleaned input
        let ctx1 = MatchingContext::simple("raw input");
        assert_eq!(ctx1.effective_input(), "raw input");

        // With cleaned input
        let ctx2 = MatchingContext::builder()
            .raw_input("/search query")
            .cleaned_input("query")
            .build();
        assert_eq!(ctx2.effective_input(), "query");
    }

    #[test]
    fn test_matching_context_with_urls() {
        let ctx = MatchingContext::simple("Check https://example.com");

        assert!(ctx.has_urls());
        assert_eq!(ctx.features.urls.len(), 1);
    }

    #[test]
    fn test_matching_context_with_conversation() {
        let mut conversation = ConversationContext::new();
        conversation.record_intent("weather");
        conversation.add_pending_param(PendingParam::new("location", "weather", "Where?"));

        let ctx = MatchingContext::builder()
            .raw_input("Beijing")
            .conversation(conversation)
            .build();

        assert!(ctx.has_pending_params());
        assert!(ctx.get_pending_param("weather").is_some());
    }

    #[test]
    fn test_matching_context_with_cjk() {
        let ctx_zh = MatchingContext::simple("今天天气怎么样？");
        assert!(ctx_zh.has_cjk());

        let ctx_en = MatchingContext::simple("What's the weather?");
        assert!(!ctx_en.has_cjk());
    }

    #[test]
    fn test_matching_context_counts() {
        let ctx = MatchingContext::simple("Hello beautiful world!");

        assert_eq!(ctx.word_count(), 3);
        assert_eq!(ctx.char_count(), 22);
    }

    #[test]
    fn test_matching_context_builder_with_features() {
        let features = InputFeatures {
            urls: vec!["https://custom.com".to_string()],
            has_question_mark: true,
            word_count: 5,
            char_count: 25,
            has_cjk: false,
        };

        let ctx = MatchingContext::builder()
            .raw_input("test")
            .features(features)
            .build();

        // Should use provided features, not extract from raw_input
        assert_eq!(ctx.features.urls.len(), 1);
        assert_eq!(ctx.features.word_count, 5);
        assert!(ctx.is_question());
    }
}
