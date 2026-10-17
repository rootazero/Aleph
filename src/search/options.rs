//! Compatibility re-export shim — the definitions moved to
//! [`aleph_slot_api::search`] in Task 3 of the P0 modularization
//! (`Recency`, `SearchOptions`, `MAX_SEARCH_RESULTS`, all constructors,
//! defaults, serde attributes, and the per-provider mapper bodies).
//!
//! This is a `pub use`, not a second definition: both paths share one
//! `TypeId`, pinned by `tests/modularization_contracts.rs`. The capability
//! census reads the API-side source directly (`OPTIONS_SRC` in
//! `providers/capability_census.rs`) — never this shim.

pub use aleph_slot_api::search::{Recency, SearchOptions, MAX_SEARCH_RESULTS};
