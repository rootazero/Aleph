//! Claude-Code / hermes JSON decision contract for hook stdout.
//!
//! Aleph hooks historically spoke a line-prefix protocol on stdout
//! (`block: …`, `deny: …`, `context: …`, see [`super::parse_command_output`]).
//! That contract is Aleph-proprietary, so hooks written for the wider
//! Claude-Code ecosystem — which emit a single JSON object on stdout — were
//! silently ignored even though Aleph already accepts Claude-Code *config*
//! (`PreToolUse` / `PostToolUse` aliases).
//!
//! This module closes that interop gap. When a hook's entire stdout parses as
//! a JSON object, it is interpreted as a structured decision and mapped onto
//! the existing [`HookResult`] fields — no new result state is introduced. The
//! line-prefix protocol remains the fallback for non-JSON output, so existing
//! hooks are byte-for-byte unaffected.
//!
//! ## Recognised shapes (a superset of Claude Code + hermes)
//!
//! ```json
//! { "decision": "block", "reason": "…" }                 // Claude-Code legacy
//! { "action":   "block", "message": "…" }                // hermes canonical
//! { "continue": false, "stopReason": "…" }               // halt the loop
//! { "systemMessage": "…" }                               // surface a notice
//! { "hookSpecificOutput": {                              // Claude-Code modern
//!     "permissionDecision": "deny",                      //   allow|deny|ask
//!     "permissionDecisionReason": "…",
//!     "additionalContext": "…" } }
//! ```
//!
//! Block messages follow the hermes precedence invariant: canonical `message`
//! wins over `reason`, and a block with no message falls back to a default so
//! a hook can never block silently.

use serde::Deserialize;

use super::{HookResult, PermissionDecision};

/// Fallback message when a hook blocks without supplying a reason.
const DEFAULT_BLOCK_MESSAGE: &str = "Blocked by hook.";

/// Top-level JSON hook-output envelope (camelCase = Claude-Code wire format).
///
/// Every field is optional: a hook emits only the keys it cares about, and an
/// empty object is a valid no-op. Unknown keys are ignored, keeping the
/// contract forward-compatible as the upstream schema grows.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct JsonHookOutput {
    /// Claude-Code legacy decision: `"block"` | `"approve"`.
    decision: Option<String>,
    /// Reason paired with `decision`.
    reason: Option<String>,
    /// hermes canonical decision: `"block"`.
    action: Option<String>,
    /// Message paired with `action` (wins over `reason` for block text).
    message: Option<String>,
    /// `false` asks the agent loop to stop after this hook.
    #[serde(rename = "continue")]
    continue_: Option<bool>,
    /// Human-readable reason shown when `continue` is `false`.
    stop_reason: Option<String>,
    /// Advisory notice surfaced to the next LLM turn.
    system_message: Option<String>,
    /// Modern Claude-Code per-event decision payload.
    hook_specific_output: Option<HookSpecificOutput>,
}

/// Nested `hookSpecificOutput` block (Claude-Code modern PreToolUse form).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct HookSpecificOutput {
    /// `"allow"` | `"deny"` | `"ask"`.
    permission_decision: Option<String>,
    /// Reason paired with `permission_decision`.
    permission_decision_reason: Option<String>,
    /// Extra context injected as a `<system-reminder>` next turn.
    additional_context: Option<String>,
}

/// Try to interpret `stdout` as a JSON decision object.
///
/// Returns `true` when the whole (trimmed) output is a JSON object — in which
/// case `result` has been updated and the caller should NOT fall through to the
/// line-prefix parser. Returns `false` for empty input or any non-object JSON
/// (string, array, number, malformed), letting the legacy parser handle it.
pub(super) fn apply_json_decision(stdout: &str, result: &mut HookResult) -> bool {
    let trimmed = stdout.trim();
    // Fast reject: a JSON object must start with `{`. This keeps the common
    // line-prefix path allocation-free and avoids mis-parsing plain messages
    // that merely contain JSON-ish fragments.
    if !trimmed.starts_with('{') {
        return false;
    }

    let parsed: JsonHookOutput = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => return false,
    };

    parsed.apply(result);
    true
}

