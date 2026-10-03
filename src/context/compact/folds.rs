//! The fold registry: addressable fold blocks over the session event log
//! (Context Fabric, spec 2026-10-01 §3.1a).
//!
//! A *fold* is one compaction's retired span made addressable: the seq range
//! it covered, the summary that replaced it, the strategy and trigger, and
//! the token accounting the fold ledger (§3.1c) reads. The registry builds no
//! parallel table — `session_events` is the single source of truth — so this
//! module is a pure derived view: [`list_folds`] scans an event slice and
//! returns one [`FoldRecord`] per fold, in seq order.
//!
//! Two sources feed the view:
//!
//! - [`SessionEvent::FoldRecorded`] — the explicit record, emitted in the
//!   SAME `emit_batch` as the `CompactionPerformed` it annotates (both land
//!   or neither does), carrying the token accounting the checkpoint lacks.
//! - [`SessionEvent::CompactionPerformed`] with no matching `FoldRecorded` —
//!   every session compacted before this module existed. These derive a
//!   `Legacy` fold with zeroed token accounting and a deterministic fold_id,
//!   so old sessions stay addressable without a migration.
//!
//! A recorded fold supersedes the legacy derivation for the SAME
//! `(from_seq, to_seq)` span: the two events travel in one batch, and
//! counting both would double-list one fold.

use crate::session::events::{EventSeq, SessionEvent, Timestamp};

/// How the fold's summary was produced.
///
/// Closed set — unlike the wire string it parses from, which stays open so a
/// newer build's strategies still decode. An unrecognized word maps to
/// [`FoldStrategy::Legacy`]: the fold is addressable, its provenance unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldStrategy {
    /// User-driven `/compact` (the `manual` module): summarize-and-retire
    /// over the event log.
    Manual,
    /// Pressure-driven in-turn LLM compaction. Not currently recorded —
    /// transient, touches no event log (spec §6, O1 ruling).
    AutoLlm,
    /// Deterministic-truncation fallback. Never recorded: no summary artifact
    /// to address.
    AutoDeterministic,
    /// Zero-cost reuse of the session-memory summary.
    SessionMemoryReuse,
    /// Derived from a bare `CompactionPerformed` written before the registry
    /// existed, or from a strategy word this build does not know.
    Legacy,
}

impl FoldStrategy {
    /// The wire word written into `FoldRecorded.strategy`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AutoLlm => "auto-llm",
            Self::AutoDeterministic => "auto-deterministic",
            Self::SessionMemoryReuse => "session-memory-reuse",
            Self::Legacy => "legacy",
        }
    }

    fn from_wire(word: &str) -> Self {
        match word {
            "manual" => Self::Manual,
            "auto-llm" => Self::AutoLlm,
            "auto-deterministic" => Self::AutoDeterministic,
            "session-memory-reuse" => Self::SessionMemoryReuse,
            _ => Self::Legacy,
        }
    }
}

/// What initiated the fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldTrigger {
    /// A user-initiated surface: `/compact`, `/compress`, the
    /// `session.compact` RPC.
    ManualCommand,
    /// Context-pressure escalation in the run loop.
    PressureEscalation,
    /// Preventive, ahead of pressure.
    Preventive,
    /// The model's own `session_compact` tool call.
    ModelTool,
    /// Unknown or pre-registry.
    Legacy,
}

impl FoldTrigger {
    /// The wire word written into `FoldRecorded.trigger`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManualCommand => "manual-command",
            Self::PressureEscalation => "pressure-escalation",
            Self::Preventive => "preventive",
            Self::ModelTool => "model-tool",
            Self::Legacy => "legacy",
        }
    }

    fn from_wire(word: &str) -> Self {
        match word {
            "manual-command" => Self::ManualCommand,
            "pressure-escalation" => Self::PressureEscalation,
            "preventive" => Self::Preventive,
            "model-tool" => Self::ModelTool,
            _ => Self::Legacy,
        }
    }
}

