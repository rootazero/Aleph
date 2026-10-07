//! Read-only terminal observation tool.
//!
//! Observation itself lives in the borrowed `gateway::pty::runtime` adapter.
//! This module keeps only the tool schema, operator gate, envelope, and the
//! journal-specific compatibility adapter for `lost_with_restart`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::gateway::pty;
#[cfg(test)]
use crate::gateway::pty::runtime::WaitOutcome;
use crate::gateway::pty::runtime::{self, ObservationCaller, TerminalRuntime};
use crate::tools::AlephTool;
use tokio_util::sync::CancellationToken;

/// `terminal`'s five read-only actions. There is no write verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAction {
    /// List the caller's own PTY sessions: `session_id`, `shell`, `cwd`
    /// (where the shell was spawned; empty when it inherited the server's
    /// directory, and not updated by a later `cd`), `created_at` (epoch
    /// seconds), and `closed`.
    List,
    /// Read one session's current visible screen (no scrollback). Requires
    /// `session_id`.
    Read,
    /// Report each of the caller's sessions' detected agent state — the same
    /// table `runtime.agents.list` serves.
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
    /// Required for `read`, `wait`, and `explain`: the PTY `session_id` from `list`. Ignored for `list` and `status`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `wait` only: the states that end the wait. Defaults to
    /// `[blocked, idle]` — the two that mean "it wants you now".
    #[serde(default)]
    pub until: Option<Vec<aleph_protocol::runtime::RuntimeAgentState>>,
    /// `wait` only: how long to block, in milliseconds. Defaults to 60000 and
    /// is clamped to 150000, never refused — a blocking call has to return
    /// inside this tool's foreground budget.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "is_false")]
    pub lost_with_restart: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalRefusal {
    Message(String),
    LostWithRestart(crate::builtin_tools::process_journal::TombstoneReport),
}

impl From<String> for TerminalRefusal {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

fn caller_is_operator() -> bool {
    crate::tools::turn_context::current_turn_context().is_none_or(|ctx| ctx.caller_is_operator())
}

fn refusal_from_error(error: crate::tools::service::ToolError) -> TerminalRefusal {
    let message = match error {
        crate::tools::service::ToolError::Execution { cause, .. }
        | crate::tools::service::ToolError::ValidationFailed { cause, .. } => cause,
        other => other.to_string(),
    };
    TerminalRefusal::Message(message)
}

#[derive(Clone, Default)]
pub struct TerminalTool;

#[async_trait]
impl AlephTool for TerminalTool {
    const NAME: &'static str = "terminal";
    const DESCRIPTION: &'static str = "Read-only view of the terminal sessions you own on this server; empty when the embedded terminal is disabled in policy. It lists sessions, reads the current visible screen, and reports each agent's detected state (working / blocked / idle / unknown). It cannot type into a terminal or run commands — a human does that. `wait` blocks until one session reaches a state instead of polling `status`, and answers `timeout` with the current entry rather than a guess. `explain` says why a state was reported — which manifest rule matched, over which screen text and terminal title — which is the only way to tell a wrong detection from a truly idle agent.";
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
            let message = "terminal requires operator; refused. An operator approving this call's own escalation card does not currently lift this refusal — nothing re-stamps the caller's role after approval.".to_owned();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(TerminalOutput {
                success: false,
                message,
                data: None,
                lost_with_restart: false,
            });
        }

        let actor = crate::gateway::visibility::ambient_actor();
        let caller = ObservationCaller::Tool {
            actor: actor.clone(),
        };
        let runtime = TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents());
        let result = match args.action {
            TerminalAction::List => serde_json::to_value(runtime.list(&caller))
                .map_err(|error| TerminalRefusal::Message(format!("encode failed: {error}"))),
            TerminalAction::Status => serde_json::to_value(runtime.status(&caller))
                .map_err(|error| TerminalRefusal::Message(format!("encode failed: {error}"))),
            TerminalAction::Read => {
                read_session_with(&runtime, &caller, args.session_id.as_deref())
            }
            TerminalAction::Wait => {
                wait_for_session_with(
                    &runtime,
                    &caller,
                    args.session_id.as_deref(),
                    args.until.as_deref(),
                    args.timeout_ms,
                )
                .await
            }
            TerminalAction::Explain => {
                explain_session_with(&runtime, &caller, args.session_id.as_deref())
            }
        };

        match result {
            Ok(data) => {
                notify_tool_result(Self::NAME, action_label, true);
                Ok(TerminalOutput {
                    success: true,
                    message: action_label.to_owned(),
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
                let output = lost_with_restart_output(report);
                notify_tool_result(Self::NAME, &output.message, false);
                Ok(output)
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

fn to_json<T: Serialize>(value: T) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    serde_json::to_value(value)
        .map_err(|error| TerminalRefusal::Message(format!("encode failed: {error}")))
}

/// Resolve only the adapter-specific tombstone path. Live-session ownership
/// is admitted exactly once by `TerminalRuntime`; doing it here as well would
/// make the tool face carry a second copy of that predicate.
fn session_id_or_tombstone<'a>(
    session_id: Option<&'a str>,
    actor: Option<&str>,
    action: &str,
) -> std::result::Result<&'a str, TerminalRefusal> {
    let id = session_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| TerminalRefusal::Message(format!("{action} requires `session_id`")))?;
    if matches!(pty::manager().owner_of(id), pty::SessionOwner::Unknown) {
        if let Some(job) = crate::builtin_tools::process_journal::lookup_pty(id) {
            if runtime::terminal_admits(Some(job.record.owner.as_str()), actor) {
                if let Some(report) = crate::builtin_tools::process_journal::tombstone_report(&job)
                {
                    return Err(TerminalRefusal::LostWithRestart(report));
                }
            }
        }
    }
    Ok(id)
}

