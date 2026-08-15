//! Stream-family decode census: the two halves of one cross-crate wire
//! contract, related mechanically.
//!
//! A frame with a `stream_method()` is published as a JSON-RPC notification
//! whose `params` are the serde-tagged frame body. Every terminal client (TUI,
//! CLI, anything on `shared/client`) decodes those params into the *typed*
//! `aleph_protocol::StreamEvent` and drops whatever fails to parse with a
//! `debug!` line and nothing else (`shared/client/src/connection.rs`). So a
//! frame that grows a `stream_method()` without a matching `StreamEvent`
//! variant is not a missing feature on those clients — it is a silent drop,
//! for every one of them, with no error on either side. That is exactly how
//! `stream.clarification_ended` reached the Panel for months while the TUI kept
//! rendering a card for a question that had already ended.
//!
//! `#[serde(tag = "type")]` is what the client actually branches on, while the
//! Panel branches on the `stream.<name>` method prefix, so the two must name
//! the same thing — a frame whose method and tag disagree is handled by one
//! client family and invisible to the other, which is the other half asserted
//! here.
//!
//! Both directions are asserted. A `StreamEvent` variant no frame produces is
//! the mirror defect: a client parsing for something the wire never carries.
//!
//! The protocol crate is read, never written, via `include_str!` — this crate
//! cannot see the enum's variant names at compile time, only its source text.

use crate::gateway::source_census::production_prefix;

/// `stream.*` methods with no `StreamEvent` twin, on purpose.
///
/// All three carry session-list state that only the Panel has a surface for —
/// sidebar rows (`session_updated`), the running-run dot
/// (`running_set_changed`), and another room member's message echo
/// (`session_user_message`). A terminal client attached to one conversation has
/// nowhere to put any of them.
///
/// This list may only shrink without a deliberate edit: the census asserts it
/// names exactly the un-twinned methods, so an entry that acquires a twin goes
/// red as stale, and a NEW un-twinned frame cannot join it silently — someone
/// has to type the name here and say why.
const PANEL_ONLY_STREAM_METHODS: &[&str] = &[
    "running_set_changed",
    "session_updated",
    "session_user_message",
];

/// `(variant, method suffix)` for every `Some("stream.…")` arm of
/// `GatewayEventFrame::stream_method`.
///
/// Pairing them at the arm — rather than scraping the enum body and the match
/// table separately — is what lets the tag/method agreement below be checked at
/// all; the variant taken is the nearest `Self::` preceding the literal, which
/// is that arm's own pattern.
fn stream_method_arms(production: &str) -> Vec<(String, String)> {
    const NEEDLE: &str = "Some(\"stream.";
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = production[cursor..].find(NEEDLE) {
        let at = cursor + rel;
        cursor = at + NEEDLE.len();
        let head = &production[..at];
        let variant = head.rfind("Self::").map_or_else(String::new, |i| {
            head[i + "Self::".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        });
        let rest = &production[cursor..];
        let Some(close) = rest.find('"') else {
            continue;
        };
        out.push((variant, rest[..close].to_string()));
    }
    out
}

/// Variant names of the enum whose declaration line is `header`.
///
/// Walks brace depth from the enum body so nested struct-variant fields are
/// skipped; comment lines are already gone (the caller passes
/// [`production_prefix`] output) so a doc comment cannot contribute a name.
fn enum_variant_names(src: &str, header: &str) -> Vec<String> {
    let Some(start) = src.find(header) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut depth = 0i32;
    for line in src[start + header.len()..].lines() {
        if depth == 0 {
            if let Some(name) = leading_variant_name(line.trim()) {
                names.push(name);
            }
        }
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            break; // the enum's own closing brace
        }
    }
    names
}

fn leading_variant_name(line: &str) -> Option<String> {
    if !line.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    let name: String = line
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let rest = line[name.len()..].trim_start();
    (rest.starts_with('{') || rest.starts_with(',') || rest.starts_with('(')).then_some(name)
}

