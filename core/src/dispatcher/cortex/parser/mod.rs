//! StreamParser - Streaming JSON extraction with repair
//!
//! This module provides robust JSON extraction from mixed text streams,
//! handling partial data, nested structures, and malformed input gracefully.

pub mod detector;
pub mod repair;

pub use detector::{JsonFragment, JsonStreamDetector};
pub use repair::try_repair;
