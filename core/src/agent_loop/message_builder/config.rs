//! Configuration for MessageBuilder

/// Configuration for MessageBuilder
#[derive(Debug, Clone)]
pub struct MessageBuilderConfig {
    /// Maximum number of messages to include (default: 100)
    pub max_messages: usize,

    /// Whether to inject system reminders (default: true)
    pub inject_reminders: bool,

    /// Inject reminders after this many iterations (default: 1)
    pub reminder_threshold: u32,

    /// Maximum iterations before warning (default: 50)
    pub max_iterations: u32,
}

impl Default for MessageBuilderConfig {
    fn default() -> Self {
        Self {
            max_messages: 100,
            inject_reminders: true,
            reminder_threshold: 1,
            max_iterations: 50,
        }
    }
}

impl MessageBuilderConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set max messages
    pub fn with_max_messages(mut self, max: usize) -> Self {
        self.max_messages = max;
        self
    }

    /// Builder: set inject reminders flag
    pub fn with_inject_reminders(mut self, inject: bool) -> Self {
        self.inject_reminders = inject;
        self
    }

    /// Builder: set reminder threshold
    pub fn with_reminder_threshold(mut self, threshold: u32) -> Self {
        self.reminder_threshold = threshold;
        self
    }

    /// Builder: set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = max;
        self
    }
}
