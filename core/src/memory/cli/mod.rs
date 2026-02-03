//! Memory CLI Module
//!
//! Provides command-line interface for memory management operations.
//! Supports direct SQLite access with file locking for concurrent safety.

mod lock;

pub use lock::{LockError, LockMode, MemoryLock};

// Commands will be added in subsequent tasks
// pub mod commands;
