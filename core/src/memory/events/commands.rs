//! Memory Event Sourcing — Command Structs
//!
//! Command objects dispatched to [`super::handler::MemoryCommandHandler`].
//! Each command represents an intent to mutate a fact; the handler validates
//! the command against current state and produces zero or more [`super::MemoryEvent`]s.
