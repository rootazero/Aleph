//! Collaborative session management for team agents.
//!
//! Provides types and a SQLite-backed store for managing multi-agent
//! collaborative sessions with turn tracking and outcome recording.

pub mod coordinator;
pub mod store;
pub mod types;

pub use coordinator::SessionCoordinator;
pub use store::{SessionStore, SqliteSessionStore};
pub use types::{
    CollaborativeSession, NewSession, SessionOutcome, SessionStatus, SessionTrigger, SessionTurn,
};
