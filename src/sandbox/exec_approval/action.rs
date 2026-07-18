//! [`ApprovalAction`] — WHAT an approval gate is asking the human to allow.
//!
//! An approval carried only `(tool_name, reason)`, so every surface could show
//! at most a bare tool name: the card said `bash`, never *which* command, and
//! the session grant it produced was keyed on that name — one "allow session"
//! on `file_ops list` authorized `file_ops delete` for the rest of the session,
//! defeating the exec tier's own argument filter.
//!
//! The action is therefore the unit of approval, not the tool:
//!   * [`ApprovalAction::summary`] is the redacted, length-capped rendering of
//!     the actual call — the one string every surface (Panel card, Telegram /
//!     Slack prompt, CLI, cluster center) puts in front of the human (R6);
//!   * [`grant_fingerprint`] keys the session grant and the denial ledger on
//!     `(tool, canonical arguments)`, so a grant covers the call that was
//!     shown and nothing else.
//!
//! Mirrors codex's `ApprovalStore` (`core/src/tools/sandboxing.rs`), whose cache
//! key is the canonicalized argv + cwd and never the tool name. Pure mechanism:
//! no model reasoning is replaced (R7).

use serde_json::Value;
use std::path::Path;

use crate::exec::analysis::CommandAnalysis;
use crate::exec::masker::SecretMasker;

/// Cap on the human-visible summary. Long enough for a real command line,
/// short enough for a Telegram button prompt or an inline card.
const MAX_SUMMARY_CHARS: usize = 200;

/// Tools whose payload IS a shell command, and whose summary therefore gets a
/// real [`CommandAnalysis`] instead of a stub.
const SHELL_TOOLS: &[&str] = &["bash", "code_exec"];

/// The action an approval gate is blocking on.
///
/// Built at the gate (tool dispatch / sandbox elevation), consumed by every
/// [`ApprovalRequester`](super::gate::ApprovalRequester) implementation. It
/// deliberately carries the *redacted* summary, never the raw arguments: the
/// struct crosses process boundaries (reverse RPC to a cluster center) and
/// lands in operator-visible records, so a credential in an argument must not
/// ride along.
#[derive(Debug, Clone)]
pub struct ApprovalAction {
    /// Registered tool name (or, for a sandbox elevation, the program).
    pub tool_name: String,
    /// Secret-redacted, length-capped one-line rendering of the actual call.
    /// This is what the human reads before deciding.
    pub summary: String,
    /// Working directory, when the action has one.
    pub cwd: Option<String>,
    /// Real shell analysis for shell-shaped actions; `None` when the action is
    /// not a command line (a JSON tool call, a route escalation).
    pub analysis: Option<CommandAnalysis>,
    /// Why the gate fired — the prose beneath the summary.
    pub reason: String,
}

impl ApprovalAction {
    /// A tool call: `name` plus the call's raw JSON `input`.
    #[must_use]
    pub fn for_tool_call(name: &str, input: &Value, reason: impl Into<String>) -> Self {
        let raw = preview(name, input);
        let analysis = shell_command_of(name, input)
            .map(|cmd| crate::exec::parser::analyze_shell_command(&cmd, None, None));
        Self {
            tool_name: name.to_string(),
            summary: format!("{name}: {}", redact_and_cap(&raw)),
            cwd: None,
            analysis,
            reason: reason.into(),
        }
    }

    /// A sandboxed command asking to escalate beyond its baseline capabilities.
    #[must_use]
    pub fn for_command(
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
        reason: impl Into<String>,
    ) -> Self {
        let line = if args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", args.join(" "))
        };
        let analysis = crate::exec::parser::analyze_shell_command(&line, cwd, None);
        Self {
            tool_name: program.to_string(),
            summary: redact_and_cap(&line),
            cwd: cwd.map(|p| p.display().to_string()),
            analysis: Some(analysis),
            reason: reason.into(),
        }
    }

    /// An approval with no arguments to show — a route escalation, where the
    /// decision is about the destination, not about a command.
    #[must_use]
    pub fn bare(tool_name: &str, reason: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            summary: tool_name.to_string(),
            cwd: None,
            analysis: None,
            reason: reason.into(),
        }
    }

    /// The analysis to stamp on an [`ApprovalRequest`] record, which takes a
    /// non-optional one. A non-command action has nothing to parse, and says so
    /// with an empty (not failed) analysis.
    ///
    /// [`ApprovalRequest`]: crate::exec::decision::ApprovalRequest
    #[must_use]
    pub fn analysis_for_record(&self) -> CommandAnalysis {
        // rust-doctor-disable-next-line excessive-clone
        self.analysis.clone().unwrap_or(CommandAnalysis {
            ok: true,
            reason: None,
            segments: vec![],
            chains: None,
        })
    }
}

