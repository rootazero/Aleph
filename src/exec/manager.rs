//! Exec approval manager for handling approval requests and decisions.
//!
//! Provides async approval flow with timeout and event broadcasting.

use crate::sync_primitives::{Arc, RwLock};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use super::decision::ApprovalRequest;
use super::socket::ApprovalDecisionType;

/// Default timeout for approval requests (2 minutes)
pub const DEFAULT_APPROVAL_TIMEOUT_MS: u64 = 120_000;

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
}

impl ExecApprovalRecord {
    /// Create from `ApprovalRequest`
    #[must_use]
    pub fn from_request(request: &ApprovalRequest, timeout_ms: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

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
            expires_at_ms: now + timeout_ms,
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
        }
    }

    /// Check if expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now > self.expires_at_ms
    }

    /// Check if resolved
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
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
/// (`AllowOnce`) or for the session (`AllowSession`, remembered by
/// [`crate::sandbox::exec_approval::session_memory`]). Nothing here persists.
pub struct ExecApprovalManager {
    pending: Arc<RwLock<HashMap<String, PendingEntry>>>,
}

impl ExecApprovalManager {
    /// Create new manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
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
    pub fn create(&self, request: &ApprovalRequest, timeout_ms: u64) -> ExecApprovalRecord {
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
        Duration,
    ) {
        // Use remaining time from now, not the full original timeout window.
        // This prevents granting a full timeout if the wait begins long after
        // the record was created.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let remaining_ms = record.expires_at_ms.saturating_sub(now_ms);
        let timeout = Duration::from_millis(remaining_ms);

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
        timeout: Duration,
    ) -> ResolvedDecision {
        // Wait with timeout
        let result = tokio::time::timeout(timeout, rx).await;

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
    /// `true` if the request was found and resolved
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

        if let Some(entry) = pending.get_mut(id) {
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

            let decision = Self::clamp_decision(decision);
            entry.record.decision = Some(decision);
            entry.record.resolved_by = resolved_by;
            if decision == ApprovalDecisionType::Deny {
                entry.record.deny_reason = deny_reason;
            }
            entry.record.resolved_at_ms = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            );

            // `is_live()` above proved the receiver is still open under this
            // same lock, so the send delivers — no silent drop to a zombie.
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(Some(decision));
            }

