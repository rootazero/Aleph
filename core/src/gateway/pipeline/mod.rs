//! Message Pipeline
//!
//! Processes inbound messages through debounce, media download,
//! media understanding, and enrichment stages.

pub mod types;
pub mod debounce;
pub use debounce::{DebounceBuffer, DebounceConfig};
pub mod media_download;
pub use media_download::MediaDownloader;
pub mod media_understanding;
pub use media_understanding::{MediaUnderstander, UnderstandingProvider};

pub use types::*;
