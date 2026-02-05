//! Memory fact operations
//!
//! This module provides operations for managing compressed memory facts:
//! - CRUD operations (insert, invalidate)
//! - Vector similarity search
//! - Hybrid search (vector + FTS5)
//! - Statistics and utilities
//!
//! Note: Tests have been migrated to BDD format in `core/tests/features/memory/facts.feature`

mod crud;
mod hybrid;
mod search;
mod stats;
