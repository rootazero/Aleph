//! Message Pipeline
//!
//! Processes inbound messages through debounce, media download,
//! media understanding, and enrichment stages.

pub mod types;
// pub mod debounce;           // Task 2: DebounceBuffer
// pub mod media_download;     // Task 3: MediaDownloader
// pub mod media_understanding; // Task 4: MediaUnderstander

pub use types::*;
