//! Domain-Driven Design (DDD) building blocks for Aleph.
//!
//! This module provides minimalist marker traits to establish a ubiquitous language
//! and clear architectural boundaries within the codebase.
//!
//! ## Rule: a marker trait needs a generic-bound consumer
//!
//! A marker trait earns its place only when some generic bound actually
//! uses it (`T: Entity`). An `impl` for a concrete type proves nothing —
//! those impls exist only to satisfy the declaration, and the trait then
//! stays reachable only through a call the concrete type already provides.
//!
//! ## Note on `AggregateRoot`
//!
//! A previous `AggregateRoot: Entity` marker trait lived here and was
//! implemented by `SkillManifest`, `A2ATask`, and `MemoryFact`. The trait had
//! zero generic-bound usage in the codebase — the impls existed only to
//! satisfy the declaration. It was removed under the severed-wire-audit
//! `CUT` branch (form 1: defined-but-never-constrained) on 2026-08-16. The
//! three impls were deleted alongside it.
//!
//! The `Entity` marker trait it extended was removed on 2026-11-21 for the
//! same rot: three impls (`SkillManifest`, `A2ATask`, `MemoryFact`), zero
//! generic-bound consumers, and one UFCS call
//! (`crate::domain::Entity::id(&manifest)`) that the inherent
//! `SkillManifest::id()` already served.
//!
//! If/when a future use-case needs an entity concept (e.g. a repository
//! that operates on `T: Entity`), reintroduce the trait with at least one
//! real generic-bound consumer in the same patch — otherwise it will
//! silently rot again.

pub mod skill;
