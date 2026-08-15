//! Session Service — append-only event log per session with in-process actor.
//!
//! Phase 1 of the managed-agents refactor. Consumers (primarily `agent_loop`)
//! interact with sessions exclusively through the `SessionService` trait;
//! the underlying `InProcessActorSessionService` spawns one tokio task per
//! session and persists events synchronously to `SQLite`.
//!
//! See `docs/superpowers/specs/2026-04-18-session-service-actor-design.md`.

pub mod actor;
pub mod epoch_registrar;
pub mod events;
pub mod in_process;
pub mod observer;
pub mod projection;
pub mod service;
pub mod steer_signal;
pub mod store;
pub mod tool_trace;

pub use actor::{ActorCommand, SessionActor};
pub use events::{
    ApprovalSource, ErrorKind, EventSeq, MessageContent, SessionEvent, SessionEventRecord,
    Timestamp, ToolOutput, TurnId, TurnTrigger,
};
pub use in_process::InProcessActorSessionService;
pub use projection::project_row;
pub use service::{SessionError, SessionHandle, SessionId, SessionService};
pub use store::SessionEventStore;
pub use tool_trace::with_session_scope;

/// Re-export of the `SESSION_ID` task-local defined in `sandbox::context`.
///
/// Consumers should set this exclusively via [`with_session_scope`] and read
/// it via `crate::sandbox::current_session()`. The direct re-export exists
/// so that integration tests can inspect the task-local without pulling in
/// sandbox internals.
pub use crate::sandbox::context::SESSION_ID;
