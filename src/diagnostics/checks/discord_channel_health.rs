//! `core/discord-channel-health` — does the loaded Discord config actually
//! make sense for a bot that wants to read & reply?
//!
//! The discord channel's `start()` already refuses a config that fails
//! `DiscordConfig::validate()` (empty / too-short bot token) — but the
//! validation surface there covers only the **credential half**. Misaligned
//! intents (`guild_messages` on, `message_content` off → the bot sees message
//! events whose `content` is the empty string), a `slash_commands_enabled`
//! flag with no `application_id` (the gateway silently registers nothing),
//! or a deliberately-tweaked `max_message_length` (Discord's hard limit is
//! 2000; the only value the channel ever honours), are all surfaces where
//! the bot **starts cleanly** and then quietly fails to read or reply.
//!
//! # Why three states, not two
//!
//! The same `media_codecs.rs` reasoning applies: "the config file was
//! unloadable" is not the same answer as "the config is fine". Severity
//! alone cannot carry the distinction — `Info` and the unknown sentinel
//! would render byte-identically to a genuine pass, and every machine
//! consumer (`--json`, CI, the LLM `doctor` tool) reads severity, not the
//! English prose in the title. So each verdict carries a `TAG_DISCORD_*`
//! tag.
//!
//! # Why this check is path-only and offline
//!
//! `default_registry()` is the cold registry; `aleph-server doctor` is by
//! definition a cold process. We have a parsed `DiscordConfig` and the
//! capability table; we never open the gateway. A live gateway probe would
//! have to live on `with_runtime_checks`, where the bot is up — out of
//! scope here.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const CHECK_ID: &str = "core/discord-channel-health";

/// Tag on the `Ok` verdict's finding — every probed invariant is satisfied.
const TAG_DISCORD_OK: &str = "discord-ok";
/// Tag on the `Misconfigured` verdict's finding — at least one invariant
/// failed; the channel will start but degrade.
const TAG_DISCORD_MISCONFIGURED: &str = "discord-misconfigured";
/// Tag on the `Unknown` verdict's finding — the config file could not be
/// loaded, so we have nothing to audit. The check must not lie about
/// either health or breakage in that case.
const TAG_DISCORD_UNKNOWN: &str = "discord-unknown";

/// Discord's hard message-length ceiling, bytes. The channel's own
/// `max_message_length` constant agrees with this; any override above this
/// value is meaningless because Discord will reject the POST.
const DISCORD_MAX_MESSAGE_LENGTH: u64 = 2000;
/// The well-known Discord bot token length floor. Shorter strings fail
/// `DiscordConfig::validate` at `start()`, but a token string that
/// *parses* but is shorter than 50 chars (the validate threshold) is
/// still in the wrong shape — so we treat anything <50 chars as a
/// misconfiguration regardless of how `validate` itself reasons.
const DISCORD_MIN_BOT_TOKEN_LEN: usize = 50;

/// What the probe learned.
///
/// Each variant carries the data a `findings_for` call needs to render a
/// human-readable sentence; the verdict itself stays in the enum so the
/// tests can pin the verdict-to-finding mapping (see the sibling pattern
/// in `media_codecs::CodecVerdict`).
#[derive(Debug, PartialEq, Eq)]
enum DiscordVerdict {
    /// Every invariant is satisfied.
    Ok,
    /// One or more invariants failed. Each item is a short, machine-readable
    /// tag a follow-up tool can join on (`bot_token_short`,
    /// `intents_misaligned`, ...).
    Misconfigured(Vec<&'static str>),
    /// The config file could not be read or parsed. `reason` is the
    /// surfaced error string — sanitised by the engine's
    /// `Finding::redacted` pass before reaching the operator.
    Unknown(String),
}

/// Lightweight deserialisation shape. We don't want this check to import
/// the full `DiscordConfig` (which would pull in `serde` defaults the
/// check does not care about); the fields we read are enough to detect
/// every invariant we test for.
///
/// All fields are `Option` / default-fillable so a config that lacks a
/// field we don't probe still parses cleanly — the *only* failure mode
/// we recognise is "the file is not on disk" or "the bytes are not JSON".
#[derive(Debug, Deserialize, Default)]
struct DiscordConfigShape {
    #[serde(default)]
    bot_token: String,
    #[serde(default)]
    application_id: Option<u64>,
    #[serde(default)]
    slash_commands_enabled: Option<bool>,
    #[serde(default)]
    intents: IntentsShape,
    #[serde(default)]
    max_message_length: Option<u64>,
}

/// Intents subset of `DiscordConfig`. Defaults mirror
/// `IntentsConfig::default()` so a misconfigured-but-loadable file does
/// not silently read as "no intents" here.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct IntentsShape {
    guild_messages: bool,
    direct_messages: bool,
    message_content: bool,
    guild_members: bool,
    guild_threads: bool,
}

