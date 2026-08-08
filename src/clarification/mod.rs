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
//!     "What style would you like?",
//!     vec![
//!         ClarificationOption::new("professional", "Professional"),
//!         ClarificationOption::new("casual", "Casual"),
//!         ClarificationOption::new("humorous", "Humorous"),
//!     ],
//! );
//!
//! // Create a text-type clarification
//! let request = ClarificationRequest::text("Enter target language:");
//! ```

pub mod session;

pub use session::{ClarificationManager, DEFAULT_CLARIFY_TIMEOUT};

// `PendingClarification` is constructed by `ClarificationManager::list_pending`
// and serialized as part of the gateway's clarification API response. It is
// not part of the public Rust surface — Panel / TUI consume it via the JSON
// gateway, not by importing the type — so it stays a private detail of the
// `session` module.

use serde::{Deserialize, Serialize};

/// Type of clarification request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ClarificationType {
    /// Option list (menu-driven selection)
    #[default]
    Select,
    /// Free-form text input
    Text,
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
    /// Create a new option with value and label.
    #[must_use]
    pub fn new(value: &str, label: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
            description: None,
        }
    }

    /// Attach an explanatory description, helping the user choose between
    /// otherwise-terse options. An empty/whitespace description is ignored so
    /// callers can pass through optional input without a branch.
    #[must_use]
    pub fn with_description(mut self, description: &str) -> Self {
        let trimmed = description.trim();
        if !trimmed.is_empty() {
            self.description = Some(trimmed.to_string());
        }
        self
    }
}

/// Request for user clarification.
///
/// # Why there is no `id` here
///
/// The registry is keyed by `session_key` — one live question per session —
/// and all three read paths (`list_pending`, `interpret_reply`, `resolve`)
/// address it that way. An `id` field existed, was written by every caller
/// (`ask_user` minted a fresh UUID per call), and was read by **nothing**
/// outside this module's own unit tests. Removed under P6 rather than left as
/// "a hook for future id-addressing": the approval twin genuinely is
/// id-addressed (`ExecApprovalManager::resolve_with_reason(id)`), so if
/// clarifications ever need it, that shape is there to copy — a dead field is
/// not a head start, it is a claim that something is wired when it is not.
///
/// `default_value` (always `Some("1")` for select, always `None` for text) and
/// `placeholder` (never passed by either production constructor) went the same
/// way: write-only. This type is not on any wire — Panel and TUI consume
/// `PendingClarification` — so nothing outside the crate can observe the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationRequest {
    /// Prompt text to display
    pub prompt: String,
    /// Type of clarification
    pub clarification_type: ClarificationType,
    /// Options for select-type (None for text-type)
    pub options: Option<Vec<ClarificationOption>>,
}

impl ClarificationRequest {
    /// Create a select-type clarification request
    #[must_use]
    pub fn select(prompt: &str, options: Vec<ClarificationOption>) -> Self {
        Self {
            prompt: prompt.to_string(),
            clarification_type: ClarificationType::Select,
            options: Some(options),
        }
    }

    /// Create a text-type clarification request
    #[must_use]
    pub fn text(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            clarification_type: ClarificationType::Text,
            options: None,
        }
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
}

impl ClarificationResult {
    /// Create a selected result
    #[must_use]
    pub const fn selected(index: u32, value: String) -> Self {
        Self {
            result_type: ClarificationResultType::Selected,
            selected_index: Some(index),
            value: Some(value),
        }
    }

    /// Create a text input result
    #[must_use]
    pub const fn text_input(value: String) -> Self {
        Self {
            result_type: ClarificationResultType::TextInput,
            selected_index: None,
            value: Some(value),
        }
    }

    /// Create a cancelled result
    #[must_use]
    pub const fn cancelled() -> Self {
        Self {
            result_type: ClarificationResultType::Cancelled,
            selected_index: None,
            value: None,
        }
    }

    /// Create a timeout result
    #[must_use]
    pub const fn timeout() -> Self {
        Self {
            result_type: ClarificationResultType::Timeout,
            selected_index: None,
            value: None,
        }
    }

    /// Get the value, if any
    #[must_use]
    pub fn get_value(&self) -> Option<&str> {
        self.value.as_deref()
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
            ClarificationOption::new("pro", "Professional").with_description("formal tone");
        assert_eq!(option.description.as_deref(), Some("formal tone"));
        // Blank/whitespace descriptions are ignored, keeping the field None.
        let blank = ClarificationOption::new("pro", "Professional").with_description("   ");
        assert!(blank.description.is_none());
    }

    #[test]
    fn test_clarification_request_select() {
        let request = ClarificationRequest::select(
            "Choose style:",
            vec![
                ClarificationOption::new("a", "Option A"),
                ClarificationOption::new("b", "Option B"),
            ],
        );

        assert_eq!(request.prompt, "Choose style:");
        assert_eq!(request.clarification_type, ClarificationType::Select);
        assert!(request.options.is_some());
        assert_eq!(request.options.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_clarification_request_text() {
        let request = ClarificationRequest::text("Enter name:");

        assert_eq!(request.prompt, "Enter name:");
        assert_eq!(request.clarification_type, ClarificationType::Text);
        assert!(request.options.is_none());
    }

    #[test]
    fn test_clarification_result_selected() {
        let result = ClarificationResult::selected(2, "humorous".to_string());

        assert_eq!(result.result_type, ClarificationResultType::Selected);
        assert_eq!(result.selected_index, Some(2));
        assert_eq!(result.value, Some("humorous".to_string()));
    }

    #[test]
    fn test_clarification_result_text_input() {
        let result = ClarificationResult::text_input("Hello world".to_string());

        assert_eq!(result.result_type, ClarificationResultType::TextInput);
        assert!(result.selected_index.is_none());
        assert_eq!(result.value, Some("Hello world".to_string()));
    }

    #[test]
    fn test_clarification_result_cancelled() {
        let result = ClarificationResult::cancelled();

        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
    }

    #[test]
    fn test_clarification_result_timeout() {
        let result = ClarificationResult::timeout();

        assert_eq!(result.result_type, ClarificationResultType::Timeout);
    }

    #[test]
    fn test_clarification_type_default() {
        let default = ClarificationType::default();
        assert_eq!(default, ClarificationType::Select);
    }
}
