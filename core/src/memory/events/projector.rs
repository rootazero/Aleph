//! Memory Event Sourcing — Event Projector
//!
//! [`EventProjector`] folds a stream of [`super::MemoryEvent`]s into a
//! current-state [`crate::memory::context::MemoryFact`] projection.
//! Used both for rebuilding read-side state and for time-travel queries.
