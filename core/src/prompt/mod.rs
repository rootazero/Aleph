//! Unified prompt management module.
//!
//! This module provides a clean separation between execution and conversation modes.
//! The key principle: prompts only describe "how to do", never "whether to do".
//! The execution/conversation decision is made by `ExecutionIntentDecider` before
//! prompts are selected.
//!
//! # Architecture
//!
//! ```text
//! ExecutionIntentDecider (decides mode)
//!         │
//!         ├─→ ExecutionMode::Execute(category)
//!         │      → PromptBuilder::executor_prompt(category, tools)
//!         │
//!         └─→ ExecutionMode::Converse
//!                → PromptBuilder::conversational_prompt()
//! ```
//!
//! # Design Principles
//!
//! 1. **No decision-making in prompts**: Prompts don't contain "if X then do Y" logic
//! 2. **No negative instructions**: Avoid "don't do X" - instead, mode selection prevents X
//! 3. **Category-specific tools**: Only inject relevant tools to reduce confusion
//! 4. **Minimal token usage**: ~300 tokens vs ~2000 tokens in old system

mod builder;
mod conversational;
mod executor;
mod templates;

pub use builder::{PromptBuilder, PromptConfig};
pub use conversational::ConversationalPrompt;
pub use executor::ExecutorPrompt;
pub use templates::{PromptTemplate, TemplateVar};
