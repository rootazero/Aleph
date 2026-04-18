//! Session Service — append-only event log per session with in-process actor.
//!
//! Phase 1 of the managed-agents refactor. Consumers (primarily `agent_loop`)
//! interact with sessions exclusively through the `SessionService` trait;
//! the underlying `InProcessActorSessionService` spawns one tokio task per
//! session and persists events synchronously to SQLite.
//!
//! See `docs/superpowers/specs/2026-04-18-session-service-actor-design.md`.

pub mod events;
pub mod service;
pub mod state;
pub mod store;
pub mod actor;
pub mod in_process;
pub mod shim;

pub use events::{
    ApprovalSource, ErrorKind, EventSeq, MessageContent, SessionEvent,
    SessionEventRecord, Timestamp, ToolOutput, TurnId, TurnOutcome, TurnTrigger,
};
pub use service::{SessionError, SessionHandle, SessionId, SessionService};
pub use state::SessionState;
pub use store::SessionEventStore;
// The following re-exports are filled in by later Phase 1 tasks.
// They are commented out here so the scaffold compiles; uncomment as each
// module type is introduced.
// pub use actor::{ActorCommand, SessionActor};
// pub use in_process::InProcessActorSessionService;
