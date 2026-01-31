# Agent-Action Interaction System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance agent_loop with rich user interaction types (Confirmation/SingleChoice/MultiChoice/TextInput) and structured responses.

**Architecture:** Extend existing Decision/Action/ActionResult types with QuestionKind enum. Unify AskUser/AskUserMultigroup into single variant. Add structured UserAnswer replacing plain String responses.

**Tech Stack:** Rust, serde, async-trait, inquire (CLI interactions)

---

## Phase 1: Add New Types (Non-breaking)

### Task 1.1: Create question.rs module

**Files:**
- Create: `core/src/agent_loop/question.rs`

**Step 1: Write the test file first**

```rust
// At the end of question.rs, add tests module
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
```

**Step 2: Write the implementation**

```rust
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
```

**Step 3: Run test to verify**

Run: `cargo test -p aethecore question::tests --lib 2>&1 | tail -20`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/agent_loop/question.rs
git commit -m "feat(agent-loop): add QuestionKind types for structured user interaction"
```

---

### Task 1.2: Create answer.rs module

**Files:**
- Create: `core/src/agent_loop/answer.rs`

**Step 1: Write the test first**

```rust
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
```

**Step 2: Write the implementation**

```rust
//! User answer types for structured responses
//!
//! This module defines structured user responses that replace
//! plain String responses, enabling type-safe answer handling.

use serde::{Deserialize, Serialize};

/// Structured user answer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
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

impl Default for UserAnswer {
    fn default() -> Self {
        UserAnswer::Cancelled
    }
}
```

**Step 3: Run test to verify**

Run: `cargo test -p aethecore answer::tests --lib 2>&1 | tail -20`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/agent_loop/answer.rs
git commit -m "feat(agent-loop): add UserAnswer type for structured responses"
```

---

### Task 1.3: Update mod.rs to export new modules

**Files:**
- Modify: `core/src/agent_loop/mod.rs`

**Step 1: Add module declarations and re-exports**

Add after line 59 (after `pub mod state;`):

```rust
pub mod answer;
pub mod question;
```

Add to re-exports section (after line 74):

```rust
pub use answer::UserAnswer;
pub use question::{ChoiceOption, QuestionKind, TextValidation};
```

**Step 2: Run build to verify**

Run: `cargo build -p aethecore 2>&1 | tail -10`
Expected: Build succeeds

**Step 3: Commit**

```bash
git add core/src/agent_loop/mod.rs
git commit -m "feat(agent-loop): export question and answer modules"
```

---

## Phase 2: Update Decision Types

### Task 2.1: Add new Decision::AskUser variant with QuestionKind

**Files:**
- Modify: `core/src/agent_loop/decision.rs`

**Step 1: Add import at top of file (after line 6)**

```rust
use super::question::QuestionKind;
use super::answer::UserAnswer;
```

**Step 2: Add new AskUserRich variant to Decision enum (after AskUserMultigroup, around line 41)**

```rust
    /// Request rich user input with structured question type
    AskUserRich {
        question: String,
        kind: QuestionKind,
        #[serde(default)]
        question_id: Option<String>,
    },
```

**Step 3: Update Decision::decision_type() method (around line 70)**

Add case:
```rust
Decision::AskUserRich { .. } => "ask_user_rich",
```

**Step 4: Add new Action::UserInteractionRich variant (after UserInteractionMultigroup, around line 96)**

```rust
    /// Rich user interaction request
    UserInteractionRich {
        question: String,
        kind: QuestionKind,
        #[serde(default)]
        question_id: Option<String>,
    },
```

**Step 5: Update Action::action_type() method (around line 138)**

Add case:
```rust
Action::UserInteractionRich { .. } => "ask_user_rich".to_string(),
```

**Step 6: Update Action::args_summary() method**

Add case:
```rust
Action::UserInteractionRich { question, kind, .. } => {
    format!("{} (type: {:?})", question, std::mem::discriminant(kind))
}
```

**Step 7: Update Action::is_terminal() method**

No change needed (user interactions are not terminal).

**Step 8: Update From<Decision> for Action impl (around line 178)**

Add case:
```rust
Decision::AskUserRich { question, kind, question_id } => {
    Action::UserInteractionRich { question, kind, question_id }
}
```

**Step 9: Add new ActionResult::UserResponseRich variant (after UserResponse, around line 207)**

```rust
    /// User provided structured response
    UserResponseRich {
        response: UserAnswer,
    },
```

**Step 10: Update ActionResult::is_success() method**

Add case:
```rust
| ActionResult::UserResponseRich { .. }
```

