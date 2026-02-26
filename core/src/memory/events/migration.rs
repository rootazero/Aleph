//! Memory Event Sourcing — Legacy Migration
//!
//! One-shot migration from the legacy CRUD-based LanceDB store to the
//! event-sourced model. Reads all existing [`crate::memory::context::MemoryFact`]
//! records and emits a [`super::MemoryEvent::FactMigrated`] + [`super::MemoryEvent::FactCreated`]
//! pair for each, establishing the initial event history.
