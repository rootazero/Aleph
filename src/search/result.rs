//! Compatibility re-export shim — the definition moved to
//! [`aleph_slot_api::search`] in Task 3 of the P0 modularization
//! (`SearchResult`, its seven fields with their serde attributes, and the
//! exact `new(title, url, snippet)` constructor).
//!
//! This is a `pub use`, not a second definition: both paths share one
//! `TypeId`, pinned by `tests/modularization_contracts.rs`.

pub use aleph_slot_api::search::SearchResult;
