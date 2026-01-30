//! Markdown parsing utilities.
//!
//! Provides tools for parsing and manipulating Markdown content,
//! particularly focused on code fence handling for streaming output.

pub mod fences;

pub use fences::{
    parse_fence_spans, is_safe_fence_break, find_fence_at, get_fence_split,
    FenceSpan, FenceSplit,
};
