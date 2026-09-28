//! Minimal audit log for the Telegram channel.
//!
//! Three log-style entry classes (`Update`, `Send`, `Approval`) form the
//! audit trail a doctor / post-mortem can replay without re-deriving the
//! facts from telemetry. Storage is an in-memory ring buffer keyed by
//! `(kind, account_id, conversation_id)` so the most recent events for one
//! conversation are always one read away — the doctor uses this to answer
//! "what did the bot actually do in chat X in the last N minutes".
//!
//! Why a ring buffer and not SQLite:
//! - The audit trail is a *debugging* artefact, not a regulatory record
//!   (the agent-identity ledger is the latter). Restarting the server
//!   clears it; that is the correct behaviour.
//! - The buffer is bounded, so an out-of-memory incident cannot be
//!   blamed on a runaway audit. The cap is a constant, not a config knob,
//!   on purpose — a cap that an operator can change is a cap that gets
//!   set to "infinity" by a tired operator and then forgotten.
//!
//! Writes are best-effort. The audit log MUST NOT cause a delivery to
//! fail: a full ring is dropped silently, a poisoned write (somehow) is
//! logged and dropped. The ring is a diagnostic, not a system of record.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One audit row. Public for the doctor / JSON dump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix millis when the event happened (wall-clock; pre-epoch is not
    /// expected and is logged as 0 by the same helper that warns in
    /// `ExecApprovalManager`).
    pub at_ms: u64,
    /// Which Telegram account saw / acted on the event.
    pub account_id: String,
    /// Chat id (positive for DMs / private, negative for groups /
    /// supergroups). `None` for events that pre-date a chat lookup (rare;
    /// mostly approval events).
    pub chat_id: Option<i64>,
    /// Optional forum topic id.
    pub thread_id: Option<i32>,
    /// What happened.
    pub kind: AuditKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditKind {
    /// An inbound update (message / edited message / callback query / ...)
    /// was received and forwarded to the inbound router.
    UpdateReceived { update_id: i64, kind_label: String },
    /// An outbound send was attempted. `outcome` is "succeeded" or
    /// "failed: <reason>".
    SendAttempted {
        message_id_kind: String,
        outcome: String,
    },
    /// An approval card was raised by an outgoing prompt that requires
    /// confirmation. `outcome` is "pending" / "approved" / "denied" /
    /// "expired" / "unavailable" so the doctor can read the trail without
    /// correlating with the approval ledger.
    ApprovalResolved {
        approval_id: String,
        outcome: String,
    },
}

const RING_CAPACITY: usize = 1024;

/// Thread-safe ring buffer. Cloned via `Arc::clone(&inner)` so callers
/// keep the same backing store across threads; new clones via the public
/// constructor share the same store (Clone is `Arc<Mutex<...>>` style).
pub struct AuditLog {
    inner: Mutex<VecDeque<AuditEntry>>,
}

impl AuditLog {
    /// Build a fresh, empty audit log.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
        }
    }

    /// Append an entry. If the ring is full, drop the OLDEST entry —
    /// the new one is the most diagnostically interesting one (it's the
    /// thing the operator was just told about).
    pub fn push(&self, entry: AuditEntry) {
        let Ok(mut ring) = self.inner.lock() else {
            // A poisoned mutex means another thread panicked inside.
            // The audit log is a diagnostic; surfacing a second panic on
            // top of the first is worse than dropping one row.
            return;
        };
        if ring.len() >= RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(entry);
    }

    /// Snapshot the whole ring (oldest first) for the doctor.
    #[must_use]
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.inner
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Snapshot filtered to one conversation. Useful for the doctor's
    /// "what happened in chat -100123?" answer.
    #[must_use]
    pub fn snapshot_for_conversation(&self, account_id: &str, chat_id: i64) -> Vec<AuditEntry> {
        self.inner
            .lock()
            .map(|r| {
                r.iter()
                    .filter(|e| e.account_id == account_id && e.chat_id == Some(chat_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct an `at_ms` field the same way `ExecApprovalManager` does, so
/// a pre-epoch clock surfaces the same way in both.
fn now_ms_or_warn() -> u64 {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PRE_EPOCH_LOGGED: AtomicBool = AtomicBool::new(false);
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as u64,
        Err(_) => {
            if !PRE_EPOCH_LOGGED.swap(true, Ordering::Relaxed) {
                tracing::error!(
                    "wall clock reads pre-Unix-epoch; audit timestamps will \
                     collapse to 0 until the clock is corrected"
                );
            }
            0
        }
    }
}

/// Convenience: build an entry with the standard timestamp + account id.
/// `chat_id` is taken as a raw value (callers that don't have it pass
/// `None`).
pub fn entry(
    account_id: impl Into<String>,
    chat_id: Option<i64>,
    thread_id: Option<i32>,
    kind: AuditKind,
) -> AuditEntry {
    AuditEntry {
        at_ms: now_ms_or_warn(),
        account_id: account_id.into(),
        chat_id,
        thread_id,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_overflows_drop_oldest() {
        let log = AuditLog::new();
        for i in 0..(RING_CAPACITY + 5) {
            log.push(entry(
                "acct",
                Some(1),
                None,
                AuditKind::SendAttempted {
                    message_id_kind: format!("m{i}"),
                    outcome: "succeeded".to_string(),
                },
            ));
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), RING_CAPACITY);
        // The OLDEST five (m0..m4) were dropped; the newest entry survived.
        assert!(matches!(&snap.last().unwrap().kind,
            AuditKind::SendAttempted { message_id_kind, .. }
            if message_id_kind == &format!("m{}", RING_CAPACITY + 4)));
        // The new front is m5 (m0..m4 dropped).
        assert!(matches!(&snap.first().unwrap().kind,
            AuditKind::SendAttempted { message_id_kind, .. }
            if message_id_kind == "m5"));
    }

    #[test]
    fn snapshot_for_conversation_filters_by_chat() {
        let log = AuditLog::new();
        log.push(entry(
            "acct",
            Some(1),
            None,
            AuditKind::UpdateReceived {
                update_id: 1,
                kind_label: "msg".to_string(),
            },
        ));
        log.push(entry(
            "acct",
            Some(2),
            None,
            AuditKind::UpdateReceived {
                update_id: 2,
                kind_label: "msg".to_string(),
            },
        ));
        log.push(entry(
            "other",
            Some(1),
            None,
            AuditKind::UpdateReceived {
                update_id: 3,
                kind_label: "msg".to_string(),
            },
        ));
        let s = log.snapshot_for_conversation("acct", 1);
        assert_eq!(s.len(), 1, "filter by account AND chat");
    }

    #[test]
    fn poisoned_mutex_does_not_panic_caller() {
        // Best-effort: a poisoned mutex MUST NOT panic the caller — the
        // audit log is a diagnostic. The test forces poisoning by
        // panicking inside the lock; recovery is that push() returns
        // without panicking again.
        let log = AuditLog::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = log.inner.lock().unwrap();
            panic!("poison the mutex");
        }));
        assert!(result.is_err(), "the inner panic must have happened");
        // Now push must not panic.
        log.push(entry(
            "acct",
            Some(1),
            None,
            AuditKind::SendAttempted {
                message_id_kind: "m".to_string(),
                outcome: "succeeded".to_string(),
            },
        ));
        // And snapshot must not panic either (returns empty — the
        // poisoned mutex can't be read).
        let snap = log.snapshot();
        assert!(snap.is_empty());
    }
}
