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

/// Per-field cap for the IDENTITY half of a rendered card (see `preview`).
///
/// Sized so every gate-inspected field of the widest call still fits inside
/// [`MAX_SUMMARY_CHARS`]: `action`(≤11) + `id` + `from_id` + `to_id` + `edge`
/// ≈ 195 chars at this cap. Comfortably above a real id (`cron:<uuid>` = 41).
const MAX_IDENTITY_CHARS: usize = 48;

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
    /// The session-grant identity of this action — the same
    /// [`grant_fingerprint`] key the dispatch gate's session memory and the
    /// denial ledger use. `Some` only for tool calls, where the raw input is
    /// known at construction; `None` for command elevations and bare route
    /// escalations, whose identity lives outside this struct. Stamped onto the
    /// pending approval record so a session-level grant can cascade to other
    /// ALREADY-PENDING approvals of the same action (concurrent subagent /
    /// teams broadcast), not just suppress later re-prompts.
    pub grant_key: Option<String>,
    /// The decision tiers this card may offer — derived once at the gate
    /// ([`crate::exec::allowed_decisions`]), carried to every renderer, and
    /// **enforced** when the answer comes back
    /// ([`ApprovalDecisionType::clamped_for`]). A renderer may draw fewer
    /// buttons than this; nothing may honour more.
    ///
    /// Defaults to [`session_max`](crate::exec::allowed_decisions::session_max)
    /// on every constructor: a card whose builder never thought about the
    /// question must not be the one that hands out a permanent grant.
    ///
    /// [`ApprovalDecisionType::clamped_for`]: crate::exec::socket::ApprovalDecisionType::clamped_for
    pub allowed_decisions: Vec<crate::exec::socket::ApprovalDecisionType>,
    /// Stable id of the rule that stopped this call
    /// (`tools::scoped::gate_chain::GateRule::id`), when a named rule did.
    ///
    /// The chain's own module doc has always described this token as the one
    /// "the ledger and the tests key on" — and only the tests ever did. A
    /// signed approval row that records *that* an approval happened but not
    /// *which rule required it* cannot answer the question an auditor brings to
    /// it: whether the gate that fired was one an operator could have removed.
    /// The prose in [`Self::reason`] carries the same fact for a human, but it
    /// is a sentence — it gets reworded, and it is not a key.
    ///
    /// `None` for actions raised outside the chain (sandbox capability
    /// elevation, a bare route escalation).
    pub rule_id: Option<&'static str>,
}

impl ApprovalAction {
    /// A tool call: `name` plus the call's raw JSON `input`.
    #[must_use]
    pub fn for_tool_call(name: &str, input: &Value, reason: impl Into<String>) -> Self {
        let analysis = shell_command_of(name, input)
            .map(|cmd| crate::exec::parser::analyze_shell_command(&cmd, None, None));
        Self {
            tool_name: name.to_string(),
            summary: summarize_call(name, input),
            cwd: None,
            analysis,
            reason: reason.into(),
            grant_key: Some(grant_fingerprint(name, input)),
            allowed_decisions: crate::exec::allowed_decisions::session_max(),
            rule_id: None,
        }
    }

    /// Widen (or narrow) the decision tiers this card offers. The value comes
    /// from [`crate::exec::allowed_decisions`] — the one derivation — never
    /// from a literal at a call site.
    #[must_use]
    pub fn offering(mut self, decisions: Vec<crate::exec::socket::ApprovalDecisionType>) -> Self {
        self.allowed_decisions = decisions;
        self
    }

    /// Name the chain rule that stopped this call. The value is always a
    /// `GateRule::id`, never a literal — see [`Self::rule_id`].
    #[must_use]
    pub const fn gated_by(mut self, rule_id: &'static str) -> Self {
        self.rule_id = Some(rule_id);
        self
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
            // The elevation gate keys its ledger on the normalized capability
            // summary, which this struct never sees — no grant identity here.
            grant_key: None,
            // A capability elevation is cached per-session in the workspace's
            // `granted_elevations`; there is no persistent tier for it, so the
            // card must not offer one.
            allowed_decisions: crate::exec::allowed_decisions::session_max(),
            rule_id: None,
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
            grant_key: None,
            // Nothing to remember: a route escalation has no action identity,
            // so neither grant tier can key on it.
            allowed_decisions: crate::exec::allowed_decisions::once_only(),
            rule_id: None,
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
        self.analysis
            .clone()
            .unwrap_or_else(CommandAnalysis::not_a_command)
    }
}