/// One addressable fold: a compaction's retired span, its summary reference,
/// and its token accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldRecord {
    pub fold_id: String,
    pub from_seq: EventSeq,
    pub to_seq: EventSeq,
    pub summary_ref: String,
    pub strategy: FoldStrategy,
    pub trigger: FoldTrigger,
    /// Estimated prompt tokens the folded span was costing per turn. Zero on
    /// legacy folds, which predate the accounting.
    pub folded_tokens: u64,
    /// Estimated tokens of the summary that replaced the span.
    pub summary_tokens: u64,
    pub at: Timestamp,
}

/// The deterministic fold id for one span of one session:
/// `fold_<session_hash8>_<from_seq>`.
///
/// `DefaultHasher::new` is fixed-key (unlike `RandomState`), so the id is
/// stable across runs — the same property `compaction_window::hash_window`
/// relies on for its cross-turn fingerprint. 8 hex chars of hash is the
/// collision budget of a session name, not a security boundary; `from_seq`
/// does the real distinguishing.
#[must_use]
pub fn derive_fold_id(session_key: &str, from_seq: EventSeq) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_key.hash(&mut h);
    format!("fold_{:08x}_{}", h.finish() as u32, from_seq)
}

/// The fold list of one session's event slice, seq-monotonic.
///
/// Two passes, because the batch emits the checkpoint BEFORE its
/// `FoldRecorded` (the checkpoint's existing readers key on adjacency to the
/// summary): pass one collects every recorded fold's span, pass two walks the
/// slice and emits the recorded fold for its span or — when no recorded fold
/// covers it — derives a `Legacy` fold from the bare checkpoint. The result
/// is sorted by `(from_seq, to_seq)` so callers get seq order regardless of
/// emission order inside the log.
#[must_use]
pub fn list_folds(session_key: &str, events: &[SessionEvent]) -> Vec<FoldRecord> {
    let recorded: Vec<(EventSeq, EventSeq)> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::FoldRecorded {
                from_seq, to_seq, ..
            } => Some((*from_seq, *to_seq)),
            _ => None,
        })
        .collect();
    let mut folds: Vec<FoldRecord> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::FoldRecorded {
                fold_id,
                from_seq,
                to_seq,
                summary_ref,
                strategy,
                trigger,
                folded_tokens,
                summary_tokens,
                at,
            } => Some(FoldRecord {
                fold_id: fold_id.clone(),
                from_seq: *from_seq,
                to_seq: *to_seq,
                summary_ref: summary_ref.clone(),
                strategy: FoldStrategy::from_wire(strategy),
                trigger: FoldTrigger::from_wire(trigger),
                folded_tokens: *folded_tokens,
                summary_tokens: *summary_tokens,
                at: *at,
            }),
            SessionEvent::CompactionPerformed {
                from_seq,
                to_seq,
                summary_ref,
                at,
            } if !recorded.contains(&(*from_seq, *to_seq)) => Some(FoldRecord {
                fold_id: derive_fold_id(session_key, *from_seq),
                from_seq: *from_seq,
                to_seq: *to_seq,
                summary_ref: summary_ref.clone(),
                strategy: FoldStrategy::Legacy,
                trigger: FoldTrigger::Legacy,
                folded_tokens: 0,
                summary_tokens: 0,
                at: *at,
            }),
            _ => None,
        })
        .collect();
    folds.sort_by_key(|f| (f.from_seq, f.to_seq));
    folds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::SessionEvent;

    const SESSION: &str = "agent:main:peer:s";

    fn compaction(from_seq: u64, to_seq: u64) -> SessionEvent {
        SessionEvent::CompactionPerformed {
            from_seq,
            to_seq,
            summary_ref: "turn-of-summary".to_string(),
            at: 1,
        }
    }

    fn recorded(fold_id: &str, from_seq: u64, to_seq: u64) -> SessionEvent {
        SessionEvent::FoldRecorded {
            fold_id: fold_id.to_string(),
            from_seq,
            to_seq,
            summary_ref: "turn-of-summary".to_string(),
            strategy: "manual".to_string(),
            trigger: "manual-command".to_string(),
            folded_tokens: 12_000,
            summary_tokens: 800,
            at: 42,
        }
    }

    /// The new variant survives the wire under its snake_case tag, byte for
    /// byte — the registry reads it back from `session_events` rows.
    #[test]
    fn fold_recorded_roundtrip_serde() {
        let ev = recorded("fold_ab12cd34_2", 2, 40);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"fold_recorded\""), "{json}");
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    /// Pre-registry sessions carry only `CompactionPerformed`. They must still
    /// list a fold — derived, `Legacy`, zeroed accounting — or their folded
    /// spans are unaddressable (plan Review Focus #1).
    #[test]
    fn list_folds_derives_legacy_from_compaction_performed() {
        let events = vec![compaction(2, 40)];
        let folds = list_folds(SESSION, &events);
        assert_eq!(folds.len(), 1);
        let fold = &folds[0];
        assert_eq!(fold.strategy, FoldStrategy::Legacy);
        assert_eq!((fold.from_seq, fold.to_seq), (2, 40));
        assert_eq!(fold.summary_ref, "turn-of-summary");
        assert_eq!(fold.folded_tokens, 0);
        assert_eq!(fold.summary_tokens, 0);
        assert_eq!(fold.fold_id, derive_fold_id(SESSION, 2));
    }

    /// A recorded fold and its checkpoint describe ONE fold: the recorded
    /// event wins (it carries the accounting), the derived legacy entry for
    /// the same span must not double-list. A later bare `CompactionPerformed`
    /// still derives its own legacy fold.
    #[test]
    fn list_folds_prefers_recorded_over_derived() {
        let events = vec![
            compaction(2, 40),
            recorded("fold_ab12cd34_2", 2, 40),
            compaction(41, 80),
        ];
        let folds = list_folds(SESSION, &events);
        assert_eq!(
            folds.len(),
            2,
            "recorded supersedes derived for the same span; the bare checkpoint still derives"
        );
        assert_eq!(folds[0].fold_id, "fold_ab12cd34_2");
        assert_eq!(folds[0].strategy, FoldStrategy::Manual);
        assert_eq!(folds[0].trigger, FoldTrigger::ManualCommand);
        assert_eq!(folds[0].folded_tokens, 12_000);
        assert_eq!(folds[0].summary_tokens, 800);
        assert_eq!(folds[1].strategy, FoldStrategy::Legacy);
        assert_eq!((folds[1].from_seq, folds[1].to_seq), (41, 80));
        // The view is seq-monotonic regardless of emission order.
        assert!(folds[0].from_seq < folds[1].from_seq);
    }

    /// Forgery guard (bili #717, Context Fabric Task 4): assistant text that
    /// merely *claims* a fold happened — a `[Context folded]`-style marker the
    /// model typed itself — must not change the fold view. Compaction is
    /// system-side in Aleph, so confirmation only ever arrives as
    /// `FoldRecorded` / `CompactionPerformed` events; this test locks the
    /// registry against ever believing the transcript instead.
    #[test]
    fn assistant_marker_text_creates_no_fold_events() {
        use crate::session::events::MessageContent;
        let forged = |text: &str| SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: 7,
        };

        // A log of ONLY forged markers lists no folds at all.
        let forgery_only = vec![
            forged("[Context folded] 12,000 tokens into a summary."),
            forged("📦 [ACP] Compressed m0010–m0042 — done."),
        ];
        assert!(
            list_folds(SESSION, &forgery_only).is_empty(),
            "assistant-typed fold markers are text, not folds"
        );

        // Interleaved into a log with a real fold, the forgery changes
        // nothing: the effect that must arrive is byte-identical.
        let real = vec![compaction(2, 40), recorded("fold_ab12cd34_2", 2, 40)];
        let mut interleaved = vec![forged("[Context folded] everything above.")];
        interleaved.extend(real.iter().cloned());
        interleaved.push(forged("📦 folded again, trust me"));
        assert_eq!(
            list_folds(SESSION, &interleaved),
            list_folds(SESSION, &real),
            "forged marker text must leave the fold view untouched"
        );
    }

    #[test]
    fn fold_id_deterministic() {
        let a = derive_fold_id(SESSION, 5);
        assert_eq!(a, derive_fold_id(SESSION, 5), "same inputs, same id");
        assert!(a.starts_with("fold_"), "{a}");
        assert_ne!(
            a,
            derive_fold_id(SESSION, 6),
            "a different span start yields a different id"
        );
        assert_ne!(
            a,
            derive_fold_id("agent:main:peer:t", 5),
            "a different session yields a different id"
        );
    }
}
