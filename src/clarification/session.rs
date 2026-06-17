//! Clarification Session Management
//!
//! Tracks pending clarification requests. The agent asks the user a question
//! mid-task via the `ask_user` tool, parks on a oneshot, and resumes when the
//! user's reply is routed back here by the inbound router.
//!
//! This is the clarification twin of [`crate::exec::manager::ExecApprovalManager`]:
//! a per-session registry of pending interactions, each backed by a
//! `oneshot::Sender` the resolver fires.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use alephcore::clarification::{ClarificationManager, ClarificationRequest};
//!
//! # async fn example() {
//! let manager = ClarificationManager::new();
//! let request = ClarificationRequest::text("ask-1", "Which language?", None);
//! let rx = manager
//!     .register("telegram:bot:123:user", request, Duration::from_secs(600))
//!     .await;
//! // ... inbound router calls `manager.resolve(session_key, reply)` ...
//! let result = rx.await;
//! # let _ = result;
//! # }
//! ```

use super::{ClarificationRequest, ClarificationResult, ClarificationType};
use crate::sync_primitives::{Arc, AsyncRwLock};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Default time a clarification waits for the user before timing out.
///
/// Generous on purpose: users type thoughtful answers, unlike a yes/no
/// approval. Mirrors hermes-agent's 600s `clarify` timeout.
pub const DEFAULT_CLARIFY_TIMEOUT: Duration = Duration::from_secs(600);

/// A clarification awaiting the user's reply.
struct PendingEntry {
    request: ClarificationRequest,
    sender: Option<oneshot::Sender<ClarificationResult>>,
    created_at: Instant,
    timeout: Duration,
}

impl PendingEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.timeout
    }
}

/// Per-session registry of pending clarifications.
///
/// Keyed by channel session key. At most one clarification is pending per
/// session — registering a second one supersedes the first, and the
/// superseded waiter receives [`ClarificationResult::cancelled`] so its
/// `ask_user` tool call unblocks instead of hanging.
#[derive(Clone, Default)]
pub struct ClarificationManager {
    pending: Arc<AsyncRwLock<HashMap<String, PendingEntry>>>,
}

impl ClarificationManager {
    /// Create an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a clarification for `session_key`.
    ///
    /// Returns a receiver that resolves when the user replies (via
    /// [`resolve`](Self::resolve)), is superseded, or times out
    /// ([`cleanup_expired`](Self::cleanup_expired)).
    pub async fn register(
        &self,
        session_key: impl Into<String>,
        request: ClarificationRequest,
        timeout: Duration,
    ) -> oneshot::Receiver<ClarificationResult> {
        // Opportunistic sweep: an `ask_user` whose future was dropped (aborted
        // agent run) never reaches its own `cancel`, so its entry would linger
        // forever — `cleanup_expired` has no scheduled caller. Each new
        // registration reaps anything already past its deadline, mirroring
        // [`ExecApprovalManager::register_pending`]. Bounded work, no background
        // task (R10).
        self.cleanup_expired().await;

        let (tx, rx) = oneshot::channel();
        let entry = PendingEntry {
            request,
            sender: Some(tx),
            created_at: Instant::now(),
            timeout,
        };
        let mut pending = self.pending.write().await;
        if let Some(mut old) = pending.insert(session_key.into(), entry) {
            // Supersede: unblock the old waiter so its tool call doesn't hang.
            if let Some(sender) = old.sender.take() {
                let _ = sender.send(ClarificationResult::cancelled());
            }
        }
        rx
    }

    /// Whether `session_key` has a live (non-expired) pending clarification.
    pub async fn has_pending(&self, session_key: &str) -> bool {
        self.pending
            .read()
            .await
            .get(session_key)
            .is_some_and(|e| !e.is_expired())
    }

