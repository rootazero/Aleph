//! Canonical `terminal_sessions_*` observation capabilities.
//!
//! Six read-only capabilities (`list`, `read`, `status`, `wait`, `explain`,
//! `attach`), one registry entry each, replace the former single `terminal`
//! tool and its `action` selector. All six handlers funnel through the one
//! [`invoke_observation`] entrypoint, so the operator gate, the envelope and
//! the tombstone adapter exist exactly once.
//!
//! The legacy `terminal{action}` shape survives only as a compatibility
//! rewrite on the `tools.invoke` RPC ([`normalize_terminal_compat_call`]); it
//! is not registered as a second identity.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    attach_session_with, explain_session_with, lost_with_restart_output, read_session_with,
    to_json, wait_for_session_with, TerminalOutput, TerminalRefusal,
};
use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::gateway::pty;
use crate::gateway::pty::runtime::{ObservationCaller, TerminalRuntime};
use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
use crate::tools::{ToolError, ToolHandlerRegistry, ToolRegistrationScope};

/// The legacy tool name the compatibility rewrite recognises.
const LEGACY_NAME: &str = "terminal";

/// Prefix shared by the six canonical names.
const CANONICAL_PREFIX: &str = "terminal_sessions_";

/// One canonical verb. The single place the verb ↔ name ↔ schema mapping is
/// derived from; the registration table, the compat rewrite and the dispatcher
/// all read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    List,
    Read,
    Status,
    Wait,
    Explain,
    Attach,
}

impl Verb {
    const ALL: [Verb; 6] = [
        Verb::List,
        Verb::Read,
        Verb::Status,
        Verb::Wait,
        Verb::Explain,
        Verb::Attach,
    ];

    fn label(self) -> &'static str {
        match self {
            Verb::List => "list",
            Verb::Read => "read",
            Verb::Status => "status",
            Verb::Wait => "wait",
            Verb::Explain => "explain",
            Verb::Attach => "attach",
        }
    }

    fn canonical_name(self) -> String {
        format!("{CANONICAL_PREFIX}{}", self.label())
    }

    fn from_canonical(name: &str) -> Option<Verb> {
        let label = name.strip_prefix(CANONICAL_PREFIX)?;
        Self::ALL.into_iter().find(|verb| verb.label() == label)
    }

    /// The five verbs the legacy `action` selector could name. `attach` never
    /// was one, so the compatibility rewrite must not mint it.
    fn from_legacy_action(action: &str) -> Option<Verb> {
        match action {
            "list" => Some(Verb::List),
            "read" => Some(Verb::Read),
            "status" => Some(Verb::Status),
            "wait" => Some(Verb::Wait),
            "explain" => Some(Verb::Explain),
            _ => None,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Verb::List => "List the terminal sessions you own on this server: `session_id`, `shell`, `cwd` (where the shell was spawned; empty when it inherited the server's directory, and not updated by a later `cd`), `created_at` (epoch seconds), and `closed`. Read-only; empty when the embedded terminal is disabled in policy. It cannot type into a terminal or run commands — a human does that.",
            Verb::Read => "Read one terminal session's current visible screen (no scrollback). Requires `session_id`. Read-only; it cannot type into a terminal or run commands — a human does that.",
            Verb::Status => "Report each of your terminal sessions' detected agent state (working / blocked / idle / unknown) — the same table `runtime.agents.list` serves. Read-only; use `terminal_sessions_wait` instead of polling this.",
            Verb::Wait => "Block until one terminal session's agent state enters `until`, then return it. Requires `session_id`. Answers `timeout` with the current entry at `timeout_ms` rather than a guess, and `gone` if the session ends first. Read-only.",
            Verb::Explain => "Explain one terminal session's detected agent state: which manifest rule matched, at which manifest version, over which screen text and terminal title. Requires `session_id`. The only way to tell a wrong detection from a truly idle agent. Read-only.",
            Verb::Attach => "Return an attach snapshot of one terminal session you own. Requires `session_id`. Read-only observation; it cannot type into the terminal.",
        }
    }

    fn input_schema(self) -> Value {
        let session_id = json!({
            "type": "string",
            "description": "Required: the PTY `session_id` from `terminal_sessions_list`."
        });
        match self {
            Verb::List | Verb::Status => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Verb::Read | Verb::Explain | Verb::Attach => json!({
                "type": "object",
                "properties": { "session_id": session_id },
                "required": ["session_id"],
                "additionalProperties": false
            }),
            Verb::Wait => json!({
                "type": "object",
                "properties": {
                    "session_id": session_id,
                    "until": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["working", "blocked", "idle", "unknown"],
                            "description": "Detected state: working, blocked, idle, or unknown."
                        },
                        "description": "The states that end the wait. Defaults to `[blocked, idle]` — the two that mean \"it wants you now\"."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "How long to block, in milliseconds. Defaults to 60000 and is clamped to 150000, never refused — a blocking call has to return inside this tool's foreground budget."
                    }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }),
        }
    }
}