impl Default for IntentsShape {
    fn default() -> Self {
        // Mirrors `crate::gateway::interfaces::discord::config::IntentsConfig::default`.
        Self {
            guild_messages: true,
            direct_messages: true,
            message_content: true,
            guild_members: false,
            guild_threads: false,
        }
    }
}

impl DiscordConfigShape {
    fn invariant_violations(&self) -> Vec<&'static str> {
        let mut problems = Vec::new();

        // bot_token: DiscordConfig::validate rejects <50 chars at start();
        // we mirror that threshold so a misconfigured config that has not
        // yet been fed to start() still surfaces here.
        if self.bot_token.is_empty() {
            problems.push("bot_token_missing");
        } else if self.bot_token.len() < DISCORD_MIN_BOT_TOKEN_LEN {
            problems.push("bot_token_short");
        }

        // intents: `message_content` is the privileged intent that lets the
        // bot actually read `content` on incoming messages. With it off,
        // `guild_messages` fires for events whose `content` is the empty
        // string — the bot literally cannot see what users wrote. Discord
        // documents this combination as a footgun; we surface it so an
        // operator who flipped the flag by accident sees the line.
        if self.intents.guild_messages && !self.intents.message_content {
            problems.push("intents_misaligned");
        }

        // slash_commands_enabled without an application_id: serenity's
        // global-command registration requires both, and the gateway
        // silently registers nothing if only one is present. The bot still
        // starts; the slash UI never appears.
        if self.slash_commands_enabled.unwrap_or(true) && self.application_id.is_none() {
            problems.push("application_id_missing");
        }

        // max_message_length override above Discord's hard ceiling. The
        // `discord` channel hard-codes 2000 in its `capabilities()`
        // builder; a config that promises something larger is meaningless
        // and indicates a stale or hand-edited file.
        if let Some(limit) = self.max_message_length {
            if limit > DISCORD_MAX_MESSAGE_LENGTH {
                problems.push("max_message_length_too_high");
            }
        }

        problems
    }
}

/// `findings_for` — pure mapping from the verdict to the rendered finding.
/// Pure so the mapping is unit-testable without standing up a probe.
fn findings_for(verdict: &DiscordVerdict) -> Vec<Finding> {
    match verdict {
        DiscordVerdict::Ok => vec![Finding::ok(
            CHECK_ID,
            "Discord channel config looks healthy",
            "bot_token, intents, slash-command wiring, and message-length \
             ceiling all line up with what the discord channel expects.",
        )
        .with_tag(TAG_DISCORD_OK)],
        DiscordVerdict::Misconfigured(tags) => {
            // Build a human-readable sentence by mapping each tag back to
            // a short explanation. The tag stays on the finding so the
            // --json surface and the LLM doctor tool can join on it.
            let details: Vec<&str> = tags
                .iter()
                .map(|t| match *t {
                    "bot_token_missing" => "bot_token is empty",
                    "bot_token_short" => "bot_token is shorter than 50 chars",
                    "intents_misaligned" => {
                        "guild_messages is on but message_content is off — \
                         incoming message content will read as the empty string"
                    }
                    "application_id_missing" => {
                        "slash_commands_enabled is true but no application_id \
                         is set — slash commands will silently not register"
                    }
                    "max_message_length_too_high" => {
                        "max_message_length exceeds Discord's 2000-char ceiling — \
                         replies longer than 2000 chars will be rejected by Discord"
                    }
                    _ => "unknown invariant violation",
                })
                .collect();
            vec![Finding::problem(
                CHECK_ID,
                Severity::Warning,
                "Discord channel config is misconfigured",
                format!("{} invariant(s) failed: {}", tags.len(), details.join("; "),),
            )
            .with_fix_hint(
                "Edit the discord section of the config: provide a real bot \
                 token, pair guild_messages with message_content, set \
                 application_id when slash_commands_enabled is true, and \
                 leave max_message_length at the Discord ceiling (2000).",
            )
            .with_tag(TAG_DISCORD_MISCONFIGURED)]
        }
        DiscordVerdict::Unknown(reason) => vec![Finding::problem(
            CHECK_ID,
            Severity::Info,
            "Discord channel config status unknown",
            format!(
                "Could not load the discord config to audit it: {reason}. \
                 The bot may still start if the config is correct on disk."
            ),
        )
        .with_fix_hint(
            "Verify the config file parses (`aleph config dump`) and that \
             the discord section is well-formed.",
        )
        .with_tag(TAG_DISCORD_UNKNOWN)],
    }
}

