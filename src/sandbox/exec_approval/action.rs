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
/// `serde_json` is built without `preserve_order` in this workspace, so
/// `Value::to_string()` is `BTreeMap`-ordered and therefore canonical: the same
/// arguments always hash to the same key, whatever order the model emitted them
/// in.
#[must_use]
pub fn grant_fingerprint(tool: &str, input: &Value) -> String {
    super::denial_ledger::action_fingerprint(tool, &input.to_string())
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
