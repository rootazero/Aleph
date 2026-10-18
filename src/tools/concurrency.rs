//! Re-export shim — concurrency contract lives in `aleph-slot-api`.
//!
//! As of P0 Task 2, the pure concurrency contract — `ConcurrencyClaim`,
//! `ExclusiveScope`, the four constructors (`paths`, `nodes`,
//! `sessions`, `global`), the four helpers (`claims_conflict`,
//! `scopes_conflict`, `paths_overlap`, `path_pair_overlap`), and
//! the three private helpers (`normalize_path`,
//! `batch_parallelizable`, `partition_parallel_groups`) — moved to
//! `aleph_slot_api::concurrency`. This file re-exports them so every
//! existing `crate::tools::concurrency::*` path keeps compiling.
//!
//! ## Behavior preserved verbatim
//!
//! The migration is a pure re-export — no code changed, only the
//! file moved. `Paths` / `Nodes` claims stay non-conflicting;
//! `Sessions` continues to conflict with every other scope;
//! `normalize_path` keeps treating both `/` and `\\` as separators
//! on every host (the `windows_separators_fold_onto_the_same_scope`
//! test pins this). The 28 inline tests that exercise the rules
//! are now in the API crate.

pub use aleph_slot_api::concurrency::*;
