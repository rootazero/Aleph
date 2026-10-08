//! Read-only terminal observation capabilities.
//!
//! Observation itself lives in the borrowed `gateway::pty::runtime` adapter.
//! This module keeps the shared envelope, the tombstone adapter for
//! `lost_with_restart`, and the per-verb action bodies; the canonical
//! `terminal_sessions_*` registrations, the operator gate and the legacy
//! `terminal` compatibility rewrite live in [`capabilities`].

use serde::{Deserialize, Serialize};

use crate::gateway::pty;
#[cfg(test)]
use crate::gateway::pty::runtime::WaitOutcome;
use crate::gateway::pty::runtime::{self, ObservationCaller, TerminalRuntime};
use tokio_util::sync::CancellationToken;

pub mod capabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_false")]
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

fn refusal_from_error(error: crate::tools::service::ToolError) -> TerminalRefusal {
    let message = match error {
        crate::tools::service::ToolError::Execution { cause, .. }
        | crate::tools::service::ToolError::ValidationFailed { cause, .. } => cause,
        other => other.to_string(),
    };
    TerminalRefusal::Message(message)
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

fn attach_session_with(
    runtime: &TerminalRuntime<'_>,
    caller: &ObservationCaller,
    session_id: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let id = session_id_or_tombstone(session_id, caller_actor(caller), "attach")?;
    runtime
        .attach(caller, id)
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

// Compatibility adapters for the existing terminal tests. They delegate all observation and waiting to the borrowed runtime.
#[cfg(test)]
fn list_sessions(
    manager: &pty::PtyManager,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    to_json(
        TerminalRuntime::new(manager, crate::gateway::runtime::agents()).list(
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
fn status(
    manager: &pty::PtyManager,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    to_json(
        TerminalRuntime::new(manager, crate::gateway::runtime::agents()).status(
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
    manager: &pty::PtyManager,
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    read_session_with(
        &TerminalRuntime::new(manager, crate::gateway::runtime::agents()),
        &caller,
        session_id,
    )
}

#[cfg(test)]
async fn wait_for_session(
    manager: &pty::PtyManager,
    session_id: Option<&str>,
    until: Option<&[aleph_protocol::runtime::RuntimeAgentState]>,
    timeout_ms: Option<u64>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    wait_for_session_with(
        &TerminalRuntime::new(manager, crate::gateway::runtime::agents()),
        &caller,
        session_id,
        until,
        timeout_ms,
    )
    .await
}

#[cfg(test)]
fn explain_session(
    manager: &pty::PtyManager,
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, TerminalRefusal> {
    let caller = ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    };
    explain_session_with(
        &TerminalRuntime::new(manager, crate::gateway::runtime::agents()),
        &caller,
        session_id,
    )
}

#[cfg(test)]
async fn wait_for_state(
    manager: &pty::PtyManager,
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    until: &[aleph_protocol::runtime::RuntimeAgentState],
    window: std::time::Duration,
) -> WaitOutcome {
    runtime::wait_for_state(
        manager,
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
