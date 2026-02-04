// Aleph/core/src/question/mod.rs
//! Structured User Interaction System
//!
//! This module implements a structured Q&A system for agent-user interaction,
//! replacing simple callback-based input with rich, typed questions.
//!
//! # Features
//!
//! - Multi-select options
//! - Custom text input
//! - Batch questions
//! - Timeout support
//! - Event-driven async flow
//!
//! # Usage
//!
//! ```rust,ignore
//! let question = QuestionInfo::new(
//!     "Which database should we use?",
//!     "Database",
//!     vec![
//!         QuestionOption::new("PostgreSQL", "Relational database"),
//!         QuestionOption::new("MongoDB", "Document database"),
//!     ],
//! );
//!
//! let request = QuestionRequest::single("q-1", "session-1", question);
//! let answers = question_manager.ask(request).await?;
//! ```

mod error;
mod manager;

pub use error::QuestionError;
pub use manager::{PendingQuestion, QuestionManager, QuestionManagerConfig};

// Re-export from event module for convenience
pub use crate::event::question::{
    Answer, QuestionEvent, QuestionInfo, QuestionOption, QuestionReply, QuestionRequest,
};