/// Load and parse the discord config subset from `path`.
///
/// Returns `Unknown(reason)` on any I/O or parse failure — the same shape
/// `media_codecs` uses for `gst-inspect-1.0` being absent. The path is
/// always the same on every supported platform (`config.toml` next to
/// the data dir), so we resolve it eagerly and surface the resolution
/// failure as `Unknown`.
async fn probe(path: &std::path::Path) -> DiscordVerdict {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => return DiscordVerdict::Unknown(format!("read failed: {e}")),
    };
    // Extract just the discord section. A whole-file parse would let a
    // bad [channels.config] in another channel poison the verdict; we
    // only care about the discord table.
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return DiscordVerdict::Unknown(format!("not valid JSON/TOML: {e}")),
    };
    let discord_section = value
        .get("channels")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|entry| entry.get("channel_type").and_then(|t| t.as_str()) == Some("discord"))
        })
        .and_then(|entry| entry.get("config"));
    let Some(section) = discord_section else {
        // No discord channel is configured at all — that's an *OK* for the
        // discord check (we have nothing to audit), not a misconfiguration.
        // The operator may simply not have turned discord on.
        return DiscordVerdict::Ok;
    };
    let shape: DiscordConfigShape = match serde_json::from_value(section.clone()) {
        Ok(s) => s,
        Err(e) => return DiscordVerdict::Unknown(format!("discord section malformed: {e}")),
    };
    let problems = shape.invariant_violations();
    if problems.is_empty() {
        DiscordVerdict::Ok
    } else {
        DiscordVerdict::Misconfigured(problems)
    }
}

/// `core/discord-channel-health` check instance.
pub struct DiscordChannelHealthCheck {
    /// Resolved path of the config file the probe reads.
    config_path: PathBuf,
}

impl DiscordChannelHealthCheck {
    /// Construct against an explicit config path. Tests use a temp file;
    /// production calls `from_default_path()` to mirror the
    /// `VaultCheck::from_default_path()` shape.
    #[must_use]
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    /// Resolve the production config path the same way
    /// `DiagnosticEngine::default_registry()` does (`Config::effective_path`).
    #[must_use]
    pub fn from_default_path() -> Self {
        Self::new(crate::config::Config::effective_path())
    }
}

impl Default for DiscordChannelHealthCheck {
    fn default() -> Self {
        Self::from_default_path()
    }
}

