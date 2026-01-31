//! Wizard type definitions.
//!
//! Core types for the wizard session system.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Wizard session status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WizardStatus {
    /// Wizard is running
    #[default]
    Running,
    /// Wizard completed successfully
    Done,
    /// Wizard was cancelled by user
    Cancelled,
    /// Wizard encountered an error
    Error,
}

/// Step type for wizard steps
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepType {
    /// Informational note (no input)
    Note,
    /// Single selection from options
    Select,
    /// Multiple selection from options
    MultiSelect,
    /// Free text input
    Text,
    /// Yes/No confirmation
    Confirm,
    /// Progress indicator
    Progress,
    /// Background action (server-executed)
    Action,
}

/// Who executes this step
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepExecutor {
    /// Server executes and streams progress
    Gateway,
    /// Client renders and collects input
    #[default]
    Client,
}

/// A wizard step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardStep {
    /// Unique step ID
    pub id: String,

    /// Step type
    #[serde(rename = "type")]
    pub step_type: StepType,

    /// Step title
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Step message/description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Available options (for select/multiselect)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<WizardOption>>,

    /// Initial value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_value: Option<Value>,

    /// Input placeholder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,

    /// Whether input is sensitive (password-style)
    #[serde(default)]
    pub sensitive: bool,

    /// Who executes this step
    #[serde(default)]
    pub executor: StepExecutor,

    /// Validation pattern (regex)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<String>,

    /// Error message for validation failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<String>,
}

impl WizardStep {
    /// Create a new step with required fields
    pub fn new(id: impl Into<String>, step_type: StepType) -> Self {
        Self {
            id: id.into(),
            step_type,
            title: None,
            message: None,
            options: None,
            initial_value: None,
            placeholder: None,
            sensitive: false,
            executor: StepExecutor::Client,
            validation: None,
            validation_error: None,
        }
    }

    /// Create a note step
    pub fn note(id: impl Into<String>, message: impl Into<String>) -> Self {
        let mut step = Self::new(id, StepType::Note);
        step.message = Some(message.into());
        step
    }

    /// Create a select step
    pub fn select(
        id: impl Into<String>,
        message: impl Into<String>,
        options: Vec<WizardOption>,
    ) -> Self {
        let mut step = Self::new(id, StepType::Select);
        step.message = Some(message.into());
        step.options = Some(options);
        step
    }

    /// Create a text input step
    pub fn text(id: impl Into<String>, message: impl Into<String>) -> Self {
        let mut step = Self::new(id, StepType::Text);
        step.message = Some(message.into());
        step
    }

    /// Create a confirm step
    pub fn confirm(id: impl Into<String>, message: impl Into<String>) -> Self {
        let mut step = Self::new(id, StepType::Confirm);
        step.message = Some(message.into());
        step
    }

    /// Create a progress step
    pub fn progress(id: impl Into<String>, message: impl Into<String>) -> Self {
        let mut step = Self::new(id, StepType::Progress);
        step.message = Some(message.into());
        step.executor = StepExecutor::Gateway;
        step
    }

    /// Set title
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set placeholder
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Mark as sensitive input
    pub fn with_sensitive(mut self, sensitive: bool) -> Self {
        self.sensitive = sensitive;
        self
    }

    /// Set initial value
    pub fn with_initial(mut self, value: impl Into<Value>) -> Self {
        self.initial_value = Some(value.into());
        self
    }

    /// Set validation pattern
    pub fn with_validation(mut self, pattern: impl Into<String>, error: impl Into<String>) -> Self {
        self.validation = Some(pattern.into());
        self.validation_error = Some(error.into());
        self
    }
}

/// An option for select/multiselect steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardOption {
    /// Option value (returned on selection)
    pub value: Value,
    /// Display label
    pub label: String,
    /// Optional hint/description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Whether this option is disabled
    #[serde(default)]
    pub disabled: bool,
}

impl WizardOption {
    /// Create a new option
    pub fn new(value: impl Into<Value>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            hint: None,
            disabled: false,
        }
    }

    /// Add a hint
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Mark as disabled
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

/// Result of calling wizard.next()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardNextResult {
    /// Whether the wizard is done
    pub done: bool,
    /// Current step (if not done)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<WizardStep>,
    /// Current status
    pub status: WizardStatus,
    /// Error message (if status is Error)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WizardNextResult {
    /// Create a "next step" result
    pub fn step(step: WizardStep) -> Self {
        Self {
            done: false,
            step: Some(step),
            status: WizardStatus::Running,
            error: None,
        }
    }

    /// Create a "done" result
    pub fn done() -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Done,
            error: None,
        }
    }

    /// Create a "cancelled" result
    pub fn cancelled() -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Cancelled,
            error: None,
        }
    }

    /// Create an "error" result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Error,
            error: Some(message.into()),
        }
    }
}

/// Answer to a wizard step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardAnswer {
    /// Step ID being answered
    pub step_id: String,
    /// Answer value
    pub value: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_step_builder() {
        let step = WizardStep::text("api-key", "Enter your API key")
            .with_title("Authentication")
            .with_placeholder("sk-...")
            .with_sensitive(true);

        assert_eq!(step.id, "api-key");
        assert_eq!(step.step_type, StepType::Text);
        assert_eq!(step.title.as_deref(), Some("Authentication"));
        assert!(step.sensitive);
    }

    #[test]
    fn test_option_builder() {
        let opt = WizardOption::new("anthropic", "Anthropic Claude")
            .with_hint("Recommended for most use cases");

        assert_eq!(opt.value, json!("anthropic"));
        assert_eq!(opt.label, "Anthropic Claude");
        assert!(opt.hint.is_some());
    }

    #[test]
    fn test_next_result_serialization() {
        let result = WizardNextResult::step(WizardStep::note("intro", "Welcome!"));
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"done\":false"));
        assert!(json.contains("\"type\":\"note\""));

        let result = WizardNextResult::done();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"done\":true"));
        assert!(json.contains("\"status\":\"done\""));
    }

    #[test]
    fn test_status_values() {
        assert_eq!(WizardStatus::default(), WizardStatus::Running);

        let json = serde_json::to_string(&WizardStatus::Cancelled).unwrap();
        assert_eq!(json, "\"cancelled\"");
    }
}
