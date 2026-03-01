//! Conditional sync primitives for loom compatibility.
//!
//! Under normal compilation, these re-export `std::sync` types at zero cost.
//! Under `--features loom`, Mutex/RwLock/atomics switch to loom's instrumented
//! versions that enable exhaustive concurrency testing.
//!
//! Note: `Arc` is always `std::sync::Arc` because `loom::sync::Arc` is not a
//! drop-in replacement when used with external crate APIs (tokio, etc.).
//!
//! ## Lock Hierarchy
//!
//! Acquire locks in this order to prevent deadlock:
//!
//! - Level 0: StateDatabase (resilience/database)
//! - Level 1: MemoryStore (memory/)
//! - Level 2: ToolRegistry, ChannelRegistry (dispatcher/, gateway/)
//! - Level 3: UI state, progress monitors

// Arc is always std::sync::Arc — loom::sync::Arc is incompatible with
// external crate APIs that expect std::sync::Arc (e.g. tokio::sync).
pub(crate) use std::sync::Arc;

#[cfg(feature = "loom")]
pub(crate) use loom::sync::{Mutex, RwLock};
#[cfg(feature = "loom")]
pub(crate) use loom::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

#[cfg(not(feature = "loom"))]
pub(crate) use std::sync::{Mutex, RwLock};
#[cfg(not(feature = "loom"))]
#[allow(unused_imports)] // AtomicUsize/MutexGuard used by test code only
pub(crate) use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