    /// Resolve the pending clarification for `session_key` from the user's
    /// reply text. Returns `true` if a pending entry was resolved.
    ///
    /// The reply is interpreted against the request: for a select-type
    /// request a 1-based numeric reply picks that option, a reply matching an
    /// option label/value (case-insensitive) selects it, and anything else is
    /// taken as free text.
    pub async fn resolve(&self, session_key: &str, reply: &str) -> bool {
        let mut pending = self.pending.write().await;
        let Some(mut entry) = pending.remove(session_key) else {
            return false;
        };
        if entry.is_expired() {
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(ClarificationResult::timeout());
            }
            return true;
        }
        let result = interpret_reply(&entry.request, reply);
        match entry.sender.take() {
            Some(sender) => sender.send(result).is_ok(),
            None => false,
        }
    }

    /// Cancel the pending clarification for `session_key`, if any, unblocking
    /// its waiter with [`ClarificationResult::cancelled`].
    pub async fn cancel(&self, session_key: &str) -> bool {
        let mut pending = self.pending.write().await;
        match pending.remove(session_key) {
            Some(mut entry) => {
                if let Some(sender) = entry.sender.take() {
                    let _ = sender.send(ClarificationResult::cancelled());
                }
                true
            }
            None => false,
        }
    }

    /// Drop expired entries, unblocking their waiters with a timeout result.
    /// Returns the number of entries reaped.
    pub async fn cleanup_expired(&self) -> usize {
        let mut pending = self.pending.write().await;
        let mut count = 0;
        pending.retain(|_, entry| {
            if entry.is_expired() {
                if let Some(sender) = entry.sender.take() {
                    let _ = sender.send(ClarificationResult::timeout());
                }
                count += 1;
                false
            } else {
                true
            }
        });
        count
    }
}

/// Maximum length of a user reply before truncation.
const MAX_REPLY_LENGTH: usize = 10_000;