fn read_session_with(
    runtime: &TerminalRuntime<'_>,
    caller: &ObservationCaller,
    session_id: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let id = session_id_or_tombstone(session_id, caller_actor(caller), "read")?;
    runtime
        .read(caller, id)
        .map(to_json)
        .map_err(refusal_from_error)?
}

async fn wait_for_session_with(
    runtime: &TerminalRuntime<'_>,
    caller: &ObservationCaller,
    session_id: Option<&str>,
    until: Option<&[aleph_protocol::runtime::RuntimeAgentState]>,
    timeout_ms: Option<u64>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let id = session_id_or_tombstone(session_id, caller_actor(caller), "wait")?;
    let params = aleph_protocol::terminal::TerminalWaitParams {
        session_id: id.to_owned(),
        until: until.map(<[_]>::to_vec),
        timeout_ms,
    };
    runtime
        .wait(caller, &params, CancellationToken::new())
        .await
        .map(to_json)
        .map_err(refusal_from_error)?
}

fn explain_session_with(
    runtime: &TerminalRuntime<'_>,
    caller: &ObservationCaller,
    session_id: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let id = session_id_or_tombstone(session_id, caller_actor(caller), "explain")?;
    runtime
        .explain(caller, id)
        .map(to_json)
        .map_err(refusal_from_error)?
}

fn caller_actor(caller: &ObservationCaller) -> Option<&str> {
    match caller {
        ObservationCaller::Gateway { actor } | ObservationCaller::Tool { actor } => {
            actor.as_deref()
        }
    }
}

// Compatibility adapters for the existing terminal tests and the old tool
// face. They delegate all observation and waiting to the borrowed runtime.
#[cfg(test)]
fn list_sessions(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    to_json(
        TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents()).list(
            &ObservationCaller::Tool {
                actor: actor.map(str::to_owned),
            },
        ),
    )
    .map_err(|error| match error {
        TerminalRefusal::Message(message) => message,
        TerminalRefusal::LostWithRestart(report) => report.text,
    })
}

#[cfg(test)]
fn status(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    to_json(
        TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents()).status(
            &ObservationCaller::Tool {
                actor: actor.map(str::to_owned),
            },
        ),
    )
    .map_err(|error| match error {
        TerminalRefusal::Message(message) => message,
        TerminalRefusal::LostWithRestart(report) => report.text,
    })
}

#[cfg(test)]
fn read_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    read_session_with(
        &TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents()),
        &caller,
        session_id,
    )
}

#[cfg(test)]
async fn wait_for_session(
    session_id: Option<&str>,
    until: Option<&[aleph_protocol::runtime::RuntimeAgentState]>,
    timeout_ms: Option<u64>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    wait_for_session_with(
        &TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents()),
        &caller,
        session_id,
        until,
        timeout_ms,
    )
    .await
}

#[cfg(test)]
fn explain_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    explain_session_with(
        &TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents()),
        &caller,
        session_id,
    )
}

#[cfg(test)]
async fn wait_for_state(
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    until: &[aleph_protocol::runtime::RuntimeAgentState],
    window: std::time::Duration,
) -> WaitOutcome {
    runtime::wait_for_state(
        pty::manager(),
        table,
        session_id,
        until,
        window,
        CancellationToken::new(),
    )
    .await
    .expect("uncancelled compatibility wait")
}

#[cfg(test)]
const WAIT_DEFAULT_TIMEOUT_MS: u64 = 60_000;
#[cfg(test)]
const WAIT_MAX_TIMEOUT_MS: u64 = 150_000;
#[cfg(test)]
fn wait_window(requested: Option<u64>) -> std::time::Duration {
    runtime::wait_window(requested)
}

#[cfg(test)]
fn explain_detection(
    session_id: &str,
    agent: Option<agent_detect::Agent>,
    sampled: Option<&aleph_protocol::runtime::RuntimeAgentEntry>,
    screen: &crate::gateway::pty::manager::DetectionInputs,
) -> aleph_protocol::terminal::TerminalExplainResponse {
    runtime::explain_response(session_id, agent, sampled, screen)
}

#[cfg(test)]
mod tests;
