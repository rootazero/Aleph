//! Signed-ledger recording at the tool chokepoint.
//!
//! [`ScopedToolService::execute_inner`](super::ScopedToolService) is the single
//! production path into the tool registry — every gate sits above it and
//! nothing reaches a tool without passing it. That makes it the only place a
//! record of "what this agent actually did" can be written without the record
//! being structurally incomplete, which is why the ledger hangs here and not
//! off a parallel surface. (The last audit store Aleph shipped queried three
//! tables whose only writers were test helpers; an operator ran the report and
//! saw zeros. It was deleted for being worse than dead code.)
//!
//! ## What is recorded, and what is not
//!
//! * **Mutating calls** — recorded on completion, success or failure. "Mutating"
//!   is the tool's own DECLARED `is_idempotent` metadata read through
//!   [`ScopedToolService::tool_facts`], the same seam the exec tier consults.
//!   No name lists, no argument sniffing, no guess about intent (R7).
//! * **Refusals** — recorded whatever the tool's idempotence, because a refused
//!   attempt is the highest-signal thing an agent does.
//! * **Reads** — not recorded. A pure query changes nothing, and burying the
//!   consequential rows under a flood of `file_read`s is how an audit trail
//!   stops being read.
//!
//! Approval decisions are recorded separately, by
//! `ScopedToolService::record_approval_decision`, so the authority for a call
//! and the call itself are two facts rather than one conflated one.

use serde_json::Value;

use crate::identity::{LedgerAction, LedgerOutcome, NewRecord};
use crate::sandbox::exec_approval::{grant_fingerprint, redact_and_cap, summarize_call};
use crate::session::events::ToolOutput;
use crate::tools::service::ToolError;

use super::ScopedToolService;

/// Who is acting and whether this tool mutates — the two facts the ledger needs
/// that are *cheap* to determine.
///
/// The expensive parts (canonical-argument fingerprint, secret-masked summary)
/// are computed inside `commit_*`, i.e. **only when a record is actually
/// written**. Building them eagerly would run the secret masker's regex set on
/// every `file_read` just to throw the result away, and read-heavy turns are
/// the common case.
///
/// Built once per dispatch and only when a ledger is installed and an agent is
/// attributable, so an unwired build pays one `Option::is_none` per tool call.
pub(super) struct LedgerIntent {
    agent_id: String,
    tool: String,
    mutating: bool,
}

impl ScopedToolService {
    /// The agent a dispatch running under `turn` is recorded against — the
    /// **single source** for that question, shared by the call record and the
    /// approval record so the two can never disagree.
    ///
    /// Two sources, in order:
    ///
    /// 1. The **scoped actor** ([`crate::identity::actor`]), set by the
    ///    subagent spawner's wrapper. A delegated role runs on its parent's
    ///    service under the parent's `TURN_CONTEXT`, and `SessionKey::Subagent`
    ///    even resolves `agent_id()` to the parent — so without this the role's
    ///    work would be signed by whoever spawned it.
    /// 2. The turn's own session key, for every agent that owns its turn.
    ///
    /// Taking `turn` by argument rather than reading `self.turn_context` keeps
    /// the "no turn, no record" decision at the call sites that already have to
    /// make it, instead of adding an `Option` here that neither could hit. An
    /// unattributable action must not be filed under a guessed agent:
    /// `audit_identity`'s fallback to the literal `"main"` is a defensible
    /// default for a log line, and a forgery in a signed chain.
    pub(super) fn ledger_actor_for(turn: &crate::tools::turn_context::TurnContext) -> String {
        crate::identity::current_actor().unwrap_or_else(|| turn.session_key.agent_id().to_string())
    }

    /// The ledger intent for this call, or `None` when nothing is recordable —
    /// no ledger installed (unit tests, embedded use), or no turn to attribute
    /// the call to.
    pub(super) fn ledger_intent(&self, name: &str) -> Option<LedgerIntent> {
        // Bail out unless a ledger is installed — the handle itself is unused
        // here; `record_action` re-resolves it on the writer side.
        crate::identity::global()?;
        Some(LedgerIntent {
            agent_id: Self::ledger_actor_for(self.turn_context.as_ref()?),
            tool: name.to_string(),
            mutating: !self.tool_facts(name).idempotent,
        })
    }
}

