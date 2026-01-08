//! MatchingContext - Complete context for semantic matching decisions
//!
//! Provides three types of context:
//! - ConversationContext: Multi-turn dialogue history and pending parameters
//! - AppContext: Current application and window information
//! - TimeContext: Temporal information for time-based rules
//!
//! # Example
//!
//! ```rust,no_run
//! use aethecore::semantic::context::{MatchingContext, ConversationContext, AppContext, TimeContext};
//!
//! let context = MatchingContext::builder()
//!     .raw_input("What's the weather?")
//!     .conversation(ConversationContext::new())
//!     .app(AppContext::new("com.apple.Notes", "Notes"))
//!     .time(TimeContext::now())
//!     .build();
//! ```

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete context for matching decision
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
    pub fn simple(input: impl Into<String>) -> Self {
        let raw_input = input.into();
        let features = InputFeatures::extract(&raw_input);

        Self {
            raw_input,
            cleaned_input: None,
            conversation: ConversationContext::new(),
            app: AppContext::unknown(),
            time: TimeContext::now(),
            features,
        }
    }

    /// Get the effective input (cleaned if available, otherwise raw)
    pub fn effective_input(&self) -> &str {
        self.cleaned_input.as_deref().unwrap_or(&self.raw_input)
    }

    /// Check if conversation has pending parameters
    pub fn has_pending_params(&self) -> bool {
        !self.conversation.pending_params.is_empty()
    }

    /// Get pending parameter for a specific intent
    pub fn get_pending_param(&self, intent: &str) -> Option<&PendingParam> {
        self.conversation
            .pending_params
            .values()
            .find(|p| p.required_for == intent)
    }

    /// Check if input contains URLs
    pub fn has_urls(&self) -> bool {
        !self.features.urls.is_empty()
    }

    /// Check if input is a question
    pub fn is_question(&self) -> bool {
        self.features.has_question_mark
    }
}

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

/// Multi-turn conversation context
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    /// Conversation session ID
    pub session_id: Option<String>,

    /// Number of turns in current session
    pub turn_count: u32,

    /// Previous intents in this session (most recent first)
    pub previous_intents: Vec<String>,

    /// Pending parameters from previous turn
    pub pending_params: HashMap<String, PendingParam>,

    /// Last AI response summary (for context)
    pub last_response_summary: Option<String>,

    /// Recent conversation history
    pub history: Vec<ConversationTurn>,
}

