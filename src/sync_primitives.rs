//! Conditional sync primitives for loom compatibility.
//!
//! These re-export `std::sync` types at zero cost. The loom concurrency tests
//! (`*/loom_concurrency.rs`, gated behind `cfg(all(test, feature = "loom"))`)
//! import loom's instrumented types directly rather than having them swapped in
//! globally here. A global swap would break the crate's many `static` items and
//! `const fn` constructors that initialise atomics/mutexes, because loom's
//! `new()` is not `const`. The loom tests model self-contained patterns, so they
//! gain nothing from a global swap.
//!
//! Note: `Arc` is always `std::sync::Arc` because `loom::sync::Arc` is not a
//! drop-in replacement when used with external crate APIs (tokio, etc.).
//!
//! ## Lock Hierarchy
//!
//! Acquire locks in this order to prevent deadlock:
//!
//! - Level 0: `StateDatabase` (resilience/database)
//! - Level 1: `MemoryStore` (memory/)
//! - Level 2: `ToolCatalog`, `ChannelRegistry` ((`tool_metadata`/, gateway/))
//! - Level 3: UI state, progress monitors

// Arc is always std::sync::Arc — loom::sync::Arc is incompatible with
// external crate APIs that expect std::sync::Arc (e.g. tokio::sync).
pub use std::sync::Arc;

/// Async `RwLock` for tokio contexts.
///
/// Daemon and other async modules use this instead of `std::sync::RwLock`
/// to avoid deadlocks when holding a guard across `.await` points.
/// Note: loom does not instrument async `RwLock`; this is acceptable because
/// loom tests target sync concurrency patterns only.
pub use tokio::sync::RwLock as AsyncRwLock;

#[allow(unused_imports)] // AtomicUsize used by test code only
pub use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
pub use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockWriteGuard};