/// Arguments shared by the six verbs; each reads only the fields it documents.
#[derive(Debug, Default, serde::Deserialize)]
struct ObservationArgs {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    until: Option<Vec<aleph_protocol::runtime::RuntimeAgentState>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn validation(name: &str, cause: impl Into<String>) -> ToolError {
    ToolError::ValidationFailed {
        name: name.to_owned(),
        cause: cause.into(),
    }
}

/// Rewrite a legacy `terminal{action, ...}` call to its canonical form.
///
/// Pure. Only the literal name `terminal` is touched: the five read actions map
/// to `terminal_sessions_<action>` and the `action` key is stripped, every
/// other key passes through. A missing / non-string / unknown action (including
/// `attach` and any write verb) or a non-object input is refused rather than
/// guessed at. Every other name is returned unchanged.
pub fn normalize_terminal_compat_call(
    name: &str,
    input: Value,
) -> Result<(String, Value), ToolError> {
    if name != LEGACY_NAME {
        return Ok((name.to_owned(), input));
    }
    let Value::Object(mut args) = input else {
        return Err(validation(
            LEGACY_NAME,
            "arguments must be an object with an `action`",
        ));
    };
    let verb = match args.remove("action") {
        Some(Value::String(action)) => Verb::from_legacy_action(&action).ok_or_else(|| {
            validation(
                LEGACY_NAME,
                format!(
                    "unknown action `{action}`; expected one of list, read, status, wait, explain"
                ),
            )
        })?,
        Some(_) => return Err(validation(LEGACY_NAME, "`action` must be a string")),
        None => return Err(validation(LEGACY_NAME, "missing `action`")),
    };
    Ok((verb.canonical_name(), Value::Object(args)))
}

/// True for the six canonical observation capability names.
#[must_use]
pub(crate) fn is_observation_capability(name: &str) -> bool {
    Verb::from_canonical(name).is_some()
}

/// Whether the agent's tool policy admits `tool_name`, honouring the legacy
/// `terminal` entry for the five observation verbs that used to be reachable
/// through the `terminal{action}` selector.
///
/// This is the one place the compatibility answer is derived; `tools.invoke`,
/// `tools.effective` and `AllowlistToolService` all call it so they cannot
/// drift. For `terminal_sessions_{list,read,status,wait,explain}` a request-local
/// copy of the policy has its flat `terminal` entries (allowed and denied)
/// rewritten to the canonical name, and the existing
/// [`AgentDef::is_tool_allowed`] (deny-first, recursion guard, named sets,
/// wildcard) decides — no second algorithm, nothing persisted, registry
/// untouched. `attach` (never a legacy action), unknown `terminal_sessions_*`
/// names and every other tool are checked verbatim; no prefix aliasing.
#[must_use]
pub(crate) fn is_tool_allowed_with_legacy_terminal_alias(
    agent_def: &crate::agents::AgentDef,
    tool_name: &str,
) -> bool {
    let legacy_aliased = Verb::from_canonical(tool_name)
        .is_some_and(|verb| Verb::from_legacy_action(verb.label()) == Some(verb));
    if !legacy_aliased {
        return agent_def.is_tool_allowed(tool_name);
    }
    let mut local = agent_def.clone();
    for entry in local
        .allowed_tools
        .iter_mut()
        .chain(local.denied_tools.iter_mut())
        .filter(|entry| entry.as_str() == LEGACY_NAME)
    {
        *entry = tool_name.to_owned();
    }
    local.is_tool_allowed(tool_name)
}

/// The operator gate every observation shares: an absent turn context is
/// trusted (cron / internal / local no-auth daemon), exactly like every other
/// operator gate.
pub(crate) fn require_operator_caller() -> Result<(), ToolError> {
    let allowed = crate::tools::turn_context::current_turn_context()
        .is_none_or(|ctx| ctx.caller_is_operator());
    if allowed {
        Ok(())
    } else {
        Err(ToolError::PermissionDenied {
            name: LEGACY_NAME.to_owned(),
            reason: REFUSAL_MESSAGE.to_owned(),
        })
    }
}

const REFUSAL_MESSAGE: &str = "terminal requires operator; refused. An operator approving this call's own escalation card does not currently lift this refusal — nothing re-stamps the caller's role after approval.";

fn envelope(output: TerminalOutput) -> Result<ToolOutput, ToolError> {
    let value = serde_json::to_value(output).map_err(|error| ToolError::Execution {
        name: LEGACY_NAME.to_owned(),
        cause: format!("encode failed: {error}"),
    })?;
    Ok(ToolOutput {
        value,
        metadata: ToolOutputMetadata::default(),
    })
}

/// The single entrypoint behind all six registered handlers.
///
/// Operator check first — before parsing `input`, before touching the PTY
/// manager — so a refusal never carries data. A refusal is answered with the
/// legacy `TerminalOutput` envelope as an `Ok` value (the shape callers of the
/// former `terminal` tool already parse), not an `Err`.
pub(crate) async fn invoke_observation(name: &str, input: Value) -> Result<ToolOutput, ToolError> {
    if require_operator_caller().is_err() {
        notify_tool_result(name, REFUSAL_MESSAGE, false);
        return envelope(TerminalOutput {
            success: false,
            message: REFUSAL_MESSAGE.to_owned(),
            data: None,
            lost_with_restart: false,
        });
    }

    let verb = Verb::from_canonical(name)
        .ok_or_else(|| validation(name, "not a terminal observation capability"))?;
    let label = verb.label();
    notify_tool_start(name, label);

    let args: ObservationArgs = serde_json::from_value(input)
        .map_err(|error| validation(name, format!("invalid arguments: {error}")))?;

    let caller = ObservationCaller::Tool {
        actor: crate::gateway::visibility::ambient_actor(),
    };
    let runtime = TerminalRuntime::new(pty::manager(), crate::gateway::runtime::agents());
    let session_id = args.session_id.as_deref();
    let result = match verb {
        Verb::List => to_json(runtime.list(&caller)),
        Verb::Status => to_json(runtime.status(&caller)),
        Verb::Read => read_session_with(&runtime, &caller, session_id),
        Verb::Wait => {
            wait_for_session_with(
                &runtime,
                &caller,
                session_id,
                args.until.as_deref(),
                args.timeout_ms,
            )
            .await
        }
        Verb::Explain => explain_session_with(&runtime, &caller, session_id),
        Verb::Attach => attach_session_with(&runtime, &caller, session_id),
    };

    match result {
        Ok(data) => {
            notify_tool_result(name, label, true);
            envelope(TerminalOutput {
                success: true,
                message: label.to_owned(),
                data: Some(data),
                lost_with_restart: false,
            })
        }
        Err(TerminalRefusal::Message(message)) => {
            notify_tool_result(name, &message, false);
            envelope(TerminalOutput {
                success: false,
                message,
                data: None,
                lost_with_restart: false,
            })
        }
        Err(TerminalRefusal::LostWithRestart(report)) => {
            let output = lost_with_restart_output(report);
            notify_tool_result(name, &output.message, false);
            envelope(output)
        }
    }
}

/// One of the six registered handlers. Holds only its verb; the body is the
/// shared [`invoke_observation`].
struct ObservationHandler {
    verb: Verb,
}

#[async_trait]
impl ToolHandler for ObservationHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        invoke_observation(&self.verb.canonical_name(), input).await
    }

    fn definition(&self) -> ToolDefinition {
        let name = self.verb.canonical_name();
        // Same derivations `BuiltinHandler` uses: the read-only list is the
        // single authority on idempotency / concurrency, the budget table (or
        // its default) on the foreground duration.
        let idempotent = crate::tools::retry::is_idempotent_builtin_name(&name);
        let max_duration_ms = crate::tools::budget::resolve_tool_budget_ms(&name, None);
        ToolDefinition {
            description: self.verb.description().to_owned(),
            input_schema: self.verb.input_schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                hidden_from_llm: false,
                requires_approval: false,
                tags: Vec::new(),
                idempotent,
                max_duration_ms: Some(max_duration_ms),
                concurrent_safe: idempotent,
            },
            name,
        }
    }

    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        crate::tools::adapters::builtin_concurrency_claim(&self.verb.canonical_name(), input)
    }
}

