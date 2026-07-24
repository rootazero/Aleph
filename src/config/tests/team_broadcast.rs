//! `[team_broadcast]` configuration tests (§4.5 group-chat storm-prevention
//! guards — the broadcast-side parallel to `[team_dispatcher]` / §4.4).

use super::super::*;

#[test]
fn team_broadcast_absent_by_default() {
    // An unconfigured deployment leaves the section `None` ⇒ the boot site uses
    // `BroadcastConfig::default()` (byte-identical prior behaviour).
    let config = Config::default();
    assert!(config.team_broadcast.is_none());
}

#[test]
fn team_broadcast_full_toml_parses() {
    let toml_str = r#"
[team_broadcast]
max_chain_depth = 8
max_fanout_width = 3
max_total_activations = 64
transcript_token_budget = 12000
member_run_timeout_secs = 900
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    let tb = config.team_broadcast.expect("section present");
    assert_eq!(tb.max_chain_depth, Some(8));
    assert_eq!(tb.max_fanout_width, Some(3));
    assert_eq!(tb.max_total_activations, Some(64));
    assert_eq!(tb.transcript_token_budget, Some(12000));
    assert_eq!(tb.member_run_timeout_secs, Some(900));
}

#[test]
fn team_broadcast_partial_toml_leaves_rest_none() {
    // Only one guard overridden — the others stay `None` and fall back to the
    // live default at the boot site (no default duplication in the TOML layer).
    let toml_str = r#"
[team_broadcast]
max_fanout_width = 2
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    let tb = config.team_broadcast.expect("section present");
    assert_eq!(tb.max_fanout_width, Some(2));
    assert_eq!(tb.max_chain_depth, None);
    assert_eq!(tb.max_total_activations, None);
    assert_eq!(tb.transcript_token_budget, None);
    assert_eq!(tb.member_run_timeout_secs, None);
}

#[test]
fn team_broadcast_missing_section_is_none() {
    let toml_str = r#"
[general]
default_provider = "openai"
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    assert!(config.team_broadcast.is_none());
}
