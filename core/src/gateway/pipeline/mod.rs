//! Message Pipeline
//!
//! Processes inbound messages through debounce, media download,
//! media understanding, and enrichment stages.

pub mod types;
pub mod debounce;
pub use debounce::{DebounceBuffer, DebounceConfig};
pub mod media_download;
pub use media_download::MediaDownloader;
// pub mod media_understanding; // Task 4: MediaUnderstander

pub use types::*;
