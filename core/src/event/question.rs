// Aleph/core/src/event/question.rs
//! Question-related event types for structured user interaction.
//!
//! These events enable structured Q&A between the agent loop and UI layer,
//! supporting multi-select, custom input, and batch questions.

use serde::{Deserialize, Serialize};

use super::permission::ToolCallRef;

// ============================================================================
// Question Types
// ============================================================================

/// Single option for a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Display text (1-5 words, concise)
    pub label: String,
    /// Explanation of the choice
    pub description: String,
}

impl QuestionOption {
    /// Create a new question option
    pub fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
        }
    }
}

/// Single question definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionInfo {
    /// Complete question text
    pub question: String,
    /// Short label (max 30 chars, for UI chip/tag)
    pub header: String,
    /// Available choices
    pub options: Vec<QuestionOption>,
    /// Whether multiple options can be selected
    #[serde(default)]
    pub multiple: bool,
    /// Whether to allow custom text input (default: true)
    #[serde(default = "default_true")]
    pub custom: bool,
}

fn default_true() -> bool {
    true
}

impl QuestionInfo {
    /// Create a new question with options
    pub fn new(
        question: impl Into<String>,
        header: impl Into<String>,
        options: Vec<QuestionOption>,
    ) -> Self {
        Self {
            question: question.into(),
            header: header.into(),
            options,
            multiple: false,
            custom: true,
        }
    }

    /// Create a simple yes/no question
    pub fn yes_no(question: impl Into<String>, header: impl Into<String>) -> Self {
        Self::new(
            question,
            header,
            vec![
                QuestionOption::new("Yes", "Confirm and proceed"),
                QuestionOption::new("No", "Cancel the operation"),
            ],
        )
    }

    /// Enable multiple selection
    pub fn with_multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    /// Enable/disable custom input
    pub fn with_custom(mut self, custom: bool) -> Self {
        self.custom = custom;
        self
    }
}

// ============================================================================
// Question Request/Reply Types
// ============================================================================

/// Question request sent to UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequest {
    /// Unique request ID
    pub id: String,
    /// Session ID
    pub session_id: String,
    /// Questions to ask (supports batch)
    pub questions: Vec<QuestionInfo>,
    /// Associated tool call reference (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallRef>,
}

impl QuestionRequest {
    /// Create a new question request
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        questions: Vec<QuestionInfo>,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            questions,
            tool_call: None,
        }
    }

    /// Create a single-question request
    pub fn single(
        id: impl Into<String>,
        session_id: impl Into<String>,
        question: QuestionInfo,
    ) -> Self {
        Self::new(id, session_id, vec![question])
    }

    /// Set tool call reference
    pub fn with_tool_call(mut self, tool_call: ToolCallRef) -> Self {
        self.tool_call = Some(tool_call);
        self
    }
}

/// Single question's answer (may contain multiple selections)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer(Vec<String>);

impl Answer {
    /// Create a new Answer
    pub fn new(selections: Vec<String>) -> Self {
        Self(selections)
    }

    /// Create from a single selection
    pub fn single(selection: impl Into<String>) -> Self {
        Self(vec![selection.into()])
    }

    /// Get the selections
    pub fn selections(&self) -> &[String] {
        &self.0
    }

    /// Convert into inner Vec
    pub fn into_inner(self) -> Vec<String> {
        self.0
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the number of selections
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<Vec<String>> for Answer {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for Answer {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// User's reply to a question request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionReply {
    /// Answers in order of questions (each answer is a list of selected labels)
    pub answers: Vec<Answer>,
}

impl QuestionReply {
    /// Create a new reply
    pub fn new(answers: Vec<Answer>) -> Self {
        Self { answers }
    }

    /// Create a single-answer reply
    pub fn single(answer: Vec<String>) -> Self {
        Self::new(vec![Answer::from(answer)])
    }

    /// Create a simple single-selection reply
    pub fn simple(answer: impl Into<String>) -> Self {
        Self::single(vec![answer.into()])
    }

    /// Get the first answer's first selection (common case)
    pub fn first(&self) -> Option<&str> {
        self.answers.first()?.first().map(|s| s.as_str())
    }
}

// ============================================================================
// Question Events
// ============================================================================

/// Question-related events for the event bus
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QuestionEvent {
    /// Question request sent to UI
    Asked(QuestionRequest),
    /// User replied to question
    Replied {
        session_id: String,
        request_id: String,
        answers: Vec<Answer>,
    },
    /// User dismissed/rejected the question
    Rejected {
        session_id: String,
        request_id: String,
    },
}

impl QuestionEvent {
    /// Create an Asked event
    pub fn asked(request: QuestionRequest) -> Self {
        Self::Asked(request)
    }

    /// Create a Replied event
    pub fn replied(
        session_id: impl Into<String>,
        request_id: impl Into<String>,
        answers: Vec<Answer>,
    ) -> Self {
        Self::Replied {
            session_id: session_id.into(),
            request_id: request_id.into(),
            answers,
        }
    }

    /// Create a Rejected event
    pub fn rejected(session_id: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self::Rejected {
            session_id: session_id.into(),
            request_id: request_id.into(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_question_option() {
        let opt = QuestionOption::new("Yes", "Confirm the action");
        assert_eq!(opt.label, "Yes");
        assert_eq!(opt.description, "Confirm the action");
    }

    #[test]
    fn test_question_info_builder() {
        let question = QuestionInfo::new(
            "Which database should we use?",
            "Database",
            vec![
                QuestionOption::new("PostgreSQL", "Relational database"),
                QuestionOption::new("MongoDB", "Document database"),
            ],
        )
        .with_multiple(false)
        .with_custom(true);

        assert!(!question.multiple);
        assert!(question.custom);
        assert_eq!(question.options.len(), 2);
    }

    #[test]
    fn test_yes_no_question() {
        let question = QuestionInfo::yes_no("Continue with the operation?", "Confirm");

        assert_eq!(question.options.len(), 2);
        assert_eq!(question.options[0].label, "Yes");
        assert_eq!(question.options[1].label, "No");
    }

    #[test]
    fn test_question_request() {
        let request = QuestionRequest::single(
            "q-1",
            "session-1",
            QuestionInfo::yes_no("Proceed?", "Confirm"),
        );

        assert_eq!(request.id, "q-1");
        assert_eq!(request.questions.len(), 1);
    }

    #[test]
    fn test_question_reply() {
        let reply = QuestionReply::simple("Yes");
        assert_eq!(reply.first(), Some("Yes"));

        let multi_reply = QuestionReply::single(vec!["Option A".into(), "Option B".into()]);
        assert_eq!(multi_reply.answers[0].len(), 2);
    }

    #[test]
    fn test_question_event_serialization() {
        let request = QuestionRequest::single(
            "q-1",
            "session-1",
            QuestionInfo::yes_no("Continue?", "Confirm"),
        );
        let event = QuestionEvent::asked(request);

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Asked"));

        let replied = QuestionEvent::replied("session-1", "q-1", vec![vec!["Yes".into()]]);
        let json = serde_json::to_string(&replied).unwrap();
        assert!(json.contains("Replied"));

        let rejected = QuestionEvent::rejected("session-1", "q-1");
        let json = serde_json::to_string(&rejected).unwrap();
        assert!(json.contains("Rejected"));
    }
}
