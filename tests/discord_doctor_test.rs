//! Integration tests for the `discord_channel_health` doctor check.
//!
//! The unit tests live next to the module
//! (`src/diagnostics/checks/discord_channel_health.rs`); these exercise
//! the same check from a test-crate angle so the public surface —
//! `DiscordChannelHealthCheck::new` / `from_default_path`, the
//! `HealthCheck` trait impl, the verdict-to-finding mapping — stays
//! honest across the crate boundary.
//!
//! Companion tests live in:
//!
//! - `discord_security_test.rs` — the permission/startup audit surface.
//! - `discord_audit_test.rs` — the audit-hook pipeline.

mod common;

use std::io::Write;

use alephcore::diagnostics::check::{HealthCheck, Posture};
use alephcore::diagnostics::checks::DiscordChannelHealthCheck;

/// Tag constants must match the values used by the unit tests; the
/// `discord-ok` tag is the only way the `--json` surface can tell a
/// real "discord section was found and looked fine" from a
/// "no discord section was configured" report — both are
/// `Severity::Info`. Any rename here is a breaking change for the
/// doctor's downstream consumers.
#[tokio::test]
async fn healthy_config_yields_discord_ok_tag() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let payload = serde_json::json!({
        "channels": [{
            "channel_type": "discord",
            "config": {
                "bot_token": "x".repeat(60),
                "application_id": 999,
                "slash_commands_enabled": true,
                "intents": {
                    "guild_messages": true,
                    "message_content": true,
                    "direct_messages": true,
                },
                "max_message_length": 2000,
            }
        }]
    });
    tmp.write_all(serde_json::to_vec(&payload).unwrap().as_slice())
        .unwrap();

    let check = DiscordChannelHealthCheck::new(tmp.path().to_path_buf());
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    // The healthy verdict renders as a single Info finding whose title
    // starts with "Discord channel config" and whose detail notes the
    // invariants that line up — there is no "healthy" substring in the
    // detail sentence itself (the verdict is read off the severity +
    // tag, not the prose). Asserting on the tag is the load-bearing
    // check.
    assert!(
        findings[0].has_tag("discord-ok"),
        "missing discord-ok tag: {:?}",
        findings[0]
    );
    assert!(!findings[0].is_problem());
}

/// A config that has both an `intents_misaligned` violation and an
/// `application_id_missing` violation must surface BOTH in the
/// single `Misconfigured` finding, not just the first one.
#[tokio::test]
async fn misaligned_intents_and_missing_application_id_both_surfaced() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let payload = serde_json::json!({
        "channels": [{
            "channel_type": "discord",
            "config": {
                "bot_token": "x".repeat(60),
                "slash_commands_enabled": true,
                "application_id": null,
                "intents": {
                    "guild_messages": true,
                    "message_content": false,
                }
            }
        }]
    });
    tmp.write_all(serde_json::to_vec(&payload).unwrap().as_slice())
        .unwrap();

    let check = DiscordChannelHealthCheck::new(tmp.path().to_path_buf());
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0]
            .detail
            .contains("guild_messages is on but message_content is off"),
        "{:?}",
        findings[0]
    );
    assert!(
        findings[0].detail.contains("application_id"),
        "{:?}",
        findings[0]
    );
}

/// A config file that doesn't exist surfaces as a single
/// `Unknown`-tagged finding — never as `Ok`. This is the third state
/// the check maintains; severity alone (`Info`) cannot distinguish
/// "config is fine" from "config could not be read".
#[tokio::test]
async fn missing_file_yields_unknown_tag() {
    let check = DiscordChannelHealthCheck::new(std::path::PathBuf::from(
        "/nonexistent/path/discord-config.toml",
    ));
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    assert!(findings[0].detail.contains("Could not load"));
    // The tag must be `discord-unknown`, NOT `discord-ok`, otherwise a
    // future CI consumer would misread "I don't know" as "it's fine".
    assert!(
        findings[0].has_tag("discord-unknown"),
        "tag absent: {:?}",
        findings[0]
    );
    assert!(
        !findings[0].has_tag("discord-ok"),
        "tag false-positive: {:?}",
        findings[0]
    );
}

/// `id()` and `title()` are part of the `HealthCheck` contract —
/// confirm they match the strings the engine's `--json` output and
/// the doctor tool render verbatim.
#[test]
fn check_metadata_matches_documented_strings() {
    let check = DiscordChannelHealthCheck::default();
    assert_eq!(check.id(), "core/discord-channel-health");
    assert_eq!(check.title(), "Discord channel config");
}

/// A config file that does NOT mention discord at all should produce
/// a single Ok finding — we have nothing to audit, so "fine" is the
/// only honest answer. The operator may simply not have turned the
/// channel on.
#[tokio::test]
async fn a_config_without_discord_is_ok_not_misconfigured() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let payload = serde_json::json!({
        "channels": [{
            "channel_type": "telegram",
            "config": {"token": "y"}
        }]
    });
    tmp.write_all(serde_json::to_vec(&payload).unwrap().as_slice())
        .unwrap();

    let check = DiscordChannelHealthCheck::new(tmp.path().to_path_buf());
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    assert!(findings[0].has_tag("discord-ok"));
    assert!(!findings[0].has_tag("discord-misconfigured"));
}
