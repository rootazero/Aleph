//! User answer types for structured responses
//!
//! This module defines structured user responses that replace
//! plain String responses, enabling type-safe answer handling.

use serde::{Deserialize, Serialize};

/// Structured user answer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum UserAnswer {
    /// Confirmation result
    Confirmation { confirmed: bool },
    /// Single choice result
    SingleChoice {
        selected_index: usize,
        selected_label: String,
    },
    /// Multiple choice result
    MultiChoice {
        selected_indices: Vec<usize>,
        selected_labels: Vec<String>,
    },
    /// Text input result
    TextInput { text: String },
    /// User cancelled (applies to all types)
    #[default]
    Cancelled,
}

impl UserAnswer {
    /// Convert to LLM-understandable text feedback
    pub fn to_llm_feedback(&self) -> String {
        match self {
            Self::Confirmation { confirmed } => {
                if *confirmed {
                    "User confirmed: Yes".into()
                } else {
                    "User confirmed: No".into()
                }
            }
            Self::SingleChoice { selected_label, .. } => {
                format!("User selected: {}", selected_label)
            }
            Self::MultiChoice { selected_labels, .. } => {
                format!("User selected: {}", selected_labels.join(", "))
            }
            Self::TextInput { text } => {
                format!("User input: {}", text)
            }
            Self::Cancelled => "User cancelled the operation".into(),
        }
    }

    /// Check if the answer represents a cancellation
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Get the raw text value for backward compatibility
    pub fn as_text(&self) -> String {
        match self {
            Self::Confirmation { confirmed } => confirmed.to_string(),
            Self::SingleChoice { selected_label, .. } => selected_label.clone(),
            Self::MultiChoice { selected_labels, .. } => selected_labels.join(", "),
            Self::TextInput { text } => text.clone(),
            Self::Cancelled => String::new(),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_answer_serialization() {
        let answer = UserAnswer::Confirmation { confirmed: true };
        let json = serde_json::to_string(&answer).unwrap();
        assert!(json.contains("confirmation"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_to_llm_feedback_confirmation() {
        let yes = UserAnswer::Confirmation { confirmed: true };
        assert_eq!(yes.to_llm_feedback(), "User confirmed: Yes");

        let no = UserAnswer::Confirmation { confirmed: false };
        assert_eq!(no.to_llm_feedback(), "User confirmed: No");
    }

    #[test]
    fn test_to_llm_feedback_single_choice() {
        let answer = UserAnswer::SingleChoice {
            selected_index: 1,
            selected_label: "Option B".to_string(),
        };
        assert_eq!(answer.to_llm_feedback(), "User selected: Option B");
    }

    #[test]
    fn test_to_llm_feedback_multi_choice() {
        let answer = UserAnswer::MultiChoice {
            selected_indices: vec![0, 2],
            selected_labels: vec!["A".to_string(), "C".to_string()],
        };
        assert_eq!(answer.to_llm_feedback(), "User selected: A, C");
    }

    #[test]
    fn test_to_llm_feedback_text_input() {
        let answer = UserAnswer::TextInput { text: "Hello world".to_string() };
        assert_eq!(answer.to_llm_feedback(), "User input: Hello world");
    }

    #[test]
    fn test_to_llm_feedback_cancelled() {
        let answer = UserAnswer::Cancelled;
        assert_eq!(answer.to_llm_feedback(), "User cancelled the operation");
    }
}
