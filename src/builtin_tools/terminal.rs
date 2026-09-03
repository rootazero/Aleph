//! `TerminalTool` — read-only view of the terminal sessions this server owns
//! (herdr runtime port, phase 1, Task 11).
//!
//! Three actions, no write verb: `list` (which PTY sessions exist),
//! `status` (each session's detected agent state, the same table
//! `runtime.agents.list` serves), `read` (one session's current visible
//! screen, no scrollback). A human types into a terminal; the model only
//! looks.
//!
//! # Same disclosure as `pty.*` / `runtime.*`, so it is gated the same way
//!
//! This tool is a THIRD lens over the exact data `pty.*` and `runtime.*`
//! already gate operator-only on both their RPC and event faces (see
//! `gateway::handlers::pty`'s and `gateway::handlers::runtime`'s module
//! docs). A session id, its cwd, and its live screen contents are not less
//! sensitive because an LLM is asking instead of a JSON-RPC client — so
//! `terminal` is listed in [`method_authz::OPERATOR_TOOLS`]
//! (`crate::gateway::method_authz`), which walls it from chat-tier channels
//! and members at the tool-dispatch gate, AND checked again inline in
//! [`TerminalTool::call`].
//!
//! Unlike `select_model`'s `moa:` arm and `loop_manage`'s cross-session arm —
//! which gate their one sensitive action inline INSTEAD OF listing the whole
//! tool in `OPERATOR_TOOLS` — `terminal` gates the WHOLE tool both ways, and
//! the two halves do not agree on every path. Say that out loud rather than
//! leaving the next reader to derive it:
//!
//! - On the `ScopedToolService` path (`tools/scoped/dispatch.rs`), a
//!   chat-tier or member caller trips `OPERATOR_TOOLS` membership and
//!   `check_operator_gate` raises a live operator-approval card. **Even when
//!   a human approves that card, this tool still refuses anyway** — approval
//!   only flips `authorized` for the dispatch pipeline; nothing re-stamps
//!   [`crate::tools::turn_context::TurnContext`], so [`caller_is_operator`]
//!   below reads the same unchanged `caller_role` and answers `false`
//!   regardless. The refusal message that follows must not be read as "go
//!   get an operator" — an operator may have just said yes.
//! - On the `tools.invoke` path (`handlers/tools_invoke.rs`), no
//!   `TurnContext` is ever set, so [`caller_is_operator`] returns `true`
//!   unconditionally and contributes nothing there; `OPERATOR_TOOLS`
//!   membership (checked directly against `caller_role` by that handler,
//!   not through this tool) is the only thing closing that path.
//!
//! **Do not remove the inline check to "fix" the disagreement on the first
//! path above** — the fix belongs in `gate_chain.rs`'s card text (see the
//! `OPERATOR_TOOLS` entry's own comment in `method_authz.rs`), not here:
//! dropping the inline check would make that (currently misleading) card
//! actually grant a read of another principal's live terminal screen. This
//! is also exactly how `plugin_manage` once shipped ungated on one face
//! while its RPC twin stayed closed (see `method_authz.rs`'s own module
//! doc) — removing either half without re-deriving why the other is enough
//! on its own reopens that failure mode.
//!
//! Absent `caller_role` reads as operator (`role_is_operator`, "no identity
//! was resolved" — internal wiring, cron, a test — not "a stranger"), the
//! same convention every other inline gate in this crate follows.
//!
//! # Ownership filtering
//!
//! Every action is scoped to the caller's own sessions, using the SAME
//! predicate the two existing faces use — [`pty::PtyManager::owner_of`] +
//! [`pty::SessionOwner::admits`] (`list`/`status`) and [`pty::owner_admits`]
//! (`list`, matching `handle_list`'s own predicate on
//! [`pty::SessionInfo::created_by`] directly) — copied, not re-derived, so
//! three lenses on the same sessions cannot silently disagree about which
//! rows a caller may see (判据 §9). A session the caller does not own is
//! reported as "no such session" for `read`, byte for byte the same wording
//! `require_owned` uses in `gateway::handlers::pty` — a distinct "not yours"
//! would turn `read` into an oracle for enumerating other operators'
//! session ids.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::gateway::pty;
use crate::tools::AlephTool;