/// Stable fingerprint of `(tool, canonical arguments)` — the key for BOTH the
/// session grant and the denial ledger.
///
/// Keyed on the RAW input, never on the rendered summary: redaction collapses
/// distinct secrets to one placeholder, so a summary-derived key would let an
/// "allow session" on one credential authorize a call carrying another.
///
/// The input is *canonicalized* before hashing ([`canonical_args`]): key order
/// is already `BTreeMap`-normalized (`serde_json` has no `preserve_order` here),
/// explicit `null`s are dropped, and advisory non-identity keys (the model's
/// free-text `justification`) are dropped. Without those, a model could mint a
/// fresh fingerprint for a semantically identical call — by appending a
/// tool-ignored `"_":null`, or by rewording the `justification` — defeating the
/// denial ledger's blind-retry guard (a refused action re-prompts) and silently
/// invalidating a session grant (the same action re-prompts). Only `null` and
/// the known advisory keys are stripped, not every default: an absent optional
/// and an explicitly-passed value are genuinely different calls, and collapsing
/// them could let a grant on one authorize the other.
#[must_use]
pub fn grant_fingerprint(tool: &str, input: &Value) -> String {
    super::denial_ledger::action_fingerprint(tool, &canonical_args(tool, input).to_string())
}

/// `input` with every explicit `null` and every advisory non-identity key
/// (see [`is_non_identity_key`]) removed, recursively. Object key order is left
/// to `serde_json`'s `BTreeMap` serialization (canonical in this workspace). A
/// non-container value is returned unchanged.
fn canonical_args(tool: &str, input: &Value) -> Value {
    match input {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, v)| !v.is_null() && !is_non_identity_key(tool, k))
                // rust-doctor-disable-next-line excessive-clone
                .map(|(k, v)| (k.clone(), canonical_args(tool, v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| canonical_args(tool, v)).collect()),
        // rust-doctor-disable-next-line excessive-clone
        other => other.clone(),
    }
}

/// Advisory, model-controlled fields that must NOT participate in a grant /
/// denial fingerprint. `justification` is the free-text reason a model attaches
/// to a **sandbox capability escalation** (`bash` / `code_exec`): it is
/// shown to the human but is not part of the action's identity. Letting it into
/// the fingerprint would let the model mint a fresh key on every call (defeating
/// the denial ledger's blind-retry guard) or silently invalidate a session grant
/// just by rewording it — the same defect class the `null`-strip closes.
///
/// Scoped to those two tools by name: an external / MCP tool Aleph does not
/// control could carry a `justification` argument that IS identity-bearing (part
/// of a persisted record), and stripping it there would collapse two genuinely
/// different calls onto one grant. The names match the tools' registered
/// `AlephTool::NAME` (`BashExecTool` is `"bash"`, not `"bash_exec"`), the same
/// identity `grant_fingerprint` receives via `action.tool_name` at the confirm
/// gate — a stale `"bash_exec"` here left the guard inert for the bash tool.
fn is_non_identity_key(tool: &str, key: &str) -> bool {
    key == "justification" && matches!(tool, "bash" | "code_exec")
}

/// The shell command a shell-shaped tool call carries, if any.
fn shell_command_of(name: &str, input: &Value) -> Option<String> {
    if !SHELL_TOOLS.contains(&name) {
        return None;
    }
    ["cmd", "command", "code", "script"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_string)
}

