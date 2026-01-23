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

mod app;
mod conversation;
mod input;
mod matching;
mod pending;
mod time;

pub use app::AppContext;
pub use conversation::ConversationContext;
pub use input::InputFeatures;
pub use matching::{MatchingContext, MatchingContextBuilder};
pub use pending::PendingParam;
pub use time::TimeContext;

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