/// `terminal`'s three read-only actions. No write verb — see the tool's own
/// [`TerminalTool::DESCRIPTION`] for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAction {
    /// List the caller's own PTY sessions (id, shell, closed).
    List,
    /// Read one session's current visible screen (no scrollback). Requires
    /// `session_id`.
    Read,
    /// Report each of the caller's sessions' detected agent state — the
    /// same table `runtime.agents.list` serves.
    Status,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TerminalArgs {
    /// What to do.
    pub action: TerminalAction,
    /// Required for `read`: the PTY session id (from `list`'s output).
    /// Ignored for `list` / `status`.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Output envelope shared by all three actions — same shape as
/// `MoaManageOutput`: a flat `success`/`message`/`data` triple rather than a
/// per-action type, since the three actions have nothing in common to
/// factor beyond "did it work" and "here is the payload".
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Absent role reads as operator — matches `role_is_operator` and every
/// other inline cross-cutting gate in this crate (`select_model`'s `moa:`
/// arm, `loop_manage`'s cross-session arm). Do not invent a stricter
/// default here: a cron/A2A/internal run has no channel-stamped role to
/// read, and that is not the same thing as a stranger asking.
fn caller_is_operator() -> bool {
    crate::tools::turn_context::current_turn_context().is_none_or(|ctx| ctx.caller_is_operator())
}

#[derive(Clone, Default)]
pub struct TerminalTool;

#[async_trait]
impl AlephTool for TerminalTool {
    const NAME: &'static str = "terminal";
    const DESCRIPTION: &'static str = "Read-only view of the terminal sessions this server owns. \
        Lists sessions, reads the current visible screen, and reports each agent's detected \
        state (working / blocked / idle / unknown). It cannot type into a terminal or run \
        commands — a human does that.";

    type Args = TerminalArgs;
    type Output = TerminalOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let action_label = match args.action {
            TerminalAction::List => "list",
            TerminalAction::Read => "read",
            TerminalAction::Status => "status",
        };
        notify_tool_start(Self::NAME, action_label);

        if !caller_is_operator() {
            // Do not shorten this back to "requires operator; refused." — an
            // operator-approval card for THIS call may have just been shown
            // and answered "yes" (see the module doc's `ScopedToolService`
            // paragraph). This message exists so that outcome does not read
            // as "go get an operator", which is exactly what may have
            // already happened.
            let message = "terminal requires operator; refused, and an operator approving \
                this call's own escalation card does not lift the refusal — nothing \
                re-stamps the caller's role after approval."
                .to_string();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(TerminalOutput {
                success: false,
                message,
                data: None,
            });
        }

        let actor = crate::gateway::visibility::ambient_actor();
        let result = match args.action {
            TerminalAction::List => list_sessions(actor.as_deref()),
            TerminalAction::Status => status(actor.as_deref()),
            TerminalAction::Read => read_session(args.session_id.as_deref(), actor.as_deref()),
        };

        match result {
            Ok(data) => {
                notify_tool_result(Self::NAME, action_label, true);
                Ok(TerminalOutput {
                    success: true,
                    message: action_label.to_string(),
                    data: Some(data),
                })
            }
            Err(message) => {
                notify_tool_result(Self::NAME, &message, false);
                Ok(TerminalOutput {
                    success: false,
                    message,
                    data: None,
                })
            }
        }
    }
}

/// `list` — the caller's own sessions, filtered exactly as
/// `handle_list` in `gateway::handlers::pty` does: `pty::owner_admits`
/// against each [`pty::SessionInfo::created_by`] directly (no
/// `owner_of` round trip needed — `list()` already carries the field).
fn list_sessions(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    let sessions: Vec<serde_json::Value> = pty::manager()
        .list()
        .into_iter()
        .filter(|s| pty::owner_admits(s.created_by.as_deref(), actor))
        .map(|s| {
            serde_json::json!({
                "session_id": s.session_id,
                "shell": s.shell,
                "created_at": s.created_at,
                "closed": s.closed,
            })
        })
        .collect();
    Ok(serde_json::json!({ "sessions": sessions }))
}

/// `status` — the same table `runtime.agents.list` serves, filtered with
/// the identical predicate `handle_list` in `gateway::handlers::runtime`
/// uses: `pty::manager().owner_of(&entry.session_id).admits(actor)`.
fn status(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    let agents: Vec<_> = crate::gateway::runtime::agents()
        .snapshot()
        .into_iter()
        .filter(|entry| pty::manager().owner_of(&entry.session_id).admits(actor))
        .collect();
    let body = aleph_protocol::runtime::RuntimeAgentsListResponse { agents };
    serde_json::to_value(&body).map_err(|e| format!("encode failed: {e}"))
}

