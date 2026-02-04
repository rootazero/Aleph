//! Video transcript extraction module
//!
//! This module provides capabilities for extracting transcripts from video platforms
//! (currently YouTube) and integrating them into the AI context.
//!
//! # Architecture
//!
//! - `transcript`: Core data structures for video transcripts
//! - `youtube`: YouTube-specific extraction logic
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::video::{YouTubeExtractor, VideoTranscript};
//! use alephcore::config::VideoConfig;
//!
//! let config = VideoConfig::default();
//! let extractor = YouTubeExtractor::new(config);
//! let transcript = extractor.extract_transcript("https://youtube.com/watch?v=...").await?;
//! println!("{}", transcript.format_for_context());
//! ```

pub mod transcript;
mod youtube;

// Re-exports
pub use transcript::{TranscriptSegment, VideoTranscript};
pub use youtube::{extract_youtube_url, YouTubeExtractor};
