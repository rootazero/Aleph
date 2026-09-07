//! Routing configuration types
//!
//! Contains routing rule configuration:
//! - `RoutingRuleConfig`: custom slash-command rules (`[[rules]]`)
//!
//! # Keyword rules are retired (2026-09-05)
//!
//! `[[rules]]` once documented two kinds, discriminated by whether `regex`
//! starts with `^/`. Only the command kind was ever wired: the single
//! production reader,
//! `tool_metadata::registry::registration::register_custom_commands`, skips
//! every rule that is not [`is_registered_command`](RoutingRuleConfig::is_registered_command).
//! The "all keyword rules match, their prompts are combined with `\n\n`"
//! machinery this file used to describe was never written — a keyword rule's
//! `system_prompt`, which the old doc called "the main purpose of keyword
//! rules", reached nothing at all.
//!
//! Connecting them was not an option: matching a regex against the user's
//! message to pick a prompt is intent routing by rule engine, which
//! `CLAUDE.md`'s Do-NOT list forbids outright (R7/P8). So the concept is cut,
//! not wired. The cut has three faces, because "keyword rule" is discriminated
//! by a *value* rather than by a key:
//!
//! * **Load — fail-open.** `Config` has no `deny_unknown_fields` and
//!   `[[rules]]` survives as a section with all its fields, so an existing file
//!   keeps parsing and the daemon keeps booting. `Config::validate` names each
//!   retired rule at `warn!` (`Config::retired_keyword_rules`) so the operator
//!   can find out *why* his rule stopped working. A gate whose subject cannot
//!   learn why is fail-dead, not fail-closed.
//! * **Write — fail-closed.** `routing_rules.create` / `.update` refuse a rule
//!   that would not be registered, so no client can add a new one. Without
//!   this the retirement would leave a button that writes a rule which
//!   silently does nothing — worse than before the cut.
//! * **Panel.** The rule-type selector that offered "keyword" is gone.
//!
//! **`config::dead_keys` deliberately carries no entry for this.** That
//! scanner reports the *key paths* `serde_ignored` discarded; a retired keyword
//! rule spells every one of its keys exactly like a live command rule and
//! differs only in the *value* of `regex`. An entry there could never match,
//! and a tolerated path that cannot fire is a recognizer with no red state.
//! `dead_keys` covers removed *sections* (at their serde root) and removed
//! *fields* on surviving structs; this is neither.
//!
//! # Fields that parse and are dropped
//!
//! `provider`, `preferred_model`, `strip_prefix`, `intent_type` and `icon`
//! round-trip cleanly through serde but are never read by
//! `register_custom_commands`; only `regex`, `system_prompt` and `is_builtin`
//! are. They are kept on the struct so existing operator TOML does not break.
//!
//! `provider` is the one that reads like it works, so state it exactly:
//! `Config::validate` *requires* a command rule to carry a `provider` naming a
//! configured provider, and the file fails to load without one — but nothing
//! downstream consumes it. `CommandContext::Custom` carries only
//! `system_prompt` and `pattern`, and there is no routing pass that turns a
//! rule's `provider` into a model choice. The gate is real; the effect it
//! implies is not. A future patch that wires these fields must (a) update the
//! registration path to honor them and (b) keep these doc-comments in sync.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// RoutingRuleConfig
// =============================================================================