/// `read` — one session's current visible screen, no scrollback. Ownership
/// is checked BEFORE reading the screen (`PtyManager::visible_text` only
/// checks existence), and a session the caller does not own is refused with
/// exactly `require_owned`'s wording — an unowned session and a nonexistent
/// one must look identical, or `read` becomes an id-enumeration oracle.
fn read_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    let session_id = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "read requires `session_id`".to_string())?;
    if !pty::manager().owner_of(session_id).admits(actor) {
        return Err(format!("no such session: {session_id}"));
    }
    let text = pty::manager().visible_text(session_id)?;
    Ok(serde_json::json!({ "session_id": session_id, "text": text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the accepted action strings out of a tool schema, whichever of
    /// the two shapes schemars emitted.
    ///
    /// schemars 1.2 renders a fieldless enum as a flat `enum` array ONLY when
    /// no variant carries a doc comment; the moment one does — and all three
    /// of `TerminalAction`'s do, because the model reads them — it emits
    /// `oneOf` of `{const, description}` instead, to have somewhere to put
    /// the per-variant text. Both shapes mean the same thing to a provider,
    /// and which one ships is decided by something as innocent as deleting a
    /// `///` line, so the guard reads both rather than pinning the accident.
    ///
    /// Panics rather than returning an empty list when it recognises neither:
    /// "I cannot find the actions" must not be answerable as "there are no
    /// write verbs" (判据 §8).
    fn declared_actions(schema: &serde_json::Value) -> Vec<String> {
        let action = &schema["$defs"]["TerminalAction"];
        if let Some(flat) = action["enum"].as_array() {
            return flat
                .iter()
                .map(|v| v.as_str().expect("enum member is a string").to_string())
                .collect();
        }
        if let Some(variants) = action["oneOf"].as_array() {
            return variants
                .iter()
                .map(|v| {
                    v["const"]
                        .as_str()
                        .expect("oneOf member carries a const")
                        .to_string()
                })
                .collect();
        }
        panic!(
            "neither $defs.TerminalAction.enum nor .oneOf found; schema was {}",
            serde_json::to_string_pretty(schema).unwrap_or_default()
        );
    }

    /// 本期没有写入动词。多一个就是多一个授权面。
    ///
    /// Read out of `$defs`, not `properties.action`: schemars 1.2 emits a
    /// NAMED type as a `$ref`, so `properties.action` carries no action
    /// vocabulary at all and a guard reading it asserts against `Null`.
    /// That is the shape every sibling tool with an enum-typed argument
    /// already ships, and `schema_strictify` rewrites those refs explicitly.
    ///
    /// Not to be "fixed" by forcing `#[schemars(inline)]` to match
    /// `moa_manage`'s flat schema: that tool hand-writes `impl JsonSchema`
    /// because `#[serde(tag = "action")]` puts a `oneOf` at the ROOT, which
    /// grammar-constrained endpoints cannot compile — they answer with EMPTY
    /// arguments. `TerminalArgs` is a plain struct; its root is already a
    /// flat object, so that hazard is not this tool's to carry, and inlining
    /// would make `terminal` the one tool shipping a shape its nine siblings
    /// do not.
    #[test]
    fn the_tool_exposes_no_write_verb() {
        let def = TerminalTool.definition();
        let actions = declared_actions(&def.parameters);
        assert_eq!(actions, ["list", "read", "status"]);
    }

    /// DESCRIPTION 必须自己说清只读——这句话归这个工具所有，
    /// 不进 system prompt（R9 第二把尺）。不写，模型会反复试着发命令。
    #[test]
    fn the_description_says_it_is_read_only() {
        assert!(TerminalTool::DESCRIPTION
            .to_lowercase()
            .contains("read-only"));
    }

    /// No `TurnContext` at all reads as operator (cron/A2A/internal
    /// convention) — a caller with a scoped, non-operator role is refused.
    #[tokio::test]
    async fn no_turn_context_is_treated_as_operator() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::List,
                session_id: None,
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
    }

    #[tokio::test]
    async fn non_operator_caller_is_refused() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let ctx = TurnContext {
            session_key: SessionKey::Ephemeral {
                agent_id: "main".to_string(),
                ephemeral_id: "terminal-guest-test".to_string(),
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                TerminalTool
                    .call(TerminalArgs {
                        action: TerminalAction::List,
                        session_id: None,
                    })
                    .await
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("operator"), "{}", out.message);
    }

    #[tokio::test]
    async fn read_without_session_id_is_refused_not_panicking() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::Read,
                session_id: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("session_id"), "{}", out.message);
    }

    #[tokio::test]
    async fn read_of_unknown_session_is_no_such_session() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::Read,
                session_id: Some("does-not-exist".to_string()),
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("no such session"), "{}", out.message);
    }
}
