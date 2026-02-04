//! Clarification module for Phantom Flow interaction
//!
//! This module provides types for requesting clarification from users through
//! the Halo overlay. It implements the Phantom Flow interaction pattern:
//! - In-place interaction within Halo
//! - Menu-driven selection for options
//! - Inline text input for free-form responses
//!
//! # Example
//!
//! ```rust,no_run
//! use alephcore::clarification::{ClarificationRequest, ClarificationType, ClarificationOption};
//!
//! // Create a select-type clarification
//! let request = ClarificationRequest::select(
//!     "style-choice",
//!     "What style would you like?",
//!     vec![
//!         ClarificationOption::new("professional", "Professional"),
//!         ClarificationOption::new("casual", "Casual"),
//!         ClarificationOption::new("humorous", "Humorous"),
//!     ],
//! );
//!
//! // Create a text-type clarification
//! let request = ClarificationRequest::text(
//!     "target-language",
//!     "Enter target language:",
//!     Some("e.g., Spanish, French..."),
//! );
//! ```

pub mod session;

pub use session::{ClarificationManager, PendingClarification, SessionConfig};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type of clarification request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ClarificationType {
    /// Option list (menu-driven selection)
    #[default]
    Select,
    /// Free-form text input
    Text,
    /// Multiple question groups (each group is a single-select question)
    MultiGroup,
}

/// A single option in a select-type clarification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationOption {
    /// Display label for the option
    pub label: String,
    /// Value to return when selected
    pub value: String,
    /// Optional description for additional context
    pub description: Option<String>,
}

impl ClarificationOption {
    /// Create a new option with label and value (same)
    pub fn new(value: &str, label: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
            description: None,
        }
    }

    /// Create a new option with description
    pub fn with_description(value: &str, label: &str, description: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
            description: Some(description.to_string()),
        }
    }
}

/// A single question group for multi-group clarifications
///
/// Example: In a poetry configuration, you might have three groups:
/// - Group 1: Select rhyme book (3 options)
/// - Group 2: Select character type (2 options)
/// - Group 3: Select template version (2 options)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionGroup {
    /// Unique ID for this group (e.g., "yunsh", "font", "cipu")
    pub id: String,
    /// Question prompt (e.g., "请选择韵书（用于押韵与验证）")
    pub prompt: String,
    /// Options for this group
    pub options: Vec<ClarificationOption>,
    /// Default selected index (0-based)
    pub default_index: Option<u32>,
}

impl QuestionGroup {
    /// Create a new question group
    pub fn new(id: &str, prompt: &str, options: Vec<ClarificationOption>) -> Self {
        Self {
            id: id.to_string(),
            prompt: prompt.to_string(),
            options,
            default_index: Some(0), // Default to first option
        }
    }

    /// Set default selection
    pub fn with_default(mut self, index: u32) -> Self {
        self.default_index = Some(index);
        self
    }
}

/// Request for user clarification through Halo overlay
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationRequest {
    /// Unique request ID (for tracking/logging)
    pub id: String,
    /// Prompt text to display (overall instruction for multi-group)
    pub prompt: String,
    /// Type of clarification
    pub clarification_type: ClarificationType,
    /// Options for select-type (None for text-type or multi-group)
    pub options: Option<Vec<ClarificationOption>>,
    /// Question groups for multi-group type (None for select/text)
    pub groups: Option<Vec<QuestionGroup>>,
    /// Default value (index as string for select, text for text-type)
    pub default_value: Option<String>,
    /// Placeholder text for text-type input
    pub placeholder: Option<String>,
    /// Source identifier (e.g., "skill:refine-text", "mcp:git")
    pub source: Option<String>,
}

impl ClarificationRequest {
    /// Create a select-type clarification request
    pub fn select(id: &str, prompt: &str, options: Vec<ClarificationOption>) -> Self {
        Self {
            id: id.to_string(),
            prompt: prompt.to_string(),
            clarification_type: ClarificationType::Select,
            options: Some(options),
            groups: None,
            default_value: Some("0".to_string()), // Default to first option
            placeholder: None,
            source: None,
        }
    }

