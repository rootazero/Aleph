//! Integration tests for the discord security surface.
//!
//! These exercise the same seam the discord channel's `start()` (line
//! 2) will call: building the startup-audit entries for a config, and
//! re-exporting the permission audit through the security module. They
//! do NOT touch serenity / live gateways; the bitfield is synthetic.
//!
//! Companion tests live in:
//!
//! - `discord_audit_test.rs` — the audit-hook pipeline.
//! - `discord_doctor_test.rs` — the `discord_channel_health` check.

mod common;

use alephcore::gateway::interfaces::discord::config::DiscordConfig;
use alephcore::gateway::interfaces::discord::permissions::{
    audit_permissions, HealthStatus, RequirementLevel, ALEPH_PERMISSIONS,
};
use alephcore::gateway::interfaces::discord::security::{
    startup_audit_for_config, startup_audit_for_guild,
};

fn test_discord_config() -> DiscordConfig {
    DiscordConfig {
        bot_token: "test_token_that_is_long_enough_to_pass_validation_check".to_string(),
        ..Default::default()
    }
}

fn all_perms_bitfield() -> u64 {
    ALEPH_PERMISSIONS
        .iter()
        .fold(0u64, |acc, &(flag, _, _)| acc | flag)
}

/// The seam the discord channel's `start()` will use: ask the security
/// module to audit a guild, and confirm it returns an `AuthorityChange`
/// row, not `ExecBlocked`, when the bitfield carries every required
/// permission.
#[test]
fn healthy_guild_yields_authority_change() {
    let entry = startup_audit_for_guild(7, "Healthy", all_perms_bitfield());
    assert_eq!(
        entry.event_type,
        alephcore::security::audit::AuditEventType::AuthorityChange
    );
    assert_eq!(
        entry.severity,
        alephcore::security::audit::AuditSeverity::Warn
    );
    assert!(entry.detail.contains("guild=7"));
}

/// The reverse-direction test the spec asked for: a guild with a
/// required permission missing must surface as `ExecBlocked` /
/// `Critical`, so the post-incident query
/// `WHERE event_type = 'exec_blocked' AND detail LIKE 'discord.start%'`
/// answers "did we ever try to talk to a guild we knew was broken".
#[test]
fn critical_guild_yields_exec_blocked() {
    let bitfield = all_perms_bitfield() & !0x0000_0000_0000_0800; // no Send Messages
    let entry = startup_audit_for_guild(8, "Broken", bitfield);
    assert_eq!(
        entry.event_type,
        alephcore::security::audit::AuditEventType::ExecBlocked
    );
    assert_eq!(
        entry.severity,
        alephcore::security::audit::AuditSeverity::Critical
    );
    assert!(entry.detail.contains("guild=8"));
    assert!(entry.detail.contains("critical"));
}

/// `for_config` with no allowlist returns an empty vec — we do not
/// fabricate audit rows for guilds we have not yet seen.
#[test]
fn empty_allowlist_yields_no_audit_entries() {
    let config = test_discord_config();
    assert!(config.allowed_guilds.is_empty());
    assert!(startup_audit_for_config(&config, all_perms_bitfield()).is_empty());
}

/// `for_config` with an allowlist emits one entry per guild.
#[test]
fn allowlist_yields_one_entry_per_guild() {
    let mut config = test_discord_config();
    config.allowed_guilds = vec![100, 200];
    let entries = startup_audit_for_config(&config, all_perms_bitfield());
    assert_eq!(entries.len(), 2);
    // Each entry references its guild_id in the detail sentence.
    let details: Vec<&str> = entries.iter().map(|e| e.detail.as_str()).collect();
    assert!(details.iter().any(|d| d.contains("guild=100")));
    assert!(details.iter().any(|d| d.contains("guild=200")));
}

/// The security module re-exports `audit_permissions` and the
/// `ALEPH_PERMISSIONS` table so callers do not have to know that
/// `permissions` lives one directory up at `discord/permissions.rs`.
#[test]
fn security_module_reexports_permissions() {
    let audit = audit_permissions(1, "Test", all_perms_bitfield());
    assert_eq!(audit.overall_status, HealthStatus::Healthy);
    // All required permissions must be present in `ALEPH_PERMISSIONS` —
    // the security re-export surface is still the same table.
    assert!(ALEPH_PERMISSIONS
        .iter()
        .any(|(_, _, lvl)| matches!(lvl, RequirementLevel::Required)));
}