#[async_trait]
impl HealthCheck for DiscordChannelHealthCheck {
    fn id(&self) -> &'static str {
        CHECK_ID
    }

    fn title(&self) -> &'static str {
        "Discord channel config"
    }

    fn timeout(&self) -> Duration {
        // Probe is one file read + JSON parse. 5s covers any sane
        // filesystem; the engine's 20s default would be wasted slack.
        Duration::from_secs(5)
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        findings_for(&probe(&self.config_path).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Pin the verdict-to-tag mapping: a misconfigured report must carry
    /// the `discord-misconfigured` tag, not `discord-ok`. The whole point
    /// of the three-state machine is that machine consumers can read it.
    #[test]
    fn ok_finding_carries_ok_tag() {
        let f = findings_for(&DiscordVerdict::Ok);
        assert_eq!(f.len(), 1);
        assert!(f[0].has_tag(TAG_DISCORD_OK));
        assert!(!f[0].has_tag(TAG_DISCORD_MISCONFIGURED));
        assert!(!f[0].has_tag(TAG_DISCORD_UNKNOWN));
    }

    #[test]
    fn misconfigured_lists_each_problem() {
        let verdict =
            DiscordVerdict::Misconfigured(vec!["intents_misaligned", "application_id_missing"]);
        let f = findings_for(&verdict);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].has_tag(TAG_DISCORD_MISCONFIGURED));
        assert!(
            f[0].detail
                .contains("guild_messages is on but message_content is off"),
            "detail: {}",
            f[0].detail
        );
        assert!(
            f[0].detail.contains("application_id"),
            "detail: {}",
            f[0].detail
        );
    }

    #[test]
    fn unknown_is_info_with_unknown_tag() {
        let verdict = DiscordVerdict::Unknown("parse error".to_string());
        let f = findings_for(&verdict);
        assert_eq!(f.len(), 1);
        // Severity::Info, same as `Ok` — the distinction is the tag.
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].has_tag(TAG_DISCORD_UNKNOWN));
        assert!(f[0].detail.contains("parse error"));
    }

    /// `intent_violations` is the heart of the check: a healthy config
    /// (full token + aligned intents + application_id when slash on)
    /// reports no violations.
    #[test]
    fn healthy_config_has_no_violations() {
        let shape = DiscordConfigShape {
            bot_token: "x".repeat(DISCORD_MIN_BOT_TOKEN_LEN + 1),
            application_id: Some(123),
            slash_commands_enabled: Some(true),
            intents: IntentsShape::default(),
            max_message_length: Some(DISCORD_MAX_MESSAGE_LENGTH),
        };
        assert!(shape.invariant_violations().is_empty());
    }

    #[test]
    fn empty_bot_token_violates() {
        let shape = DiscordConfigShape {
            bot_token: String::new(),
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(v.contains(&"bot_token_missing"));
    }

    #[test]
    fn short_bot_token_violates() {
        let shape = DiscordConfigShape {
            bot_token: "short".to_string(),
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(v.contains(&"bot_token_short"));
    }

    #[test]
    fn misaligned_intents_violate() {
        let shape = DiscordConfigShape {
            intents: IntentsShape {
                guild_messages: true,
                message_content: false,
                ..IntentsShape::default()
            },
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(v.contains(&"intents_misaligned"));
    }

    #[test]
    fn slash_without_application_id_violates() {
        let shape = DiscordConfigShape {
            slash_commands_enabled: Some(true),
            application_id: None,
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(v.contains(&"application_id_missing"));
    }

    #[test]
    fn slash_off_without_application_id_is_fine() {
        // slash_commands_enabled=false means application_id is irrelevant.
        let shape = DiscordConfigShape {
            slash_commands_enabled: Some(false),
            application_id: None,
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(!v.contains(&"application_id_missing"));
    }

    #[test]
    fn oversized_message_length_violates() {
        let shape = DiscordConfigShape {
            max_message_length: Some(DISCORD_MAX_MESSAGE_LENGTH + 1),
            ..Default::default()
        };
        let v = shape.invariant_violations();
        assert!(v.contains(&"max_message_length_too_high"));
    }

    /// End-to-end: write a healthy discord config to a temp file and
    /// confirm the probe reports `Ok`.
    #[tokio::test]
    async fn probe_a_healthy_config_reports_ok() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let payload = serde_json::json!({
            "channels": [{
                "channel_type": "discord",
                "config": {
                    "bot_token": "x".repeat(DISCORD_MIN_BOT_TOKEN_LEN + 1),
                    "application_id": 999,
                    "slash_commands_enabled": true,
                    "intents": {
                        "guild_messages": true,
                        "message_content": true,
                        "direct_messages": true,
                    },
                    "max_message_length": DISCORD_MAX_MESSAGE_LENGTH,
                }
            }]
        });
        tmp.write_all(serde_json::to_vec(&payload).unwrap().as_slice())
            .unwrap();
        let verdict = probe(tmp.path()).await;
        assert_eq!(verdict, DiscordVerdict::Ok);
    }

    /// End-to-end: a config whose discord section is missing should read
    /// as `Ok` (we have nothing to audit), not `Misconfigured`.
    #[tokio::test]
    async fn probe_a_config_without_discord_reports_ok() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let payload = serde_json::json!({
            "channels": [{
                "channel_type": "telegram",
                "config": {"token": "y"}
            }]
        });
        tmp.write_all(serde_json::to_vec(&payload).unwrap().as_slice())
            .unwrap();
        let verdict = probe(tmp.path()).await;
        assert_eq!(verdict, DiscordVerdict::Ok);
    }

    /// End-to-end: a config whose discord section violates two invariants
    /// at once reports both in a single `Misconfigured` verdict.
    #[tokio::test]
    async fn probe_a_misaligned_config_reports_each_violation() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let payload = serde_json::json!({
            "channels": [{
                "channel_type": "discord",
                "config": {
                    "bot_token": "x".repeat(DISCORD_MIN_BOT_TOKEN_LEN + 1),
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
        let verdict = probe(tmp.path()).await;
        match verdict {
            DiscordVerdict::Misconfigured(tags) => {
                assert!(tags.contains(&"intents_misaligned"));
                assert!(tags.contains(&"application_id_missing"));
            }
            other => panic!("expected Misconfigured, got {other:?}"),
        }
    }

    /// A file that doesn't exist surfaces as `Unknown`, not as `Ok`.
    /// Same rule as `media_codecs`: "I couldn't tell" is not the same
    /// answer as "it's fine".
    #[tokio::test]
    async fn probe_a_missing_file_reports_unknown() {
        let verdict = probe(std::path::Path::new("/nonexistent/path/config.toml")).await;
        match verdict {
            DiscordVerdict::Unknown(reason) => assert!(reason.contains("read failed")),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// A malformed file surfaces as `Unknown` with a parse error, not as
    /// `Misconfigured`.
    #[tokio::test]
    async fn probe_a_malformed_file_reports_unknown() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"this is not json {{{").unwrap();
        let verdict = probe(tmp.path()).await;
        match verdict {
            DiscordVerdict::Unknown(reason) => assert!(reason.contains("JSON/TOML")),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }
}
