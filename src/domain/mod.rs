//! Domain-Driven Design (DDD) building blocks for Aleph.
//!
//! This module provides minimalist marker traits to establish a ubiquitous language
//! and clear architectural boundaries within the codebase.
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
//! If/when a future use-case needs an aggregate-root concept (e.g. a
//! repository that operates on `T: AggregateRoot`), reintroduce the trait
//! with at least one real generic-bound consumer in the same patch —
//! otherwise it will silently rot again.

/// Represents a Domain Entity: an object defined by its identity rather than its attributes.
pub trait Entity {
    /// The unique identifier type for this entity.
    type Id: Eq + Clone + std::fmt::Display;

    /// Returns a reference to the entity's unique identifier.
    fn id(&self) -> &Self::Id;
}

pub mod skill;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct TaskId(String);

    impl std::fmt::Display for TaskId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    struct Task {
        id: TaskId,
    }

    impl Entity for Task {
        type Id = TaskId;
        fn id(&self) -> &Self::Id {
            &self.id
        }
    }

    #[test]
    fn test_entity_trait() {
        let task = Task {
            id: TaskId("task-1".to_string()),
        };
        assert_eq!(task.id().0, "task-1");
    }
}