**Step 11: Update ActionResult::summary() and full_output() methods**

Add case for UserResponseRich:
```rust
ActionResult::UserResponseRich { response } => {
    format!("User: {}", response.to_llm_feedback())
}
```

**Step 12: Update LlmAction enum (around line 294)**

Add variant:
```rust
AskUserRich {
    question: String,
    kind: QuestionKind,
    #[serde(default)]
    question_id: Option<String>,
},
```

**Step 13: Update From<LlmAction> for Decision impl**

Add case:
```rust
LlmAction::AskUserRich { question, kind, question_id } => {
    Decision::AskUserRich { question, kind, question_id }
}
```

**Step 14: Add test for new types**

```rust
#[test]
fn test_ask_user_rich_serialization() {
    use super::super::question::{QuestionKind, ChoiceOption};

    let decision = Decision::AskUserRich {
        question: "Choose an option".to_string(),
        kind: QuestionKind::SingleChoice {
            choices: vec![
                ChoiceOption::new("Option A"),
                ChoiceOption::new("Option B"),
            ],
            default_index: Some(0),
        },
        question_id: None,
    };

    let json = serde_json::to_string(&decision).unwrap();
    assert!(json.contains("ask_user_rich"));
    assert!(json.contains("single_choice"));

    let parsed: Decision = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, Decision::AskUserRich { .. }));
}

#[test]
fn test_user_response_rich() {
    use super::super::answer::UserAnswer;

    let result = ActionResult::UserResponseRich {
        response: UserAnswer::SingleChoice {
            selected_index: 0,
            selected_label: "Option A".to_string(),
        },
    };

    assert!(result.is_success());
    assert!(result.summary().contains("Option A"));
}
```

**Step 15: Run tests**

Run: `cargo test -p aethecore decision::tests --lib 2>&1 | tail -20`
Expected: All tests pass

**Step 16: Commit**

```bash
git add core/src/agent_loop/decision.rs
git commit -m "feat(agent-loop): add AskUserRich decision variant with QuestionKind"
```

---

## Phase 3: Update LoopCallback

### Task 3.1: Add on_user_question method to LoopCallback

**Files:**
- Modify: `core/src/agent_loop/callback.rs`

**Step 1: Add imports at top (after line 8)**

```rust
use super::question::QuestionKind;
use super::answer::UserAnswer;
```

**Step 2: Add new method to LoopCallback trait (after on_user_multigroup_required, around line 60)**

```rust
    /// Called when LLM asks for rich user input with structured question type
    /// Returns the user's structured response
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        // Default implementation: convert to legacy format for backward compatibility
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                let response = self.on_user_input_required(question, None).await;
                let confirmed = response.to_lowercase() == "yes"
                    || response.to_lowercase() == "y"
                    || response == "true"
                    || (response.is_empty() && *default);
                UserAnswer::Confirmation { confirmed }
            }
            QuestionKind::SingleChoice { choices, .. } => {
                let options: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
                let response = self.on_user_input_required(question, Some(&options)).await;
                let selected_index = choices.iter()
                    .position(|c| c.label == response)
                    .unwrap_or(0);
                UserAnswer::SingleChoice {
                    selected_index,
                    selected_label: response,
                }
            }
            QuestionKind::MultiChoice { choices, .. } => {
                let options: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
                let response = self.on_user_input_required(question, Some(&options)).await;
                // Parse comma-separated selections
                let selections: Vec<&str> = response.split(',').map(|s| s.trim()).collect();
                let mut indices = Vec::new();
                let mut labels = Vec::new();
                for sel in selections {
                    if let Some(idx) = choices.iter().position(|c| c.label == sel) {
                        indices.push(idx);
                        labels.push(sel.to_string());
                    }
                }
                UserAnswer::MultiChoice {
                    selected_indices: indices,
                    selected_labels: labels,
                }
            }
            QuestionKind::TextInput { .. } => {
                let response = self.on_user_input_required(question, None).await;
                UserAnswer::TextInput { text: response }
            }
        }
    }
```

**Step 3: Update blanket implementation for &T (around line 140)**

Add after on_user_multigroup_required impl:

```rust
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        (*self).on_user_question(question, kind).await
    }
```

**Step 4: Update NoOpLoopCallback (around line 209)**

Add after on_user_multigroup_required impl:

```rust
    async fn on_user_question(&self, _question: &str, kind: &QuestionKind) -> UserAnswer {
        // Auto-respond based on question type
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            QuestionKind::SingleChoice { choices, default_index } => {
                let idx = default_index.unwrap_or(0);
                let label = choices.get(idx)
                    .map(|c| c.label.clone())
                    .unwrap_or_default();
                UserAnswer::SingleChoice {
                    selected_index: idx,
                    selected_label: label,
                }
            }
            QuestionKind::MultiChoice { choices, .. } => {
                // Select first option by default
                let label = choices.first()
                    .map(|c| c.label.clone())
                    .unwrap_or_default();
                UserAnswer::MultiChoice {
                    selected_indices: vec![0],
                    selected_labels: vec![label],
                }
            }
            QuestionKind::TextInput { .. } => {
                UserAnswer::TextInput { text: "ok".to_string() }
            }
        }
    }
```

**Step 5: Update LoggingCallback (around line 298)**

Add after on_user_multigroup_required impl:

```rust
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        tracing::warn!(
            "{} Rich user input required: {} (type: {:?}) (auto-responding)",
            self.prefix,
            question,
            std::mem::discriminant(kind)
        );
        // Delegate to default implementation
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            _ => UserAnswer::TextInput { text: "continue".to_string() }
        }
    }
```

**Step 6: Add LoopEvent::UserQuestionRequired variant (around line 377)**

```rust
    UserQuestionRequired { question: String, kind_type: String },
```

**Step 7: Update CollectingCallback (around line 453)**

Add after on_user_multigroup_required impl:

```rust
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        self.push(LoopEvent::UserQuestionRequired {
            question: question.to_string(),
            kind_type: format!("{:?}", std::mem::discriminant(kind)),
        });
        // Return based on type
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            QuestionKind::SingleChoice { choices, default_index } => {
                let idx = default_index.unwrap_or(0);
                UserAnswer::SingleChoice {
                    selected_index: idx,
                    selected_label: choices.get(idx).map(|c| c.label.clone()).unwrap_or_default(),
                }
            }
            QuestionKind::MultiChoice { .. } => {
                UserAnswer::MultiChoice {
                    selected_indices: vec![],
                    selected_labels: vec![],
                }
            }
            QuestionKind::TextInput { .. } => {
                UserAnswer::TextInput { text: "test_response".to_string() }
            }
        }
    }
```

**Step 8: Add test for new callback method**

```rust
#[tokio::test]
async fn test_collecting_callback_user_question() {
    use super::super::question::{QuestionKind, ChoiceOption};
    use super::super::answer::UserAnswer;

    let callback = CollectingCallback::new();

    let kind = QuestionKind::SingleChoice {
        choices: vec![
            ChoiceOption::new("A"),
            ChoiceOption::new("B"),
        ],
        default_index: Some(1),
    };

    let answer = callback.on_user_question("Pick one", &kind).await;

    assert!(matches!(answer, UserAnswer::SingleChoice { selected_index: 1, .. }));

    let events = callback.events();
    assert!(events.iter().any(|e| matches!(e, LoopEvent::UserQuestionRequired { .. })));
}
```

**Step 9: Run tests**

Run: `cargo test -p aethecore callback::tests --lib 2>&1 | tail -20`
Expected: All tests pass

**Step 10: Commit**

```bash
git add core/src/agent_loop/callback.rs
git commit -m "feat(agent-loop): add on_user_question method to LoopCallback"
```

---

## Phase 4: Update agent_loop.rs

### Task 4.1: Handle AskUserRich in main loop

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs`

**Step 1: Add import (after line 12)**

```rust
use super::answer::UserAnswer;
use super::question::QuestionKind;
```

**Step 2: Add handling for Decision::AskUserRich in run() method (after AskUserMultigroup handling, around line 455)**

```rust
                Decision::AskUserRich { question, kind, question_id } => {
                    let answer = callback
                        .on_user_question(question, kind)
                        .await;

                    // Record user interaction as a step
                    let step = LoopStep {
                        step_id: state.step_count,
                        observation_summary: String::new(),
                        thinking: thinking.clone(),
                        action: Action::UserInteractionRich {
                            question: question.clone(),
                            kind: kind.clone(),
                            question_id: question_id.clone(),
                        },
                        result: ActionResult::UserResponseRich { response: answer },
                        tokens_used: 0,
                        duration_ms: 0,
                    };
                    state.record_step(step);
                    guard.record_action("ask_user_rich");
                    continue;
                }