/// Map a user's free-text reply onto a [`ClarificationResult`] for `request`.
///
/// `pub(crate)` so the inbound router can reuse the exact same number / label /
/// free-text interpretation when resolving a workflow `clarify` step (which is
/// backed by a durable `coord_task`, not this in-memory registry).
pub(crate) fn interpret_reply(request: &ClarificationRequest, reply: &str) -> ClarificationResult {
    let trimmed = reply.trim();
    let trimmed = if trimmed.len() > MAX_REPLY_LENGTH {
        let mut end = MAX_REPLY_LENGTH;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };

    match request.clarification_type {
        ClarificationType::Select => {
            if let Some(options) = &request.options {
                // 1-based numeric selection.
                if let Ok(n) = trimmed.parse::<usize>() {
                    if n >= 1 && n <= options.len() {
                        let opt = &options[n - 1];
                        return ClarificationResult::selected((n - 1) as u32, opt.value.clone());
                    }
                }
                // Match against an option value or label (case-insensitive).
                let lower = trimmed.to_lowercase();
                for (i, opt) in options.iter().enumerate() {
                    if opt.value.to_lowercase() == lower || opt.label.to_lowercase() == lower {
                        return ClarificationResult::selected(i as u32, opt.value.clone());
                    }
                }
            }
            ClarificationResult::text_input(trimmed.to_string())
        }
        ClarificationType::Text => ClarificationResult::text_input(trimmed.to_string()),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::{ClarificationOption, ClarificationResultType};

    fn text_request() -> ClarificationRequest {
        ClarificationRequest::text("ask-1", "Which language?", None)
    }

    fn select_request() -> ClarificationRequest {
        ClarificationRequest::select(
            "ask-2",
            "Pick a style:",
            vec![
                ClarificationOption::new("pro", "Professional"),
                ClarificationOption::new("casual", "Casual"),
            ],
        )
    }

    #[tokio::test]
    async fn register_then_resolve_delivers_text_reply() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-a", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;
        assert!(mgr.has_pending("sess-a").await);

        assert!(mgr.resolve("sess-a", "  Spanish  ").await);
        let result = rx.await.unwrap();
        assert_eq!(result.get_value(), Some("Spanish"));
        assert!(!mgr.has_pending("sess-a").await);
    }

    #[tokio::test]
    async fn resolve_select_by_number_picks_option() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-b", select_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.resolve("sess-b", "2").await);
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index, Some(1));
        assert_eq!(result.get_value(), Some("casual"));
    }

    #[tokio::test]
    async fn resolve_select_by_label_picks_option() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-c", select_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.resolve("sess-c", "professional").await);
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index, Some(0));
        assert_eq!(result.get_value(), Some("pro"));
    }

    #[tokio::test]
    async fn resolve_select_out_of_range_falls_back_to_text() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-d", select_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        // "9" is not a valid 1-based index for a 2-option request.
        assert!(mgr.resolve("sess-d", "9").await);
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index, None);
        assert_eq!(result.get_value(), Some("9"));
    }

    #[tokio::test]
    async fn resolve_unknown_session_returns_false() {
        let mgr = ClarificationManager::new();
        assert!(!mgr.resolve("nope", "anything").await);
    }

    #[tokio::test]
    async fn registering_again_supersedes_and_cancels_old_waiter() {
        let mgr = ClarificationManager::new();
        let rx_old = mgr
            .register("sess-e", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;
        let _rx_new = mgr
            .register("sess-e", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        // The superseded waiter is cancelled, not left hanging.
        let old = rx_old.await.unwrap();
        assert_eq!(
            old.result_type,
            crate::clarification::ClarificationResultType::Cancelled
        );
    }

    #[tokio::test]
    async fn cancel_unblocks_waiter() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-f", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.cancel("sess-f").await);
        let result = rx.await.unwrap();
        assert_eq!(
            result.result_type,
            crate::clarification::ClarificationResultType::Cancelled
        );
        assert!(!mgr.has_pending("sess-f").await);
    }

    #[tokio::test]
    async fn cleanup_expired_times_out_stale_entries() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-g", text_request(), Duration::from_millis(1))
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(!mgr.has_pending("sess-g").await);
        assert_eq!(mgr.cleanup_expired().await, 1);
        let result = rx.await.unwrap();
        assert_eq!(
            result.result_type,
            crate::clarification::ClarificationResultType::Timeout
        );
    }

    #[tokio::test]
    async fn register_sweeps_orphaned_expired_entries() {
        // An `ask_user` future dropped before its timeout (aborted run) leaves
        // an entry that `cancel`/its own timeout never reap. The next
        // registration — even for a DIFFERENT session — must sweep it, so
        // orphans don't accumulate without a scheduled cleanup pass.
        let mgr = ClarificationManager::new();
        let rx_orphan = mgr
            .register("orphan-sess", text_request(), Duration::from_millis(1))
            .await;
        drop(rx_orphan); // waiter abandoned — simulates an aborted agent run
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Registering a live clarification for another session sweeps the orphan.
        let _rx_live = mgr
            .register("live-sess", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;
        assert!(
            !mgr.has_pending("orphan-sess").await,
            "expired orphan must be swept on the next registration"
        );
        assert!(
            mgr.has_pending("live-sess").await,
            "the live entry must be kept"
        );
    }

    #[tokio::test]
    async fn manager_clone_shares_state() {
        let mgr = ClarificationManager::new();
        let clone = mgr.clone();
        let _rx = mgr
            .register("sess-h", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;
        // The clone sees the same pending entry.
        assert!(clone.has_pending("sess-h").await);
        assert!(clone.resolve("sess-h", "via clone").await);
    }

    #[tokio::test]
    async fn resolve_whitespace_only_reply_falls_back_to_text() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-ws", select_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.resolve("sess-ws", "   ").await);
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::TextInput);
        assert_eq!(result.get_value(), Some(""));
    }

    #[tokio::test]
    async fn resolve_after_cancel_returns_false() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-cancel", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.cancel("sess-cancel").await);
        assert!(!mgr.resolve("sess-cancel", "reply").await);
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
    }

    #[tokio::test]
    async fn cleanup_after_resolve_is_noop() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-clean", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        assert!(mgr.resolve("sess-clean", "reply").await);
        assert_eq!(mgr.cleanup_expired().await, 0);
        let result = rx.await.unwrap();
        assert_eq!(result.get_value(), Some("reply"));
    }

    #[tokio::test]
    async fn resolve_long_reply_truncates() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-long", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        let long_reply = "x".repeat(MAX_REPLY_LENGTH + 100);
        assert!(mgr.resolve("sess-long", &long_reply).await);
        let result = rx.await.unwrap();
        assert_eq!(
            result.get_value(),
            Some("x".repeat(MAX_REPLY_LENGTH).as_str())
        );
    }

    #[tokio::test]
    async fn resolve_long_multibyte_reply_truncates_at_char_boundary() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-utf8", text_request(), DEFAULT_CLARIFY_TIMEOUT)
            .await;

        // Chinese characters are 3 bytes each in UTF-8.
        // Construct a reply where MAX_REPLY_LENGTH falls inside a character.
        let char_len = "中".len(); // 3 bytes
        let repeat_count = MAX_REPLY_LENGTH / char_len + 10;
        let long_reply = "中".repeat(repeat_count);
        assert!(mgr.resolve("sess-utf8", &long_reply).await);
        let result = rx.await.unwrap();
        let value = result.get_value().unwrap();
        assert!(
            value.len() <= MAX_REPLY_LENGTH,
            "truncated value length {} exceeds MAX_REPLY_LENGTH {}",
            value.len(),
            MAX_REPLY_LENGTH
        );
        // Must be valid UTF-8 (no panic means we survived truncation).
        assert!(value.chars().all(|c| c == '中'));
    }
}
