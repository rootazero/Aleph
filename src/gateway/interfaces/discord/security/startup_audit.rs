//! Discord channel startup audit — produce one audit entry per guild
//! describing its initial permission posture, so a misconfigured bot's
//! "will start but will silently fail to read" state leaves a trace.
//!
//! # Why this lives in `discord/security` and not the generic audit module
//!
//! The audit pipeline is generic; what makes an audit entry
//! **discord-shaped** is the `(event_type, severity, detail)` triple we
//! derive from [`super::permissions::PermissionAudit`]. That translation
//! is the only place in the tree that knows both surfaces, so it has to
//! live on the discord side of the seam — and keeping it out of
//! `crate::security::audit` keeps that module small (R3).
//!
//! # How to call it
//!
//! From the discord channel's `start()` (line 2 will wire this up):
//!
//! ```ignore
//! let entries = startup_audit::for_config(&self.config);
//! crate::security::audit::install_global(&audit_log);
//! for entry in entries {
//!     if let Some(log) = crate::security::audit::global() {
//!         log.log(entry).await;
//!     }
//! }
//! ```
//!
//! Each call produces one [`AuditEntry`]. The translation rules are:
//!
//! - `HealthStatus::Healthy`  → `AuthorityChange` (Warn) "started; bot
//!   has every required permission in guild X".
//! - `HealthStatus::Degraded` → `AuthorityChange` (Warn) "started with
//!   degraded permission set in guild X (missing recommended: ...)".
//! - `HealthStatus::Critical` → `ExecBlocked` (Critical) "refused to
//!   start: missing required permission in guild X".
//!
//! The `AuthorityChange` choice for the degraded path mirrors how the
//! rest of the codebase treats "the system is up but the boundary moved":
//! a Warn-severity record with the verb in `detail`, not a
//! refusal-severity one. Critical is reserved for the path where the
//! bot genuinely cannot read or reply — that is the post-incident
//! question worth answering in one `WHERE` clause.
//!
//! [`AuditEntry`]: crate::security::audit::AuditEntry

use crate::gateway::interfaces::discord::config::DiscordConfig;
use crate::gateway::interfaces::discord::permissions::{
    audit_permissions, HealthStatus, PermissionAudit,
};
use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity};

/// Build the startup audit entries for a freshly-loaded discord config.
///
/// One entry per `allowed_guilds` entry. If `allowed_guilds` is empty
/// (the default — "allow any guild") we synthesise **no** entries: the
/// audit trail would have to take a position on every guild the bot
/// might join in the future, which is a fabrication — there is no
/// permission bitfield to audit yet, and the discord channel will only
/// ask for one the first time a guild messages arrives. The first call
/// to the permission gate (line 2's responsibility) is the right place
/// to surface the per-guild verdict.
///
/// `bot_permissions` is the raw bitfield the discord gateway reports
/// for the bot in that guild. In a real start() the channel fetches it
/// once per guild (REST `GET /guilds/{id}/members/{bot_id}`); in tests
/// the caller injects a synthetic bitfield so the verdict can be
/// exercised without a live gateway.
#[must_use]
pub fn for_guild(guild_id: u64, guild_name: &str, bot_permissions: u64) -> AuditEntry {
    let audit = audit_permissions(guild_id, guild_name, bot_permissions);
    build_entry(guild_id, guild_name, &audit)
}

/// Build the entries for every guild listed in the config's allowlist,
/// using a single `bot_permissions` value for all of them.
///
/// The bitfield in practice is per-guild; this helper is for tests and
/// for the "no live gateway" path. Real callers will want
/// [`Self::for_guild`] in a loop.
#[must_use]
pub fn for_config(config: &DiscordConfig, bot_permissions: u64) -> Vec<AuditEntry> {
    if config.allowed_guilds.is_empty() {
        return Vec::new();
    }
    config
        .allowed_guilds
        .iter()
        .map(|&id| {
            let name = format!("guild_{id}");
            for_guild(id, &name, bot_permissions)
        })
        .collect()
}

/// Translate a [`PermissionAudit`] into the discord-specific audit entry.
///
/// Pure so the verdict-to-entry mapping is unit-testable without
/// touching the global audit log.
fn build_entry(guild_id: u64, guild_name: &str, audit: &PermissionAudit) -> AuditEntry {
    let detail = format!(
        "discord.start guild={guild_id} name={guild_name} {} | {}",
        audit.overall_status.label(),
        audit.summary,
    );
    match audit.overall_status {
        // `Healthy` and `Degraded` are both `AuthorityChange` — the bot
        // *did* start, so this is the moved-boundary variant, not the
        // refused-the-call variant. Severity is `Warn` either way: the
        // change is ratified by the operator having typed the token, so
        // it is not a violation.
        HealthStatus::Healthy | HealthStatus::Degraded => AuditEntry {
            event_type: AuditEventType::AuthorityChange,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: None,
            actor_user: Some(format!("discord:start:guild_{guild_id}")),
            detail,
        },
        // `Critical` is the refusal shape — the bot cannot read or reply
        // in this guild because a required permission is missing, and
        // the post-incident question worth answering is "did we ever try
        // to talk to guild X despite knowing it was broken". `Critical`
        // severity mirrors the other "we refused to act" entries in the
        // audit pipeline (see `AuditEntry::command_policy` /
        // `AuditEntry::ssrf_blocked`).
        HealthStatus::Critical => AuditEntry {
            event_type: AuditEventType::ExecBlocked,
            severity: AuditSeverity::Critical,
            source_ip: None,
            session_id: None,
            actor_user: Some(format!("discord:start:guild_{guild_id}")),
            detail,
        },
    }
}

