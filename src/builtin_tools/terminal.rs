//! `TerminalTool` — read-only view of the terminal sessions the caller owns.
//!
//! Five actions, no write verb. Observations delegate to
//! [`TerminalRuntime`]; this boundary retains schema, operator gating, the
//! output envelope and the execution-journal restart adapter.
//!
//! The RPC/event faces are operator-only too. The inline check MUST remain:
//! `ScopedToolService` approval does not re-stamp `TurnContext::caller_role`,
//! so an approved member call is still refused today. The decided fix is a
//! per-call approval seam, not deleting this check. `tools.invoke` has no
//! `TurnContext` and relies on its own dispatch operator gate.
//!
//! Identified callers see only exact-owner sessions. An actorless tool admits
//! only known-unowned sessions, unlike the unrestricted actorless gateway.
//! Unknown ownership is denied on the tool face. Addressed refusals say
//! `no such session` to avoid an ownership oracle. The journal adapter may
//! instead return `lost_with_restart`, only to the admitted recorded owner.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::gateway::pty;
use crate::gateway::pty::runtime::{ObservationCaller, TerminalRuntime};
use crate::tools::AlephTool;

/// `terminal`'s five read-only actions. There is no write verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAction {
    /// List the caller's own PTY sessions: `session_id`, `shell`, `cwd`
    /// (where the shell was SPAWNED — empty when it inherited the
    /// server's, and not updated by a later `cd`), `created_at` (epoch
    /// seconds) and `closed`.
    List,
    /// Read one session's current visible screen (no scrollback). Requires
    /// `session_id`.
    Read,
    /// Report each of the caller's sessions' detected agent state — the
    /// same table `runtime.agents.list` serves.
    Status,
    /// Block until one session's agent state enters `until`, then return it.
    /// Requires `session_id`. Answers `timeout` with the current entry at
    /// `timeout_ms`, and `gone` if the session ends first.
    Wait,
    /// Explain one session's detected state: which manifest rule matched, at
    /// which manifest version, over which screen inputs. Requires
    /// `session_id`.
    Explain,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TerminalArgs {
    /// What to do.
    pub action: TerminalAction,
    /// Required for `read` / `wait` / `explain`: the PTY session id (from
    /// `list`'s output). Ignored for `list` / `status`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `wait` only: the states that end the wait. Defaults to
    /// `["blocked", "idle"]` — the two that mean "it wants you now".
    #[serde(default)]
    pub until: Option<Vec<aleph_protocol::runtime::RuntimeAgentState>>,
    /// `wait` only: how long to block, in milliseconds. Defaults to 60000 and
    /// is CLAMPED to 150000, never refused — a blocking call has to return
    /// inside this harness's foreground tool budget.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// The unchanged outer envelope for all actions, including restart refusals.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
    /// True only for a session a previous server process owned. Skipped when
    /// false so ordinary envelopes keep their existing shape.
    #[serde(skip_serializing_if = "is_false")]
    pub lost_with_restart: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalRefusal {
    Message(String),
    LostWithRestart(crate::builtin_tools::process_journal::TombstoneReport),
}

impl From<String> for TerminalRefusal {
    fn from(m: String) -> Self {
        Self::Message(m)
    }
}

/// Absent role reads as operator, matching the other inline cross-cutting gates.
fn caller_is_operator() -> bool {
    crate::tools::turn_context::current_turn_context().is_none_or(|ctx| ctx.caller_is_operator())
}

#[derive(Clone, Default)]
pub struct TerminalTool;

#[async_trait]
impl AlephTool for TerminalTool {
    const NAME: &'static str = "terminal";
    const DESCRIPTION: &'static str = "Read-only view of the terminal sessions you own on this \
        server; empty when the embedded terminal is disabled in policy. Lists sessions, reads \
        the current visible screen, and reports each agent's detected state (working / blocked \
        / idle / unknown). It cannot type into a terminal or run commands — a human does that. \
        `wait` blocks until one session reaches a state instead of polling `status`, and \
        answers `timeout` with the current entry rather than a guess. `explain` says WHY a \
        state was reported — which manifest rule matched, over which screen text and terminal \
        title — which is the only way to tell a wrong detection from an idle agent.";

    type Args = TerminalArgs;
    type Output = TerminalOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let action_label = match args.action {
            TerminalAction::List => "list",
            TerminalAction::Read => "read",
            TerminalAction::Status => "status",
            TerminalAction::Wait => "wait",
            TerminalAction::Explain => "explain",
        };
        notify_tool_start(Self::NAME, action_label);

        if !caller_is_operator() {
            // Approval does not re-stamp the role today. Keep the inline gate.
            let message = "terminal requires operator; refused. An operator approving this \
                call's own escalation card does not currently lift this refusal — nothing \
                re-stamps the caller's role after approval."
                .to_string();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(TerminalOutput {
                success: false,
                message,
                data: None,
                lost_with_restart: false,
            });
        }

        let actor = crate::gateway::visibility::ambient_actor();
        let result = match args.action {
            TerminalAction::List => list_sessions(actor.as_deref()).map_err(TerminalRefusal::from),
            TerminalAction::Status => status(actor.as_deref()).map_err(TerminalRefusal::from),
            TerminalAction::Read => read_session(args.session_id.as_deref(), actor.as_deref()),
            TerminalAction::Wait => {
                wait_for_session(
                    args.session_id.as_deref(),
                    args.until.as_deref(),
                    args.timeout_ms,
                    actor.as_deref(),
                )
                .await
            }
            TerminalAction::Explain => {
                explain_session(args.session_id.as_deref(), actor.as_deref())
            }
        };

        match result {
            Ok(data) => {
                notify_tool_result(Self::NAME, action_label, true);
                Ok(TerminalOutput {
                    success: true,
                    message: action_label.to_string(),
                    data: Some(data),
                    lost_with_restart: false,
                })
            }
            Err(TerminalRefusal::Message(message)) => {
                notify_tool_result(Self::NAME, &message, false);
                Ok(TerminalOutput {
                    success: false,
                    message,
                    data: None,
                    lost_with_restart: false,
                })
            }
            Err(TerminalRefusal::LostWithRestart(report)) => {
                let out = lost_with_restart_output(report);
                notify_tool_result(Self::NAME, &out.message, false);
                Ok(out)
            }
        }
    }
}

