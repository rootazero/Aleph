//! Processing types and options

use crate::core::MediaAttachment;

/// Processing options
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Application context (bundle ID)
    pub app_context: Option<String>,
    /// Window title of the active application
    pub window_title: Option<String>,
    /// Topic ID for multi-turn conversations (None = "single-turn")
    pub topic_id: Option<String>,
    /// Enable streaming mode
    pub stream: bool,
    /// Media attachments for multimodal content (images, etc.)
    pub attachments: Option<Vec<MediaAttachment>>,
    /// Preferred language for AI responses (e.g., "zh-Hans", "en")
    /// When set, the AI will respond in this language by default,
    /// unless the task requires a different language (e.g., translation)
    pub preferred_language: Option<String>,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            app_context: None,
            window_title: None,
            topic_id: None,         // None means "single-turn"
            stream: true,           // Streaming enabled by default
            attachments: None,
            preferred_language: None, // System default
        }
    }
}

impl ProcessOptions {
    /// Create new processing options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application context
    pub fn with_app_context(mut self, context: String) -> Self {
        self.app_context = Some(context);
        self
    }

    /// Set the window title
    pub fn with_window_title(mut self, title: String) -> Self {
        self.window_title = Some(title);
        self
    }

    /// Set streaming mode
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Set the preferred language for AI responses
    pub fn with_preferred_language(mut self, language: String) -> Self {
        self.preferred_language = Some(language);
        self
    }
}
