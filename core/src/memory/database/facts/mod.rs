//! Memory fact operations
//!
//! This module provides operations for managing compressed memory facts:
//! - CRUD operations (insert, invalidate)
//! - Vector similarity search
//! - Hybrid search (vector + FTS5)
//! - Statistics and utilities

mod crud;
mod hybrid;
mod search;
mod stats;

#[cfg(test)]
mod tests;
