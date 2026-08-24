//! Crash durability for the busy-input wait lane.
//!
//! The lane registry ([`super::lanes`]) is process memory: a `HashMap` behind
//! a `OnceLock`, nothing on disk, and a [`super::TicketGuard`] carries no
//! payload — the full `RunRequest` lives only in the two spawn closures
//! ([`super::spawn`] for Panel/CLI, `inbound_router::executor` for channels).
//! A daemon that died mid-queue therefore resumed the *interrupted run* (via
//! `ResumeCoordinator`) while every message queued behind it vanished without
//! a trace: the `Rejected`/`TimedOut`/`Purged` receipts are emitted by the
//! waiting task itself, and a dead process emits nothing. Unlike cron /
//! heartbeat / loop / goal units (each re-spawned by its own scheduler — see
//! [`crate::gateway::resume_coordinator::has_own_scheduler`]), a hand-typed
//! message has no regenerator. This module is the sidecar that closes that
//! gap.
//!
//! The shape is deliberately copied from
//! [`crate::builtin_tools::process_journal`] / `background_persistence`,
//! which solve the identical problem for background bash jobs and sub-agents:
//!
//! * layout `<dir>/run-<run_id>/state.json`, atomically rewritten via
//!   [`crate::utils::atomic_io::write_atomic`] — once at enqueue, once at the
//!   terminal tombstone;
//! * a two-phase [`QueuedPhase`]: `Queued` → `Settled`. A tombstone is a
//!   state rewrite, never a deletion — a mechanism that only records "it
//!   finished" cannot tell "it never ran" from "it ran and the write was
//!   lost";
//! * a 7-day retention sweep of settled entries **at boot only** (per-write
//!   would turn every enqueue into an O(entries) stat storm);
//! * persistence is **opt-in**: until [`init`] runs, every entry point is a
//!   zero-I/O no-op, so tests, the CLI and every non-daemon embedding behave
//!   exactly as they did before.
//!
//! # The four tombstone arms, and why admission is one of them
//!
//! A record closes at: [`super::mark_admitted`] (the message became a run —
//! its *own* lifecycle and the resume coordinator take over from here),
//! [`super::purge`] / [`super::cancel_queued_run`] (explicit stop), and the
//! spawn seam's terminal [`super::DeliveryOutcome`] (covers `TimedOut`, plus
//! `Executed(Err)` for an attempt the gate refused without ever admitting).
//! Admission — not completion — is the right "it left the queue" edge: the
//! window between admission and completion is owned by the run's own crash
//! recovery, and tombstoning only at completion would let a crash mid-run
//! reinject a message the resume coordinator is *also* re-triggering (double
//! drive of one user message).
//!
//! Crash between admission and its tombstone write replays as one duplicate
//! delivery at boot. That is the explicitly chosen direction: a duplicate is
//! visible, a loss is silent.
//!
//! # Reinjection preserves the lane invariants for free
//!
//! Boot reinjection re-enters through the ordinary arrival path —
//! [`super::register_run`] + [`super::deliver_with_ticket`] — so the survivor
//! gets a *fresh* ticket with a fresh monotonic `enqueued_at`. No original
//! timestamp is restored, and none is needed: the interrupt-burst predicate
//! (`steering::interrupt_targets_an_unseen_run`) compares the ticket's
//! `waiting_since` against the *current* run's `admitted_at`, both of which
//! are this-process instants after a restart. A survivor re-registered before
//! its lane-mate is admitted still counts as "was already waiting", so the
//! Round-8 burst protection holds unchanged.
//!
//! # What is deliberately NOT persisted
//!
//! * Inline attachment bytes beyond [`MAX_INLINE_ATTACHMENT_BYTES`]: the
//!   whole record is skipped (with a `warn`) rather than written truncated —
//!   a reinjected message that silently lost its images is a lie of the same
//!   kind this module exists to remove. Reference-style attachments
//!   (`path`/`url`) persist as-is.
//! * `pending_media` shared buffers and `sandbox_override` handles — neither
//!   is serializable, and neither is set by the two lane surfaces.
//! * The payload is **not** secret-masked: it must round-trip byte-exact or
//!   the reinjected run answers a different message than the user typed. The
//!   journal lives under the same trust domain as the session transcript
//!   store (`~/.aleph`), which already holds the same text.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::gateway::channel::Attachment;
use crate::gateway::model_override::ModelOverride;
use crate::sync_primitives::Mutex;

