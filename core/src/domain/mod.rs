//! Domain-Driven Design (DDD) building blocks for Aleph.
//!
//! This module provides minimalist marker traits to establish a ubiquitous language
//! and clear architectural boundaries within the codebase.

/// Represents a Domain Entity: an object defined by its identity rather than its attributes.
pub trait Entity {
    /// The unique identifier type for this entity.
    type Id: Eq + Clone + std::fmt::Display;

    /// Returns a reference to the entity's unique identifier.
    fn id(&self) -> &Self::Id;
}

/// Represents an Aggregate Root: the entry point to a cluster of associated objects
/// that are treated as a unit for data changes.
pub trait AggregateRoot: Entity {}

/// Represents a Value Object: an object that describes a characteristic but has no identity.
/// Equality is based on its attributes.
pub trait ValueObject: Eq + Clone {}

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

    impl AggregateRoot for Task {}

    #[test]
    fn test_entity_trait() {
        let task = Task {
            id: TaskId("task-1".to_string()),
        };
        assert_eq!(task.id().0, "task-1");
    }
}