impl HealthStatus {
    /// Lowercase label used in the `detail` sentence. Mirrors the
    /// `serde(rename_all = "lowercase")` on the enum.
    fn label(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Critical => "critical",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::discord::permissions::{
        PermissionCheck, RequirementLevel, TrafficLight, ALEPH_PERMISSIONS,
    };

    fn all_perms_bitfield() -> u64 {
        ALEPH_PERMISSIONS
            .iter()
            .fold(0u64, |acc, &(flag, _, _)| acc | flag)
    }

    fn synthetic_audit(status: HealthStatus, missing: &[&str]) -> PermissionAudit {
        let permissions: Vec<PermissionCheck> = ALEPH_PERMISSIONS
            .iter()
            .map(|&(flag, name, level)| PermissionCheck {
                name: name.to_string(),
                discord_flag: flag,
                has: !missing.contains(&name),
                required: matches!(level, RequirementLevel::Required),
                recommended: matches!(level, RequirementLevel::Recommended),
                status: TrafficLight::Green,
            })
            .collect();
        PermissionAudit {
            guild_id: 1234,
            guild_name: "Test".to_string(),
            permissions,
            overall_status: status,
            summary: format!("synthetic {status:?}"),
            fix_suggestions: Vec::new(),
        }
    }

    #[test]
    fn healthy_maps_to_authority_change_warn() {
        let entry = build_entry(1, "G", &synthetic_audit(HealthStatus::Healthy, &[]));
        assert_eq!(entry.event_type, AuditEventType::AuthorityChange);
        assert_eq!(entry.severity, AuditSeverity::Warn);
        assert!(entry.detail.contains("healthy"));
        assert!(entry.detail.contains("guild=1"));
    }

    #[test]
    fn degraded_maps_to_authority_change_warn() {
        let entry = build_entry(
            2,
            "G2",
            &synthetic_audit(HealthStatus::Degraded, &["Embed Links"]),
        );
        assert_eq!(entry.event_type, AuditEventType::AuthorityChange);
        assert_eq!(entry.severity, AuditSeverity::Warn);
        assert!(entry.detail.contains("degraded"));
    }

    /// The reverse-direction test the spec asked for: a bot that **cannot
    /// operate** in a guild must leave a `Critical` audit row, not the
    /// `Warn` row the healthy path produces. This is the "should be
    /// rejected but audit recorded it" assertion the wire-up will hinge on.
    #[test]
    fn critical_maps_to_exec_blocked_critical() {
        let entry = build_entry(
            3,
            "G3",
            &synthetic_audit(HealthStatus::Critical, &["Send Messages"]),
        );
        assert_eq!(entry.event_type, AuditEventType::ExecBlocked);
        assert_eq!(entry.severity, AuditSeverity::Critical);
        assert!(entry.detail.contains("critical"));
        assert!(entry.detail.contains("guild=3"));
    }

    #[test]
    fn healthy_for_guild_end_to_end() {
        let entry = for_guild(7, "Healthy", all_perms_bitfield());
        assert_eq!(entry.event_type, AuditEventType::AuthorityChange);
        assert_eq!(entry.severity, AuditSeverity::Warn);
        assert!(entry.detail.contains("guild=7"));
        assert!(entry.detail.contains("Healthy"));
    }

    #[test]
    fn critical_for_guild_end_to_end() {
        // Bitfield without "Send Messages" (0x800) — required permission.
        let bitfield = all_perms_bitfield() & !0x0000_0000_0000_0800;
        let entry = for_guild(8, "Broken", bitfield);
        assert_eq!(entry.event_type, AuditEventType::ExecBlocked);
        assert_eq!(entry.severity, AuditSeverity::Critical);
    }

    /// The audit layer requires a string label for `actor_user`, not an
    /// `Option<String>` — verify we always set one. Without this the
    /// entry silently drops attribution downstream.
    #[test]
    fn actor_user_is_always_set() {
        for status in [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Critical,
        ] {
            let entry = build_entry(42, "G", &synthetic_audit(status, &[]));
            assert!(
                entry.actor_user.is_some(),
                "actor_user missing for {status:?}"
            );
        }
    }

    /// `for_config` with no allowlist returns an empty vec — we must
    /// not fabricate audit rows for guilds we have not yet seen.
    #[test]
    fn for_config_with_no_allowlist_returns_empty() {
        let config = DiscordConfig {
            bot_token: "x".repeat(60),
            ..Default::default()
        };
        assert!(config.allowed_guilds.is_empty());
        let entries = for_config(&config, all_perms_bitfield());
        assert!(entries.is_empty());
    }

    #[test]
    fn for_config_with_allowlist_emits_one_per_guild() {
        let config = DiscordConfig {
            bot_token: "x".repeat(60),
            allowed_guilds: vec![100, 200, 300],
            ..Default::default()
        };
        let entries = for_config(&config, all_perms_bitfield());
        assert_eq!(entries.len(), 3);
        for (i, entry) in entries.iter().enumerate() {
            assert!(
                entry
                    .detail
                    .contains(&format!("guild={}", (i + 1) * 100).to_string()),
                "detail: {}",
                entry.detail
            );
        }
    }
}