impl ConversationContext {
    /// Create a new empty conversation context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with session ID
    pub fn with_session(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
            ..Default::default()
        }
    }

    /// Add a turn to history
    pub fn add_turn(&mut self, turn: ConversationTurn) {
        self.history.push(turn);
        self.turn_count = self.history.len() as u32;
    }

    /// Record an intent
    pub fn record_intent(&mut self, intent_type: impl Into<String>) {
        self.previous_intents.insert(0, intent_type.into());
        // Keep only last 10 intents
        if self.previous_intents.len() > 10 {
            self.previous_intents.truncate(10);
        }
    }

    /// Add a pending parameter
    pub fn add_pending_param(&mut self, param: PendingParam) {
        self.pending_params.insert(param.param_name.clone(), param);
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
        self.previous_intents.first().map(|s| s.as_str())
    }

    /// Check if a specific intent was used recently
    pub fn has_recent_intent(&self, intent: &str, within_turns: usize) -> bool {
        self.previous_intents
            .iter()
            .take(within_turns)
            .any(|i| i == intent)
    }

    // =========================================================================
    // L3 Routing Context Methods
    // =========================================================================

    /// Build a summary for L3 routing context
    ///
    /// Creates a concise summary of recent conversation history suitable
    /// for injection into L3 routing prompts.
    ///
    /// # Arguments
    ///
    /// * `max_turns` - Maximum number of recent turns to include
    ///
    /// # Returns
    ///
    /// A formatted string summarizing the conversation context
    pub fn build_l3_context_summary(&self, max_turns: usize) -> Option<String> {
        // Check if there's any context worth summarizing
        let has_active_pending = self.pending_params.values().any(|p| !p.is_expired());
        if self.history.is_empty() && self.previous_intents.is_empty() && !has_active_pending {
            return None;
        }

        let mut summary = String::new();

        // Add recent intents summary
        if !self.previous_intents.is_empty() {
            let recent_intents: Vec<&str> = self
                .previous_intents
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect();
            summary.push_str(&format!(
                "Recent intents: {}\n",
                recent_intents.join(" → ")
            ));
        }

        // Add recent conversation turns
        if !self.history.is_empty() {
            summary.push_str("Recent exchanges:\n");

            let recent_turns: Vec<&ConversationTurn> = self
                .history
                .iter()
                .rev()
                .take(max_turns)
                .collect();

            for (i, turn) in recent_turns.iter().rev().enumerate() {
                // Truncate long inputs/responses
                let user_preview = truncate_text(&turn.user_input, 100);
                let ai_preview = truncate_text(&turn.ai_response, 150);

                summary.push_str(&format!(
                    "Turn {}: User: \"{}\" → AI: \"{}\"\n",
                    i + 1,
                    user_preview,
                    ai_preview
                ));
            }
        }

        // Add pending parameters
        if !self.pending_params.is_empty() {
            summary.push_str("Pending parameters:\n");
            for param in self.pending_params.values() {
                if !param.is_expired() {
                    summary.push_str(&format!(
                        "- {} (for {}): \"{}\"\n",
                        param.param_name, param.required_for, param.prompt_text
                    ));
                }
            }
        }

        if summary.is_empty() {
            None
        } else {
            Some(summary.trim().to_string())
        }
    }

    /// Extract entity hints for pronoun resolution
    ///
    /// Analyzes recent conversation history to extract mentioned entities
    /// that pronouns in the current input might refer to.
    ///
    /// # Returns
    ///
    /// A vector of entity descriptions suitable for pronoun resolution
    pub fn extract_entity_hints(&self) -> Vec<String> {
        let mut entities = Vec::new();

        // Extract entities from recent turns
        for turn in self.history.iter().rev().take(3) {
            // Extract from user input
            entities.extend(extract_entities_from_text(&turn.user_input));

            // Extract from AI response (may mention important entities)
            entities.extend(extract_entities_from_text(&turn.ai_response));
        }

        // Add last response summary if available
        if let Some(ref summary) = self.last_response_summary {
            entities.push(format!("Previous response: {}", truncate_text(summary, 100)));
        }

        // Deduplicate and limit
        entities.sort();
        entities.dedup();
        entities.truncate(10);

        entities
    }

    /// Get context for a specific previous turn
    pub fn get_turn_context(&self, turns_back: usize) -> Option<&ConversationTurn> {
        if turns_back == 0 || turns_back > self.history.len() {
            return None;
        }
        self.history.get(self.history.len() - turns_back)
    }

    /// Check if the conversation has meaningful context for routing
    pub fn has_routing_context(&self) -> bool {
        !self.history.is_empty()
            || !self.previous_intents.is_empty()
            || !self.pending_params.is_empty()
    }
}

/// A single turn in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Timestamp of the turn
    pub timestamp: i64,

    /// User input
    pub user_input: String,

    /// AI response (may be truncated)
    pub ai_response: String,

    /// Detected intent for this turn
    pub intent: Option<String>,

    /// Application context at the time
    pub app_bundle_id: Option<String>,
}

impl ConversationTurn {
    /// Create a new conversation turn
    pub fn new(user_input: impl Into<String>, ai_response: impl Into<String>) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            user_input: user_input.into(),
            ai_response: ai_response.into(),
            intent: None,
            app_bundle_id: None,
        }
    }

    /// Set the intent for this turn
    pub fn with_intent(mut self, intent: impl Into<String>) -> Self {
        self.intent = Some(intent.into());
        self
    }

    /// Set the app context for this turn
    pub fn with_app(mut self, bundle_id: impl Into<String>) -> Self {
        self.app_bundle_id = Some(bundle_id.into());
        self
    }
}

