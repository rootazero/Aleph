//! Submodules of the session projector.
//!
//! Split out of `crate::gateway::session_projector` to keep that file focused
//! on the live projection path (drain / heal / observe); the per-run span
//! book-keeping and the missed-seq set are independently testable and lived
//! here. Public re-exports live in the parent module.

pub mod missed_seqs;
pub mod run_span;
