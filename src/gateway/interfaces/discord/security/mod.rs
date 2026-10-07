//! Discord Security Module
//!
//! Security auditing and policy enforcement for the discord channel.
//!
//! # Layout
//!
//! - [`permissions`] — pure permission bitfield audit. No I/O, no
//!   network; safe to call from anywhere that holds a bot-permission
//!   bitfield (tests, the startup audit, the gateway-time permission
//!   check line 2 will add).
//! - [`startup_audit`] — translate a [`PermissionAudit`] into the
//!   shape the global audit pipeline understands
//!   ([`crate::security::audit::AuditEntry`]). One
//!   `AuthorityChange` row per guild at healthy / degraded
//!   [`HealthStatus`]; one `ExecBlocked` row at critical
//!   [`HealthStatus`]. The discord channel's `start()` (line 2's
//!   responsibility) calls [`startup_audit::for_config`] for each
//!   configured guild and feeds the rows through
//!   [`crate::security::audit::SecurityAuditLog::log`].
//!
//! [`HealthStatus`]: permissions::HealthStatus
//! [`PermissionAudit`]: permissions::PermissionAudit

pub mod audit_hooks;
pub mod startup_audit;

// `permissions` lives one directory up at `discord/permissions.rs`; re-export it
// through the security module so callers see a single security surface.
pub use crate::gateway::interfaces::discord::permissions::{audit_permissions, ALEPH_PERMISSIONS};
pub use audit_hooks::{
    label_for_outcome as audit_label_for_outcome,
    record_approval_blocked as record_discord_approval_blocked,
    record_approval_requested as record_discord_approval_requested,
    record_approval_resolved as record_discord_approval_resolved,
};
pub use startup_audit::{
    for_config as startup_audit_for_config, for_guild as startup_audit_for_guild,
};
