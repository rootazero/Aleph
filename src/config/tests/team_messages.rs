//! `[team_messages]` configuration tests (§4.5 message-router thread-escalation
//! guard — the third teams storm/escalation knob alongside `[team_dispatcher]`
//! / §4.4 and `[team_broadcast]` / §4.5 broadcast).

use super::super::*;

#[test]
fn team_messages_absent_by_default() {
    // An unconfigured deployment leaves the section `None` ⇒ the boot site uses
    // `EscalationRule::default()` (byte-identical prior behaviour).
    let config = Config::default();
    assert!(config.team_messages.is_none());
}

#[test]
fn team_messages_full_toml_parses() {
    let toml_str = r#"
[team_messages]
thread_message_threshold = 10
escalation_enabled = false
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    let tm = config.team_messages.expect("section present");
    assert_eq!(tm.thread_message_threshold, Some(10));
    assert_eq!(tm.escalation_enabled, Some(false));
}

#[test]
fn team_messages_partial_toml_leaves_rest_none() {
    // Only the threshold overridden — the switch stays `None` and falls back to
    // the live default at the boot site (no default duplication in the TOML).
    let toml_str = r#"
[team_messages]
thread_message_threshold = 3
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    let tm = config.team_messages.expect("section present");
    assert_eq!(tm.thread_message_threshold, Some(3));
    assert_eq!(tm.escalation_enabled, None);
}

#[test]
fn team_messages_missing_section_is_none() {
    let toml_str = r#"
[general]
default_provider = "openai"
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    assert!(config.team_messages.is_none());
}
