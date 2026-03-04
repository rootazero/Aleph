//! Config module tests
//!
//! This module contains all tests for the config module, organized by category.
//!
//! Note: basic and validation tests have been migrated to BDD cucumber tests.
//! See: core/tests/features/config/basic.feature
//! See: core/tests/features/config/validation.feature

// Test modules
mod serialization;
mod migration;
mod dispatcher;
mod tools;
mod save_incremental;
mod schema_integration;
mod agents_integration;