/// Cap on summed inline attachment bytes (`Attachment.data`) a record may
/// carry. Past it the message is not persisted at all — see the module doc.
const MAX_INLINE_ATTACHMENT_BYTES: usize = 256 * 1024;

/// Settled journal directories older than this are swept at boot.
const SETTLED_RETENTION: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// Where the journal lives. `None` until [`init`] — every entry point is a
/// zero-I/O no-op before that (mirrors `process_journal::STORE_DIR`).
static STORE_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

fn store_dir() -> Option<PathBuf> {
    STORE_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The serializable subset of `RunRequest` the wait lane needs to re-deliver
/// a queued message after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedRunPayload {
    /// The run this message would become (the id the client already knows).
    pub run_id: String,
    /// Lane key: the session the run will **execute** on — for a `/btw` side
    /// question this is the derived session, not the addressed one. Stored
    /// explicitly so reinjection never re-derives it against a future,
    /// possibly changed, derivation rule.
    pub lane_key: String,
    /// The session the message was **addressed** to (receipts and the
    /// run→session visibility seed resolve against this one).
    pub addressed_session_key: String,
    /// Message text.
    pub input: String,
    /// Request metadata (slash-mode stamps, origin identity, locale, …).
    pub metadata: HashMap<String, String>,
    /// Optional per-run timeout override.
    pub timeout_secs: Option<u64>,
    /// Per-run workspace override (project mode), if any.
    pub workspace_override: Option<PathBuf>,
    /// Per-run Think→Act iteration cap override, if any.
    pub max_iterations_override: Option<u32>,
    /// Chat-window model pin, if any.
    pub model_override: Option<ModelOverride>,
    /// Attachments (reference-style or small inline payloads).
    pub attachments: Vec<Attachment>,
    /// `i18n::Locale` wire tag for queue-stage failure receipts.
    pub locale: Option<String>,
    /// Wall-clock enqueue time, for boot ordering only. Never fed back into
    /// the monotonic `Ticket.enqueued_at` — see the module doc.
    pub enqueued_at_ms: i64,
}

/// The lifecycle of one journaled message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QueuedPhase {
    /// Waiting in the lane (or believed to be — a crash may have lost the
    /// tombstone; boot treats this as "reinfect").
    Queued,
    /// Closed. The reason is recorded, never the payload erased.
    Settled,
}

impl QueuedRunPayload {
    /// Snapshot a to-be-queued request. Single constructor so the two spawn
    /// seams cannot drift on which fields persist — and so the **lane key is
    /// derived exactly the way [`super::register_run`] derives it** (a `/btw`
    /// side question's lane is its execution session, not the addressed one;
    /// getting this wrong re-queues a side question behind the very run it
    /// asks about).
    #[must_use]
    pub fn from_request(
        request: &crate::gateway::execution_engine::RunRequest,
        locale: crate::gateway::i18n::Locale,
    ) -> Self {
        let lane = crate::gateway::btw::execution_session(&request.session_key, &request.metadata);
        Self {
            run_id: request.run_id.clone(),
            lane_key: lane.to_key_string(),
            addressed_session_key: request.session_key.to_key_string(),
            input: request.input.clone(),
            metadata: request.metadata.clone(),
            timeout_secs: request.timeout_secs,
            workspace_override: request.workspace_override.clone(),
            max_iterations_override: request.max_iterations_override,
            model_override: request.model_override.clone(),
            attachments: request.attachments.clone(),
            locale: Some(
                match locale {
                    crate::gateway::i18n::Locale::En => "en",
                    crate::gateway::i18n::Locale::Zh => "zh",
                }
                .to_string(),
            ),
            enqueued_at_ms: crate::session::events::now_ms(),
        }
    }
}