/// A compact, one-line rendering of a tool call's arguments.
///
/// `file_ops` gets the operation-first shape because that is the argument the
/// tier's hard filter gates on (`delete` / `move` / …) and the one the human
/// must see. Everything else falls back to `key=value` pairs — the object's
/// keys are already `BTreeMap`-ordered, so the rendering is stable.
fn preview(name: &str, input: &Value) -> String {
    if let Some(cmd) = shell_command_of(name, input) {
        return cmd;
    }
    let Some(obj) = input.as_object() else {
        return input.to_string();
    };
    if obj.is_empty() {
        return "(no arguments)".to_string();
    }
    if name == "file_ops" {
        let op = obj.get("operation").map_or("?", |v| v.as_str().unwrap_or("?"));
        let path = ["path", "file_path", "source"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .unwrap_or("");
        let dest = obj
            .get("destination")
            .and_then(Value::as_str)
            .map(|d| format!(" destination={d}"))
            .unwrap_or_default();
        return format!("operation={op} path={path}{dest}");
    }
    obj.iter()
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Redact credentials, then cap on a CHAR boundary (`&s[..n]` panics mid-UTF-8).
fn redact_and_cap(s: &str) -> String {
    let masked = SecretMasker::new().mask(s);
    // Collapse newlines: a multi-line script must still render as one card line.
    let flat = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(MAX_SUMMARY_CHARS) {
        Some((idx, _)) => format!("{}…", &flat[..idx]),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shell_tool_summary_is_the_command_and_analysis_is_real() {
        let action = ApprovalAction::for_tool_call(
            "bash",
            &json!({"cmd": "rm -rf /home/u/Documents"}),
            "gated",
        );
        assert!(
            action.summary.contains("rm -rf /home/u/Documents"),
            "the human must see the command, not the tool name: {}",
            action.summary
        );
        let analysis = action.analysis.expect("bash gets a real analysis");
        assert!(analysis.ok, "{analysis:?}");
        assert!(
            analysis.executables().contains(&"rm"),
            "the stub analysis (empty segments) is what left the operator blind"
        );
    }

    #[test]
    fn fingerprint_ignores_key_order_and_explicit_nulls() {
        // Same effective call, three spellings the model might emit. All must
        // hash to one fingerprint, or a refused/approved action re-prompts.
        let base = grant_fingerprint("file_ops", &json!({"operation": "delete", "path": "/a"}));
        let reordered = grant_fingerprint("file_ops", &json!({"path": "/a", "operation": "delete"}));
        let null_padded = grant_fingerprint(
            "file_ops",
            &json!({"operation": "delete", "path": "/a", "destination": null}),
        );
        assert_eq!(base, reordered, "key order must not change the fingerprint");
        assert_eq!(
            base, null_padded,
            "a tool-ignored explicit null must not mint a fresh fingerprint (denial-ledger bypass)"
        );
    }

    #[test]
    fn fingerprint_ignores_model_justification() {
        // The model-controlled `justification` on a capability escalation is
        // advisory (shown to the human, not part of identity). Rewording it must
        // NOT mint a fresh fingerprint, or the model defeats the denial ledger /
        // silently invalidates a session grant by varying the reason text.
        // Use the tool's real registered NAME (`"bash"`, per BashExecTool::NAME),
        // which is the identity `grant_fingerprint` receives at the confirm gate.
        let plain = grant_fingerprint(
            "bash",
            &json!({"command": "curl example.com", "allow_network": true}),
        );
        let justified = grant_fingerprint(
            "bash",
            &json!({"command": "curl example.com", "allow_network": true,
                    "justification": "need to fetch the changelog"}),
        );
        let reworded = grant_fingerprint(
            "bash",
            &json!({"command": "curl example.com", "allow_network": true,
                    "justification": "totally different words here"}),
        );
        assert_eq!(plain, justified, "justification must not enter the fingerprint");
        assert_eq!(justified, reworded, "rewording justification must not re-key");
    }

    #[test]
    fn fingerprint_keeps_justification_for_non_exec_tools() {
        // For a tool Aleph does not control, a `justification` argument may be
        // identity-bearing (part of a persisted record). Two calls differing only
        // in that field are genuinely different actions and must NOT collapse onto
        // one grant — the strip is scoped to bash/code_exec by name.
        let a = grant_fingerprint(
            "external_policy_write",
            &json!({"target": "acl", "justification": "grant read"}),
        );
        let b = grant_fingerprint(
            "external_policy_write",
            &json!({"target": "acl", "justification": "grant write"}),
        );
        assert_ne!(a, b, "an external tool's justification stays in the identity");
    }

    #[test]
    fn fingerprint_separates_genuinely_different_actions() {
        // The invariant the whole action-aware refactor exists for: two
        // different calls of the SAME tool must not share a fingerprint, or a
        // grant on one authorizes the other.
        let a = grant_fingerprint("file_ops", &json!({"operation": "delete", "path": "/tmp/junk"}));
        let b = grant_fingerprint(
            "file_ops",
            &json!({"operation": "delete", "path": "/home/u/Documents"}),
        );
        assert_ne!(a, b, "distinct paths must not collide onto one grant");
    }

    #[test]
    fn file_ops_summary_names_the_operation_and_path() {
        let action = ApprovalAction::for_tool_call(
            "file_ops",
            &json!({"operation": "delete", "path": "/home/u/Documents"}),
            "gated",
        );
        assert_eq!(
            action.summary,
            "file_ops: operation=delete path=/home/u/Documents"
        );
    }

    #[test]
    fn generic_tool_summary_lists_arguments() {
        let action =
            ApprovalAction::for_tool_call("vault_store", &json!({"key": "openai"}), "gated");
        assert_eq!(action.summary, "vault_store: key=openai");
    }

    #[test]
    fn secrets_in_arguments_are_redacted() {
        const KEY: &str = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let cmd = format!("curl -H 'Authorization: Bearer {KEY}' https://x");
        let action = ApprovalAction::for_tool_call("bash", &json!({ "cmd": cmd }), "gated");
        assert!(
            !action.summary.contains(KEY),
            "a credential must never reach a human-visible card or a log: {}",
            action.summary
        );
    }

    /// The HTTP header form `Authorization: Bearer <token>` — a space after
    /// `Bearer`, which the keyword=value masker rule cannot reach. Uses a
    /// generic JWT (no `sk-`/`gh` prefix), so it can only pass if the
    /// space-form bearer rule fires: the summary flows to the operator card, a
    /// Telegram/Slack prompt, and a cluster reverse-RPC frame.
    #[test]
    fn http_bearer_header_is_redacted_in_summary() {
        const TOKEN: &str =
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N";
        let cmd = format!("curl -H 'Authorization: Bearer {TOKEN}' https://x");
        let action = ApprovalAction::for_tool_call("bash", &json!({ "cmd": cmd }), "gated");
        assert!(
            !action.summary.contains(TOKEN),
            "an HTTP bearer token must not reach a card/log/cluster frame: {}",
            action.summary
        );
    }

    /// `curl -u user:password` basic-auth flag — redacted whole, since the
    /// summary is human-facing and never re-executed.
    #[test]
    fn curl_basic_auth_is_redacted_in_summary() {
        let action = ApprovalAction::for_tool_call(
            "bash",
            &json!({ "cmd": "curl -u admin:hunter2hunter2 https://x" }),
            "gated",
        );
        assert!(
            !action.summary.contains("hunter2hunter2"),
            "a basic-auth password must not reach a card/log: {}",
            action.summary
        );
    }

    #[test]
    fn long_summary_is_capped_on_a_char_boundary() {
        // Multi-byte chars straddling the cap: `&s[..n]` would panic here.
        let long = "删".repeat(500);
        let action = ApprovalAction::for_tool_call("bash", &json!({ "cmd": long }), "gated");
        assert!(action.summary.chars().count() <= MAX_SUMMARY_CHARS + 10);
        assert!(action.summary.ends_with('…'));
    }

    #[test]
    fn fingerprint_separates_arguments_of_the_same_tool() {
        let list = grant_fingerprint("file_ops", &json!({"operation": "list", "path": "/tmp"}));
        let delete = grant_fingerprint("file_ops", &json!({"operation": "delete", "path": "/tmp"}));
        assert_ne!(
            list, delete,
            "a grant on `list` must not key the same bucket as `delete`"
        );
        // Emission order must not change the key — the store is canonical.
        assert_eq!(
            grant_fingerprint("file_ops", &json!({"operation": "list", "path": "/tmp"})),
            grant_fingerprint("file_ops", &json!({"path": "/tmp", "operation": "list"}))
        );
    }
}
