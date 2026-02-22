//! Virtual Filesystem (VFS) layer for hierarchical memory organization
//!
//! Provides the aleph:// URI scheme for organizing facts into
//! a navigable directory structure.

pub mod hash;
pub mod l1_generator;

pub use hash::compute_directory_hash;
pub use l1_generator::L1Generator;