    /// Create a text-type clarification request
    pub fn text(id: &str, prompt: &str, placeholder: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            prompt: prompt.to_string(),
            clarification_type: ClarificationType::Text,
            options: None,
            groups: None,
            default_value: None,
            placeholder: placeholder.map(|s| s.to_string()),
            source: None,
        }
    }

    /// Create a multi-group clarification request
    ///
    /// Example:
    /// ```rust,no_run
    /// use alephcore::clarification::{ClarificationRequest, QuestionGroup, ClarificationOption};
    ///
    /// let request = ClarificationRequest::multi_group(
    ///     "poetry-config",
    ///     "需要确认3项信息",
    ///     vec![
    ///         QuestionGroup::new(
    ///             "yunsh",
    ///             "请选择韵书（用于押韵与验证）",
    ///             vec![
    ///                 ClarificationOption::new("pingshui", "平水韵（传统韵书）"),
    ///                 ClarificationOption::new("cilin", "词林正韵（专门用于词的韵书）"),
    ///                 ClarificationOption::new("xingyun", "中华新韵（现代韵书）"),
    ///             ],
    ///         ),
    ///         QuestionGroup::new(
    ///             "font",
    ///             "用字：简体字还是繁体字？",
    ///             vec![
    ///                 ClarificationOption::new("simplified", "简体"),
    ///                 ClarificationOption::new("traditional", "繁体"),
    ///             ],
    ///         ),
    ///     ],
    /// );
    /// ```
    pub fn multi_group(id: &str, prompt: &str, groups: Vec<QuestionGroup>) -> Self {
        Self {
            id: id.to_string(),
            prompt: prompt.to_string(),
            clarification_type: ClarificationType::MultiGroup,
            options: None,
            groups: Some(groups),
            default_value: None,
            placeholder: None,
            source: None,
        }
    }

    /// Set the source identifier
    pub fn with_source(mut self, source: &str) -> Self {
        self.source = Some(source.to_string());
        self
    }

    /// Set the default value
    pub fn with_default(mut self, default: &str) -> Self {
        self.default_value = Some(default.to_string());
        self
    }
}

/// Result type for clarification response
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClarificationResultType {
    /// User selected an option
    Selected,
    /// User entered text
    TextInput,
    /// User cancelled the request
    Cancelled,
    /// Request timed out
    Timeout,
}

/// Result of a clarification request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationResult {
    /// Type of result
    pub result_type: ClarificationResultType,
    /// Selected option index (for Select type)
    pub selected_index: Option<u32>,
    /// Value (selected option value or text input)
    pub value: Option<String>,
    /// Group answers for multi-group type (group_id -> selected_value)
    /// Example: {"yunsh": "cilin", "font": "simplified", "cipu": "qinding"}
    pub group_answers: Option<HashMap<String, String>>,
}

impl ClarificationResult {
    /// Create a selected result
    pub fn selected(index: u32, value: String) -> Self {
        Self {
            result_type: ClarificationResultType::Selected,
            selected_index: Some(index),
            value: Some(value),
            group_answers: None,
        }
    }

    /// Create a text input result
    pub fn text_input(value: String) -> Self {
        Self {
            result_type: ClarificationResultType::TextInput,
            selected_index: None,
            value: Some(value),
            group_answers: None,
        }
    }

    /// Create a multi-group result
    ///
    /// Example:
    /// ```rust
    /// use std::collections::HashMap;
    /// use alephcore::clarification::ClarificationResult;
    ///
    /// let mut answers = HashMap::new();
    /// answers.insert("yunsh".to_string(), "cilin".to_string());
    /// answers.insert("font".to_string(), "simplified".to_string());
    /// answers.insert("cipu".to_string(), "qinding".to_string());
    ///
    /// let result = ClarificationResult::multi_group(answers);
    /// ```
    pub fn multi_group(answers: HashMap<String, String>) -> Self {
        Self {
            result_type: ClarificationResultType::Selected,
            selected_index: None,
            value: None,
            group_answers: Some(answers),
        }
    }

    /// Create a cancelled result
    pub fn cancelled() -> Self {
        Self {
            result_type: ClarificationResultType::Cancelled,
            selected_index: None,
            value: None,
            group_answers: None,
        }
    }

    /// Create a timeout result
    pub fn timeout() -> Self {
        Self {
            result_type: ClarificationResultType::Timeout,
            selected_index: None,
            value: None,
            group_answers: None,
        }
    }

    /// Check if the result is successful (selected or text input)
    pub fn is_success(&self) -> bool {
        matches!(
            self.result_type,
            ClarificationResultType::Selected | ClarificationResultType::TextInput
        )
    }