impl JsonHookOutput {
    /// Map the parsed envelope onto [`HookResult`], honouring last-writer-wins
    /// for `permission_decision` exactly like the line-prefix parser.
    fn apply(self, result: &mut HookResult) {
        // 1. Modern nested permission decision (highest fidelity signal).
        if let Some(hso) = self.hook_specific_output {
            if let Some(reason) = hso.additional_context {
                if !reason.trim().is_empty() {
                    result.additional_contexts.push(reason);
                }
            }
            if let Some(decision) = hso.permission_decision.as_deref() {
                apply_permission_decision(
                    decision,
                    hso.permission_decision_reason,
                    result,
                );
            }
        }

        // 2. Legacy / canonical top-level decision. `action` (hermes) and
        //    `decision` (Claude-Code) are synonyms; either present means block
        //    or approve.
        let verb = self.action.as_deref().or(self.decision.as_deref());
        match verb {
            Some("block") => {
                let reason = block_message(self.message, self.reason);
                result.blocked = true;
                result.block_reason = Some(reason.clone());
                result.permission_decision = Some(PermissionDecision::Block { reason });
            }
            Some("approve") | Some("allow") => {
                result.blocked = false;
                result.block_reason = None;
                result.denied = false;
                result.deny_reason = None;
                result.permission_decision = Some(PermissionDecision::Allow);
            }
            _ => {}
        }

        // 3. `continue: false` halts the loop (Claude-Code semantics). The
        //    optional stopReason rides along as context so the LLM sees why.
        if self.continue_ == Some(false) {
            result.prevent_continuation = true;
            if let Some(reason) = self.stop_reason {
                if !reason.trim().is_empty() {
                    result.additional_contexts.push(reason);
                }
            }
        }

        // 4. Advisory system message → surfaced as next-turn context.
        if let Some(msg) = self.system_message {
            if !msg.trim().is_empty() {
                result.additional_contexts.push(msg);
            }
        }
    }
}

/// Apply a `permissionDecision` string to the result (allow/deny/ask).
fn apply_permission_decision(
    decision: &str,
    reason: Option<String>,
    result: &mut HookResult,
) {
    match decision {
        "allow" => {
            result.blocked = false;
            result.block_reason = None;
            result.denied = false;
            result.deny_reason = None;
            result.permission_decision = Some(PermissionDecision::Allow);
        }
        "deny" => {
            let reason = reason.unwrap_or_else(|| DEFAULT_BLOCK_MESSAGE.to_string());
            result.denied = true;
            result.deny_reason = Some(reason.clone());
            result.permission_decision = Some(PermissionDecision::Deny { reason });
        }
        "ask" => {
            result.permission_decision = Some(PermissionDecision::Ask {
                reason: reason.unwrap_or_default(),
            });
        }
        _ => {}
    }
}