            debug!(id = %id, ?decision, "Resolved approval");
            true
        } else {
            warn!(id = %id, "Approval not found or already resolved");
            false
        }
    }

    /// Resolve the oldest unresolved approval for `session_key`.
    ///
    /// A channel reply (`/approve` / `/deny` or a button callback) carries no
    /// request id, so the inbound router resolves by session: the oldest LIVE
    /// pending entry for that session is picked (FIFO). Returns the EFFECTIVE
    /// decision applied (post clamp — a legacy `AllowAlways` narrows to
    /// `AllowSession`), or `None` when nothing was pending for the session.
    ///
    /// Liveness ([`PendingEntry::is_live`]) is what keeps the FIFO honest: a
    /// cancelled run leaves an entry whose waiter is gone but whose `sender` is
    /// still `Some`, and being the oldest it would otherwise win this pick and
    /// swallow the approval meant for the card the user is actually looking at.
    pub fn resolve_for_session(
        &self,
        session_key: &str,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        deny_reason: Option<String>,
    ) -> Option<ApprovalDecisionType> {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());

        let target = pending
            .iter()
            .filter(|(_, e)| e.is_live() && e.record.session_key == session_key)
            .min_by(|(id_a, e_a), (id_b, e_b)| {
                e_a.created_at
                    .cmp(&e_b.created_at)
                    .then_with(|| id_a.cmp(id_b))
            })
            .map(|(id, _)| id.clone());

        let Some(id) = target else {
            warn!(session_key = %session_key, "No pending approval for session");
            return None;
        };

        if let Some(entry) = pending.get_mut(&id) {
            let decision = Self::clamp_decision(decision);
            entry.record.decision = Some(decision);
            entry.record.resolved_by = resolved_by;
            if decision == ApprovalDecisionType::Deny {
                entry.record.deny_reason = deny_reason;
            }
            entry.record.resolved_at_ms = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            );
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(Some(decision));
            }
            debug!(id = %id, ?decision, "Resolved approval by session");
            Some(decision)
        } else {
            None
        }
    }

    /// Clamp `requested` to a grant scope the system can actually honor.
    ///
    /// No persistent allowlist exists, so `AllowAlways` cannot outlive the
    /// process: it is a legacy wire value (old inline-keyboard callbacks,
    /// `/approve always` text replies, external RPC clients) and resolves to
    /// the session tier. An explicit human approval is never escalated or
    /// turned into a denial here — only the grant scope is narrowed.
    const fn clamp_decision(requested: ApprovalDecisionType) -> ApprovalDecisionType {
        match requested {
            ApprovalDecisionType::AllowAlways => ApprovalDecisionType::AllowSession,
            other => other,
        }
    }

    /// Get snapshot of a pending approval
    #[must_use]
    pub fn get_pending(&self, id: &str) -> Option<PendingApproval> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        pending.get(id).map(|entry| {
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
    pub fn cleanup_expired(&self) {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        pending.retain(|id, entry| {
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

    fn mock_request() -> ApprovalRequest {
        ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: "npm install".to_string(),
            cwd: Some("/project".to_string()),
            analysis: CommandAnalysis {
                ok: true,
                reason: None,
                segments: vec![],
                chains: None,
            },
            agent_id: "main".to_string(),
            session_key: "agent:main:main".to_string(),
            reason: None,
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

    /// Defect D: with two tool calls in flight the client cannot pair approvals
    /// to tool rows by position — the pending map is unordered. Each record must
    /// carry the id of the tool call it actually gates, and `list_pending` must
    /// come out in a stable, oldest-first order.
    #[tokio::test]
    async fn concurrent_approvals_carry_distinct_tool_call_ids() {
        use crate::approval::with_tool_call_id;

        let manager = ExecApprovalManager::new();
        let mut first = mock_request();
        first.id = "ap-a".to_string();
        let mut second = mock_request();
        second.id = "ap-b".to_string();

        let rec_a = with_tool_call_id(Some("toolu_a".to_string()), async {
            manager.create(&first, 60_000)
        })
        .await;
        let rec_b = with_tool_call_id(Some("toolu_b".to_string()), async {
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

    #[tokio::test]
    async fn test_resolve_approval() {
        let manager = ExecApprovalManager::new();
        let request = mock_request();
        let record = manager.create(&request, 60_000);
        let id = record.id.clone();

        // Spawn wait task
        let manager_clone = ExecApprovalManager::new();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        manager_clone
            .pending
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.clone(),
                PendingEntry {
                    record: record.clone(),
                    sender: Some(tx),
                    created_at: Instant::now(),
                },
            );

        // Resolve
        let resolved = manager_clone.resolve(&id, ApprovalDecisionType::AllowOnce, None);
        assert!(resolved);

        // Check pending
        let pending = manager_clone.get_pending(&id);
        assert!(pending.is_some());
        assert_eq!(
            pending.unwrap().record.decision,
            Some(ApprovalDecisionType::AllowOnce)
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

        // The user types `/approve` on the channel.
        assert_eq!(
            manager.resolve_for_session(&session_key, ApprovalDecisionType::AllowOnce, None, None),
            Some(ApprovalDecisionType::AllowOnce)
        );

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
        assert_eq!(
            manager.resolve_for_session(
                &session_key,
                ApprovalDecisionType::Deny,
                None,
                Some("wrong directory, use /tmp".to_string()),
            ),
            Some(ApprovalDecisionType::Deny)
        );
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
        // No persistent allowlist exists, so `AllowAlways` can never mean more
        // than a session grant — at any risk level.
        assert_eq!(
            ExecApprovalManager::clamp_decision(ApprovalDecisionType::AllowAlways),
            ApprovalDecisionType::AllowSession
        );
        // Other decisions pass through untouched; approvals are never escalated
        // or turned into denials here.
        for decision in [
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::Deny,
        ] {
            assert_eq!(ExecApprovalManager::clamp_decision(decision), decision);
        }
    }

    #[test]
    fn resolve_reports_session_grant_for_allow_always() {
        // The effective decision a resolver sees must never claim permanence.
        let manager = ExecApprovalManager::new();
        let record = manager.create(&mock_request(), 60_000);
        let session_key = record.session_key.clone();
        let (_id, _rx, _timeout) = manager.register_pending(record);

        assert_eq!(
            manager.resolve_for_session(
                &session_key,
                ApprovalDecisionType::AllowAlways,
                None,
                None
            ),
            Some(ApprovalDecisionType::AllowSession)
        );
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
}
