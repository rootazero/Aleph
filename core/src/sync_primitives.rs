//! Conditional sync primitives for loom compatibility.
//!
//! Under normal compilation, these re-export `std::sync` types at zero cost.
//! Under `--features loom` (with `RUSTFLAGS="--cfg loom"`), these switch to
//! loom's instrumented versions that enable exhaustive concurrency testing.

#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
