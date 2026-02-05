//! Signal detection for memory importance scoring

use serde::{Deserialize, Serialize};

/// Signals detected in memory content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    /// User expresses a preference
    UserPreference,
    /// Factual information
    FactualInfo,
    /// Greeting or salutation
    Greeting,
    /// Small talk or casual conversation
    SmallTalk,
    /// Question asked
    Question,
    /// Answer provided
    Answer,
    /// Decision or commitment
    Decision,
    /// Personal information
    PersonalInfo,
}

/// Detects signals in text for importance scoring
pub struct SignalDetector {
    preference_keywords: Vec<String>,
    greeting_keywords: Vec<String>,
    small_talk_keywords: Vec<String>,
    decision_keywords: Vec<String>,
    personal_keywords: Vec<String>,
}

impl SignalDetector {
    /// Create a new signal detector with default keywords
    pub fn new() -> Self {
        Self {
            preference_keywords: vec![
                "prefer".to_string(),
                "like".to_string(),
                "favorite".to_string(),
                "love".to_string(),
                "enjoy".to_string(),
                "hate".to_string(),
                "dislike".to_string(),
            ],
            greeting_keywords: vec![
                "hello".to_string(),
                "hi".to_string(),
                "hey".to_string(),
                "good morning".to_string(),
                "good afternoon".to_string(),
                "good evening".to_string(),
                "goodbye".to_string(),
                "bye".to_string(),
            ],
            small_talk_keywords: vec![
                "weather".to_string(),
                "how are you".to_string(),
                "nice day".to_string(),
                "thanks".to_string(),
                "thank you".to_string(),
            ],
            decision_keywords: vec![
                "will".to_string(),
                "going to".to_string(),
                "plan to".to_string(),
                "decided".to_string(),
                "choose".to_string(),
            ],
            personal_keywords: vec![
                "my name".to_string(),
                "i am".to_string(),
                "i'm".to_string(),
                "i live".to_string(),
                "my birthday".to_string(),
                "my email".to_string(),
                "my phone".to_string(),
            ],
        }
    }

    /// Detect signals in text
    pub fn detect(&self, text: &str) -> Vec<Signal> {
        let lower_text = text.to_lowercase();
        let mut signals = Vec::new();

        // Check for preferences
        if self.preference_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::UserPreference);
        }

        // Check for greetings
        if self.greeting_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::Greeting);
        }

        // Check for small talk
        if self.small_talk_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::SmallTalk);
        }

        // Check for decisions
        if self.decision_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::Decision);
        }

        // Check for personal info
        if self.personal_keywords.iter().any(|kw| lower_text.contains(kw)) {
            signals.push(Signal::PersonalInfo);
        }

        // Check for questions
        if text.contains('?') {
            signals.push(Signal::Question);
        }

        // Check for answers (heuristic: contains "is", "are", "was", "were")
        if lower_text.contains(" is ") || lower_text.contains(" are ") ||
           lower_text.contains(" was ") || lower_text.contains(" were ") {
            signals.push(Signal::Answer);
        }

        signals
    }
}

impl Default for SignalDetector {
    fn default() -> Self {
        Self::new()
    }
}