```

**Step 3: Add test for AskUserRich handling**

```rust
#[tokio::test]
async fn test_ask_user_rich_handling() {
    use crate::agent_loop::question::{QuestionKind, ChoiceOption};
    use crate::agent_loop::callback::LoopEvent;

    let thinker = Arc::new(MockThinker::new(vec![
        Decision::AskUserRich {
            question: "Choose option".to_string(),
            kind: QuestionKind::SingleChoice {
                choices: vec![
                    ChoiceOption::new("A"),
                    ChoiceOption::new("B"),
                ],
                default_index: Some(0),
            },
            question_id: None,
        },
        Decision::Complete {
            summary: "Done after user choice".to_string(),
        },
    ]));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let agent_loop = AgentLoop::new(
        thinker,
        executor,
        compressor,
        LoopConfig::for_testing(),
    );

    let callback = CollectingCallback::new();

    let result = agent_loop
        .run(
            "Test rich question".to_string(),
            RequestContext::empty(),
            vec![],
            &callback,
            None,
            None,
        )
        .await;

    assert!(matches!(result, LoopResult::Completed { steps: 1, .. }));

    let events = callback.events();
    assert!(events.iter().any(|e| matches!(e, LoopEvent::UserQuestionRequired { .. })));
}
```

**Step 4: Run tests**

Run: `cargo test -p aethecore agent_loop::tests --lib 2>&1 | tail -30`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/agent_loop/agent_loop.rs
git commit -m "feat(agent-loop): handle AskUserRich decision in main loop"
```

---

## Phase 5: Add CLI Callback (Optional)

### Task 5.1: Add inquire dependency

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add dependency**

Find the `[dependencies]` section and add:

```toml
inquire = { version = "0.7", optional = true }
```

**Step 2: Add feature flag**

Find or create `[features]` section and add:

```toml
cli-interaction = ["inquire"]
```

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "chore: add optional inquire dependency for CLI interaction"
```

---

### Task 5.2: Create callback_cli.rs module

**Files:**
- Create: `core/src/agent_loop/callback_cli.rs`

**Step 1: Write the implementation**

```rust
//! CLI callback implementation using inquire
//!
//! This module provides a CLI-based LoopCallback implementation
//! that uses the `inquire` crate for interactive user prompts.

#[cfg(feature = "cli-interaction")]
use inquire::{Confirm, MultiSelect, Select, Text};

use async_trait::async_trait;
use serde_json::Value;

use super::callback::LoopCallback;
use super::decision::{Action, ActionResult};
use super::guards::GuardViolation;
use super::question::QuestionKind;
use super::answer::UserAnswer;
use super::state::{LoopState, Thinking};

/// CLI callback that uses inquire for interactive prompts
#[cfg(feature = "cli-interaction")]
pub struct CliLoopCallback {
    /// Auto-approve all confirmations (for automation)
    pub auto_approve: bool,
}

#[cfg(feature = "cli-interaction")]
impl CliLoopCallback {
    pub fn new() -> Self {
        Self { auto_approve: false }
    }

    pub fn with_auto_approve(mut self, auto: bool) -> Self {
        self.auto_approve = auto;
        self
    }

