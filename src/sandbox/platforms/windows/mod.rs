//! Windows sandbox platform implementation.
//!
//! # What this module actually provides (Cycle 1 hardening)
//!
//! - **Job Object** (`job::SandboxJob`): active-process limit, kill-on-close,
//!   die-on-unhandled-exception, optional virtual-memory ceiling, UI
//!   restrictions (no clipboard / desktop / handle access).
//! - **Driver** (`driver::WindowsSandboxDriver`): translates `SandboxPolicy`
//!   into the JobObject configuration and spawns the target via
//!   `tokio::process::Command` inside the job.
//!
//! # Deferred work
//!
//! The following primitives are explicitly NOT implemented in this cycle and
//! are tracked by separate specs (see `docs/superpowers/specs/`):
//!
//! - **Restricted Token** (spec SP-3): requires `CreateProcessAsUserW` with
//!   manual stdio pipe management, which tokio's Command does not expose.
//! - **WFP** network filtering (spec SP-3): admin-only Win32 surgery.
//! - **AppContainer** (spec SP-6): requires a `windows-sys` major-version
//!   upgrade.
//! - **ACL deny-write enforcement**: depends on Restricted Token to mean
//!   anything; deferred with SP-3.
//!
//! The prior `acl.rs`, `appcontainer.rs`, `filter.rs`, `token.rs`, and
//! `wfp.rs` modules contained stub façades pretending to implement these
//! primitives — every method either returned a hardcoded `Err` or a no-op
//! `Ok(())`, with zero production callers. Cycle 1 deleted them to avoid
//! the "looks-like-defense-in-depth but isn't" anti-pattern.

pub mod driver;
mod job;

pub use driver::WindowsSandboxDriver;
