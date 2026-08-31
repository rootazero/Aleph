//! Exec approval manager for handling approval requests and decisions.
//!
//! Provides async approval flow with timeout and event broadcasting.

use crate::sync_primitives::{Arc, RwLock};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, error, warn};

/// Wall-clock time in milliseconds since the Unix epoch, with a pre-epoch
/// detector that fires `error!` once per process.
///
/// A pre-epoch wall clock (NTP misconfig, VM resume from suspend, manual
/// clock rollback) collapses every approval timestamp to 0: every card is
/// stamped "born 1970" and either auto-expires (`0 < created_at`) or
/// auto-lives forever, depending on direction. The expiry math is
/// deliberately wall-clock here (the field is on the wire — see
/// `ExecApprovalRecord::expires_at_ms`), so the only safe posture is to
/// surface the problem loudly; silent fallback to 0 makes the worst class
/// of bug invisible.
fn now_ms_or_warn() -> u64 {
    static PRE_EPOCH_LOGGED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as u64,
        Err(_) => {
            if !PRE_EPOCH_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                error!(
                    "wall clock reads pre-Unix-epoch; approval timestamps will \
                     collapse to 0 until the clock is corrected (NTP, VM resume, \
                     or manual rollback)"
                );
            }
            0
        }
    }
}

use super::decision::ExecApprovalRequest;
use super::socket::ApprovalDecisionType;

/// Default timeout for approval requests (2 minutes)
pub const DEFAULT_APPROVAL_TIMEOUT_MS: u64 = 120_000;

/// The no-expiry sentinel for [`ExecApprovalRecord::expires_at_ms`]: the card
/// waits forever. Ruled 2026-08-28 (verbatim: "不要使用超时，应该使用通知+永
/// 久等待") — an attended approval notifies and parks until answered. The
/// banner/card IS the persistent notification; the wait has no deadline.
/// Unattended turns never get this value: see
/// [`crate::approval::approval_timeout_for_current_turn`].
pub const NO_APPROVAL_TIMEOUT: u64 = 0;

/// Record of an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRecord {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Primary executable
    pub executable: String,
    /// Resolved executable path
    pub resolved_path: Option<String>,
    /// Creation timestamp (Unix ms)
    pub created_at_ms: u64,
    /// Expiration timestamp (Unix ms)
    pub expires_at_ms: u64,
    /// Resolution timestamp (Unix ms)
    pub resolved_at_ms: Option<u64>,
    /// Decision (if resolved)
    pub decision: Option<ApprovalDecisionType>,
    /// Who resolved (display name)
    pub resolved_by: Option<String>,
    /// Why approval was requested (escalation / confirmation context),
    /// surfaced to resolving UIs. Absent on records persisted before this
    /// field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Harness tool-call id (`ToolStart.tool_id`) this approval gates. Lets a
    /// client key `tool_id → approval` instead of pairing by position against
    /// an unordered pending map — which mis-renders the card under the wrong
    /// tool as soon as two tool calls run concurrently. `None` for approvals
    /// raised outside tool dispatch (cluster node approvals, raw exec commands).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Free-text reason the human attached to a denial (`/deny <reason>` from
    /// a channel, or the `reason` field on `exec.approval.resolve`). Relayed
    /// to the model through [`ResolvedDecision`] so it can change approach
    /// instead of blindly retrying. Only ever set on a `Deny`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_reason: Option<String>,
    /// Raw channel user id of the human whose message triggered this approval
    /// (the "originator"). Set only for channel-originated runs. The channel
    /// button-callback gate (`ManagerCallbackSink::handle_callback`) refuses a
    /// resolution from anyone but this user, closing the group-chat bypass where
    /// any paired member could approve another member's action. Absent on
    /// records persisted before this field existed and on non-channel approvals
    /// — both skip the gate (best-effort, preserving prior behaviour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originator_user_id: Option<String>,
    /// Session-grant identity of the approved action
    /// ([`crate::sandbox::exec_approval::grant_fingerprint`]). When a
    /// session-wide answer — a standing grant, or a refusal — lands on one
    /// record, the manager cascades it to every OTHER live pending record in
    /// the same session carrying the same key: the concurrent-subagent case,
    /// where identical calls each parked their own card before the user
    /// answered the first. `None` (no action identity: cluster node approvals,
    /// bare escalations) never cascades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_key: Option<String>,
    /// This card is an **operator-tier escalation**: it was raised BECAUSE the
    /// requester is not allowed to make this decision, so it belongs to the
    /// operator and not to the session that triggered it. `session_key` above
    /// still names that session — the manager needs it for the session-grant
    /// cascade and for session-scoped cleanup — which is exactly why the fact
    /// "who may answer this" cannot be derived from it and has to ride on the
    /// record instead.
    ///
    /// Consumed by `handlers::exec_approvals::approval_addressable_by_caller`.
    /// Absent on records persisted before this field existed, and `false` for
    /// every other requester — both mean "owner-scoped", the prior behaviour.
    #[serde(default)]
    pub operator_only: bool,
    /// The decision tiers this card was raised with
    /// ([`crate::exec::allowed_decisions::for_confirm_gate`]). Renderers draw
    /// from it; [`ExecApprovalManager::resolve_with_reason`] **enforces** it, so
    /// an `allow-always` posted straight at `exec.approval.resolve` for a card
    /// that never offered the tier is narrowed to a session grant.
    ///
    /// The serde default is deliberately the SESSION ceiling, not
    /// [`crate::exec::allowed_decisions::full_set`]: a record persisted before
    /// this field existed must not be readable as permission to create a
    /// permanent grant. A missing field may narrow; it may never widen.
    #[serde(default = "crate::exec::allowed_decisions::session_max")]
    pub allowed_decisions: Vec<ApprovalDecisionType>,
}

impl ExecApprovalRecord {
    /// Create from `ExecApprovalRequest`
    #[must_use]
    pub fn from_request(request: &ExecApprovalRequest, timeout_ms: u64) -> Self {
        let now = now_ms_or_warn();

        let executable = request
            .analysis
            .segments
            .first()
            .and_then(|s| s.resolution.as_ref())
            .map(|r| r.executable_name.clone())
            .unwrap_or_default();

        let resolved_path = request
            .analysis
            .segments
            .first()
            .and_then(|s| s.resolution.as_ref())
            .and_then(|r| r.resolved_path.as_ref())
            .map(|p| p.to_string_lossy().to_string());

        Self {
            id: request.id.clone(),
            command: request.command.clone(),
            cwd: request.cwd.clone(),
            agent_id: request.agent_id.clone(),
            session_key: request.session_key.clone(),
            executable,
            resolved_path,
            created_at_ms: now,
            // `NO_APPROVAL_TIMEOUT` (0) is a SENTINEL, not a duration:
            // `now + 0` would stamp "expires immediately", and the first
            // sweep would retire the card as a silent timeout — the exact
            // failure the no-timeout ruling exists to remove. Map it to the
            // no-expiry `expires_at_ms == 0` instead.
            expires_at_ms: if timeout_ms == NO_APPROVAL_TIMEOUT {
                0
            } else {
                now.saturating_add(timeout_ms)
            },
            resolved_at_ms: None,
            decision: None,
            resolved_by: None,
            reason: request.reason.clone(),
            // Ambient, not a request field: the requester receives an
            // `ApprovalAction` (redacted tool name, summary, cwd, analysis,
            // reason) but no per-call id. The identity is scoped at the
            // tool-dispatch chokepoint that raised the gate.
            tool_call_id: crate::approval::current_tool_call_id(),
            deny_reason: None,
            originator_user_id: request.originator_user_id.clone(),
            grant_key: request.grant_key.clone(),
            // Owner-scoped by default. The one requester that raises cards on
            // the operator's behalf stamps this to `true` on the record it gets
            // back, before `register_pending` publishes it.
            operator_only: false,
            allowed_decisions: request.allowed_decisions.clone(),
        }
    }

