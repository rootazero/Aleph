//! Virtual Filesystem (VFS) layer for hierarchical memory organization
//!
//! Provides the aleph:// URI scheme for organizing facts into
//! a navigable directory structure.

pub mod hash;

pub use hash::compute_directory_hash;