/// The redacted, capped one-line rendering of a tool call — what
/// [`ApprovalAction::for_tool_call`] puts on a card, without the
/// [`CommandAnalysis`] an approval needs and a ledger record does not.
///
/// Split out so the signed operation ledger ([`crate::identity`]) can record
/// every mutating call without paying a shell parse on calls no human will ever
/// be asked about.
#[must_use]
pub fn summarize_call(name: &str, input: &Value) -> String {
    format!("{name}: {}", redact_and_cap(&preview(name, input)))
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
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| canonical_args(tool, v)).collect())
        }
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
/// `file_ops` and `loop_graph` get an identity-first shape because those are
/// the arguments their tier rule gates on and the ones the human must see.
/// Everything else falls back to `key=value` pairs — the object's keys are
/// already `BTreeMap`-ordered, so the rendering is stable.
///
/// Why the second case is not cosmetic: the whole output is capped at
/// [`MAX_SUMMARY_CHARS`] and `BTreeMap` order is alphabetical, so for
/// `loop_graph` the gated identifiers sort BEHIND unbounded model-authored
/// prose — `to_id` last of all, `id` behind `body`. A `link` carrying 200+
/// characters of plausible rationale in `note` therefore raised a card reading
/// `action=link edge=owns_reference from_id=… note=<prose>…` with
/// `to_id=root:aleph` truncated clean off, under the generic reason line "Tool
/// `loop_graph` requires your confirmation to run". The gate fired correctly
/// and the human was shown a card containing no evidence of what it fired on —
/// and `grant_fingerprint` then bound a session grant to that exact call.
/// Identity first means the cap can only ever eat prose.
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
        let op = obj
            .get("operation")
            .map_or("?", |v| v.as_str().unwrap_or("?"));
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
    if name == "loop_graph" {
        let mut out = String::new();
        // Identity first, in gate order — every key the tier rule inspects,
        // and the three it actually discriminates on (`id`, `from_id`,
        // `to_id`, plus `edge` for the `unlink owns_reference` arm) ahead of
        // the two it does not (`kind`, `origin`).
        //
        // Each identity VALUE is capped too, which is the other half of the
        // same lesson. Ordering identity first only guarantees the cap eats
        // prose if identity is itself bounded — and node ids are model-chosen
        // free text with no length limit anywhere (`loop_graph node` validates
        // only the kind prefix, `upsert_node` has no length check, the column
        // is plain TEXT). So one un-carded `node` call could register
        // `cron:<190 chars>` and a follow-up `link` from it would push
        // `to_id=root:aleph` past [`MAX_SUMMARY_CHARS`] — the same card with
        // no evidence on it, reached by a different route. The cap is generous
        // enough for a real id (`cron:<uuid>` is 41 chars) and truncates from
        // the END, so the `root:`/`frozen:` prefix the gate fired on always
        // survives.
        for k in ["action", "id", "from_id", "to_id", "edge", "kind", "origin"] {
            if let Some(v) = obj.get(k).and_then(Value::as_str) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&format!(
                    "{k}={}",
                    crate::utils::text_format::truncate_with_marker(v, MAX_IDENTITY_CHARS, "…")
                ));
            }
        }
        // Then the free text, which is the only thing the cap may eat.
        for k in ["label", "cadence", "cron_expr", "note", "body", "prompt"] {
            if let Some(v) = obj.get(k).and_then(Value::as_str) {
                out.push_str(&format!(" {k}={v}"));
            }
        }
        return out;
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
/// `pub` because the signed operation ledger ([`crate::identity`]) is the
/// second operator-visible surface that renders tool arguments and tool-error
/// text, and it must mask and cap them by the same rule as an approval card. A
/// second copy of this logic is exactly how the `Authorization: Bearer` leak
/// got in the first time — one masker, one cap, one place.
pub fn redact_and_cap(s: &str) -> String {
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

    /// The card must contain the thing the gate fired on, even when the model
    /// fills the free-text fields with enough prose to blow the cap.
    #[test]
    fn loop_graph_card_shows_the_gated_id_before_any_prose() {
        let long = "合理的理由".repeat(80); // >> MAX_SUMMARY_CHARS
        let summary = redact_and_cap(&preview(
            "loop_graph",
            &json!({
                "action": "link",
                "edge": "owns_reference",
                "from_id": "cron:x",
                "note": long,
                "to_id": "root:aleph",
            }),
        ));
        assert!(
            summary.contains("to_id=root:aleph"),
            "the gated id must survive the cap: {summary}"
        );
        assert!(summary.contains("action=link"));

        // Same for a `node` write whose body is a long root reference.
        let summary = redact_and_cap(&preview(
            "loop_graph",
            &json!({
                "action": "node",
                "body": "什么算更好".repeat(80),
                "id": "root:aleph",
                "kind": "root",
                "origin": "human",
            }),
        ));
        assert!(summary.contains("id=root:aleph"), "{summary}");
        assert!(summary.contains("kind=root"), "{summary}");
        assert!(summary.contains("origin=human"), "{summary}");
    }

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
        let reordered =
            grant_fingerprint("file_ops", &json!({"path": "/a", "operation": "delete"}));
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
        assert_eq!(
            plain, justified,
            "justification must not enter the fingerprint"
        );
        assert_eq!(
            justified, reworded,
            "rewording justification must not re-key"
        );
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
        assert_ne!(
            a, b,
            "an external tool's justification stays in the identity"
        );
    }

    #[test]
    fn fingerprint_separates_genuinely_different_actions() {
        // The invariant the whole action-aware refactor exists for: two
        // different calls of the SAME tool must not share a fingerprint, or a
        // grant on one authorizes the other.
        let a = grant_fingerprint(
            "file_ops",
            &json!({"operation": "delete", "path": "/tmp/junk"}),
        );
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

    /// Ordering identity first only bounds the card if identity is itself
    /// bounded. Node ids are model-chosen free text with no length limit, so an
    /// un-carded `node` call can mint `cron:<190 chars>` and the follow-up
    /// `link` — which IS carded — would otherwise render with `to_id=root:aleph`
    /// truncated clean off: a card carrying no evidence of what it fired on,
    /// which `grant_fingerprint` then binds a session grant to.
    #[test]
    fn a_long_identity_field_cannot_push_the_gated_one_off_the_card() {
        let action = ApprovalAction::for_tool_call(
            "loop_graph",
            &json!({
                "action": "link",
                "from_id": format!("cron:{}", "x".repeat(190)),
                "to_id": "root:aleph",
                "edge": "watches",
                "origin": "llm",
                "note": "合理的理由".repeat(60),
            }),
            "gated",
        );
        assert!(
            action.summary.contains("to_id=root:aleph"),
            "the gated identifier must survive every cap: {}",
            action.summary
        );
        assert!(
            action.summary.contains("from_id=cron:"),
            "the truncation must keep the discriminating PREFIX: {}",
            action.summary
        );
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
