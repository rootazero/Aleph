//! One durable correction consumer record per corpus. Not a general stage journal.

use crate::error::AlephError;
use crate::memory::dreaming::{DistillAction, DistillOutcome};
use crate::memory::notes::indexer::PreparedDistillEffect;
use crate::memory::store::raw_memory::{
    CorrectionStatusCursor, RawMemory, RawMemoryStore, SequencedCorrection,
};
use crate::memory::store::SqliteMemoryBackend;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_FEEDBACK_BATCH: usize = 128;
const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;
const VERSION: u8 = 1;
pub(crate) const LEGACY_FEEDBACK_CONSUMER: &str = "feedback_distill";

pub(super) fn batch_limit(max_per_cycle: usize) -> usize {
    max_per_cycle.min(MAX_FEEDBACK_BATCH)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FeedbackState {
    version: u8,
    pub committed_seq: i64,
    pub pending: Option<PendingBatch>,
}

impl Default for FeedbackState {
    fn default() -> Self {
        Self {
            version: VERSION,
            committed_seq: 0,
            pending: None,
        }
    }
}

impl FeedbackState {
    fn validate(&self) -> Result<(), AlephError> {
        if self.version != VERSION || self.committed_seq < 0 {
            return Err(AlephError::other(
                "invalid feedback state version or sequence",
            ));
        }
        let Some(batch) = &self.pending else {
            return Ok(());
        };
        batch.validate_after(self.committed_seq)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingBatch {
    sources: Vec<BatchSource>,
    pub entries: Vec<PlannedAction>,
    pub candidate_paths: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchSource {
    seq: i64,
    id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlannedAction {
    pub action: DistillAction,
    pub status: ActionStatus,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ActionStatus {
    Pending,
    Prepared {
        effect: PreparedDistillEffect,
    },
    Settled {
        outcome: DistillOutcome,
        reason: Option<String>,
    },
}

impl PendingBatch {
    pub fn new(
        page: &[SequencedCorrection],
        actions: Vec<DistillAction>,
        candidate_paths: Vec<String>,
    ) -> Self {
        Self {
            sources: page
                .iter()
                .map(|row| BatchSource {
                    seq: row.seq,
                    id: row.raw.id.clone(),
                })
                .collect(),
            entries: actions
                .into_iter()
                .map(|action| PlannedAction {
                    action,
                    status: ActionStatus::Pending,
                })
                .collect(),
            candidate_paths,
        }
    }

    pub fn last_seq(&self) -> Result<i64, AlephError> {
        self.sources
            .last()
            .map(|row| row.seq)
            .ok_or_else(|| AlephError::other("feedback plan has no sources"))
    }

    fn validate_after(&self, committed: i64) -> Result<(), AlephError> {
        if self.sources.is_empty()
            || self.sources.len() > MAX_FEEDBACK_BATCH
            || self.entries.is_empty()
            || self.entries.len() > self.sources.len()
            || self.sources[0].seq <= committed
            || self
                .sources
                .windows(2)
                .any(|pair| pair[0].seq >= pair[1].seq)
            || self.sources.iter().any(|row| row.id.is_empty())
        {
            return Err(AlephError::other(
                "invalid feedback batch bounds or ordering",
            ));
        }
        let mut seen_pending = false;
        for entry in &self.entries {
            match &entry.status {
                ActionStatus::Pending => seen_pending = true,
                ActionStatus::Prepared { .. }
                    if !seen_pending && !matches!(entry.action, DistillAction::Skip { .. }) =>
                {
                    seen_pending = true;
                }
                ActionStatus::Settled { outcome, .. }
                    if !seen_pending && *outcome != DistillOutcome::Error => {}
                _ => return Err(AlephError::other("invalid feedback action progress")),
            }
        }
        validate_exact_coverage(
            self.sources.iter().map(|row| row.id.as_str()),
            self.entries.iter().map(|entry| &entry.action),
        )
    }
}

/// Validate the whole response before recording a plan or performing any mutation.
pub(super) fn validate_exact_coverage<'a>(
    ids: impl Iterator<Item = &'a str>,
    actions: impl Iterator<Item = &'a DistillAction>,
) -> Result<(), AlephError> {
    let mut remaining = HashSet::new();
    for id in ids {
        if !remaining.insert(id) {
            return Err(AlephError::other("duplicate presented correction ID"));
        }
    }
    for action in actions {
        let sources = match action {
            DistillAction::New { source_facts, .. }
            | DistillAction::Strengthen { source_facts, .. }
            | DistillAction::Supersede { source_facts, .. } => source_facts.as_slice(),
            DistillAction::Skip { source_fact, .. } => std::slice::from_ref(source_fact),
        };
        if sources.is_empty() || sources.iter().any(|id| !remaining.remove(id.as_str())) {
            return Err(AlephError::other(
                "feedback response contains missing, duplicate, or foreign correction IDs",
            ));
        }
    }
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(AlephError::other(
            "feedback response does not cover every presented correction",
        ))
    }
}

/// The exact bytes last read/written are the CAS revision, not a second source
/// of truth. A stale snapshot may not supersede someone else's plan or receipt.
pub(super) struct FeedbackSnapshot {
    pub state: FeedbackState,
    revision: Option<String>,
}

impl FeedbackSnapshot {
    pub fn load(store: &SqliteMemoryBackend, corpus: &str) -> Result<Self, AlephError> {
        let revision = store.feedback_batch_record(corpus)?;
        let state = match revision.as_deref() {
            None => FeedbackState::default(),
            Some(raw) if raw.len() <= MAX_STATE_BYTES => serde_json::from_str(raw)
                .map_err(|_| AlephError::other("invalid persisted feedback batch"))?,
            Some(_) => return Err(AlephError::other("persisted feedback batch exceeds limit")),
        };
        state.validate()?;
        Ok(Self { state, revision })
    }

    pub fn save(&mut self, store: &SqliteMemoryBackend, corpus: &str) -> Result<(), AlephError> {
        self.state.validate()?;
        let raw = serde_json::to_string(&self.state)
            .map_err(|_| AlephError::other("encode feedback batch failed"))?;
        if raw.len() > MAX_STATE_BYTES {
            return Err(AlephError::other(
                "feedback batch exceeds persistence limit",
            ));
        }
        if !store.compare_exchange_feedback_batch(corpus, self.revision.as_deref(), &raw)? {
            return Err(AlephError::other("feedback batch changed concurrently"));
        }
        self.revision = Some(raw);
        Ok(())
    }
}

pub(crate) async fn list_corrections(
    store: &SqliteMemoryBackend,
    raw_store: &dyn RawMemoryStore,
    corpus: &str,
    include_distilled: bool,
    limit: usize,
) -> Result<Vec<(RawMemory, bool)>, AlephError> {
    let committed = FeedbackSnapshot::load(store, corpus)?.state.committed_seq;
    if let Some(rows) = raw_store
        .corrections_with_status(
            corpus,
            CorrectionStatusCursor::Committed(committed),
            include_distilled,
            limit,
        )
        .await?
    {
        return Ok(rows);
    }
    let legacy = store.get_dream_watermark(LEGACY_FEEDBACK_CONSUMER, corpus)?;
    raw_store
        .corrections_with_status(
            corpus,
            CorrectionStatusCursor::Legacy(legacy),
            include_distilled,
            limit,
        )
        .await?
        .ok_or_else(|| AlephError::other("correction status unavailable before activation"))
}