/// Why a journaled message closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettleReason {
    /// `mark_admitted`: the message became a run; the run's own lifecycle
    /// (and crash recovery) owns it from here.
    Admitted,
    /// `/stop` purged the lane, or a run-scoped abort reached it.
    Purged,
    /// `cancel_queued_run`: an explicit stop by run id.
    Cancelled,
    /// The wait outlived `max_wait_secs`.
    TimedOut,
    /// The delivery attempt ran and returned (any verdict) without the record
    /// having closed at admission — e.g. the gate refused for a non-busy
    /// reason. Belt-and-braces twin of [`SettleReason::Admitted`].
    AttemptConcluded,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalEntry {
    phase: QueuedPhase,
    reason: Option<SettleReason>,
    payload: QueuedRunPayload,
}

/// Enable persistence rooted at `dir` (created if missing). Until this runs,
/// every entry point below is a zero-I/O no-op.
pub fn init(dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "busy-queue journal disabled (cannot create dir)");
        return;
    }
    *STORE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// `true` when persistence is armed. Read by the spawn seams so the no-op
/// path does not even serialize the payload.
#[must_use]
pub fn is_armed() -> bool {
    STORE_DIR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

fn entry_dir(root: &std::path::Path, run_id: &str) -> PathBuf {
    // run_id is gateway-minted (uuid-ish) but a derived-session key is not —
    // keep only path-safe characters regardless of what a future caller hands
    // in. The full id stays inside the JSON; the directory name only needs to
    // be unique and safe.
    let safe: String = run_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(format!("run-{safe}"))
}

fn write_entry(root: &std::path::Path, entry: &JournalEntry) -> std::io::Result<()> {
    let dir = entry_dir(root, &entry.payload.run_id);
    std::fs::create_dir_all(&dir)?;
    let bytes = serde_json::to_vec_pretty(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::utils::atomic_io::write_atomic(&dir.join("state.json"), &bytes)
}

/// Persist a freshly-queued message. Called from the two spawn seams right
/// after [`super::register_run`] succeeds (a `None` ticket means REJECT_NEWEST
/// — nothing to persist, the sender already heard no).
///
/// Returns `false` (and logs `warn`) when the inline attachment bytes exceed
/// [`MAX_INLINE_ATTACHMENT_BYTES`] — the record is skipped whole rather than
/// written degraded.
pub fn record_enqueued(payload: QueuedRunPayload) -> bool {
    let Some(root) = store_dir() else {
        return true; // unarmed: the pre-existing in-memory behaviour, not a failure
    };
    let inline_bytes: usize = payload
        .attachments
        .iter()
        .filter_map(|a| a.data.as_ref().map(Vec::len))
        .sum();
    if inline_bytes > MAX_INLINE_ATTACHMENT_BYTES {
        tracing::warn!(
            run_id = %payload.run_id,
            inline_bytes,
            cap = MAX_INLINE_ATTACHMENT_BYTES,
            "queued message NOT journaled: inline attachments exceed the byte cap"
        );
        return false;
    }
    let entry = JournalEntry {
        phase: QueuedPhase::Queued,
        reason: None,
        payload,
    };
    let run_id = entry.payload.run_id.clone();
    if let Err(e) = write_entry(&root, &entry) {
        // Durability is best-effort: a failed write degrades to the
        // pre-feature behaviour (crash loses the message), it must never
        // fail the queueing itself.
        tracing::warn!(run_id = %run_id, error = %e, "busy-queue journal write failed");
    }
    true
}

/// Tombstone a journaled message. Idempotent (the admission arm and the spawn
/// seam both close a record on the normal path), and a no-op for a run_id the
/// journal never saw (unarmed, cap-skipped, or pre-feature).
pub fn record_settled(run_id: &str, reason: SettleReason) {
    let Some(root) = store_dir() else { return };
    let path = entry_dir(&root, run_id).join("state.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return; // never journaled — the common case is uninteresting
    };
    let mut entry: JournalEntry = match serde_json::from_slice(&bytes) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(run_id = %run_id, error = %e, "busy-queue journal entry unreadable; leaving as-is");
            return;
        }
    };
    if entry.phase == QueuedPhase::Settled {
        return;
    }
    entry.phase = QueuedPhase::Settled;
    entry.reason = Some(reason);
    if let Err(e) = write_entry(&root, &entry) {
        tracing::warn!(run_id = %run_id, error = %e, "busy-queue tombstone write failed");
    }
}

