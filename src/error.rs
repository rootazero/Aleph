//! Re-export shim — canonical authority lives in `aleph_slot_api`.
//!
//! The error types, classification enum, exception enum, and `Result` alias
//! that used to live here have moved to the `aleph-slot-api` workspace
//! member as part of the P0 modularization (Task 1). This file is now a
//! pure re-export so that:
//!
//!   1. The thousands of existing `crate::error::AlephError` (and
//!      `crate::error::ErrorClass`, `AlephException`, `Result`) call sites
//!      in alephcore continue to compile unchanged.
//!   2. The underlying Rust type is **the same** as `aleph_slot_api::*` —
//!      `TypeId::of()` matches, `is::<aleph_slot_api::AlephError>()` is
//!      `true` for every value produced on either side. Verified by
//!      `tests/modularization_contracts.rs::core_type_identity_matches_api`.
//!
//! Adding any **new** error API to this crate is a layering violation: every
//! constructor, method, and `From` impl belongs in `aleph_slot_api`. The
//! only legitimate content here is the re-exports below.
//!
//! ## Why not a second definition?
//!
//! A second copy of `AlephError` (or even a `pub type AlephError = ...`
//! alias for a different underlying type) would silently double the type
//! universe: `aleph_slot_api::AlephError` and `crate::error::AlephError`
//! would no longer be assignable to each other without an explicit
//! `From`, every `match` would need both names, and `Result<_, AlephError>`
//! in one crate would no longer compose with the same alias in another.
//! `pub use` is the only migration that preserves identity.

pub use aleph_slot_api::error::{AlephError, AlephException, ErrorClass, Result};
