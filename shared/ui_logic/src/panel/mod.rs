//! Runtime-neutral Panel model shared by every Panel renderer.
//!
//! These modules depend only on `aleph_protocol`: the descriptor registry, the
//! route projection, snapshot discovery and builtin descriptor sources. The
//! Leptos Panel and the TSX Panel read the same code, so the read-side index
//! has a single implementation.

pub mod discovery;
pub mod projection;
pub mod registry;
pub mod source;
