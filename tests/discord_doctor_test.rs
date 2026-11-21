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
    // The doctor reads the real config file format: TOML, with one
    // `[channels.<id>]` table per channel and the channel's own settings
    // inline.
    let payload = format!(
        r#"[channels.discord]
type = "discord"
bot_token = "{}"
application_id = 999
slash_commands_enabled = true
max_message_length = 2000

[channels.discord.intents]
guild_messages = true
message_content = true
direct_messages = true
"#,
        "x".repeat(60)
    );
    tmp.write_all(payload.as_bytes()).unwrap();

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
    let payload = format!(
        r#"[channels.discord]
type = "discord"
bot_token = "{}"
slash_commands_enabled = true

[channels.discord.intents]
guild_messages = true
message_content = false
"#,
        "x".repeat(60)
    );
    tmp.write_all(payload.as_bytes()).unwrap();

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

/// Missing configuration means no Discord channel is configured.
/// Existing but malformed or unreadable configuration still reports Unknown.
#[tokio::test]
async fn missing_file_is_unconfigured_with_ok_tag() {
    let check = DiscordChannelHealthCheck::new(std::path::PathBuf::from(
        "/nonexistent/path/discord-config.toml",
    ));
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0].has_tag("discord-ok"),
        "unconfigured: {:?}",
        findings[0]
    );
    assert!(
        !findings[0].has_tag("discord-unknown"),
        "not an I/O failure: {:?}",
        findings[0]
    );
}

/// `id()` is part of the `HealthCheck` contract — confirm it matches the
/// string the engine's `--json` output and the doctor tool render verbatim.
///
/// The check has no `title()` method any more (2026-11-20 wire audit): the
/// renderer prints the *finding's* own title, so a second name for the same
/// check could only drift away from it. The finding title is asserted in the
/// module's own unit tests.
#[test]
fn check_metadata_matches_documented_strings() {
    let check = DiscordChannelHealthCheck::from_default_path();
    assert_eq!(check.id(), "core/discord-channel-health");
}

/// A config file that does NOT mention discord at all should produce
/// a single Ok finding — we have nothing to audit, so "fine" is the
/// only honest answer. The operator may simply not have turned the
/// channel on.
#[tokio::test]
async fn a_config_without_discord_is_ok_not_misconfigured() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let payload = r#"[channels.telegram]
type = "telegram"
token = "y"
"#;
    tmp.write_all(payload.as_bytes()).unwrap();

    let check = DiscordChannelHealthCheck::new(tmp.path().to_path_buf());
    let findings = check.run(Posture::Inspect).await;
    assert_eq!(findings.len(), 1);
    assert!(findings[0].has_tag("discord-ok"));
    assert!(!findings[0].has_tag("discord-misconfigured"));
}