    fn prompt_user_sync(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        match kind {
            QuestionKind::Confirmation { default, labels } => {
                if self.auto_approve {
                    return UserAnswer::Confirmation { confirmed: *default };
                }

                let (yes, no) = labels.clone().unwrap_or_else(||
                    ("Yes".to_string(), "No".to_string())
                );

                match Confirm::new(question)
                    .with_default(*default)
                    .with_help_message(&format!("{} / {}", yes, no))
                    .prompt()
                {
                    Ok(confirmed) => UserAnswer::Confirmation { confirmed },
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::SingleChoice { choices, default_index } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                let mut select = Select::new(question, labels);
                if let Some(idx) = default_index {
                    select = select.with_starting_cursor(*idx);
                }

                match select.prompt() {
                    Ok(selected) => {
                        let idx = choices.iter()
                            .position(|c| c.label == selected)
                            .unwrap_or(0);
                        UserAnswer::SingleChoice {
                            selected_index: idx,
                            selected_label: selected.to_string(),
                        }
                    }
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::MultiChoice { choices, min_selections, .. } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                match MultiSelect::new(question, labels).prompt() {
                    Ok(selected) => {
                        if selected.len() < *min_selections {
                            return UserAnswer::Cancelled;
                        }
                        let indices: Vec<usize> = selected.iter()
                            .filter_map(|s| choices.iter().position(|c| &c.label == *s))
                            .collect();
                        UserAnswer::MultiChoice {
                            selected_indices: indices,
                            selected_labels: selected.into_iter().map(String::from).collect(),
                        }
                    }
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::TextInput { placeholder, .. } => {
                let mut prompt = Text::new(question);
                if let Some(p) = placeholder {
                    prompt = prompt.with_placeholder(p);
                }

                match prompt.prompt() {
                    Ok(text) => UserAnswer::TextInput { text },
                    Err(_) => UserAnswer::Cancelled,
                }
            }
        }
    }
}

#[cfg(feature = "cli-interaction")]
impl Default for CliLoopCallback {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "cli-interaction")]
#[async_trait]
impl LoopCallback for CliLoopCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        println!("🚀 Starting loop: {}", state.session_id);
    }

    async fn on_step_start(&self, step: usize) {
        println!("📍 Step {}", step);
    }

    async fn on_thinking_start(&self, _step: usize) {
        println!("🤔 Thinking...");
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        println!("💡 Decision: {:?}", thinking.decision.decision_type());
    }

    async fn on_action_start(&self, action: &Action) {
        println!("⚡ Executing: {}", action.action_type());
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        let status = if result.is_success() { "✅" } else { "❌" };
        println!("{} {} completed", status, action.action_type());
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        if self.auto_approve {
            return true;
        }

        let prompt = format!("Allow execution of '{}'?", tool_name);
        match Confirm::new(&prompt).with_default(false).prompt() {
            Ok(confirmed) => confirmed,
            Err(_) => false,
        }
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        if let Some(opts) = options {
            let opts_ref: Vec<&str> = opts.iter().map(|s| s.as_str()).collect();
            match Select::new(question, opts_ref).prompt() {
                Ok(selected) => selected.to_string(),
                Err(_) => String::new(),
            }
        } else {
            match Text::new(question).prompt() {
                Ok(text) => text,
                Err(_) => String::new(),
            }
        }
    }

    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[super::decision::QuestionGroup],
    ) -> String {
        println!("📋 {}", question);
        let mut results = serde_json::Map::new();

        for group in groups {
            let opts: Vec<&str> = group.options.iter().map(|s| s.as_str()).collect();
            if let Ok(selected) = Select::new(&group.prompt, opts).prompt() {
                results.insert(group.id.clone(), serde_json::Value::String(selected.to_string()));
            }
        }

        serde_json::to_string(&results).unwrap_or_default()
    }

    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        self.prompt_user_sync(question, kind)
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        eprintln!("⚠️  Guard triggered: {}", violation.description());
    }

    async fn on_complete(&self, summary: &str) {
        println!("✅ Complete: {}", summary);
    }

    async fn on_failed(&self, reason: &str) {
        eprintln!("❌ Failed: {}", reason);
    }
}
```

**Step 2: Update mod.rs to conditionally export**

Add to `core/src/agent_loop/mod.rs`:

```rust
#[cfg(feature = "cli-interaction")]
pub mod callback_cli;

#[cfg(feature = "cli-interaction")]
pub use callback_cli::CliLoopCallback;
```

**Step 3: Commit**

```bash
git add core/src/agent_loop/callback_cli.rs core/src/agent_loop/mod.rs
git commit -m "feat(agent-loop): add CLI callback implementation with inquire"
```

---

## Phase 6: Final Verification

### Task 6.1: Run all tests and verify build

**Step 1: Build without cli-interaction feature**

Run: `cargo build -p aethecore 2>&1 | tail -10`
Expected: Build succeeds

**Step 2: Build with cli-interaction feature**

Run: `cargo build -p aethecore --features cli-interaction 2>&1 | tail -10`
Expected: Build succeeds

**Step 3: Run all agent_loop tests**

Run: `cargo test -p aethecore agent_loop --lib 2>&1 | tail -30`
Expected: All tests pass (may skip cli-interaction tests without feature)

**Step 4: Final commit (if any cleanup needed)**

```bash
git status
# If clean, no action needed
```

---

## Summary

| Phase | Task | Description |
|-------|------|-------------|
| 1.1 | question.rs | QuestionKind, ChoiceOption, TextValidation types |
| 1.2 | answer.rs | UserAnswer type with to_llm_feedback() |
| 1.3 | mod.rs | Export new modules |
| 2.1 | decision.rs | Add AskUserRich variant to Decision/Action/ActionResult |
| 3.1 | callback.rs | Add on_user_question() method |
| 4.1 | agent_loop.rs | Handle AskUserRich in main loop |
| 5.1 | Cargo.toml | Add inquire dependency (optional) |
| 5.2 | callback_cli.rs | CLI callback implementation |
| 6.1 | Verification | Build and test all |

**Total estimated tasks:** 9 tasks with ~35 individual steps
