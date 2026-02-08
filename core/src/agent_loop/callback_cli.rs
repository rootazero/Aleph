//! CLI callback implementation using inquire
//!
//! This module provides a CLI-based LoopCallback implementation
//! that uses the `inquire` crate for interactive user prompts.

use inquire::{Confirm, MultiSelect, Select, Text};

use async_trait::async_trait;
use serde_json::Value;

use super::answer::UserAnswer;
use super::callback::LoopCallback;
use super::decision::{Action, ActionResult, QuestionGroup};
use super::guards::GuardViolation;
use super::question::QuestionKind;
use super::state::{LoopState, Thinking};

/// CLI callback that uses inquire for interactive prompts
pub struct CliLoopCallback {
    /// Auto-approve all confirmations (for automation)
    pub auto_approve: bool,
}

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

                let (yes, no) = labels
                    .clone()
                    .unwrap_or_else(|| ("Yes".to_string(), "No".to_string()));

                match Confirm::new(question)
                    .with_default(*default)
                    .with_help_message(&format!("{} / {}", yes, no))
                    .prompt()
                {
                    Ok(confirmed) => UserAnswer::Confirmation { confirmed },
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::SingleChoice {
                choices,
                default_index,
            } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                let mut select = Select::new(question, labels);
                if let Some(idx) = default_index {
                    select = select.with_starting_cursor(*idx);
                }

                match select.prompt() {
                    Ok(selected) => {
                        let idx = choices
                            .iter()
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

            QuestionKind::MultiChoice {
                choices,
                min_selections,
                ..
            } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                match MultiSelect::new(question, labels).prompt() {
                    Ok(selected) => {
                        if selected.len() < *min_selections {
                            return UserAnswer::Cancelled;
                        }
                        let indices: Vec<usize> = selected
                            .iter()
                            .filter_map(|s| choices.iter().position(|c| c.label == *s))
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

impl Default for CliLoopCallback {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LoopCallback for CliLoopCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        println!("Starting loop: {}", state.session_id);
    }

    async fn on_step_start(&self, step: usize) {
        println!("Step {}", step);
    }

    async fn on_thinking_start(&self, _step: usize) {
        println!("Thinking...");
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        println!("Decision: {:?}", thinking.decision.decision_type());
    }

    async fn on_action_start(&self, action: &Action) {
        println!("Executing: {}", action.action_type());
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        let status = if result.is_success() { "OK" } else { "FAIL" };
        println!("[{}] {} completed", status, action.action_type());
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        if self.auto_approve {
            return true;
        }

        let prompt = format!("Allow execution of '{}'?", tool_name);
        Confirm::new(&prompt).with_default(false).prompt().unwrap_or_default()
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
            Text::new(question).prompt().unwrap_or_default()
        }
    }

    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[QuestionGroup],
    ) -> String {
        println!("{}", question);
        let mut results = serde_json::Map::new();

        for group in groups {
            let opts: Vec<&str> = group.options.iter().map(|s| s.as_str()).collect();
            if let Ok(selected) = Select::new(&group.prompt, opts).prompt() {
                results.insert(
                    group.id.clone(),
                    serde_json::Value::String(selected.to_string()),
                );
            }
        }

        serde_json::to_string(&results).unwrap_or_default()
    }

    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        self.prompt_user_sync(question, kind)
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        eprintln!("Guard triggered: {}", violation.description());
    }

    async fn on_complete(&self, summary: &str) {
        println!("Complete: {}", summary);
    }

    async fn on_failed(&self, reason: &str) {
        eprintln!("Failed: {}", reason);
    }

    async fn on_aborted(&self) {
        println!("Aborted by user");
    }

    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        _arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        eprintln!(
            "Doom loop detected: {} called {} times with identical arguments",
            tool_name, repeat_count
        );

        if self.auto_approve {
            return false;
        }

        Confirm::new("Continue anyway?").with_default(false).prompt().unwrap_or_default()
    }

    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        println!(
            "Retry scheduled: attempt {}/{}, delay {}ms, error: {}",
            attempt, max_retries, delay_ms, error
        );
    }

    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        eprintln!("Retries exhausted after {} attempts: {}", attempts, error);
    }
}