/// Pending parameter from previous turn (for follow-up completion)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingParam {
    /// Parameter name (e.g., "location", "url")
    pub param_name: String,

    /// Intent type this parameter is required for
    pub required_for: String,

    /// Timestamp when the prompt was shown
    pub prompted_at: i64,

    /// The prompt text shown to user (for context)
    pub prompt_text: String,

    /// Whether this parameter is required or optional
    pub is_required: bool,
}

impl PendingParam {
    /// Create a new pending parameter
    pub fn new(
        param_name: impl Into<String>,
        required_for: impl Into<String>,
        prompt_text: impl Into<String>,
    ) -> Self {
        Self {
            param_name: param_name.into(),
            required_for: required_for.into(),
            prompted_at: chrono::Utc::now().timestamp(),
            prompt_text: prompt_text.into(),
            is_required: true,
        }
    }

    /// Mark as optional
    pub fn optional(mut self) -> Self {
        self.is_required = false;
        self
    }

    /// Check if this pending param has expired (default: 5 minutes)
    pub fn is_expired(&self) -> bool {
        self.is_expired_after(300) // 5 minutes
    }

    /// Check if expired after given seconds
    pub fn is_expired_after(&self, seconds: i64) -> bool {
        let now = chrono::Utc::now().timestamp();
        now - self.prompted_at > seconds
    }
}

/// Application context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub bundle_id: String,

    /// Application name (e.g., "Notes")
    pub app_name: String,

    /// Window title (if available)
    pub window_title: Option<String>,

    /// Attachments in the input
    pub attachments: Vec<AttachmentType>,
}

