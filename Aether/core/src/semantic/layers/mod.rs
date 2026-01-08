//! Matching layer implementations.
//!
//! This module provides concrete implementations of the `MatchingLayer` trait
//! for different matching strategies.
//!
//! # Layers
//!
//! - `CommandLayer`: Exact ^/xxx command matching (priority 0)
//! - `RegexLayer`: Pattern-based regex matching (priority 1)
//! - `KeywordLayer`: Weighted keyword scoring (priority 2)
//! - `ContextLayer`: Context-aware inference (priority 3)

pub mod command;
pub mod context;
pub mod keyword;
pub mod regex;

pub use command::CommandLayer;
pub use context::ContextLayer;
pub use keyword::KeywordLayer;
pub use regex::RegexLayer;
