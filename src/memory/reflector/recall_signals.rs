//! Record presented materialized notes in one recall-signal transaction per owner.

use crate::error::AlephError;
use crate::memory::reflector::packet_adapter::NoteExposure;
use crate::memory::reflector::types::ReflectOpts;
use crate::memory::store::sqlite::recall_signals::RecallHit;
use std::collections::BTreeMap;

pub const REFLECT_CHANNEL: &str = "reflect";

/// One owning partition's hits, persisted together through `record_signals`.
#[derive(Debug, Clone)]
pub struct SignalBatch {
    pub owner: String,
    pub hits: Vec<RecallHit>,
    pub query_text: String,
    pub channel: String,
    pub session_id: Option<String>,
    pub namespace: String,
}

/// Deduplicate presented identities, then attempt every owner independently.
/// The request agent and model citation choices confer no note provenance.
pub(crate) async fn record_reflect_signals<F, Fut>(
    record: F,
    query: &str,
    exposures: &[NoteExposure],
    opts: &ReflectOpts,
) where
    F: Fn(SignalBatch) -> Fut,
    Fut: std::future::Future<Output = Result<(), AlephError>>,
{
    let mut by_owner: BTreeMap<&str, BTreeMap<&str, f32>> = BTreeMap::new();
    for exposure in exposures {
        by_owner
            .entry(&exposure.note.owner)
            .or_default()
            .entry(&exposure.note.path)
            .or_insert(exposure.relevance);
    }
    for (owner, notes) in by_owner {
        let batch = SignalBatch {
            owner: owner.to_string(),
            hits: notes
                .into_iter()
                .map(|(path, score)| RecallHit {
                    note_path: path.to_string(),
                    score: f64::from(score),
                })
                .collect(),
            query_text: query.to_string(),
            channel: REFLECT_CHANNEL.to_string(),
            session_id: opts.session_id.clone(),
            namespace: opts.namespace.to_namespace_value(),
        };
        if let Err(error) = record(batch).await {
            tracing::warn!(owner, %error, "reflector: failed to write recall_signals for owner");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::NoteProvenance;
    use crate::sync_primitives::{Arc, Mutex};

    fn exposure(owner: &str, path: &str, relevance: f32) -> NoteExposure {
        NoteExposure {
            note: NoteProvenance {
                owner: owner.into(),
                path: path.into(),
            },
            relevance,
        }
    }

    #[tokio::test]
    async fn groups_and_deduplicates_presented_notes_by_real_owner() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = move |batch: SignalBatch| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(batch);
                Ok::<(), AlephError>(())
            }
        };
        let exposures = vec![
            exposure("actual", "reference/a", 0.5),
            exposure("actual", "reference/b", 0.8),
            exposure("actual", "reference/a", 0.5),
            exposure("other", "reference/a", 0.6),
        ];
        record_reflect_signals(
            record,
            "q",
            &exposures,
            &ReflectOpts::for_agent("request-agent"),
        )
        .await;
        let batches = captured.lock().unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].owner, "actual");
        assert_eq!(batches[0].hits.len(), 2);
        assert_eq!(batches[0].hits[0].note_path, "reference/a");
        assert_eq!(batches[0].hits[0].score, 0.5);
        assert_eq!(batches[1].owner, "other");
        assert_eq!(batches[1].hits.len(), 1);
        assert!(batches
            .iter()
            .all(|b| b.channel == "reflect" && b.query_text == "q"));
    }

    #[tokio::test]
    async fn empty_exposure_writes_no_batches() {
        record_reflect_signals(
            |_| async { panic!("no owner to write") },
            "q",
            &[],
            &ReflectOpts::for_agent("x"),
        )
        .await;
    }

    #[tokio::test]
    async fn namespace_owner_serializes_to_expected_token() {
        let opts = ReflectOpts::for_agent("x");
        record_reflect_signals(
            |batch| async move {
                assert_eq!(batch.namespace, "owner");
                assert_eq!(batch.owner, "actual");
                Ok::<(), AlephError>(())
            },
            "q",
            &[exposure("actual", "p", 0.1)],
            &opts,
        )
        .await;
    }
}