impl AppContext {
    /// Create a new app context
    pub fn new(bundle_id: impl Into<String>, app_name: impl Into<String>) -> Self {
        Self {
            bundle_id: bundle_id.into(),
            app_name: app_name.into(),
            window_title: None,
            attachments: Vec::new(),
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

    /// Add attachments
    pub fn with_attachments(mut self, attachments: Vec<AttachmentType>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Check if the app matches a bundle ID pattern
    pub fn matches_bundle(&self, pattern: &str) -> bool {
        // Support wildcards: com.apple.* matches com.apple.Notes
        if pattern.ends_with(".*") {
            let prefix = &pattern[..pattern.len() - 2];
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
        ];

        BROWSERS.iter().any(|browser| self.matches_bundle(browser))
    }
}

impl Default for AppContext {
    fn default() -> Self {
        Self::unknown()
    }
}

/// Attachment type in input
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentType {
    Image,
    Video,
    Audio,
    Document,
    Pdf,
    Other,
}

/// Time context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeContext {
    /// Unix timestamp
    pub timestamp: i64,

    /// Day of week (0 = Sunday, 6 = Saturday)
    pub day_of_week: u8,

    /// Hour of day (0-23)
    pub hour: u8,

    /// Minute (0-59)
    pub minute: u8,

    /// Is weekend (Saturday or Sunday)
    pub is_weekend: bool,

    /// Timezone name
    pub timezone: String,
}

impl TimeContext {
    /// Create time context for current time
    pub fn now() -> Self {
        let now = Local::now();

        Self {
            timestamp: now.timestamp(),
            day_of_week: now.weekday().num_days_from_sunday() as u8,
            hour: now.hour() as u8,
            minute: now.minute() as u8,
            is_weekend: matches!(
                now.weekday(),
                chrono::Weekday::Sat | chrono::Weekday::Sun
            ),
            timezone: now.offset().to_string(),
        }
    }

    /// Create time context for a specific timestamp
    pub fn from_timestamp(timestamp: i64) -> Self {
        use chrono::{DateTime, Utc};

        if let Some(dt) = DateTime::<Utc>::from_timestamp(timestamp, 0) {
            let local = dt.with_timezone(&Local);
            Self {
                timestamp,
                day_of_week: local.weekday().num_days_from_sunday() as u8,
                hour: local.hour() as u8,
                minute: local.minute() as u8,
                is_weekend: matches!(
                    local.weekday(),
                    chrono::Weekday::Sat | chrono::Weekday::Sun
                ),
                timezone: local.offset().to_string(),
            }
        } else {
            Self::now()
        }
    }

    /// Check if current time is within a time range
    ///
    /// # Arguments
    ///
    /// * `start_hour` - Start hour (0-23)
    /// * `end_hour` - End hour (0-23), can be less than start for overnight ranges
    pub fn is_within_hours(&self, start_hour: u8, end_hour: u8) -> bool {
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

/// Pre-extracted input features for efficient matching
#[derive(Debug, Clone, Default)]
pub struct InputFeatures {
    /// Extracted URLs from input
    pub urls: Vec<String>,

    /// Whether input contains question mark
    pub has_question_mark: bool,

    /// Detected language (ISO 639-1 code, e.g., "en", "zh")
    pub detected_language: Option<String>,

    /// Approximate token count
    pub token_count: usize,

    /// Extracted entities by type
    pub extracted_entities: HashMap<String, Vec<String>>,
}

impl InputFeatures {
    /// Extract features from input text
    pub fn extract(input: &str) -> Self {
        Self {
            urls: Self::extract_urls(input),
            has_question_mark: input.contains('?') || input.contains('？'),
            detected_language: Self::detect_language(input),
            token_count: Self::estimate_tokens(input),
            extracted_entities: Self::extract_entities(input),
        }
    }

    /// Extract URLs from text
    fn extract_urls(input: &str) -> Vec<String> {
        // Simple URL regex - can be enhanced
        let url_pattern = regex::Regex::new(
            r"https?://[^\s<>\[\]{}|\\^`\x00-\x1f]+"
        ).ok();

        url_pattern
            .map(|re| {
                re.find_iter(input)
                    .map(|m| m.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Simple language detection based on character ranges
    fn detect_language(input: &str) -> Option<String> {
        let chinese_count = input.chars().filter(|c| {
            matches!(*c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF)
        }).count();

        let total_chars = input.chars().filter(|c| !c.is_whitespace()).count();

        if total_chars == 0 {
            return None;
        }

        let chinese_ratio = chinese_count as f64 / total_chars as f64;

        if chinese_ratio > 0.3 {
            Some("zh".to_string())
        } else {
            Some("en".to_string())
        }
    }

    /// Estimate token count (rough approximation)
    fn estimate_tokens(input: &str) -> usize {
        // Rough estimate: 1 token ≈ 4 characters for English, 1-2 for Chinese
        let char_count = input.chars().count();
        (char_count as f64 / 3.5).ceil() as usize
    }

    /// Extract entities from text (basic implementation)
    fn extract_entities(input: &str) -> HashMap<String, Vec<String>> {
        let mut entities = HashMap::new();

        // Extract YouTube URLs
        let youtube_urls: Vec<String> = Self::extract_urls(input)
            .into_iter()
            .filter(|url| {
                url.contains("youtube.com") || url.contains("youtu.be")
            })
            .collect();

        if !youtube_urls.is_empty() {
            entities.insert("youtube_url".to_string(), youtube_urls);
        }

        entities
    }

    /// Check if input contains YouTube URL
    pub fn has_youtube_url(&self) -> bool {
        self.extracted_entities.contains_key("youtube_url")
    }

    /// Get YouTube URLs if present
    pub fn get_youtube_urls(&self) -> Option<&Vec<String>> {
        self.extracted_entities.get("youtube_url")
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Truncate text to a maximum length, adding ellipsis if needed
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_len - 3).collect();
        format!("{}...", truncated)
    }
}

/// Extract entities from text for pronoun resolution
///
/// This is a simple heuristic-based extraction. For production use,
/// consider using NER (Named Entity Recognition) or more sophisticated methods.
fn extract_entities_from_text(text: &str) -> Vec<String> {
    let mut entities = Vec::new();

    // Extract URLs (common reference targets)
    let url_pattern = regex::Regex::new(r"https?://[^\s]+").ok();
    if let Some(re) = url_pattern {
        for cap in re.find_iter(text) {
            let url = cap.as_str();
            // Categorize URLs
            if url.contains("youtube.com") || url.contains("youtu.be") {
                entities.push(format!("YouTube video: {}", url));
            } else if url.contains("github.com") {
                entities.push(format!("GitHub link: {}", url));
            } else {
                entities.push(format!("URL: {}", url));
            }
        }
    }

    // Extract quoted strings (often specific references)
    let quote_pattern = regex::Regex::new(r#""([^"]+)""#).ok();
    if let Some(re) = quote_pattern {
        for cap in re.captures_iter(text) {
            if let Some(quoted) = cap.get(1) {
                let content = quoted.as_str();
                if content.len() > 2 && content.len() < 100 {
                    entities.push(format!("Quoted: \"{}\"", content));
                }
            }
        }
    }

    // Extract capitalized phrases (potential proper nouns/titles)
    // Only for English text
    let caps_pattern = regex::Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").ok();
    if let Some(re) = caps_pattern {
        for cap in re.captures_iter(text) {
            if let Some(phrase) = cap.get(1) {
                let content = phrase.as_str();
                if content.len() >= 3 {
                    entities.push(format!("Named: {}", content));
                }
            }
        }
    }

    // Extract file paths (common in technical contexts)
    let path_pattern = regex::Regex::new(r"(/[\w./\-]+\.\w+|[\w./\-]+\.(rs|py|js|ts|swift|md))").ok();
    if let Some(re) = path_pattern {
        for cap in re.find_iter(text) {
            entities.push(format!("File: {}", cap.as_str()));
        }
    }

    entities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matching_context_simple() {
        let ctx = MatchingContext::simple("What's the weather?");

        assert_eq!(ctx.raw_input, "What's the weather?");
        assert!(ctx.is_question());
        assert!(!ctx.has_pending_params());
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
    fn test_conversation_context() {
        let mut ctx = ConversationContext::new();

        ctx.record_intent("weather");
        ctx.record_intent("translation");

        assert_eq!(ctx.last_intent(), Some("translation"));
        assert!(ctx.has_recent_intent("weather", 5));
        assert!(!ctx.has_recent_intent("code", 5));
    }

    #[test]
    fn test_pending_param() {
        let param = PendingParam::new("location", "weather", "Please provide a location:");

        assert_eq!(param.param_name, "location");
        assert_eq!(param.required_for, "weather");
        assert!(param.is_required);
        assert!(!param.is_expired());
    }

    #[test]
    fn test_app_context_matching() {
        let ctx = AppContext::new("com.apple.Notes", "Notes");

        assert!(ctx.matches_bundle("com.apple.Notes"));
        assert!(ctx.matches_bundle("com.apple.*"));
        assert!(!ctx.matches_bundle("com.google.*"));
    }

    #[test]
    fn test_app_context_detection() {
        let vscode = AppContext::new("com.microsoft.VSCode", "Visual Studio Code");
        assert!(vscode.is_code_editor());
        assert!(!vscode.is_browser());

        let safari = AppContext::new("com.apple.Safari", "Safari");
        assert!(!safari.is_code_editor());
        assert!(safari.is_browser());
    }

    #[test]
    fn test_time_context() {
        let time = TimeContext::now();

        // Basic sanity checks
        assert!(time.hour < 24);
        assert!(time.minute < 60);
        assert!(time.day_of_week < 7);
    }

    #[test]
    fn test_time_within_hours() {
        let mut time = TimeContext::now();

        // Test normal range
        time.hour = 10;
        assert!(time.is_within_hours(9, 17));
        assert!(!time.is_within_hours(11, 17));

        // Test overnight range
        time.hour = 23;
        assert!(time.is_within_hours(22, 6));

        time.hour = 3;
        assert!(time.is_within_hours(22, 6));
    }

    #[test]
    fn test_input_features() {
        let features = InputFeatures::extract(
            "Check out this video: https://www.youtube.com/watch?v=abc123"
        );

        assert!(features.has_youtube_url());
        // Note: URL contains '?' so has_question_mark is true
        // This is by design - simple detection without URL parsing
        assert!(features.has_question_mark);
        assert_eq!(features.detected_language, Some("en".to_string()));
    }

    #[test]
    fn test_input_features_chinese() {
        let features = InputFeatures::extract("今天北京天气怎么样？");

        assert!(features.has_question_mark);
        assert_eq!(features.detected_language, Some("zh".to_string()));
    }

    #[test]
    fn test_url_extraction() {
        let features = InputFeatures::extract(
            "Visit https://example.com and https://google.com/search"
        );

        assert_eq!(features.urls.len(), 2);
        assert!(features.urls.contains(&"https://example.com".to_string()));
    }

    // =========================================================================
    // L3 Context Tests
    // =========================================================================

    #[test]
    fn test_build_l3_context_summary_empty() {
        let ctx = ConversationContext::new();
        assert!(ctx.build_l3_context_summary(3).is_none());
    }

    #[test]
    fn test_build_l3_context_summary_with_intents() {
        let mut ctx = ConversationContext::new();
        ctx.record_intent("search");
        ctx.record_intent("weather");

        let summary = ctx.build_l3_context_summary(3).unwrap();
        assert!(summary.contains("Recent intents:"));
        assert!(summary.contains("weather"));
        assert!(summary.contains("search"));
    }

    #[test]
    fn test_build_l3_context_summary_with_history() {
        let mut ctx = ConversationContext::new();
        ctx.add_turn(ConversationTurn::new(
            "What's the weather?",
            "It's sunny today.",
        ));
        ctx.add_turn(ConversationTurn::new(
            "What about tomorrow?",
            "Tomorrow will be cloudy.",
        ));

        let summary = ctx.build_l3_context_summary(3).unwrap();
        assert!(summary.contains("Recent exchanges:"));
        assert!(summary.contains("weather"));
        assert!(summary.contains("sunny"));
    }

    #[test]
    fn test_build_l3_context_summary_with_pending_params() {
        let mut ctx = ConversationContext::new();
        ctx.add_pending_param(PendingParam::new(
            "location",
            "weather",
            "Please provide a location:",
        ));

        let summary = ctx.build_l3_context_summary(3).unwrap();
        assert!(summary.contains("Pending parameters:"));
        assert!(summary.contains("location"));
    }

    #[test]
    fn test_extract_entity_hints_empty() {
        let ctx = ConversationContext::new();
        let hints = ctx.extract_entity_hints();
        assert!(hints.is_empty());
    }

    #[test]
    fn test_extract_entity_hints_with_urls() {
        let mut ctx = ConversationContext::new();
        ctx.add_turn(ConversationTurn::new(
            "Check this video: https://youtube.com/watch?v=abc123",
            "I'll analyze that video for you.",
        ));

        let hints = ctx.extract_entity_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("YouTube")));
    }

    #[test]
    fn test_extract_entity_hints_with_quoted_text() {
        let mut ctx = ConversationContext::new();
        ctx.add_turn(ConversationTurn::new(
            "Translate \"Hello World\" to Chinese",
            "你好世界",
        ));

        let hints = ctx.extract_entity_hints();
        assert!(hints.iter().any(|h| h.contains("Hello World")));
    }

    #[test]
    fn test_has_routing_context() {
        let mut ctx = ConversationContext::new();
        assert!(!ctx.has_routing_context());

        ctx.record_intent("search");
        assert!(ctx.has_routing_context());
    }

    #[test]
    fn test_get_turn_context() {
        let mut ctx = ConversationContext::new();
        ctx.add_turn(ConversationTurn::new("First", "Response 1"));
        ctx.add_turn(ConversationTurn::new("Second", "Response 2"));
        ctx.add_turn(ConversationTurn::new("Third", "Response 3"));

        // Get most recent turn (1 turn back)
        let turn = ctx.get_turn_context(1).unwrap();
        assert_eq!(turn.user_input, "Third");

        // Get 2 turns back
        let turn = ctx.get_turn_context(2).unwrap();
        assert_eq!(turn.user_input, "Second");

        // Invalid turns_back
        assert!(ctx.get_turn_context(0).is_none());
        assert!(ctx.get_turn_context(10).is_none());
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("short", 10), "short");
        assert_eq!(truncate_text("this is a very long text", 10), "this is...");
    }

    #[test]
    fn test_extract_entities_from_text() {
        let text = "Check https://github.com/test/repo and file.rs";
        let entities = extract_entities_from_text(text);

        assert!(entities.iter().any(|e| e.contains("GitHub")));
        assert!(entities.iter().any(|e| e.contains("file.rs")));
    }
}