    /// Check if expired. `expires_at_ms == 0` is the NO-EXPIRY sentinel
    /// (ruled 2026-08-28: an attended approval notifies and waits forever —
    /// see [`approval_timeout_for_current_turn`]); such a record never
    /// expires, and `list_pending`/`cleanup_expired` retire it only when its
    /// waiter is gone (`PendingEntry::is_live`).
    ///
    /// [`approval_timeout_for_current_turn`]:
    /// crate::approval::approval_timeout_for_current_turn
    #[must_use]
    pub(crate) fn is_expired(&self) -> bool {
        if self.expires_at_ms == 0 {
            return false;
        }
        let now = now_ms_or_warn();
        now > self.expires_at_ms
    }

    /// Check if resolved
    #[must_use]
    #[allow(dead_code)]
    const fn is_resolved(&self) -> bool {
        self.decision.is_some()
    }
}

/// What [`ExecApprovalManager::await_registered`] resolves to: the decision
/// (`None` = timed out / channel closed) plus any free-text reason the human
/// attached to a denial (`/deny <reason>` or the RPC `reason` field). The
/// reason is what turns a bare "denied" into something the model can act on.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolvedDecision {
    pub decision: Option<ApprovalDecisionType>,
    pub deny_reason: Option<String>,
}

/// Internal pending entry with channel
struct PendingEntry {
    record: ExecApprovalRecord,
    sender: Option<oneshot::Sender<Option<ApprovalDecisionType>>>,
    created_at: Instant,
}

impl PendingEntry {
    /// Whether a waiter is still parked on this entry.
    ///
    /// A CLOSED oneshot receiver proves nobody is waiting: the only holder is
    /// the `await_registered` future, so a closed channel means that future was
    /// dropped — a cancelled run, an expired run deadline, or a per-call
    /// tool-budget overrun in the confirm gate. Such an entry is a zombie: it
    /// can never be delivered to, yet it is OLDER than any live card and would
    /// win `resolve_for_session`'s FIFO, absorbing the user's `/approve` while
    /// the real card silently times out. Mirrors
    /// [`crate::clarification::session::PendingEntry::is_live`].
    ///
    /// Expiry is read off `record` (one wall clock), not `created_at`.
    fn is_live(&self) -> bool {
        !self.record.is_expired() && self.sender.as_ref().is_some_and(|s| !s.is_closed())
    }
}

/// Pending approval info for external access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub record: ExecApprovalRecord,
    pub remaining_ms: u64,
}

/// Manager for exec approval requests
///
/// Handles the lifecycle of approval requests:
/// 1. Create request and wait for decision
/// 2. Resolve with user decision
///
/// Purely in-memory: a granted approval lives for one execution
/// (`AllowOnce`), for the session (`AllowSession`) or until revoked
/// (`AllowAlways`) — the last two remembered by
/// [`crate::sandbox::exec_approval::grants`], which is also where the
/// persistent tier is written. Nothing *here* persists.
/// Outcome of a session-addressed (id-less) approval reply — see
/// [`ExecApprovalManager::resolve_for_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionResolveOutcome {
    /// Applied to the addressed entry. Carries the EFFECTIVE decision (post
    /// clamp — a legacy `AllowAlways` narrows to `AllowSession`) and the
    /// resolved action's display line, so the confirmation reply can echo
    /// WHAT was approved instead of a bare "Approved."
    Resolved {
        decision: ApprovalDecisionType,
        summary: String,
    },
    /// Nothing live is pending for this session.
    NothingPending,
    /// Several cards are live and the reply named none of them (or named an
    /// out-of-range index). Oldest-first `(1-based index, display line)`
    /// listing for an indexed retry — nothing was resolved.
    Ambiguous(Vec<(usize, String)>),
}

pub struct ExecApprovalManager {
    pending: Arc<RwLock<HashMap<String, PendingEntry>>>,
    /// Per-session snapshot of the last [`SessionResolveOutcome::Ambiguous`]
    /// listing SHOWN — the only thing a positional `/approve <n>` may address.
    /// Binding indices to the live list instead would race: resolve card 1,
    /// have a NEW card arrive, and a still-in-range `/approve 2` lands on the
    /// newcomer the user never read. Refreshed on every listing; an index
    /// whose snapshot entry is gone (or with no snapshot at all) re-lists
    /// instead of guessing. Lock order: `pending` first, then this — always.
    session_listings: RwLock<HashMap<String, Vec<String>>>,
}