/// serde's `RenameRule::SnakeCase`, which is what `rename_all = "snake_case"`
/// applies to both enums. `frame::tests::token_rotated_serializes_with_snake_case_tag`
/// pins that serde really does this, so the rule cannot be re-derived here
/// incorrectly without that test disagreeing.
fn serde_snake_case(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len() + 4);
    for (i, ch) in variant.char_indices() {
        if i > 0 && ch.is_uppercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn frame_source() -> String {
    production_prefix(include_str!("frame.rs"))
}

fn protocol_stream_variants() -> Vec<String> {
    let src = production_prefix(include_str!("../../../shared/protocol/src/events.rs"));
    enum_variant_names(&src, "pub enum StreamEvent {")
}

fn scraped_arms() -> Vec<(String, String)> {
    let arms = stream_method_arms(&frame_source());
    assert!(
        arms.len() >= 15,
        "only {} `Some(\"stream.…\")` arm(s) scraped from frame.rs — \
         `stream_method` publishes far more than that, so the scanner stopped \
         matching the call shape and this census has become vacuous",
        arms.len()
    );
    arms
}

/// The clients branch on the serde tag; the Panel branches on the method
/// prefix. A variant whose method spells its tag differently is handled by one
/// and invisible to the other, and neither side reports anything.
#[test]
fn every_stream_method_names_its_variants_serde_tag() {
    for (variant, method) in scraped_arms() {
        assert!(
            !variant.is_empty(),
            "a `Some(\"stream.{method}\")` arm has no `Self::` pattern before \
             it — the match table changed shape and the pairing is unreliable"
        );
        assert_eq!(
            serde_snake_case(&variant),
            method,
            "`GatewayEventFrame::{variant}` serializes as \
             `\"type\":\"{}\"` but is published as `stream.{method}` — the \
             typed clients decode by tag and the Panel dispatches by method, \
             so one of the two will never see this frame",
            serde_snake_case(&variant)
        );
    }
}

/// `(variant, method)` for every stream method with no `StreamEvent` twin,
/// exemptions included — the input to both halves of the forward census.
fn untwinned_stream_methods() -> Vec<(String, String)> {
    let protocol = protocol_stream_variants();
    assert!(
        protocol.len() >= 10,
        "only {} variant(s) scraped from `aleph_protocol::StreamEvent` — the \
         enum was moved or reshaped and this census has become vacuous",
        protocol.len()
    );
    let twins: Vec<String> = protocol.iter().map(|v| serde_snake_case(v)).collect();
    scraped_arms()
        .into_iter()
        .filter(|(_, method)| !twins.contains(method))
        .collect()
}

/// Forward half: nothing may ride the `stream.*` plane that the typed clients
/// cannot decode, unless it is named as Panel-only.
#[test]
fn every_stream_method_has_a_typed_twin_in_the_protocol_enum() {
    let missing: Vec<String> = untwinned_stream_methods()
        .iter()
        .filter(|(_, method)| !PANEL_ONLY_STREAM_METHODS.contains(&method.as_str()))
        .map(|(variant, method)| format!("{variant} (stream.{method})"))
        .collect();
    assert!(
        missing.is_empty(),
        "{missing:?} publish a `stream.*` method with no `StreamEvent` variant \
         in shared/protocol/src/events.rs — every TUI/CLI client drops those \
         frames at connection.rs with a debug! line and nothing else. Add the \
         twin, or name the method in PANEL_ONLY_STREAM_METHODS with a reason"
    );
}

/// The exemption list may only shrink on its own: an entry that grew a twin,
/// or that names a frame which stopped publishing, is red here. Without this
/// the list is a licence — it would keep excusing methods long after the
/// reason expired.
#[test]
fn the_panel_only_exemptions_are_all_still_untwinned() {
    let untwinned = untwinned_stream_methods();
    for exempt in PANEL_ONLY_STREAM_METHODS {
        assert!(
            untwinned.iter().any(|(_, method)| method == exempt),
            "PANEL_ONLY_STREAM_METHODS names `stream.{exempt}`, which is no \
             longer an un-twinned stream method — drop the entry so the list \
             keeps naming exactly the frames the typed clients cannot decode"
        );
    }
}

/// Reverse half: a `StreamEvent` variant the gateway never publishes is a
/// client parsing for a frame that does not exist — the same severed wire seen
/// from the other end.
#[test]
fn every_protocol_stream_variant_has_a_gateway_producer() {
    let published: Vec<String> = scraped_arms().into_iter().map(|(_, m)| m).collect();
    let orphans: Vec<String> = protocol_stream_variants()
        .into_iter()
        .filter(|variant| !published.contains(&serde_snake_case(variant)))
        .collect();
    assert!(
        orphans.is_empty(),
        "`aleph_protocol::StreamEvent` declares {orphans:?}, which no \
         `GatewayEventFrame::stream_method` arm publishes — either the producer \
         was never wired or it was renamed on one side only"
    );
}

// The scanners themselves, on hand-built input. A census whose scanner is
// wrong fails open — it scrapes nothing and agrees with everything.

#[test]
fn the_arm_scanner_pairs_a_variant_with_its_literal() {
    let src = "Self::RunAccepted { .. } => Some(\"stream.run_accepted\"),\n\
               Self::AskUser { .. } => Some(\"stream.ask_user\"),";
    assert_eq!(
        stream_method_arms(src),
        vec![
            ("RunAccepted".to_string(), "run_accepted".to_string()),
            ("AskUser".to_string(), "ask_user".to_string()),
        ]
    );
}

#[test]
fn the_enum_scanner_skips_fields_and_stops_at_the_closing_brace() {
    let src = "pub enum StreamEvent {\n    RunAccepted {\n        run_id: String,\n\
                       Nested: u8,\n    },\n    AcpSessionsChanged,\n}\n\
               pub enum Other {\n    NotMine,\n}";
    assert_eq!(
        enum_variant_names(src, "pub enum StreamEvent {"),
        vec!["RunAccepted".to_string(), "AcpSessionsChanged".to_string()]
    );
}

#[test]
fn snake_case_matches_serdes_rule() {
    assert_eq!(serde_snake_case("RunAccepted"), "run_accepted");
    assert_eq!(serde_snake_case("AskUser"), "ask_user");
    assert_eq!(serde_snake_case("Reasoning"), "reasoning");
}
