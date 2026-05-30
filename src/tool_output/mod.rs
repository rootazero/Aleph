//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Compression of verbose tool outputs (e.g. Chrome DevTools MCP)
//! - Semantic distillation of command / log output (errors + paths)

pub(crate) mod compressor;
pub mod distill;
