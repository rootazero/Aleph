//! `browser_companion`: Aleph-managed, sandboxed, long-lived dev/design
//! companion processes whose page is published to the browser-live viewer.
//!
//! * `protocol` - launch secret, ready report, endpoint challenge-response.
//! * `record` - the internal managed-launch attestation (never deserialized).
//! * `process` - the real launcher: sandbox admission, supervisor task that
//!   owns the `Child` until `wait`, registered in the existing process registry.
//! * `manager` - lifecycle state machine, rollback and close-before-kill order.
//! * `tool` - the model-facing `browser_companion` start/status/stop tool.

pub(crate) mod manager;
pub(crate) mod process;
pub(crate) mod protocol;
pub(crate) mod record;
pub mod tool;

pub use manager::shutdown_all_companions;
pub(crate) use record::{ManagedLaunchRecord, LAUNCHER};
pub use tool::{BrowserCompanionArgs, BrowserCompanionOutput, BrowserCompanionTool};