    /// Get the value, if any
    pub fn get_value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// Get group answers, if any
    pub fn get_group_answers(&self) -> Option<&HashMap<String, String>> {
        self.group_answers.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clarification_option_new() {
        let option = ClarificationOption::new("pro", "Professional");
        assert_eq!(option.value, "pro");
        assert_eq!(option.label, "Professional");
        assert!(option.description.is_none());
    }

    #[test]
    fn test_clarification_option_with_description() {
        let option =
            ClarificationOption::with_description("pro", "Professional", "Formal business tone");
        assert_eq!(option.value, "pro");
        assert_eq!(option.label, "Professional");
        assert_eq!(option.description, Some("Formal business tone".to_string()));
    }

    #[test]
    fn test_clarification_request_select() {
        let request = ClarificationRequest::select(
            "test-id",
            "Choose style:",
            vec![
                ClarificationOption::new("a", "Option A"),
                ClarificationOption::new("b", "Option B"),
            ],
        );

        assert_eq!(request.id, "test-id");
        assert_eq!(request.prompt, "Choose style:");
        assert_eq!(request.clarification_type, ClarificationType::Select);
        assert!(request.options.is_some());
        assert_eq!(request.options.as_ref().unwrap().len(), 2);
        assert_eq!(request.default_value, Some("0".to_string()));
    }

    #[test]
    fn test_clarification_request_text() {
        let request = ClarificationRequest::text("test-id", "Enter name:", Some("e.g., John Doe"));

        assert_eq!(request.id, "test-id");
        assert_eq!(request.prompt, "Enter name:");
        assert_eq!(request.clarification_type, ClarificationType::Text);
        assert!(request.options.is_none());
        assert_eq!(request.placeholder, Some("e.g., John Doe".to_string()));
    }

    #[test]
    fn test_clarification_request_with_source() {
        let request =
            ClarificationRequest::select("test", "Prompt", vec![]).with_source("skill:refine-text");

        assert_eq!(request.source, Some("skill:refine-text".to_string()));
    }

    #[test]
    fn test_clarification_result_selected() {
        let result = ClarificationResult::selected(2, "humorous".to_string());

        assert_eq!(result.result_type, ClarificationResultType::Selected);
        assert_eq!(result.selected_index, Some(2));
        assert_eq!(result.value, Some("humorous".to_string()));
        assert!(result.is_success());
    }

    #[test]
    fn test_clarification_result_text_input() {
        let result = ClarificationResult::text_input("Hello world".to_string());

        assert_eq!(result.result_type, ClarificationResultType::TextInput);
        assert!(result.selected_index.is_none());
        assert_eq!(result.value, Some("Hello world".to_string()));
        assert!(result.is_success());
    }

    #[test]
    fn test_clarification_result_cancelled() {
        let result = ClarificationResult::cancelled();

        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
        assert!(!result.is_success());
    }

    #[test]
    fn test_clarification_result_timeout() {
        let result = ClarificationResult::timeout();

        assert_eq!(result.result_type, ClarificationResultType::Timeout);
        assert!(!result.is_success());
    }

    #[test]
    fn test_clarification_type_default() {
        let default = ClarificationType::default();
        assert_eq!(default, ClarificationType::Select);
    }

    #[test]
    fn test_question_group_new() {
        let group = QuestionGroup::new(
            "test-group",
            "Select an option",
            vec![
                ClarificationOption::new("a", "Option A"),
                ClarificationOption::new("b", "Option B"),
            ],
        );

        assert_eq!(group.id, "test-group");
        assert_eq!(group.prompt, "Select an option");
        assert_eq!(group.options.len(), 2);
        assert_eq!(group.default_index, Some(0));
    }

    #[test]
    fn test_question_group_with_default() {
        let group = QuestionGroup::new("test", "Prompt", vec![])
            .with_default(3);

        assert_eq!(group.default_index, Some(3));
    }

    #[test]
    fn test_clarification_request_multi_group() {
        let request = ClarificationRequest::multi_group(
            "poetry-config",
            "需要确认3项信息",
            vec![
                QuestionGroup::new(
                    "yunsh",
                    "请选择韵书",
                    vec![
                        ClarificationOption::new("pingshui", "平水韵"),
                        ClarificationOption::new("cilin", "词林正韵"),
                    ],
                ),
                QuestionGroup::new(
                    "font",
                    "用字类型",
                    vec![
                        ClarificationOption::new("simplified", "简体"),
                        ClarificationOption::new("traditional", "繁体"),
                    ],
                ),
            ],
        );

        assert_eq!(request.id, "poetry-config");
        assert_eq!(request.prompt, "需要确认3项信息");
        assert_eq!(request.clarification_type, ClarificationType::MultiGroup);
        assert!(request.options.is_none());
        assert!(request.groups.is_some());
        assert_eq!(request.groups.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_clarification_result_multi_group() {
        let mut answers = HashMap::new();
        answers.insert("yunsh".to_string(), "cilin".to_string());
        answers.insert("font".to_string(), "simplified".to_string());

        let result = ClarificationResult::multi_group(answers.clone());

        assert_eq!(result.result_type, ClarificationResultType::Selected);
        assert!(result.selected_index.is_none());
        assert!(result.value.is_none());
        assert_eq!(result.group_answers, Some(answers));
        assert!(result.is_success());
    }

    #[test]
    fn test_clarification_result_get_group_answers() {
        let mut answers = HashMap::new();
        answers.insert("key1".to_string(), "value1".to_string());

        let result = ClarificationResult::multi_group(answers.clone());
        let retrieved = result.get_group_answers();

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().get("key1"), Some(&"value1".to_string()));
    }
}
