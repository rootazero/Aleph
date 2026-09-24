//! Shared types for the workflow `collect` step — a step that waits on a
//! list of upstream steps and folds their outputs into a single deliverable.
//!
//! This module is the single source of truth shared by the three layers that
//! touch a collect step:
//! - the **compiler** ([`crate::workflow::compile`]) stamps a
//!   [`CollectTaskMeta`] into the materialised `coord_task`'s metadata,
//!   owned by [`COLLECT_OWNER`];
//! - the **dispatcher** ([`crate::teams::dispatcher`]) detects the task via
//!   [`is_collect_task`], waits for every `collect_from` step to settle,
//!   folds their outputs with [`CollectReduce`], and writes the result back
//!   onto the task's `description`/`result`;
//! - the **downstream consumer** (`build_handoff_context`, `status`) reads
//!   the reduced output the same way it reads an agent step's deliverable.
//!
//! The `coord_task` row itself is the durable awaiting record — the source
//! list, the reducer choice, and the originating run all live in its
//! metadata — so a collect step survives a process restart with no
//! in-memory state to reconstruct (R10 — no new scheduler, no new persistence
//! layer; reuse the task table).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workflow::def::CollectReduce;

/// Sentinel `coord_task.owner` for collect steps. Never a real team member —
/// the dispatcher branches on [`is_collect_task`] before owner resolution,
/// so a collect task is never routed to an agent. The sentinel owner only
/// satisfies the scheduler's "has an owner" selection predicate.
pub const COLLECT_OWNER: &str = "__collect__";

/// Metadata key under which [`CollectTaskMeta`] is stored on a `coord_task`.
pub const COLLECT_META_KEY: &str = "collect";

/// Read a step's [`CollectTaskMeta`] off its materialised `coord_task`
/// metadata, if present. Pure.
#[must_use]
pub fn collect_task_meta(metadata: &Value) -> Option<CollectTaskMeta> {
    let raw = metadata.get(COLLECT_META_KEY)?;
    serde_json::from_value(raw.clone()).ok()
}

/// True when `metadata` carries a [`CollectTaskMeta`] under
/// [`COLLECT_META_KEY`] — the dispatcher's branch condition for "this task
/// is the workflow's own collect fold, route through the reducer rather than
/// a team member run". Pure.
#[must_use]
pub fn is_collect_task(metadata: &Value) -> bool {
    collect_task_meta(metadata).is_some()
}

/// Durable awaiting record for one collect step, stored verbatim in the
/// `coord_task` metadata under [`COLLECT_META_KEY`]. The shape mirrors
/// [`crate::workflow::def::WorkflowStepDef`]'s collect fields, but as a
/// single owned value (the wire shape, not the in-memory struct) so the
/// materialised row is self-contained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectTaskMeta {
    /// The upstream step ids whose outputs are reduced into the deliverable.
    /// Order is significant: [`CollectReduce::Concat`] walks in this order,
    /// [`CollectReduce::First`] picks the first non-empty.
    pub collect_from: Vec<String>,
    /// How the upstream outputs are folded.
    pub reduce: CollectReduce,
}

impl CollectTaskMeta {
    /// Serialise to a `serde_json::Value` suitable for stamping into the
    /// `coord_task`'s metadata. Centralised so the on-disk shape lives in
    /// exactly one place.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("CollectTaskMeta is serialisable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_task_meta_roundtrips_through_json() {
        // The exact shape the materialiser stamps and the dispatcher reads.
        let m = CollectTaskMeta {
            collect_from: vec!["a".into(), "b".into()],
            reduce: CollectReduce::JsonArray,
        };
        let v = m.to_value();
        assert_eq!(v["collect_from"][0], "a");
        assert_eq!(v["reduce"], "json_array");
        let back: CollectTaskMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn is_collect_task_is_false_without_the_metadata_key() {
        // An agent row (no collect meta) must not be mistaken for a collect
        // task — the dispatcher would otherwise try to fold the agent's
        // deliverable as the reducer's input and double-process it.
        assert!(!is_collect_task(&serde_json::json!({})));
        let meta = serde_json::json!({
            "workflow_step": "synth",
            "managed_by": "dispatcher"
        });
        assert!(!is_collect_task(&meta));
    }

    #[test]
    fn is_collect_task_is_true_with_the_metadata_key() {
        let meta = serde_json::json!({
            COLLECT_META_KEY: {
                "collect_from": ["a", "b"],
                "reduce": "concat"
            }
        });
        assert!(is_collect_task(&meta));
        assert_eq!(
            collect_task_meta(&meta),
            Some(CollectTaskMeta {
                collect_from: vec!["a".into(), "b".into()],
                reduce: CollectReduce::Concat,
            }),
        );
    }

    /// A malformed value under the key must read as `None` (and therefore
    /// `is_collect_task == false`) rather than panic the dispatcher. The
    /// meta is part of the durable row — a hand-edited file or a stale
    /// daemon could land any shape under the key, and "ignore bad rows"
    /// beats "refuse every other row too".
    #[test]
    fn malformed_collect_meta_does_not_panic() {
        let meta = serde_json::json!({
            COLLECT_META_KEY: "this is not a struct"
        });
        assert!(!is_collect_task(&meta));
        assert!(collect_task_meta(&meta).is_none());
    }
}