//! Question types for structured user interaction
//!
//! This module defines the question types that determine how
//! the UI layer should render user prompts.

use serde::{Deserialize, Serialize};

/// Question type, determines UI rendering and validation rules
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionKind {
    /// Yes/No confirmation - simplest binary choice
    Confirmation {
        /// Default value when user presses Enter directly
        #[serde(default)]
        default: bool,
        /// Custom labels, e.g., ("Approve", "Reject") instead of ("Yes", "No")
        #[serde(default)]
        labels: Option<(String, String)>,
    },

    /// Single choice - select one from multiple options
    SingleChoice {
        choices: Vec<ChoiceOption>,
        /// Default selected index
        #[serde(default)]
        default_index: Option<usize>,
    },

    /// Multiple choice - select multiple options
    MultiChoice {
        choices: Vec<ChoiceOption>,
        /// Minimum selections (0 = optional)
        #[serde(default)]
        min_selections: usize,
        /// Maximum selections (None = unlimited)
        #[serde(default)]
        max_selections: Option<usize>,
    },

    /// Free text input
    TextInput {
        #[serde(default)]
        placeholder: Option<String>,
        /// Multi-line input (for code, long text)
        #[serde(default)]
        multiline: bool,
        /// Input validation (optional)
        #[serde(default)]
        validation: Option<TextValidation>,
    },
}

impl Default for QuestionKind {
    fn default() -> Self {
        QuestionKind::TextInput {
            placeholder: None,
            multiline: false,
            validation: None,
        }
    }
}

/// Choice option with label and optional description
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChoiceOption {
    pub label: String,
    /// Detailed description (UI can show as tooltip or subtitle)
    #[serde(default)]
    pub description: Option<String>,
}

impl ChoiceOption {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Text validation rules
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextValidation {
    /// Regex match
    Regex { pattern: String, message: String },
    /// Length limit
    Length { min: Option<usize>, max: Option<usize> },
    /// Non-empty
    Required,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_question_kind_serialization() {
        let kind = QuestionKind::Confirmation { default: true, labels: None };
        let json = serde_json::to_string(&kind).unwrap();
        assert!(json.contains("confirmation"));

        let parsed: QuestionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn test_choice_option_with_description() {
        let option = ChoiceOption {
            label: "Option A".to_string(),
            description: Some("This is option A".to_string()),
        };
        let json = serde_json::to_string(&option).unwrap();
        assert!(json.contains("Option A"));
        assert!(json.contains("This is option A"));
    }

    #[test]
    fn test_text_validation_regex() {
        let validation = TextValidation::Regex {
            pattern: r"^\d+$".to_string(),
            message: "Must be a number".to_string(),
        };
        let json = serde_json::to_string(&validation).unwrap();
        assert!(json.contains("regex"));
    }
}
