//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Compression of verbose tool outputs (e.g. Chrome `DevTools` MCP)
//! - Semantic distillation of command / log output (errors + paths)
//! - Sanitization of raw command output (ANSI escapes + binary control bytes)
//! - Content-type-routed [`structured`] reduction (log / search / diff / json)
//! - The [`hygiene`] ingress cleaner that applies the above to a tool's own
//!   structured result *before* it is flattened into the model's context
//!
//! Ordering matters: [`hygiene`] runs on the tool's `serde_json::Value` while
//! its text fields still carry real newlines. Once the value is flattened with
//! `Value::to_string()` every `\n` becomes a two-character escape and the whole
//! result collapses onto one line — at which point `structured::classify` and
//! [`distill`] can no longer see the line structure they route on.

pub(crate) mod compressor;
pub mod distill;
pub mod hygiene;
pub mod sanitize;
pub mod structured;