fn lost_with_restart_output(
    report: crate::builtin_tools::process_journal::TombstoneReport,
) -> TerminalOutput {
    TerminalOutput {
        success: false,
        message: report.text_with_output(),
        data: Some(serde_json::json!({
            "tombstone": report.kind,
            "pid": report.pid,
            "stop_command": report.stop_command,
        })),
        lost_with_restart: true,
    }
}

fn runtime() -> TerminalRuntime<'static> {
    TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents())
}

// Boundary adapters only: every observation constructs the shared DTO in runtime.
fn list_sessions(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    serde_json::to_value(runtime().list(ObservationCaller::Tool { actor }))
        .map_err(|e| format!("encode failed: {e}"))
}

fn status(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    serde_json::to_value(runtime().status(ObservationCaller::Tool { actor }))
        .map_err(|e| format!("encode failed: {e}"))
}

fn read_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let session_id = owned_session_id(session_id, actor, "read")?;
    let body = runtime().read(session_id, ObservationCaller::Tool { actor })?;
    serde_json::to_value(body).map_err(|e| format!("encode failed: {e}").into())
}

/// Keep argument errors and the tool-specific journal adapter outside runtime.
fn owned_session_id<'a>(
    session_id: Option<&'a str>,
    actor: Option<&str>,
    action: &str,
) -> std::result::Result<&'a str, TerminalRefusal> {
    let session_id = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{action} requires `session_id`"))?;
    let caller = ObservationCaller::Tool { actor };
    if runtime().require_owned(session_id, caller).is_ok() {
        return Ok(session_id);
    }
    if let Some(job) = crate::builtin_tools::process_journal::lookup_pty(session_id) {
        if caller.admits(&pty::SessionOwner::Known(Some(job.record.owner.clone()))) {
            if let Some(report) = crate::builtin_tools::process_journal::tombstone_report(&job) {
                return Err(TerminalRefusal::LostWithRestart(report));
            }
        }
    }
    Err(TerminalRefusal::Message(pty::no_such_session(session_id)))
}

async fn wait_for_session(
    session_id: Option<&str>,
    until: Option<&[aleph_protocol::runtime::RuntimeAgentState]>,
    timeout_ms: Option<u64>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let session_id = owned_session_id(session_id, actor, "wait")?;
    let body = runtime()
        .wait(
            session_id,
            ObservationCaller::Tool { actor },
            until,
            timeout_ms,
            &CancellationToken::new(),
        )
        .await?;
    serde_json::to_value(body).map_err(|e| format!("encode failed: {e}").into())
}

fn explain_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let session_id = owned_session_id(session_id, actor, "explain")?;
    let body = runtime().explain(session_id, ObservationCaller::Tool { actor })?;
    serde_json::to_value(body).map_err(|e| format!("encode failed: {e}").into())
}

// Legacy tests exercise the same implementation with a never-cancelled token.
#[cfg(test)]
use crate::gateway::pty::runtime::{
    explain_detection, wait_window, WaitOutcome, WAIT_DEFAULT_TIMEOUT_MS, WAIT_MAX_TIMEOUT_MS,
};

#[cfg(test)]
async fn wait_for_state(
    manager: &pty::PtyManager,
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    until: &[aleph_protocol::runtime::RuntimeAgentState],
    window: std::time::Duration,
) -> WaitOutcome {
    TerminalRuntime::new(manager, table)
        .wait_for_state(session_id, until, window, &CancellationToken::new())
        .await
        .expect("legacy wait helper uses a never-cancelled token")
}

#[cfg(test)]
fn session_is_registered(session_id: &str) -> bool {
    runtime().session_is_registered(session_id)
}

#[cfg(test)]
mod tests;
