//! Memory Event Sourcing — Command Handler
//!
//! [`MemoryCommandHandler`] receives commands, validates them against the
//! current fact state (loaded from the event store), produces events, and
//! persists them via [`crate::memory::store::MemoryEventStore`].