/// Routing rule configuration for TOML parsing
///
/// One kind of rule is live: a **custom slash command**, whose `regex` starts
/// with `^/` (see [`is_registered_command`](Self::is_registered_command)).
/// `register_custom_commands` turns it into a tool named after the command,
/// using `system_prompt` as both the tool description and the routing prompt.
///
/// - `provider` is required by `Config::validate` and read by nothing. The
///   module doc explains that split; do not restate it as "specifies which AI
///   to use", which is what this comment said until 2026-09-05 while the module
///   doc twelve lines above said the field was dropped.
/// - The command prefix does not reach the model, but `strip_prefix` is not
///   why: `CommandParser` splits `/name` from its arguments, and the field is
///   never read.
///
/// **Keyword rules (a `regex` not starting with `^/`) are retired.** They were
/// never registered anywhere; see the module doc for the cut and for where an
/// operator is told about one.
///
/// # Example TOML
///
/// ```toml
/// [[rules]]
/// regex = "^/draw\\s+"
/// provider = "gemini"
/// system_prompt = "Draw a picture based on the prompt"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutingRuleConfig {
    // ===== Rule Type =====
    /// Operator-declared rule type. Only `"command"` names a live kind;
    /// `"keyword"` names the retired one and is refused on the write path.
    ///
    /// This field is a *label*, not an authority: whether a rule reaches
    /// `register_custom_commands` is decided solely by
    /// [`is_registered_command`](Self::is_registered_command). Kept so existing
    /// TOML parses, and reported through `get_rule_type` for display.
    #[serde(default)]
    pub rule_type: Option<String>,

    /// Whether this is a builtin rule (read-only in Settings UI)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_builtin: bool,

    // ===== Core fields =====
    /// Regex pattern to match against user input
    pub regex: String,

    /// Provider name to use when this rule matches
    /// Required for command rules, ignored for keyword rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// System prompt to guide AI behavior.
    ///
    /// For a registered command rule this becomes the tool's description
    /// (truncated to 100 chars) and its `routing_system_prompt`. Optional; a
    /// generic description is used when it is absent.
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Whether to strip the matched prefix from input before sending to AI
    /// Defaults to true for command rules, ignored for keyword rules
    #[serde(default)]
    pub strip_prefix: Option<bool>,

    // ===== Routing hints =====
    /// Intent type identifier (for logging and UI display)
    /// Examples: "translation", "research", "`code_generation`", "skills:build-macos-apps"
    /// Default: "general"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,

    /// Preferred model ID for this rule (optional)
    ///
    /// If specified, this model is used instead of automatic selection.
    /// Must be a valid model profile ID (e.g., "claude-opus", "gpt-4o").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,

    // ===== Command Mode Display fields =====
    /// SF Symbol icon name for command mode display
    /// Default: based on command type (bolt for Action, text.quote for Prompt)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl RoutingRuleConfig {
    /// Create a test config (for tests only)
    /// Note: This creates a command rule since it has an explicit provider
    #[must_use]
    pub fn test_config(regex: &str, provider: &str) -> Self {
        Self {
            rule_type: Some("command".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: None,
            strip_prefix: None,
            intent_type: None,
            preferred_model: None,
            icon: None,
        }
    }

    /// Create a command rule config
    #[must_use]
    pub fn command(regex: &str, provider: &str, system_prompt: Option<&str>) -> Self {
        Self {
            rule_type: Some("command".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: system_prompt.map(|s| s.to_string()),
            strip_prefix: Some(true),
            intent_type: None,
            preferred_model: None,
            icon: None,
        }
    }

    /// Does this rule reach `register_custom_commands`?
    ///
    /// **The single derivation of the `^/` boundary.** `registration.rs` used
    /// to spell the same test inline; two spellings of one fact is how a guard
    /// ends up reporting on a different set than the one it claims to describe,
    /// so the registration path calls this and so does the retirement warning
    /// in `Config::validate` and the refusal in the `routing_rules.*` handlers.
    ///
    /// Note it does *not* consult `rule_type`: a rule labelled `"command"`
    /// whose regex lacks the prefix is skipped by the registrar all the same,
    /// and a warning keyed on the label would miss it.
    #[must_use]
    pub fn is_registered_command(&self) -> bool {
        self.regex.starts_with("^/")
    }

    /// Get the effective rule type (with auto-detection)
    #[must_use]
    pub fn get_rule_type(&self) -> &str {
        if let Some(ref rule_type) = self.rule_type {
            return rule_type.as_str();
        }
        // Auto-detect based on regex pattern
        if self.regex.starts_with("^/") {
            "command"
        } else {
            "keyword"
        }
    }

    /// Is this rule *declared* a command rule?
    ///
    /// Answers a different question from
    /// [`is_registered_command`](Self::is_registered_command): this one reads
    /// the operator's label, and its only consumer is `Config::validate`'s
    /// `provider` requirement. Deliberately left keyed on the label so the cut
    /// does not change which existing files fail to load — a rule declared
    /// `"keyword"` with a `^/` regex boots today and must keep booting.
    #[must_use]
    pub fn is_command_rule(&self) -> bool {
        self.get_rule_type() == "command"
    }

    /// Get intent type (with default value).
    ///
    /// `pub(crate)` — the wiring pass for the (currently-unimplemented)
    /// command-rule dispatch enum is still pending; kept crate-scoped so the
    /// public capture surface stays narrow.
    #[allow(dead_code)] // lib build doesn't compile `#[cfg(test)]`; the in-module test exercises this
    #[must_use]
    pub(crate) fn get_intent_type(&self) -> &str {
        self.intent_type.as_deref().unwrap_or("general")
    }

    /// Get preferred model ID.
    ///
    /// `pub(crate)` — see [`get_intent_type`](Self::get_intent_type).
    #[allow(dead_code)] // lib build doesn't compile `#[cfg(test)]`; the in-module test exercises this
    #[must_use]
    pub(crate) fn get_preferred_model(&self) -> Option<&str> {
        self.preferred_model.as_deref()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_intent_type_default() {
        let rule = RoutingRuleConfig::default();
        assert_eq!(rule.get_intent_type(), "general");
    }

    #[test]
    fn test_get_preferred_model() {
        let rule = RoutingRuleConfig {
            preferred_model: Some("claude-opus".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_preferred_model(), Some("claude-opus"));

        let rule_none = RoutingRuleConfig::default();
        assert_eq!(rule_none.get_preferred_model(), None);
    }
}
