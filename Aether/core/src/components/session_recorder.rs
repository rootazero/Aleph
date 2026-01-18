//! Session recorder component - persists session state to SQLite.
//!
//! Subscribes to: All events (for recording)
//! Publishes: SessionCreated, SessionUpdated

/// Session Recorder - persists execution state to SQLite
pub struct SessionRecorder;