/// Register the six canonical observation capabilities on the canonical
/// registry, tracking each registration in `scope` so teardown unregisters
/// exactly what was registered.
pub fn register_observation_capabilities(
    registry: &ToolHandlerRegistry,
    scope: &mut ToolRegistrationScope,
) -> Result<(), ToolError> {
    for verb in Verb::ALL {
        let handler: Arc<dyn ToolHandler> = Arc::new(ObservationHandler { verb });
        let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
        let handle = registry.register(descriptor, handler)?;
        scope.track(handle);
    }
    Ok(())
}

#[cfg(test)]
mod legacy_alias_tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};

    fn def(allowed: &[&str], denied: &[&str]) -> AgentDef {
        AgentDef::new("a", AgentMode::Primary)
            .with_allowed_tools(allowed.iter().map(|s| (*s).to_owned()).collect())
            .with_denied_tools(denied.iter().map(|s| (*s).to_owned()).collect())
    }

    /// The alias set is derived from the Verb table: exactly the five legacy
    /// actions, never `attach`, and never an unknown `terminal_sessions_*`.
    #[test]
    fn terminal_capability_alias_covers_exactly_the_five_legacy_verbs() {
        let allow_terminal = def(&["terminal"], &[]);
        for verb in Verb::ALL {
            let name = verb.canonical_name();
            let expected = verb != Verb::Attach;
            assert_eq!(
                is_tool_allowed_with_legacy_terminal_alias(&allow_terminal, &name),
                expected,
                "{name}"
            );
        }
        // A future verb must not be admitted by prefix amplification.
        assert!(!is_tool_allowed_with_legacy_terminal_alias(
            &allow_terminal,
            "terminal_sessions_kill"
        ));
        let deny_terminal = def(&["*"], &["terminal"]);
        assert!(is_tool_allowed_with_legacy_terminal_alias(
            &deny_terminal,
            "terminal_sessions_kill"
        ));
        assert!(is_tool_allowed_with_legacy_terminal_alias(
            &deny_terminal,
            "terminal_sessions_attach"
        ));
        assert!(!is_tool_allowed_with_legacy_terminal_alias(
            &deny_terminal,
            "terminal_sessions_wait"
        ));
    }

    /// The raw policy is left byte-identical (the rewrite is request-local).
    #[test]
    fn terminal_capability_alias_does_not_mutate_the_policy() {
        let agent = def(&["*", "terminal"], &["terminal"]);
        let before = serde_json::to_vec(&agent).unwrap();
        let _ = is_tool_allowed_with_legacy_terminal_alias(&agent, "terminal_sessions_list");
        assert_eq!(serde_json::to_vec(&agent).unwrap(), before);
        assert_eq!(agent.denied_tools, vec!["terminal".to_owned()]);
    }
}