impl ExecApprovalManager {
    /// Create new manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            session_listings: RwLock::new(HashMap::new()),
        }
    }

    /// Create approval request and return record (does not wait)
    ///
    /// # Arguments
    ///
    /// * `request` - The approval request
    /// * `timeout_ms` - Timeout in milliseconds
    ///
    /// # Returns
    ///
    /// The approval record
    pub fn create(&self, request: &ExecApprovalRequest, timeout_ms: u64) -> ExecApprovalRecord {
        // The parser's `CommandAnalysis::error` is the single source of
        // "this command is unparseable". Surfacing it as an approval card
        // (delivered to a channel, presented to a human) is the wrong
        // default — the caller already knows the command is unrunnable.
        // `debug_assert!` is the right tier: a misconfigured caller logs
        // the parser's `reason` rather than silently spending a delivery
        // slot on a card that the human can only deny.
        debug_assert!(
            request.analysis.ok,
            "ExecApprovalManager::create called with !analysis.ok — caller must reject \
             before this point. Parser reason: {:?}",
            request.analysis.reason,
        );
        let record = ExecApprovalRecord::from_request(request, timeout_ms);
        debug!(id = %record.id, command = %record.command, "Created approval request");
        record
    }

    /// Register `record` as pending and return its id + receiver + remaining
    /// timeout — WITHOUT awaiting.
    ///
    /// Synchronous: the entry is in `pending` (and thus resolvable via
    /// [`Self::resolve`]) the instant this returns. Callers that publish a
    /// notification about the pending approval should call this FIRST, then
    /// publish, then [`Self::await_registered`] — so a fast resolver cannot
    /// race ahead of registration (resolve-before-register → spurious timeout).
    #[must_use]
    pub fn register_pending(
        &self,
        record: ExecApprovalRecord,
    ) -> (
        String,
        oneshot::Receiver<Option<ApprovalDecisionType>>,
        Option<Duration>,
    ) {
        // Use remaining time from now, not the full original timeout window.
        // This prevents granting a full timeout if the wait begins long after
        // the record was created. `expires_at_ms == 0` is the no-expiry
        // sentinel: the caller waits indefinitely (notify + wait, ruled
        // 2026-08-28).
        let now_ms = now_ms_or_warn();
        let timeout = if record.expires_at_ms == 0 {
            None
        } else {
            let remaining_ms = record.expires_at_ms.saturating_sub(now_ms);
            Some(Duration::from_millis(remaining_ms))
        };

        let (tx, rx) = oneshot::channel();
        let id = record.id.clone();

        // Opportunistic sweep: a waiter whose future was dropped (aborted
        // agent run) never reaches `await_registered`'s removal, so its entry
        // would otherwise linger forever. Each new registration evicts
        // anything already past its deadline — bounded work, no background
        // task needed.
        self.cleanup_expired();

        {
            let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
            if pending.contains_key(&id) {
                // `ExecApprovalRequest.id` is caller-supplied: a duplicate
                // silently drops the live entry's sender, and its waiter then
                // reports a spurious timeout. Keep the overwrite (callers use
                // uuid v4 today) but make it observable.
                warn!(id = %id, "Duplicate approval id overwrites a live pending entry");
                debug_assert!(false, "duplicate approval id registered: {id}");
            }
            pending.insert(
                id.clone(),
                PendingEntry {
                    record,
                    sender: Some(tx),
                    created_at: Instant::now(),
                },
            );
        }

        (id, rx, timeout)
    }

    /// Await a previously [`register_pending`](Self::register_pending)ed
    /// approval, removing it from `pending` on resolution or timeout.
    ///
    /// The denial reason (if the resolver attached one) is harvested from the
    /// removed entry here — the resolver stamps it on the record BEFORE waking
    /// the oneshot, so it is read atomically with the decision.
    pub async fn await_registered(
        &self,
        id: String,
        rx: oneshot::Receiver<Option<ApprovalDecisionType>>,
        timeout: Option<Duration>,
    ) -> ResolvedDecision {
        // `None` = the no-expiry sentinel: wait indefinitely (notify + wait).
        let result = match timeout {
            Some(t) => tokio::time::timeout(t, rx).await,
            None => Ok(rx.await),
        };

        // Remove from pending, harvesting the record the resolver annotated.
        let deny_reason = {
            let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
            pending.remove(&id).and_then(|e| e.record.deny_reason)
        };

        let decision = match result {
            Ok(Ok(decision)) => {
                debug!(id = %id, ?decision, "Approval resolved");
                decision
            }
            Ok(Err(_)) => {
                // Channel closed without decision
                debug!(id = %id, "Approval channel closed");
                None
            }
            Err(_) => {
                // Timeout
                debug!(id = %id, "Approval timed out");
                None
            }
        };
        ResolvedDecision {
            decision,
            deny_reason,
        }
    }

    /// Resolve an approval request with a decision
    ///
    /// # Arguments
    ///
    /// * `id` - Request ID
    /// * `decision` - The decision
    /// * `resolved_by` - Display name of resolver (optional)
    ///
    /// # Returns
    ///
    /// `true` if the request was found and resolved. A session-level grant
    /// additionally cascades to every other live pending record in the same
    /// session carrying the same `grant_key` — and so does a `Deny` (see
    /// [`Self::cascade_to_identical_cards`]); the return value only reports the
    /// addressed record.
    pub fn resolve(
        &self,
        id: &str,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    ) -> bool {
        self.resolve_with_reason(id, decision, resolved_by, None)
    }

    /// [`Self::resolve`] carrying an optional free-text denial reason, stamped
    /// onto the record BEFORE the waiter is woken so the awaiting requester
    /// reads it atomically with the decision. Stored only on a `Deny` — a
    /// reason on an approval means nothing and would only confuse the record.
    pub fn resolve_with_reason(
        &self,
        id: &str,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        deny_reason: Option<String>,
    ) -> bool {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());

        let Some(entry) = pending.get_mut(id) else {
            warn!(id = %id, "Approval not found or already resolved");
            return false;
        };
        // Liveness is the honest signal, exactly as the clarification twin
        // enforces it ([`crate::clarification::session::ClarificationManager::resolve`]):
        // a dead entry is one already resolved (`sender` taken), past its
        // deadline, OR abandoned (receiver dropped by a cancelled run). Any
        // of these makes the decision reach nobody — reporting `true` here
        // is what lets a Telegram button callback / `exec.approval.resolve`
        // reply "✅ Allowed" for an approval that was never delivered. The
        // FIFO `resolve_for_session` path already filters on `is_live`; the
        // by-id path must too, or the two disagree on the same registry.
        if !entry.is_live() {
            warn!(id = %id, "Approval already resolved, expired, or abandoned");
            return false;
        }

        // Enforced against the set THIS card was raised with, not a global
        // rule: that is what makes "the Panel does not draw the button" stop
        // being the control (an `allow-always` posted straight at
        // `exec.approval.resolve` was always accepted on the wire).
        let decision = Self::clamp_decision(decision, &entry.record.allowed_decisions);
        Self::resolve_entry(entry, decision, resolved_by.clone(), deny_reason.clone());
        debug!(id = %id, ?decision, "Resolved approval");

        let cascade = pending
            .get(id)
            .map(|e| (e.record.session_key.clone(), e.record.grant_key.clone()));
        if let Some((session_key, grant_key)) = cascade {
            Self::cascade_to_identical_cards(
                &mut pending,
                id,
                &session_key,
                grant_key.as_deref(),
                decision,
                resolved_by,
                deny_reason,
            );
        }
        true
    }

    /// Stamp a resolution onto a LIVE entry and wake its waiter. The one
    /// internal path every resolution takes — by-id, by-session, and the
    /// session-grant cascade — so a cascaded record is indistinguishable from
    /// a manually resolved one: the waiter is woken with the decision, the
    /// `resolved_by` / `resolved_at_ms` / `deny_reason` audit fields are
    /// stamped, and [`Self::await_registered`] harvests it the same way.
    fn resolve_entry(
        entry: &mut PendingEntry,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        deny_reason: Option<String>,
    ) {
        entry.record.decision = Some(decision);
        entry.record.resolved_by = resolved_by;
        if decision == ApprovalDecisionType::Deny {
            entry.record.deny_reason = deny_reason;
        }
        entry.record.resolved_at_ms = Some(now_ms_or_warn());

        // Callers proved the receiver is still open under this same lock
        // (`is_live`), so the send delivers — no silent drop to a zombie.
        if let Some(sender) = entry.sender.take() {
            let _ = sender.send(Some(decision));
        }
    }

    /// Cascade a **session-wide answer** — a standing grant or a refusal — to
    /// every OTHER live pending record in the same session carrying the same
    /// `grant_key`.
    ///
    /// The stores behind this gate only affect FUTURE prompts; without this,
    /// identical calls that were already parked (concurrent subagents, a teams
    /// broadcast) would each still wait for their own click even though the
    /// user just answered that exact action. Cascading resolves them through
    /// the same [`Self::resolve_entry`] path a manual resolve takes.
    ///
    /// # Which decisions cascade
    ///
    /// * `AllowSession`, and `AllowAlways` **a fortiori** — a grant that
    ///   outlives the process certainly covers the identical call parked next
    ///   to the one that was answered. This condition was written as
    ///   `!= AllowSession` when the persistent tier could not exist; leaving it
    ///   that way would have made the widest possible answer the ONE that fails
    ///   to release its siblings (判据 §0, enumeration goes stale).
    /// * `Deny`, carrying the human's own words with it. This arm used to be
    ///   excluded on the grounds that "a deny is about THIS call, not the
    ///   action" — but the [`DenialLedger`] two files over answers the same
    ///   question the other way: a refusal is sticky **for the action** for the
    ///   rest of the session, and the very next identical call is auto-refused
    ///   without a card. So whether one "no" covered the identical call parked
    ///   beside it came down to a **race** — the ledger caught the sibling that
    ///   arrived a moment later and missed the one already waiting, which is
    ///   precisely the concurrent case this cascade exists for. The cost of the
    ///   old behaviour was not only clicks: each extra card the user dismissed
    ///   was another refusal on the brute-force breaker (see
    ///   `DenialLedger::record_denial`, which now counts intents, not cards).
    ///   opencode cascades a rejection across the whole session; this narrows
    ///   it to the identical action, which is the strongest form that cannot
    ///   refuse something the user never read.
    ///
    /// `AllowOnce` covers one invocation by definition and never cascades. A
    /// `None` key (no action identity: bare route escalations, cluster node
    /// approvals) never cascades.
    ///
    /// [`DenialLedger`]: crate::sandbox::exec_approval::denial_ledger::DenialLedger
    fn cascade_to_identical_cards(
        pending: &mut HashMap<String, PendingEntry>,
        resolved_id: &str,
        session_key: &str,
        grant_key: Option<&str>,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        deny_reason: Option<String>,
    ) {
        let Some(key) = grant_key else {
            return;
        };
        if !matches!(
            decision,
            ApprovalDecisionType::AllowSession
                | ApprovalDecisionType::AllowAlways
                | ApprovalDecisionType::Deny
        ) {
            return;
        }
        let ids: Vec<String> = pending
            .iter()
            .filter(|(id, e)| {
                id.as_str() != resolved_id
                    && e.is_live()
                    && e.record.session_key == session_key
                    && e.record.grant_key.as_deref() == Some(key)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for cascade_id in ids {
            if let Some(entry) = pending.get_mut(&cascade_id) {
                // The reason rides along: a sibling refused by cascade must
                // reach the model with the same instruction the answered card
                // produced, or two identical calls get two different stories.
                Self::resolve_entry(entry, decision, resolved_by.clone(), deny_reason.clone());
                debug!(
                    id = %cascade_id,
                    cascaded_from = %resolved_id,
                    ?decision,
                    "Cascaded to an identical pending approval"
                );
            }
        }
    }

    /// The originator (raw channel user id) recorded on a **live** pending
    /// approval, or `None` when the id is unknown / already resolved / expired,
    /// OR when the record carries no originator (non-channel or legacy record).
    ///
    /// The channel button-callback gate
    /// ([`ManagerCallbackSink::handle_callback`](crate::approval::callback_sink))
    /// uses this to let only the originator resolve via a button: a `Some` that
    /// mismatches the clicker is refused; a `None` skips the gate (best-effort,
    /// preserving the pre-originator behaviour, and letting dead records fall
    /// through to the normal "expired" reply from [`Self::resolve`]).
    #[must_use]
    pub fn record_originator(&self, id: &str) -> Option<String> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        pending
            .get(id)
            .filter(|entry| entry.is_live())
            .and_then(|entry| entry.record.originator_user_id.clone())
    }

    /// Resolve an unresolved approval for `session_key` by position.
    ///
    /// A channel TEXT reply (`/approve` / `/deny`) carries no request id, so
    /// the inbound router resolves by session. With exactly one LIVE pending
    /// entry a bare reply is unambiguous and resolves it. With several —
    /// possible since approval-gated calls may share a parallel batch — a
    /// bare reply cannot say which card the user actually read, so nothing is
    /// resolved and [`SessionResolveOutcome::Ambiguous`] hands back the
    /// oldest-first list for an indexed retry (`/approve 2`).
    ///
    /// `index` is 1-based and addresses **the last listing SHOWN to this
    /// session** (snapshotted in `session_listings`), never the live list's
    /// current positions: between the listing and the reply, another card may
    /// resolve and a NEW one arrive, leaving the index in range but pointing
    /// at an entry the user never read. An index with no snapshot, past the
    /// snapshot's end, or addressing an entry that is no longer live,
    /// re-lists (and re-snapshots) instead of guessing.
    ///
    /// Liveness ([`PendingEntry::is_live`]) is what keeps the ordering honest:
    /// a cancelled run leaves an entry whose waiter is gone but whose `sender`
    /// is still `Some`, and being the oldest it would otherwise win this pick
    /// and swallow the approval meant for the card the user is actually
    /// looking at.
    pub fn resolve_for_session(
        &self,
        session_key: &str,
        index: Option<usize>,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        deny_reason: Option<String>,
    ) -> SessionResolveOutcome {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());

        // Oldest-first live entries for this session — the stable order every
        // `Ambiguous` listing renders (and snapshots).
        let mut live: Vec<(Instant, String)> = pending
            .iter()
            .filter(|(_, e)| e.is_live() && e.record.session_key == session_key)
            .map(|(id, e)| (e.created_at, id.clone()))
            .collect();
        live.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        // Render the listing AND snapshot it as the one thing a subsequent
        // indexed reply may address. Lock order: `pending` (held) → listings.
        let list_and_snapshot = |pending: &HashMap<String, PendingEntry>| {
            let ids: Vec<String> = live.iter().map(|(_, id)| id.clone()).collect();
            self.session_listings
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(session_key.to_string(), ids);
            live.iter()
                .enumerate()
                .map(|(i, (_, id))| {
                    let line = pending
                        .get(id)
                        .map(|e| Self::display_line(&e.record))
                        .unwrap_or_default();
                    (i + 1, line)
                })
                .collect()
        };

        let id = match (index, live.len()) {
            (_, 0) => {
                warn!(session_key = %session_key, "No pending approval for session");
                self.session_listings
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(session_key);
                return SessionResolveOutcome::NothingPending;
            }
            (None, 1) => live[0].1.clone(),
            // Several cards, bare reply: refusing to guess IS the safety
            // property — FIFO here would approve a command the user may
            // never have read.
            (None, _) => return SessionResolveOutcome::Ambiguous(list_and_snapshot(&pending)),
            (Some(n), _) => {
                // Address the SNAPSHOT the user was shown, not live positions.
                let addressed = self
                    .session_listings
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(session_key)
                    .and_then(|ids| ids.get(n.wrapping_sub(1)).cloned());
                match addressed {
                    Some(id)
                        if pending.get(&id).is_some_and(|e| {
                            e.is_live() && e.record.session_key == session_key
                        }) =>
                    {
                        id
                    }
                    // No listing was ever shown, the index is past its end, or
                    // the addressed card already resolved/expired — show a
                    // fresh listing rather than resolve something unread.
                    _ => return SessionResolveOutcome::Ambiguous(list_and_snapshot(&pending)),
                }
            }
        };

        // Group-chat / paired-chat originator gate. When the card was raised
        // by a specific user (channel with a known sender), only THAT user may
        // resolve it — `/approve` / `/deny` from any other paired member is
        // silently ignored. The button-callback path goes through a separate
        // `authorize_actor` gate; this is the text-fallback twin so the
        // capability-without-originator fallback in `ChannelApprovalBridge`
        // doesn't widen to "any paired member". A `None` originator (e.g.
        // CLI / RPC with no chat context) preserves the legacy behaviour —
        // any `/approve` is honored.
        if let Some(entry) = pending.get(&id) {
            if let Some(ref expected) = entry.record.originator_user_id {
                match resolved_by.as_deref() {
                    Some(actual) if actual == expected.as_str() => {}
                    Some(_) | None => {
                        warn!(
                            id = %id,
                            expected = %expected,
                            actual = ?resolved_by,
                            "Rejecting /approve or /deny from non-originator \
                             (group-chat approval bypass guard)"
                        );
                        return SessionResolveOutcome::NothingPending;
                    }
                }
            }
        }

        if let Some(entry) = pending.get_mut(&id) {
            let decision = Self::clamp_decision(decision, &entry.record.allowed_decisions);
            let summary = Self::display_line(&entry.record);
            Self::resolve_entry(entry, decision, resolved_by.clone(), deny_reason.clone());
            debug!(id = %id, ?decision, "Resolved approval by session");
            let grant_key = pending.get(&id).and_then(|e| e.record.grant_key.clone());
            Self::cascade_to_identical_cards(
                &mut pending,
                &id,
                session_key,
                grant_key.as_deref(),
                decision,
                resolved_by,
                deny_reason,
            );
            SessionResolveOutcome::Resolved { decision, summary }
        } else {
            SessionResolveOutcome::NothingPending
        }
    }

    /// One-line, char-safe display form of a record for replies and the
    /// ambiguous listing — the command the user is being asked to approve.
    fn display_line(record: &ExecApprovalRecord) -> String {
        const MAX: usize = 120;
        let mut line: String = record.command.chars().take(MAX).collect();
        if record.command.chars().count() > MAX {
            line.push('…');
        }
        line
    }

    /// Clamp `requested` to a grant scope THIS card may honor.
    ///
    /// Delegates to [`ApprovalDecisionType::clamped_for`] — the single source
    /// of the narrowing rule — so the decision layer and the outcome layer
    /// ([`ApprovalDecisionType::to_outcome_within`]) can never disagree on the
    /// downgrade. `allowed` is the record's own
    /// [`ExecApprovalRecord::allowed_decisions`], derived once at the gate.
    fn clamp_decision(
        requested: ApprovalDecisionType,
        allowed: &[ApprovalDecisionType],
    ) -> ApprovalDecisionType {
        requested.clamped_for(allowed)
    }

    /// Get snapshot of a pending approval
    ///
    /// Consults [`PendingEntry::is_live`] so a consumer that asks "is this
    /// id still pending?" never sees a record whose sender is gone, whose
    /// deadline has passed, or whose `decision` is set. `list_pending` is
    /// the only path that may return a still-pending roster to a panel;
    /// `get_pending` was the second one and is now in lock-step.
    #[must_use]
    pub fn get_pending(&self, id: &str) -> Option<PendingApproval> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        pending
            .get(id)
            .filter(|entry| entry.is_live())
            .map(|entry| {
                let now = Instant::now();
                let elapsed = now.duration_since(entry.created_at);
                let timeout_ms = entry
                    .record
                    .expires_at_ms
                    .saturating_sub(entry.record.created_at_ms);
                let remaining = Duration::from_millis(timeout_ms).saturating_sub(elapsed);

                PendingApproval {
                    record: entry.record.clone(),
                    remaining_ms: remaining.as_millis() as u64,
                }
            })
    }

    /// List all LIVE pending approvals, oldest first.
    ///
    /// The backing map is unordered, so the order is imposed here: a client
    /// that must still fall back to positional rendering (an approval with no
    /// `tool_call_id`) at least gets a stable, meaningful sequence rather than
    /// hash order that reshuffles between calls.
    ///
    /// Entries whose waiter is gone ([`PendingEntry::is_live`]) are skipped: a
    /// Panel that reconnects must not render a card for a cancelled run, whose
    /// resolution can never reach anyone.
    #[must_use]
    pub fn list_pending(&self) -> Vec<PendingApproval> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        let mut out: Vec<PendingApproval> = pending
            .values()
            .filter(|entry| entry.is_live())
            .map(|entry| {
                let elapsed = now.duration_since(entry.created_at);
                let timeout_ms = entry
                    .record
                    .expires_at_ms
                    .saturating_sub(entry.record.created_at_ms);
                let remaining = Duration::from_millis(timeout_ms).saturating_sub(elapsed);

                PendingApproval {
                    record: entry.record.clone(),
                    remaining_ms: remaining.as_millis() as u64,
                }
            })
            .collect();
        // `id` breaks the tie: two approvals raised in the same millisecond
        // must still come out in a fixed order across calls.
        out.sort_by(|a, b| {
            a.record
                .created_at_ms
                .cmp(&b.record.created_at_ms)
                .then_with(|| a.record.id.cmp(&b.record.id))
        });
        out
    }

    /// Clean up expired pending requests
    pub(crate) fn cleanup_expired(&self) {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        pending.retain(|id, entry| {
            // No-expiry entries (attended approvals, notify + wait) are never
            // time-swept; their one retirement is a dead waiter — an aborted
            // run drops its receiver, and a card nobody waits on can never be
            // answered usefully.
            if entry.record.expires_at_ms == 0 {
                return entry.sender.as_ref().is_some_and(|s| !s.is_closed());
            }
            let elapsed = now.duration_since(entry.created_at);
            let timeout_ms = entry
                .record
                .expires_at_ms
                .saturating_sub(entry.record.created_at_ms);

            if elapsed > Duration::from_millis(timeout_ms) {
                // Send None to waiter
                if let Some(sender) = entry.sender.take() {
                    let _ = sender.send(None);
                }
                debug!(id = %id, "Cleaned up expired approval");
                false
            } else {
                true
            }
        });
    }
}

