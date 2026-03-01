//! Conditional sync primitives for loom compatibility.
//!
//! Under normal compilation, these re-export `std::sync` types at zero cost.
//! Under `--features loom` (with `RUSTFLAGS="--cfg loom"`), these switch to
//! loom's instrumented versions that enable exhaustive concurrency testing.
//!
//! ## Lock Hierarchy
//!
//! Acquire locks in this order to prevent deadlock:
//!
//! - Level 0: StateDatabase (resilience/database)
//! - Level 1: MemoryStore (memory/)
//! - Level 2: ToolRegistry, ChannelRegistry (dispatcher/, gateway/)
//! - Level 3: UI state, progress monitors

#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