impl LedgerIntent {
    /// Record the outcome of a call that actually reached the tool.
    ///
    /// `input` is the call as the **model issued it**, not the possibly
    /// hook-rewritten `effective_input`: a `BeforeToolCall` interceptor's
    /// `update_input:` is the system acting, not the agent, and fingerprinting
    /// the original is also what lets this record correlate with the approval
    /// that authorized it (the approval gate runs above the hooks).
    pub(super) async fn commit_execution(
        &self,
        input: &Value,
        result: &Result<ToolOutput, ToolError>,
    ) {
        if !self.mutating {
            return;
        }
        let summary = summarize_call(&self.tool, input);
        let (outcome, detail) = match result {
            Ok(_) => (LedgerOutcome::Ok, summary),
            Err(e) => (
                LedgerOutcome::Error,
                // The error text can echo arguments back, so it goes through
                // the same masker as the summary.
                format!("{summary} — {}", redact_and_cap(&e.to_string())),
            ),
        };
        self.emit(input, LedgerAction::ToolCall, outcome, detail)
            .await;
    }

    /// Record a call a gate refused before it ran. Recorded whatever the tool's
    /// idempotence — a refused attempt is the highest-signal thing an agent
    /// does, and refusals are rare enough that recording all of them cannot
    /// drown the log.
    pub(super) async fn commit_refusal(&self, input: &Value, reason: &str) {
        let detail = format!(
            "{} — {}",
            summarize_call(&self.tool, input),
            redact_and_cap(reason)
        );
        self.emit(
            input,
            LedgerAction::ToolDenied,
            LedgerOutcome::Denied,
            detail,
        )
        .await;
    }

    async fn emit(
        &self,
        input: &Value,
        action: LedgerAction,
        outcome: LedgerOutcome,
        detail: String,
    ) {
        crate::identity::record_action(NewRecord {
            agent_id: self.agent_id.clone(),
            action,
            target: self.tool.clone(),
            outcome,
            // Proves *which* call happened without storing the arguments. Same
            // fingerprint the session grant and the denial ledger key on, so a
            // record correlates with the approval that authorized it.
            args_fp: Some(grant_fingerprint(&self.tool, input)),
            detail,
        })
        .await;
    }
}

/// Which decision an approval gate reached, for
/// `ScopedToolService::record_approval_decision`.
///
/// Replaces the old `Option<&str>` "denial reason or nothing", which could not
/// express *who* approved: every approval was filed as
/// [`ApprovalSource::User`](crate::session::events::ApprovalSource) even when it
/// came from a grant taken earlier in the session rather than from a human
/// answering right now.
#[derive(Clone, Copy)]
pub(super) enum ApprovalRecord<'a> {
    /// A human answered this prompt.
    GrantedByUser,
    /// A grant taken earlier in this session satisfied the gate — nobody was
    /// asked. Wires [`ApprovalSource::Trusted`](crate::session::events::ApprovalSource),
    /// which had no producer at all before.
    GrantedBySessionMemory,
    Denied(&'a str),
}

impl<'a> ApprovalRecord<'a> {
    pub(super) const fn denial_reason(self) -> Option<&'a str> {
        match self {
            Self::Denied(reason) => Some(reason),
            _ => None,
        }
    }

    /// Who the session event should name as the approver.
    pub(super) const fn approval_source(self) -> crate::session::events::ApprovalSource {
        use crate::session::events::ApprovalSource;
        match self {
            Self::GrantedBySessionMemory => ApprovalSource::Trusted,
            _ => ApprovalSource::User,
        }
    }

    pub(super) const fn ledger_action(self) -> LedgerAction {
        match self {
            Self::Denied(_) => LedgerAction::ApprovalDenied,
            _ => LedgerAction::ApprovalGranted,
        }
    }

    pub(super) const fn ledger_outcome(self) -> LedgerOutcome {
        match self {
            Self::Denied(_) => LedgerOutcome::Denied,
            _ => LedgerOutcome::Ok,
        }
    }

    pub(super) fn detail(self) -> String {
        match self {
            Self::GrantedByUser => "approved by the user".to_string(),
            Self::GrantedBySessionMemory => {
                "satisfied by an earlier session grant for this exact call".to_string()
            }
            Self::Denied(reason) => redact_and_cap(reason),
        }
    }
}