/// Survivors to re-deliver at boot: every still-`Queued` record, oldest first.
///
/// Also sweeps settled entries older than [`SETTLED_RETENTION`] — the one
/// place deletion happens, so the tombstone history survives long enough to
/// answer "what happened to my message" without growing the journal forever.
///
/// The caller reinjects each survivor through the ordinary arrival path
/// ([`super::register_run`] + [`super::deliver_with_ticket`]); sessions whose
/// work re-spawns on its own scheduler
/// ([`crate::gateway::resume_coordinator::has_own_scheduler`]) must be skipped
/// by the caller, or the message would be driven twice.
#[must_use]
pub fn survivors() -> Vec<QueuedRunPayload> {
    let Some(root) = store_dir() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<QueuedRunPayload> = Vec::new();
    let now = std::time::SystemTime::now();
    for entry in read.flatten() {
        let path = entry.path().join("state.json");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<JournalEntry>(&bytes) else {
            tracing::warn!(path = %path.display(), "busy-queue journal entry unreadable; skipped");
            continue;
        };
        match record.phase {
            QueuedPhase::Queued => out.push(record.payload),
            QueuedPhase::Settled => {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age > SETTLED_RETENTION);
                if old {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    out.sort_by_key(|p| p.enqueued_at_ms);
    out
}

// ============================================================================
// Boot reinjection
// ============================================================================

/// Re-deliver every journaled survivor through the ordinary arrival path.
///
/// Called once at gateway boot, after the event bus exists and beside the
/// `ResumeCoordinator` scan (the two never overlap: a survivor is by
/// construction a message that never became a run, and a resumable run was
/// tombstoned here at admission). Returns how many messages were re-queued.
///
/// Per survivor:
///
/// 1. **Skip `has_own_scheduler` sessions** — cron / heartbeat / loop / goal /
///    team units re-spawn on their own schedulers; re-delivering their queued
///    input here would double-drive them.
/// 2. Rebuild the `RunRequest` from the journaled payload (`pending_media`
///    starts empty, `sandbox_override` is `None` — neither is set by the two
///    lane surfaces; see the module doc).
/// 3. Emit through the gateway bus, plus the origin-channel fanout when the
///    session has a bound route — the same two-arm shape as
///    `ResumeCoordinator::retrigger`, so a channel user's re-delivered message
///    still answers back to the channel.
/// 4. Re-enter the lane via [`super::register_run`] + the shared
///    [`super::deliver_with_ticket`] loop. The fresh ticket's fresh
///    `enqueued_at` is *why* the burst/interrupt predicates need no special
///    casing (module doc, "Reinjection preserves the lane invariants").
///
/// A survivor whose session key no longer parses, or whose agent is gone, is
/// logged and left in the journal (a later boot with the agent restored can
/// still deliver it) rather than silently tombstoned.
pub async fn reinject_survivors(
    adapter: std::sync::Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    registry: std::sync::Arc<crate::gateway::agent_instance::AgentRegistry>,
    bus: std::sync::Arc<crate::gateway::event_bus::GatewayEventBus>,
    cfg: super::BusyQueueConfig,
) -> usize {
    let survivors = survivors();
    if survivors.is_empty() {
        return 0;
    }
    let mut reinjected = 0usize;
    for payload in survivors {
        let run_id = payload.run_id.clone();
        let Some(session_key) = crate::routing::session_key::SessionKey::from_key_string(
            &payload.addressed_session_key,
        ) else {
            tracing::warn!(run_id = %run_id, key = %payload.addressed_session_key,
                "busy-queue reinject: unparsable session key; leaving record queued");
            continue;
        };
        if crate::gateway::resume_coordinator::has_own_scheduler(&session_key) {
            tracing::debug!(run_id = %run_id,
                "busy-queue reinject: session has its own scheduler; skipping");
            continue;
        }
        let Some(agent) = registry.get(session_key.agent_id()).await else {
            tracing::warn!(run_id = %run_id, agent_id = %session_key.agent_id(),
                "busy-queue reinject: agent gone; leaving record queued");
            continue;
        };
        let request = crate::gateway::execution_engine::RunRequest {
            run_id: payload.run_id.clone(),
            input: payload.input.clone(),
            session_key: session_key.clone(),
            timeout_secs: payload.timeout_secs,
            metadata: payload.metadata.clone(),
            attachments: payload.attachments.clone(),
            pending_media: Default::default(),
            sandbox_override: None,
            workspace_override: payload.workspace_override.clone(),
            max_iterations_override: payload.max_iterations_override,
            model_override: payload.model_override.clone(),
        };
        // Live frames go on the bus; the final answer additionally fans out to
        // the bound origin channel when one exists (Panel-only sessions ride
        // the bus alone). Mirrors `ResumeCoordinator::retrigger`.
        //
        // A `/btw` survivor (lane key ≠ addressed key ⇒ derived side session)
        // is deliberately NOT fanned out: the four pre-existing
        // `OriginFanoutEmitter` sites are all unreachable by side questions,
        // and this one opts out by rule instead — a re-delivered side answer
        // must not land on the origin channel unmarked (`format_side_answer`'s
        // doc carries the census).
        let is_side_question = payload.lane_key != payload.addressed_session_key;
        let base: std::sync::Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            std::sync::Arc::new(crate::gateway::event_emitter::GatewayEventEmitter::new(
                std::sync::Arc::clone(&bus),
            ));
        let emitter: std::sync::Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            if is_side_question {
                base
            } else {
                match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                    Some(reg) => match agent.origin_route(&session_key).await {
                        Some((channel, conversation)) => std::sync::Arc::new(
                            crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                                base,
                                reg,
                                channel,
                                conversation,
                            ),
                        ),
                        None => base,
                    },
                    None => base,
                }
            };
        // Same arrival-path rule as every other surface: the ticket is taken
        // synchronously, before the spawn. A lane that is somehow already full
        // at boot keeps the record queued for the next boot rather than
        // dropping the message.
        let ticket = super::register_run(
            &session_key,
            &payload.metadata,
            cfg.max_per_session,
            &payload.run_id,
        );
        let Some(ticket) = ticket else {
            tracing::warn!(run_id = %run_id,
                "busy-queue reinject: lane already full at boot; leaving record queued");
            continue;
        };
        let locale = crate::gateway::i18n::Locale::from_config(payload.locale.as_deref());
        let adapter = std::sync::Arc::clone(&adapter);
        let receipt_bus = bus.clone();
        let report = super::run_queued_reporter(
            std::sync::Arc::clone(&emitter),
            payload.run_id.clone(),
            payload.addressed_session_key.clone(),
        );
        tokio::spawn(async move {
            let mut attempt = || adapter.execute(request.clone(), agent.clone(), emitter.clone());
            let mut report = report;
            let outcome = super::deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await;
            match &outcome {
                super::DeliveryOutcome::Executed(_) => {
                    record_settled(&run_id, SettleReason::AttemptConcluded);
                }
                super::DeliveryOutcome::TimedOut => {
                    record_settled(&run_id, SettleReason::TimedOut);
                }
                super::DeliveryOutcome::Purged => {
                    record_settled(&run_id, SettleReason::Purged);
                }
                super::DeliveryOutcome::Rejected => {}
            }
            // Queue-stage failures still owe the user a receipt; a run that
            // executed already reported through its own emitter (the
            // double-report rule from `spawn.rs` applies unchanged).
            let Some(e) = outcome.user_error(session_key.agent_id()) else {
                return;
            };
            let (code, message) = e.user_receipt(locale);
            let bus_emitter: std::sync::Arc<
                dyn crate::gateway::event_emitter::EventEmitter + Send + Sync,
            > = std::sync::Arc::new(crate::gateway::event_emitter::GatewayEventEmitter::new(
                receipt_bus,
            ));
            let seq = bus_emitter.next_seq();
            let frame = crate::gateway::event_emitter::StreamEvent::RunError {
                run_id: run_id.clone(),
                seq,
                error: message,
                error_code: Some(code.to_string()),
                // Never-ran frames must self-seed the run→session visibility
                // index — same rule as `spawn_queued_run`'s receipt.
                session_key: Some(payload.addressed_session_key.clone()),
            };
            if let Err(err) = bus_emitter.emit(frame).await {
                tracing::warn!(run_id = %run_id, error = %err,
                    "busy-queue reinject: failed to emit queue-stage receipt");
            }
        });
        reinjected += 1;
    }
    if reinjected > 0 {
        tracing::info!(
            reinjected,
            "busy-queue: re-delivered messages queued before a restart"
        );
    }
    reinjected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(run_id: &str) -> QueuedRunPayload {
        QueuedRunPayload {
            run_id: run_id.to_string(),
            lane_key: "main".to_string(),
            addressed_session_key: "main".to_string(),
            input: "hello".to_string(),
            metadata: HashMap::new(),
            timeout_secs: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
            attachments: Vec::new(),
            locale: None,
            enqueued_at_ms: 1,
        }
    }

    /// STORE_DIR is process-global; tests that arm it must not interleave
    /// (same discipline as `ALEPH_HOME_TEST_GUARD`).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn arm(dir: &std::path::Path) -> crate::sync_primitives::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Re-arming is legal (mirrors `process_journal`'s re-settable static)
        // and is what keeps these tests independent of process-global state.
        *STORE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
        guard
    }

    #[test]
    fn enqueue_then_survives_then_settles() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _g = arm(tmp.path());
        assert!(record_enqueued(payload("r1")));
        assert_eq!(survivors().len(), 1);
        record_settled("r1", SettleReason::Admitted);
        assert!(
            survivors().is_empty(),
            "a tombstoned record must not reinject"
        );
        // Tombstone is a rewrite, not a deletion — the directory survives.
        assert!(entry_dir(tmp.path(), "r1").join("state.json").exists());
        // Idempotent: a second settle is a no-op, not an error.
        record_settled("r1", SettleReason::Purged);
    }

    #[test]
    fn survivors_come_back_oldest_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _g = arm(tmp.path());
        let mut b = payload("b");
        b.enqueued_at_ms = 20;
        let mut a = payload("a");
        a.enqueued_at_ms = 10;
        assert!(record_enqueued(b));
        assert!(record_enqueued(a));
        let order: Vec<String> = survivors().into_iter().map(|p| p.run_id).collect();
        assert_eq!(order, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn oversized_inline_attachments_are_refused_whole() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _g = arm(tmp.path());
        let mut p = payload("big");
        p.attachments.push(Attachment {
            id: "a1".to_string(),
            mime_type: "application/octet-stream".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: Some(vec![0u8; MAX_INLINE_ATTACHMENT_BYTES + 1]),
        });
        assert!(!record_enqueued(p));
        assert!(survivors().is_empty());
    }

    #[test]
    fn settling_an_unknown_run_id_is_a_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _g = arm(tmp.path());
        record_settled("never-journaled", SettleReason::Purged);
        assert!(survivors().is_empty());
    }

    #[test]
    fn path_hostile_run_ids_get_a_safe_directory_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _g = arm(tmp.path());
        assert!(record_enqueued(payload("../escape/attempt")));
        assert_eq!(survivors().len(), 1);
        // The hostile spelling must not have escaped the journal root.
        assert!(!tmp.path().join("..").join("escape").exists());
    }

    /// The journal only works if **every** spawn seam writes on enqueue and
    /// **every** terminal arm settles. Pin both halves at the source level
    /// (the runtime tests above can't see the gateway paths): a new spawn
    /// seam or a removed tombstone call must fail here, loudly, at `cargo
    /// test` time rather than silently re-opening the crash-loss window.
    #[test]
    fn every_seam_journals_and_every_terminal_arm_settles() {
        let spawn = include_str!("spawn.rs");
        let executor = include_str!("../inbound_router/executor.rs");
        let lanes = include_str!("mod.rs");
        assert!(
            spawn.contains("record_enqueued"),
            "spawn.rs (Panel/CLI seam) must journal registered tickets"
        );
        assert!(
            executor.contains("record_enqueued"),
            "executor.rs (channel seam) must journal registered tickets"
        );
        for (arm, reason) in [
            ("pub fn mark_admitted", "Admitted"),
            ("pub fn purge", "Purged"),
            ("pub fn cancel_queued_run", "Cancelled"),
        ] {
            let start = lanes.find(arm).unwrap_or_else(|| panic!("{arm} missing"));
            let rest = &lanes[start..];
            let end = rest.find("\npub fn").map(|i| i + 1).unwrap_or(rest.len());
            let body = &rest[..end.min(rest.len())];
            assert!(
                body.contains("record_settled"),
                "{arm} must tombstone the journal ({reason})"
            );
        }
    }
}
