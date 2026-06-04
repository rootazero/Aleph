//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Compression of verbose tool outputs (e.g. Chrome DevTools MCP)
//! - Semantic distillation of command / log output (errors + paths)
//! - Sanitization of raw command output (ANSI escapes + binary control bytes)

pub(crate) mod compressor;
pub mod distill;
pub mod sanitize;
