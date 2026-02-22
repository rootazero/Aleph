//! Discord Permission Audit
//!
//! Checks Bot permissions in a Guild and reports traffic-light status
//! (Green/Yellow/Red). This is pure logic with no external dependencies
//! -- it operates on a u64 bitfield, not the Discord API.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Traffic-light status for a single permission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrafficLight {
    Green,
    Yellow,
    Red,
}

/// Overall health of the bot's permission set in a guild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Critical,
}

/// How important a permission is to Aleph's operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementLevel {
    Required,
    Recommended,
    Optional,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Result of checking a single Discord permission flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCheck {
    /// Human-readable permission name (e.g. "Send Messages").
    pub name: String,
    /// Raw Discord permission flag value.
    pub discord_flag: u64,
    /// Whether the bot currently has this permission.
    pub has: bool,
    /// Whether this permission is required for core functionality.
    pub required: bool,
    /// Whether this permission is recommended for full functionality.
    pub recommended: bool,
    /// Traffic-light assessment.
    pub status: TrafficLight,
}

/// Full permission audit result for a single guild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAudit {
    /// Discord guild (server) ID.
    pub guild_id: u64,
    /// Discord guild name.
    pub guild_name: String,
    /// Per-permission check results.
    pub permissions: Vec<PermissionCheck>,
    /// Aggregate health status.
    pub overall_status: HealthStatus,
    /// Human-readable summary sentence.
    pub summary: String,
    /// Actionable suggestions for fixing missing permissions.
    pub fix_suggestions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Permission definitions
// ---------------------------------------------------------------------------

/// Discord permission flags that Aleph cares about, together with their
/// human-readable name and requirement level.
///
/// Flag values come from the Discord developer documentation:
/// <https://discord.com/developers/docs/topics/permissions#permissions-bitwise-permission-flags>
pub const ALEPH_PERMISSIONS: &[(u64, &str, RequirementLevel)] = &[
    // Required -- core messaging
    (0x0000_0000_0000_0800, "Send Messages", RequirementLevel::Required),
    (0x0000_0000_0000_0400, "View Channel", RequirementLevel::Required),
    (0x0000_0000_0004_0000, "Read Message History", RequirementLevel::Required),
    // Recommended -- richer interaction
    (0x0000_0000_0000_4000, "Embed Links", RequirementLevel::Recommended),
    (0x0000_0000_0000_8000, "Attach Files", RequirementLevel::Recommended),
    (0x0000_0000_0000_0040, "Add Reactions", RequirementLevel::Recommended),
    // Optional -- admin-level
    (0x0000_0000_0000_2000, "Manage Messages", RequirementLevel::Optional),
    (0x0000_0000_8000_0000, "Use Slash Commands", RequirementLevel::Optional),
];

// ---------------------------------------------------------------------------
// Audit function
// ---------------------------------------------------------------------------

