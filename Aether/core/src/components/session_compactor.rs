//! Session compactor component - manages token limits via compaction.
//!
//! Subscribes to: SessionUpdated (monitors token count)
//! Publishes: SessionCompacted

/// Session Compactor - summarizes old parts when token limit approached
pub struct SessionCompactor;