impl Default for ExecApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::analysis::CommandAnalysis;
    use crate::exec::decision::ApprovalRequest;

    fn mock_request() -> ApprovalRequest {
        ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: "npm install".to_string(),
            cwd: Some("/project".to_string()),
            analysis: CommandAnalysis::not_a_command(),
            agent_id: "main".to_string(),
            session_key: "agent:main:main".to_string(),
            reason: None,
            originator_user_id: None,
            grant_key: None,
            // The default ceiling every gate but the operator-tier confirm gate
            // raises its cards with.
            allowed_decisions: crate::exec::allowed_decisions::session_max(),
        }
    }

    /// A request raised by a card that DID offer the persistent tier — what
    /// `for_confirm_gate` produces for an operator-tier turn outside the
    /// declared floor.
    fn persistent_capable_request() -> ApprovalRequest {
        ApprovalRequest {
            allowed_decisions: crate::exec::allowed_decisions::with_persistent(),
            ..mock_request()
        }
    }

    #[test]
    fn test_create_record() {
        let manager = ExecApprovalManager::new();
        let request = mock_request();

        let record = manager.create(&request, 60_000);

        assert_eq!(record.id, request.id);
        assert_eq!(record.command, "npm install");
        assert!(record.expires_at_ms > record.created_at_ms);
    }

    /// The no-timeout ruling (2026-08-28): `NO_APPROVAL_TIMEOUT` is a sentinel,
    /// not a duration — `now + 0` would stamp "expires immediately" and the
    /// first sweep would retire the card as a silent timeout. The record must
    /// carry the no-expiry `expires_at_ms == 0` and never report expired.
    #[test]
    fn no_approval_timeout_maps_to_no_expiry_sentinel() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), NO_APPROVAL_TIMEOUT);
        assert_eq!(record.expires_at_ms, 0);
        assert!(!record.is_expired());
    }

    /// End-to-end: a no-expiry card is not time-swept, waits indefinitely, and
    /// resolves when the operator answers — and the sweep still retires it once
    /// the waiter is gone (aborted run), so it cannot linger forever.
    #[tokio::test]
    async fn no_expiry_card_waits_until_resolved_and_is_not_swept() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), NO_APPROVAL_TIMEOUT);
        let (id, rx, timeout) = manager.register_pending(record);
        assert!(timeout.is_none(), "no-expiry card must not carry a deadline");

        // Two sweeps must not retire a live no-expiry card.
        manager.cleanup_expired();
        manager.cleanup_expired();
        assert!(manager.get_pending(&id).is_some());

        manager.resolve(&id, ApprovalDecisionType::AllowOnce, None);
        let resolved = manager.await_registered(id.clone(), rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }

    /// Defect D: with two tool calls in flight the client cannot pair approvals
    /// to tool rows by position — the pending map is unordered. Each record must
    /// carry the id of the tool call it actually gates, and `list_pending` must
    /// come out in a stable, oldest-first order.
    #[tokio::test]
    async fn concurrent_approvals_carry_distinct_tool_call_ids() {
        use crate::approval::{with_call_identity, CallIdentity};

        fn identity(call_id: &str) -> Option<CallIdentity> {
            Some(CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: call_id.to_string(),
            })
        }

        let manager = ExecApprovalManager::new();
        let mut first = mock_request();
        first.id = "ap-a".to_string();
        let mut second = mock_request();
        second.id = "ap-b".to_string();

        let rec_a = with_call_identity(identity("toolu_a"), async {
            manager.create(&first, 60_000)
        })
        .await;
        let rec_b = with_call_identity(identity("toolu_b"), async {
            manager.create(&second, 60_000)
        })
        .await;
        let (_ia, _rx_a, _ta) = manager.register_pending(rec_a);
        let (_ib, _rx_b, _tb) = manager.register_pending(rec_b);

        let pending = manager.list_pending();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].record.id, "ap-a", "oldest first, stable order");
        assert_eq!(pending[0].record.tool_call_id.as_deref(), Some("toolu_a"));
        assert_eq!(pending[1].record.id, "ap-b");
        assert_eq!(pending[1].record.tool_call_id.as_deref(), Some("toolu_b"));
    }

    /// An approval raised outside tool dispatch (cluster node, raw exec command)
    /// has no owning tool row and must say so rather than borrow a stale id.
    #[test]
    fn approval_outside_tool_dispatch_has_no_tool_call_id() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        assert!(record.tool_call_id.is_none());
    }

    #[tokio::test]
    async fn resolve_after_register_before_await_is_not_lost() {
        // Regression: register_pending must make the entry resolvable BEFORE the
        // caller awaits, so a resolver racing right after a published
        // notification is not lost (resolve-before-register → spurious timeout).
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);

        let (id, rx, timeout) = manager.register_pending(record);
        // Resolve immediately — entry must already exist in `pending`.
        assert!(
            manager.resolve(&id, ApprovalDecisionType::AllowOnce, None),
            "resolve must find the entry registered by register_pending"
        );
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }

    #[tokio::test]
    async fn resolve_by_id_reports_false_for_an_abandoned_waiter() {
        // Twin parity with the clarification manager: a run cancelled while its
        // approval was parked drops the receiver. A late button tap / RPC that
        // resolves this zombie by id must report `false` — the callback sink
        // and `exec.approval.resolve` speak "✅ Allowed" only on a `true`, and
        // the decision would reach nobody. Before the fix `resolve` checked only
        // `sender.is_none()` and returned `true` for the dead entry.
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let (id, rx, _timeout) = manager.register_pending(record);
        drop(rx); // the awaiting `ask_user`/tool future is gone (aborted run)

        assert!(
            !manager.resolve(&id, ApprovalDecisionType::AllowOnce, None),
            "resolving an abandoned (receiver-closed) approval must report false"
        );
    }

    #[tokio::test]
    async fn resolve_by_id_reports_false_for_an_expired_entry() {
        // An expired card is not a decision surface: resolving it by id must be
        // a no-op false, matching `resolve_for_session`'s liveness filter and
        // the clarification twin's `resolve_after_expiry`.
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 1); // 1ms window
        let (id, _rx, _timeout) = manager.register_pending(record);
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(
            !manager.resolve(&id, ApprovalDecisionType::AllowOnce, None),
            "resolving an expired approval by id must report false"
        );
    }

    /// A resolve stamps the decision, wakes the waiter, and takes the record
    /// out of the pending set.
    ///
    /// That last part is the contract `get_pending` states — it filters on
    /// `is_live()`, and an entry whose waiter has been woken is not live. This
    /// asserted `is_some()` from back when a resolved record stayed readable
    /// by id, and had been failing ever since the liveness filter (the thing
    /// that stops a resolved-or-abandoned approval from being resolved twice)
    /// landed. The decision is now read where it actually has to arrive: at
    /// the waiter.
    ///
    /// It also goes through `register_pending` rather than reaching into the
    /// map, so what is under test is the real registration path.
    #[tokio::test]
    async fn test_resolve_approval() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let (id, rx, _timeout) = manager.register_pending(record);

        assert!(manager.resolve(&id, ApprovalDecisionType::AllowOnce, None));
        assert_eq!(
            rx.await.expect("the waiter must be woken, not dropped"),
            Some(ApprovalDecisionType::AllowOnce)
        );
        assert!(
            manager.get_pending(&id).is_none(),
            "a resolved approval is no longer pending"
        );
        assert!(
            !manager.resolve(&id, ApprovalDecisionType::Deny, None),
            "a second resolve must report false rather than overwrite the decision"
        );
    }

    #[test]
    fn test_list_pending() {
        let manager = ExecApprovalManager::new();

        // Register through the real path and KEEP the receivers alive: an entry
        // whose waiter is gone is a zombie and deliberately not listed.
        let record1 = manager.create(&mock_request(), 60_000);
        let (_id1, _rx1, _t1) = manager.register_pending(record1);
        let record2 = manager.create(&mock_request(), 60_000);
        let (_id2, _rx2, _t2) = manager.register_pending(record2);

        let pending = manager.list_pending();
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn abandoned_waiter_cannot_steal_a_live_cards_session_approval() {
        let manager = ExecApprovalManager::new();

        // Run A raises a card, then is cancelled: its waiter future is dropped
        // while the entry is still well inside its 60s deadline.
        let mut cancelled = mock_request();
        cancelled.id = "zombie".to_string();
        let rec = manager.create(&cancelled, 60_000);
        let session_key = rec.session_key.clone();
        let (_zid, rx, _t) = manager.register_pending(rec);
        drop(rx); // cancelled run — receiver gone, sender still Some

        // Run B raises a real card on the same session.
        let mut live = mock_request();
        live.id = "live".to_string();
        let rec2 = manager.create(&live, 60_000);
        let (live_id, rx2, timeout) = manager.register_pending(rec2);

        // The user types `/approve` on the channel. One LIVE card — the
        // zombie must not make this ambiguous.
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowOnce,
                None,
                None,
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::AllowOnce,
                ..
            }
        ));

        // It must reach the LIVE card, not the older zombie.
        assert_eq!(
            manager
                .await_registered(live_id, rx2, timeout)
                .await
                .decision,
            Some(ApprovalDecisionType::AllowOnce),
            "the live card must receive the approval; an abandoned waiter must not \
             win the session FIFO"
        );
        // And the zombie must not be renderable as a card.
        assert!(manager
            .list_pending()
            .iter()
            .all(|p| p.record.id != "zombie"));
    }

    /// `/deny <reason>` end-to-end at the manager layer: the reason stamped by
    /// the resolver must come back out of `await_registered` with the decision,
    /// and must NOT survive onto an approval.
    #[tokio::test]
    async fn deny_reason_rides_the_resolved_decision() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let session_key = record.session_key.clone();
        let (id, rx, timeout) = manager.register_pending(record);
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::Deny,
                None,
                Some("wrong directory, use /tmp".to_string()),
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::Deny,
                ..
            }
        ));
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::Deny));
        assert_eq!(
            resolved.deny_reason.as_deref(),
            Some("wrong directory, use /tmp")
        );

        // A reason on an APPROVAL is dropped — it means nothing there.
        let record = manager.create(&mock_request(), 60_000);
        let (id, rx, timeout) = manager.register_pending(record);
        assert!(manager.resolve_with_reason(
            &id,
            ApprovalDecisionType::AllowOnce,
            None,
            Some("noise".to_string()),
        ));
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
        assert_eq!(resolved.deny_reason, None);
    }

    #[test]
    fn clamp_downgrades_allow_always_to_session() {
        // A card raised at the session ceiling can never produce more than a
        // session grant, whoever posts the decision.
        let ceiling = crate::exec::allowed_decisions::session_max();
        assert_eq!(
            ExecApprovalManager::clamp_decision(ApprovalDecisionType::AllowAlways, &ceiling),
            ApprovalDecisionType::AllowSession
        );
        // Other decisions pass through untouched; approvals are never escalated
        // or turned into denials here.
        for decision in [
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ] {
            assert_eq!(
                ExecApprovalManager::clamp_decision(decision, &ceiling),
                decision
            );
        }
    }

    /// The other half of the same rule: when the card DID offer the persistent
    /// tier, the human's answer survives to the waiter unchanged. Without this
    /// the feature is unreachable; without the test above it is unbounded.
    #[tokio::test]
    async fn a_card_that_offered_the_persistent_tier_keeps_it() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&persistent_capable_request(), 60_000);
        let (id, rx, timeout) = manager.register_pending(record);
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowAlways, None));
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowAlways));
    }

    /// The wire is the attack surface, not the button: a client that posts
    /// `allow-always` at a card raised WITHOUT the tier is narrowed, not obeyed.
    /// This is the whole reason `allowed_decisions` is enforced server-side.
    #[tokio::test]
    async fn an_unoffered_allow_always_is_narrowed_on_the_wire() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let (id, rx, timeout) = manager.register_pending(record);
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowAlways, None));
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(
            resolved.decision,
            Some(ApprovalDecisionType::AllowSession),
            "a decision the card never offered must not reach the waiter"
        );
    }

    #[test]
    fn resolve_reports_session_grant_for_allow_always() {
        // The effective decision a resolver sees must never claim permanence.
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let session_key = record.session_key.clone();
        let (_id, _rx, _timeout) = manager.register_pending(record);

        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowAlways,
                None,
                None,
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::AllowSession,
                ..
            }
        ));
    }

    /// The widest answer must not be the one that fails to release its
    /// siblings. The cascade condition was `== AllowSession`, written when the
    /// persistent tier could not exist; a user answering "always" on one of two
    /// identical parked cards would have left the other waiting for a click.
    #[tokio::test]
    async fn a_persistent_grant_cascades_to_identical_pending_cards() {
        let manager = ExecApprovalManager::new();
        let mut first = persistent_capable_request();
        first.id = "card-1".to_string();
        first.grant_key = Some("fp-shared".to_string());
        let rec1 = manager.create(&first, 60_000);
        let (id1, rx1, t1) = manager.register_pending(rec1);

        let mut second = persistent_capable_request();
        second.id = "card-2".to_string();
        second.grant_key = Some("fp-shared".to_string());
        let rec2 = manager.create(&second, 60_000);
        let (id2, rx2, t2) = manager.register_pending(rec2);

        // The human answers ONE card with the persistent tier.
        assert!(manager.resolve(&id1, ApprovalDecisionType::AllowAlways, None));

        assert_eq!(
            manager.await_registered(id1, rx1, t1).await.decision,
            Some(ApprovalDecisionType::AllowAlways)
        );
        assert_eq!(
            manager.await_registered(id2, rx2, t2).await.decision,
            Some(ApprovalDecisionType::AllowAlways),
            "the identical card parked beside it must not still be waiting"
        );
    }

    /// The multi-pending guard: with two live cards on one session a bare
    /// `/approve` must resolve NOTHING (FIFO would approve a command the user
    /// may never have read), and an indexed reply must hit exactly the card
    /// it names.
    #[test]
    fn bare_session_resolve_refuses_when_several_cards_pend() {
        let manager = ExecApprovalManager::new();
        let mut first = mock_request();
        first.id = "card-1".to_string();
        first.command = "rm -rf ./build".to_string();
        let rec1 = manager.create(&first, 60_000);
        let session_key = rec1.session_key.clone();
        let (id1, rx1, t1) = manager.register_pending(rec1);

        let mut second = mock_request();
        second.id = "card-2".to_string();
        second.command = "git push --force".to_string();
        let rec2 = manager.create(&second, 60_000);
        let (_id2, _rx2, _t2) = manager.register_pending(rec2);

        // Bare reply → ambiguous listing, oldest first, nothing resolved.
        match manager.resolve_for_session(
            &session_key,
            None,
            ApprovalDecisionType::AllowOnce,
            None,
            None,
        ) {
            SessionResolveOutcome::Ambiguous(cards) => {
                assert_eq!(cards.len(), 2);
                assert_eq!(cards[0], (1, "rm -rf ./build".to_string()));
                assert_eq!(cards[1], (2, "git push --force".to_string()));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
        assert_eq!(manager.list_pending().len(), 2, "nothing was resolved");

        // Out-of-range index re-lists rather than guessing.
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                Some(9),
                ApprovalDecisionType::AllowOnce,
                None,
                None,
            ),
            SessionResolveOutcome::Ambiguous(_)
        ));

        // `/approve 1` hits exactly the oldest card and echoes it.
        match manager.resolve_for_session(
            &session_key,
            Some(1),
            ApprovalDecisionType::AllowOnce,
            None,
            None,
        ) {
            SessionResolveOutcome::Resolved { decision, summary } => {
                assert_eq!(decision, ApprovalDecisionType::AllowOnce);
                assert_eq!(summary, "rm -rf ./build");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
        drop((id1, rx1, t1));
    }

    /// Index-drift regression: a positional reply binds to the listing the
    /// user was SHOWN. After position 1 resolves out of band and a NEW card
    /// arrives, `/approve 2` from the old listing must hit the old
    /// position-2 card — never the newcomer that now occupies live
    /// position 2 (in range, so a live-positional lookup would resolve a
    /// command the user never read).
    #[test]
    fn indexed_reply_binds_to_the_listing_shown_not_live_positions() {
        let manager = ExecApprovalManager::new();
        let mk = |id: &str, cmd: &str| {
            let mut r = mock_request();
            r.id = id.to_string();
            r.command = cmd.to_string();
            r
        };

        let rec_a = manager.create(&mk("card-a", "cmd-a"), 60_000);
        let session_key = rec_a.session_key.clone();
        let (id_a, _rx_a, _ta) = manager.register_pending(rec_a);
        let rec_b = manager.create(&mk("card-b", "cmd-b"), 60_000);
        let (_id_b, _rx_b, _tb) = manager.register_pending(rec_b);

        // The listing the user reads: [1=a, 2=b] (snapshotted).
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowOnce,
                None,
                None,
            ),
            SessionResolveOutcome::Ambiguous(_)
        ));

        // Card a resolves out of band (button / Panel resolve by exact id)…
        assert!(manager.resolve(&id_a, ApprovalDecisionType::AllowOnce, None));
        // …and a NEW card c arrives, occupying live position 2.
        let rec_c = manager.create(&mk("card-c", "cmd-c"), 60_000);
        let (_id_c, _rx_c, _tc) = manager.register_pending(rec_c);

        // `/approve 2` from the OLD listing resolves b — what the user read.
        match manager.resolve_for_session(
            &session_key,
            Some(2),
            ApprovalDecisionType::AllowOnce,
            None,
            None,
        ) {
            SessionResolveOutcome::Resolved { summary, .. } => assert_eq!(summary, "cmd-b"),
            other => panic!("expected Resolved(cmd-b), got {other:?}"),
        }

        // `/approve 1` addresses the already-resolved a → fresh listing (c
        // alone), nothing resolved.
        match manager.resolve_for_session(
            &session_key,
            Some(1),
            ApprovalDecisionType::AllowOnce,
            None,
            None,
        ) {
            SessionResolveOutcome::Ambiguous(cards) => {
                assert_eq!(cards, vec![(1, "cmd-c".to_string())]);
            }
            other => panic!("expected re-list, got {other:?}"),
        }

        // The fresh listing re-snapshotted — `/approve 1` now hits c.
        match manager.resolve_for_session(
            &session_key,
            Some(1),
            ApprovalDecisionType::AllowOnce,
            None,
            None,
        ) {
            SessionResolveOutcome::Resolved { summary, .. } => assert_eq!(summary, "cmd-c"),
            other => panic!("expected Resolved(cmd-c), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_pending_sweeps_orphaned_expired_entries() {
        // An entry whose waiter future was dropped never reaches
        // `await_registered`'s removal; the next registration must evict it.
        let manager = ExecApprovalManager::new();

        let mut stale = mock_request();
        stale.id = "stale-entry".to_string();
        let record = manager.create(&stale, 1); // 1ms lifetime
        let (stale_id, rx, _timeout) = manager.register_pending(record);
        drop(rx); // waiter abandoned — simulates an aborted agent run

        // Let the 1ms lifetime elapse for real before the sweeping call, so
        // the eviction never races the clock.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Next registration sweeps the expired orphan.
        let record2 = manager.create(&mock_request(), 60_000);
        let (live_id, _rx2, _t2) = manager.register_pending(record2);

        assert!(manager.get_pending(&stale_id).is_none(), "orphan evicted");
        assert!(manager.get_pending(&live_id).is_some(), "live entry kept");
    }

    /// A request carrying a session-grant identity (`grant_key`), as the
    /// bridge/operator requester stamp it from `ApprovalAction::grant_key`.
    fn keyed_request(id: &str, session: &str, grant_key: Option<&str>) -> ApprovalRequest {
        let mut r = mock_request();
        r.id = id.to_string();
        r.session_key = session.to_string();
        r.grant_key = grant_key.map(str::to_string);
        r
    }

    /// ① The concurrent-subagent case: two identical calls each parked a card
    /// before the user answered the first. A session-level grant on one card
    /// must resolve the other through the same internal path a manual resolve
    /// takes (mirrors kimi-cli `approval.py`'s session-approval fan-out).
    #[tokio::test]
    async fn session_grant_cascades_to_same_action_pending_cards() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("c-a", "s1", Some("k1")), 60_000);
        let rec_b = manager.create(&keyed_request("c-b", "s1", Some("k1")), 60_000);
        let (id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, rx_b, t_b) = manager.register_pending(rec_b);

        assert!(manager.resolve(&id_a, ApprovalDecisionType::AllowSession, None));

        let cascaded = manager.await_registered(id_b, rx_b, t_b).await;
        assert_eq!(
            cascaded.decision,
            Some(ApprovalDecisionType::AllowSession),
            "the identical pending card must inherit the session grant"
        );
    }

    /// ②③ A session grant is scoped to (session, action): a different
    /// `grant_key` in the same session and the same `grant_key` in a different
    /// session must both stay pending.
    #[tokio::test]
    async fn session_grant_does_not_cross_action_or_session() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("x-a", "s1", Some("k1")), 60_000);
        let rec_b = manager.create(&keyed_request("x-b", "s1", Some("k2")), 60_000);
        let rec_c = manager.create(&keyed_request("x-c", "s2", Some("k1")), 60_000);
        let (id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, _rx_b, _t_b) = manager.register_pending(rec_b);
        let (id_c, _rx_c, _t_c) = manager.register_pending(rec_c);

        assert!(manager.resolve(&id_a, ApprovalDecisionType::AllowSession, None));

        for id in [&id_b, &id_c] {
            let record = &manager.get_pending(id).expect("still pending").record;
            assert!(
                record.decision.is_none(),
                "{id} must not be resolved by an unrelated session grant"
            );
        }
    }

    /// ④ Records with no action identity (`grant_key: None` — bare route
    /// escalations, cluster node approvals) never cascade, even when two of
    /// them share a session.
    #[tokio::test]
    async fn none_grant_key_never_cascades() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("n-a", "s1", None), 60_000);
        let rec_b = manager.create(&keyed_request("n-b", "s1", None), 60_000);
        let (id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, _rx_b, _t_b) = manager.register_pending(rec_b);

        assert!(manager.resolve(&id_a, ApprovalDecisionType::AllowSession, None));

        let record = &manager.get_pending(&id_b).expect("still pending").record;
        assert!(
            record.decision.is_none(),
            "a None grant_key must never inherit a session grant"
        );
    }

    /// ⑤ `AllowOnce` covers one invocation by definition and may not resolve a
    /// sibling card.
    #[tokio::test]
    async fn allow_once_does_not_cascade() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("d-a", "s1", Some("k1")), 60_000);
        let rec_b = manager.create(&keyed_request("d-b", "s1", Some("k1")), 60_000);
        let (id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, _rx_b, _t_b) = manager.register_pending(rec_b);

        assert!(manager.resolve(&id_a, ApprovalDecisionType::AllowOnce, None));

        let record = &manager.get_pending(&id_b).expect("still pending").record;
        assert!(
            record.decision.is_none(),
            "AllowOnce must not cascade to a sibling pending card"
        );
    }

    /// ⑥ A refusal reaches the identical cards parked beside it, carrying the
    /// human's own words.
    ///
    /// The mirror of `session_grant_cascades_to_same_action_pending_cards`, and
    /// it used to be asserted the other way round. What settled it is that the
    /// denial ledger already treats a refusal as being about the ACTION — the
    /// next identical call is auto-refused with no card — so leaving the
    /// already-parked twin out made the coverage of one "no" depend on a race
    /// between the sibling's ledger check and the human's click. It also cost
    /// the user real damage: every extra card they dismissed was another
    /// refusal counted by the brute-force breaker.
    #[tokio::test]
    async fn a_refusal_reaches_the_identical_cards_parked_beside_it() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("r-a", "s1", Some("k1")), 60_000);
        let rec_b = manager.create(&keyed_request("r-b", "s1", Some("k1")), 60_000);
        let rec_c = manager.create(&keyed_request("r-c", "s1", Some("k2")), 60_000);
        let (id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, rx_b, t_b) = manager.register_pending(rec_b);
        let (id_c, _rx_c, _t_c) = manager.register_pending(rec_c);

        assert!(manager.resolve_with_reason(
            &id_a,
            ApprovalDecisionType::Deny,
            None,
            Some("not on production".to_string()),
        ));

        let cascaded = manager.await_registered(id_b, rx_b, t_b).await;
        assert_eq!(cascaded.decision, Some(ApprovalDecisionType::Deny));
        assert_eq!(
            cascaded.deny_reason.as_deref(),
            Some("not on production"),
            "a sibling refused by cascade must reach the model with the same instruction"
        );

        // A different action in the same session is untouched — the cascade is
        // keyed on the action, never on the session alone.
        assert!(manager
            .get_pending(&id_c)
            .expect("still pending")
            .record
            .decision
            .is_none());
    }

    /// The cascade also fires on the session-addressed path (`/approve
    /// session` text reply), not only on by-id resolves.
    #[tokio::test]
    async fn session_grant_cascades_via_resolve_for_session() {
        let manager = ExecApprovalManager::new();
        let rec_a = manager.create(&keyed_request("f-a", "s1", Some("k1")), 60_000);
        let rec_b = manager.create(&keyed_request("f-b", "s1", Some("k1")), 60_000);
        let (_id_a, _rx_a, _t_a) = manager.register_pending(rec_a);
        let (id_b, rx_b, t_b) = manager.register_pending(rec_b);

        // Two live cards on one session: a bare reply is ambiguous, but it
        // snapshots the listing…
        assert!(matches!(
            manager.resolve_for_session("s1", None, ApprovalDecisionType::AllowSession, None, None),
            SessionResolveOutcome::Ambiguous(_)
        ));
        // …which the indexed reply then addresses.
        assert!(matches!(
            manager.resolve_for_session(
                "s1",
                Some(1),
                ApprovalDecisionType::AllowSession,
                None,
                None,
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::AllowSession,
                ..
            }
        ));

        let cascaded = manager.await_registered(id_b, rx_b, t_b).await;
        assert_eq!(
            cascaded.decision,
            Some(ApprovalDecisionType::AllowSession),
            "an indexed /approve session must cascade to the identical card"
        );
    }

    /// The `resolve_for_session` originator gate (the group-chat / paired-chat
    /// bypass guard commented above at the `if let Some(ref expected)` block):
    /// a record raised with a known originator resolves via the
    /// session/text-reply path ONLY for that user. Historically fed only by a
    /// channel's raw sender id (`ChannelApprovalBridgeAdapter`); as of
    /// `teams::broadcast::member_run_metadata`, a group-chat member run now
    /// stamps this same field from the room's speaker
    /// (`scope::current_room_author()`), so this gate also protects a
    /// member's parked tool call from being resolved by a DIFFERENT human in
    /// the same room.
    #[tokio::test]
    async fn resolve_for_session_originator_gate_rejects_non_originator_and_admits_the_speaker() {
        let manager = ExecApprovalManager::new();
        let mut req = mock_request();
        req.originator_user_id = Some("u-bob".to_string());
        let record = manager.create(&req, 60_000);
        let session_key = record.session_key.clone();
        let (id, rx, timeout) = manager.register_pending(record);

        // u-carol (not the originator) cannot resolve by session/text-reply —
        // the entry is left untouched (still live), not consumed.
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowOnce,
                Some("u-carol".to_string()),
                None,
            ),
            SessionResolveOutcome::NothingPending
        ));
        assert!(
            manager.get_pending(&id).is_some(),
            "a non-originator's refused resolve must not have touched the entry"
        );

        // u-bob (the originator) resolves it normally.
        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowOnce,
                Some("u-bob".to_string()),
                None,
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::AllowOnce,
                ..
            }
        ));

        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }

    /// The `None`-originator side of the same gate: a record that carries no
    /// originator (legacy record, or any non-channel/non-team caller) keeps
    /// the pre-originator behaviour — the gate is a no-op and any
    /// `resolved_by` succeeds. This is also the arm that covers admin/operator
    /// resolution for THIS gate specifically: an operator resolving via
    /// `exec.approval.resolve` never goes through `resolve_for_session` at
    /// all (that RPC calls `resolve_with_reason` directly — see
    /// `gateway::handlers::exec_approvals::handle_approval_resolve` — and its
    /// own admission gate, `approval_addressable_by_caller`, short-circuits
    /// `true` for a non-member caller before ever looking at
    /// `originator_user_id`; already covered by
    /// `an_operator_still_sees_a_members_parked_approval` in
    /// `gateway::handlers::exec_approvals`, unchanged by this task).
    #[tokio::test]
    async fn resolve_for_session_originator_gate_is_a_noop_with_no_originator() {
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let session_key = record.session_key.clone();
        let (id, rx, timeout) = manager.register_pending(record);

        assert!(matches!(
            manager.resolve_for_session(
                &session_key,
                None,
                ApprovalDecisionType::AllowOnce,
                Some("anyone".to_string()),
                None,
            ),
            SessionResolveOutcome::Resolved {
                decision: ApprovalDecisionType::AllowOnce,
                ..
            }
        ));
        let resolved = manager.await_registered(id, rx, timeout).await;
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }
}
