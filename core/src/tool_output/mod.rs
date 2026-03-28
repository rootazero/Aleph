//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Compression of verbose tool outputs (e.g. Chrome DevTools MCP)

pub mod compressor;

pub use compressor::compress_tool_output;
