//! Markdown parsing utilities.
//!
//! Provides tools for parsing and manipulating Markdown content,
//! particularly focused on code fence handling for streaming output.

pub mod fences;

pub use fences::{
    find_fence_at, get_fence_split, is_safe_fence_break, parse_fence_spans, FenceSpan, FenceSplit,
};