/// Audit the bot's permissions in a guild.
///
/// `bot_permissions` is the raw Discord permission bitfield for the bot in
/// the target guild. The function checks every flag in [`ALEPH_PERMISSIONS`]
/// and returns a full [`PermissionAudit`].
pub fn audit_permissions(
    guild_id: u64,
    guild_name: &str,
    bot_permissions: u64,
) -> PermissionAudit {
    let mut checks = Vec::with_capacity(ALEPH_PERMISSIONS.len());
    let mut missing_required = Vec::new();
    let mut missing_recommended = Vec::new();

    for &(flag, name, ref level) in ALEPH_PERMISSIONS {
        let has = (bot_permissions & flag) == flag;
        let required = matches!(level, RequirementLevel::Required);
        let recommended = matches!(level, RequirementLevel::Recommended);

        let status = if has {
            TrafficLight::Green
        } else {
            match level {
                RequirementLevel::Required => {
                    missing_required.push(name);
                    TrafficLight::Red
                }
                RequirementLevel::Recommended => {
                    missing_recommended.push(name);
                    TrafficLight::Yellow
                }
                RequirementLevel::Optional => {
                    // Optional and missing is still green -- not a problem.
                    TrafficLight::Green
                }
            }
        };

        checks.push(PermissionCheck {
            name: name.to_string(),
            discord_flag: flag,
            has,
            required,
            recommended,
            status,
        });
    }

    // Overall status
    let overall_status = if !missing_required.is_empty() {
        HealthStatus::Critical
    } else if !missing_recommended.is_empty() {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    };

    // Summary
    let summary = match overall_status {
        HealthStatus::Healthy => format!(
            "All permissions OK for guild \"{}\".",
            guild_name,
        ),
        HealthStatus::Degraded => format!(
            "Guild \"{}\" is missing recommended permissions: {}.",
            guild_name,
            missing_recommended.join(", "),
        ),
        HealthStatus::Critical => format!(
            "Guild \"{}\" is missing required permissions: {}.",
            guild_name,
            missing_required.join(", "),
        ),
    };

    // Fix suggestions
    let mut fix_suggestions = Vec::new();
    for name in &missing_required {
        fix_suggestions.push(format!(
            "Grant the \"{}\" permission to the bot role in Server Settings > Roles.",
            name,
        ));
    }
    for name in &missing_recommended {
        fix_suggestions.push(format!(
            "Consider granting \"{}\" for richer interaction.",
            name,
        ));
    }

    PermissionAudit {
        guild_id,
        guild_name: guild_name.to_string(),
        permissions: checks,
        overall_status,
        summary,
        fix_suggestions,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: bitfield with ALL Aleph permissions granted.
    fn all_permissions() -> u64 {
        ALEPH_PERMISSIONS.iter().fold(0u64, |acc, &(flag, _, _)| acc | flag)
    }

    #[test]
    fn test_all_permissions_healthy() {
        let audit = audit_permissions(1234, "Test Guild", all_permissions());

        assert_eq!(audit.overall_status, HealthStatus::Healthy);
        assert_eq!(audit.guild_id, 1234);
        assert_eq!(audit.guild_name, "Test Guild");
        assert!(audit.fix_suggestions.is_empty());

        // Every check should be Green with has == true
        for check in &audit.permissions {
            assert!(check.has, "expected {} to be present", check.name);
            assert_eq!(check.status, TrafficLight::Green, "{} should be green", check.name);
        }

        assert!(audit.summary.contains("All permissions OK"));
    }

    #[test]
    fn test_missing_required_permission_is_critical() {
        // Grant everything except "Send Messages" (0x800)
        let perms = all_permissions() & !0x0000_0000_0000_0800;

        let audit = audit_permissions(5678, "Broken Guild", perms);

        assert_eq!(audit.overall_status, HealthStatus::Critical);

        // Find the Send Messages check
        let send = audit
            .permissions
            .iter()
            .find(|c| c.name == "Send Messages")
            .expect("Send Messages check missing");

        assert!(!send.has);
        assert!(send.required);
        assert_eq!(send.status, TrafficLight::Red);

        // Summary should mention the missing required permission
        assert!(audit.summary.contains("Send Messages"));
        assert!(audit.summary.contains("required"));

        // Should have a fix suggestion for the missing permission
        assert!(
            audit.fix_suggestions.iter().any(|s| s.contains("Send Messages")),
            "fix_suggestions should mention Send Messages",
        );
    }

    #[test]
    fn test_missing_recommended_permission_is_degraded() {
        // Grant only the required permissions, skip recommended and optional
        let required_only: u64 = ALEPH_PERMISSIONS
            .iter()
            .filter(|&&(_, _, ref level)| matches!(level, RequirementLevel::Required))
            .fold(0u64, |acc, &(flag, _, _)| acc | flag);

        let audit = audit_permissions(9999, "Partial Guild", required_only);

        assert_eq!(audit.overall_status, HealthStatus::Degraded);

        // Recommended checks should be Yellow
        let embed = audit
            .permissions
            .iter()
            .find(|c| c.name == "Embed Links")
            .expect("Embed Links check missing");

        assert!(!embed.has);
        assert!(embed.recommended);
        assert_eq!(embed.status, TrafficLight::Yellow);

        // Optional checks should still be Green even though missing
        let manage = audit
            .permissions
            .iter()
            .find(|c| c.name == "Manage Messages")
            .expect("Manage Messages check missing");

        assert!(!manage.has);
        assert_eq!(manage.status, TrafficLight::Green, "optional missing should still be green");

        // Summary should mention recommended
        assert!(audit.summary.contains("recommended"));
    }

    #[test]
    fn test_no_permissions_is_critical() {
        let audit = audit_permissions(0, "Empty Guild", 0);

        assert_eq!(audit.overall_status, HealthStatus::Critical);

        // All required should be Red
        let required_checks: Vec<_> = audit
            .permissions
            .iter()
            .filter(|c| c.required)
            .collect();

        assert!(!required_checks.is_empty());
        for check in &required_checks {
            assert!(!check.has);
            assert_eq!(check.status, TrafficLight::Red);
        }
    }

    #[test]
    fn test_permission_flag_values_match_discord_docs() {
        // Verify the flag constants match Discord's documented values
        let find = |name: &str| -> u64 {
            ALEPH_PERMISSIONS
                .iter()
                .find(|&&(_, n, _)| n == name)
                .map(|&(flag, _, _)| flag)
                .unwrap_or_else(|| panic!("permission {} not found", name))
        };

        assert_eq!(find("Send Messages"), 0x800);
        assert_eq!(find("View Channel"), 0x400);
        assert_eq!(find("Read Message History"), 0x40000);
        assert_eq!(find("Embed Links"), 0x4000);
        assert_eq!(find("Attach Files"), 0x8000);
        assert_eq!(find("Add Reactions"), 0x40);
        assert_eq!(find("Manage Messages"), 0x2000);
        assert_eq!(find("Use Slash Commands"), 0x80000000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let audit = audit_permissions(42, "Serde Guild", all_permissions());
        let json = serde_json::to_string(&audit).expect("serialize");
        let back: PermissionAudit = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.guild_id, 42);
        assert_eq!(back.overall_status, HealthStatus::Healthy);
        assert_eq!(back.permissions.len(), ALEPH_PERMISSIONS.len());

        // Verify serde rename_all = "lowercase" works
        assert!(json.contains("\"healthy\""), "HealthStatus should serialize as lowercase");
        assert!(json.contains("\"green\""), "TrafficLight should serialize as lowercase");
    }
}