/// Resolve the block message: canonical `message` wins over `reason`, and an
/// empty/absent value falls back to the default so blocks are never silent.
fn block_message(message: Option<String>, reason: Option<String>) -> String {
    message
        .filter(|m| !m.trim().is_empty())
        .or_else(|| reason.filter(|r| !r.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_BLOCK_MESSAGE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(stdout: &str) -> (bool, HookResult) {
        let mut result = HookResult::default();
        let consumed = apply_json_decision(stdout, &mut result);
        (consumed, result)
    }

    #[test]
    fn non_json_is_not_consumed() {
        let (consumed, result) = apply("block: line-prefix style");
        assert!(!consumed);
        assert!(!result.blocked);
    }

    #[test]
    fn empty_is_not_consumed() {
        let (consumed, _) = apply("   \n  ");
        assert!(!consumed);
    }

    #[test]
    fn json_array_is_not_consumed() {
        let (consumed, _) = apply("[1, 2, 3]");
        assert!(!consumed);
    }

    #[test]
    fn malformed_json_object_is_not_consumed() {
        let (consumed, _) = apply("{ not valid json");
        assert!(!consumed);
    }

    #[test]
    fn empty_object_is_consumed_as_noop() {
        let (consumed, result) = apply("{}");
        assert!(consumed);
        assert!(!result.blocked);
        assert!(result.permission_decision.is_none());
    }

    #[test]
    fn claude_code_legacy_block() {
        let (consumed, result) = apply(r#"{"decision": "block", "reason": "unsafe path"}"#);
        assert!(consumed);
        assert!(result.blocked);
        assert_eq!(result.block_reason.as_deref(), Some("unsafe path"));
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Block {
                reason: "unsafe path".to_string()
            })
        );
    }

    #[test]
    fn hermes_canonical_block_message_wins_over_reason() {
        // When both are present, canonical `message` is authoritative.
        let (consumed, result) =
            apply(r#"{"action": "block", "message": "canonical", "reason": "legacy"}"#);
        assert!(consumed);
        assert_eq!(result.block_reason.as_deref(), Some("canonical"));
    }

    #[test]
    fn block_without_message_uses_default() {
        let (_, result) = apply(r#"{"decision": "block"}"#);
        assert!(result.blocked);
        assert_eq!(result.block_reason.as_deref(), Some(DEFAULT_BLOCK_MESSAGE));
    }

    #[test]
    fn claude_code_approve_clears_block() {
        let (_, result) = apply(r#"{"decision": "approve"}"#);
        assert!(!result.blocked);
        assert_eq!(result.permission_decision, Some(PermissionDecision::Allow));
    }

    #[test]
    fn hook_specific_deny_with_reason() {
        let json = r#"{"hookSpecificOutput": {"permissionDecision": "deny", "permissionDecisionReason": "policy"}}"#;
        let (consumed, result) = apply(json);
        assert!(consumed);
        assert!(result.denied);
        assert_eq!(result.deny_reason.as_deref(), Some("policy"));
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Deny {
                reason: "policy".to_string()
            })
        );
    }

    #[test]
    fn hook_specific_ask() {
        let json = r#"{"hookSpecificOutput": {"permissionDecision": "ask", "permissionDecisionReason": "confirm"}}"#;
        let (_, result) = apply(json);
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Ask {
                reason: "confirm".to_string()
            })
        );
    }

    #[test]
    fn hook_specific_additional_context_injected() {
        let json = r#"{"hookSpecificOutput": {"additionalContext": "lint clean"}}"#;
        let (_, result) = apply(json);
        assert_eq!(result.additional_contexts, vec!["lint clean".to_string()]);
    }

    #[test]
    fn continue_false_prevents_continuation_and_surfaces_reason() {
        let (_, result) = apply(r#"{"continue": false, "stopReason": "budget hit"}"#);
        assert!(result.prevent_continuation);
        assert_eq!(result.additional_contexts, vec!["budget hit".to_string()]);
    }

    #[test]
    fn continue_true_is_noop() {
        let (_, result) = apply(r#"{"continue": true}"#);
        assert!(!result.prevent_continuation);
    }

    #[test]
    fn system_message_becomes_context() {
        let (_, result) = apply(r#"{"systemMessage": "heads up"}"#);
        assert_eq!(result.additional_contexts, vec!["heads up".to_string()]);
    }

    #[test]
    fn deny_via_hook_specific_then_top_level_block_both_applied() {
        // Nested deny applies first, then a top-level block overrides the
        // permission_decision (last-writer-wins) while keeping denied=true.
        let json = r#"{"hookSpecificOutput": {"permissionDecision": "deny", "permissionDecisionReason": "p"}, "action": "block", "message": "m"}"#;
        let (_, result) = apply(json);
        assert!(result.denied);
        assert!(result.blocked);
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Block {
                reason: "m".to_string()
            })
        );
    }

    #[test]
    fn unknown_keys_ignored_forward_compat() {
        let (consumed, result) = apply(r#"{"futureKey": 42, "decision": "block", "reason": "r"}"#);
        assert!(consumed);
        assert!(result.blocked);
    }
}
