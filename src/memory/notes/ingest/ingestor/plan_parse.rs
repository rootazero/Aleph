//! Plan-JSON repair + lenient parsing + report summarisation helpers.
//!
//! These free functions support `DefaultCompoundIngestor::plan` (see
//! `super`): they recover missing `kind` discriminators, parse the plan
//! element-wise so one bad item can't sink the batch, and aggregate an
//! `ApplyReport` into a per-category `IngestBatchSummary`.

use crate::memory::notes::ingest::plan::{ApplyReport, IngestPlan, PageOp, SchemaProposal};
use tracing::warn;

/// Build an `IngestBatchSummary` from an `ApplyReport` by aggregating
/// `touched_paths` (each formatted as `"{category}/{filename}"`) into per-
/// category counts.
///
/// `ApplyReport` does not split per-path created vs updated, so `added` here
/// is conservatively the *total* touched-path count for the category and
/// `updated` is left at 0. This is sufficient for cadence-only consumers
/// (e.g. `refresh_index_after_ingest`); finer-grained breakdowns belong with
/// the planner once `ApplyReport` learns to track them per op.
pub(crate) fn summary_from_report(
    agent_id: &str,
    report: &ApplyReport,
) -> crate::memory::notes::orientation::types::IngestBatchSummary {
    use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};
    use std::collections::BTreeMap;

    let mut by_cat: BTreeMap<String, u32> = BTreeMap::new();
    for path in &report.touched_paths {
        if let Some((cat, _name)) = path.split_once('/') {
            *by_cat.entry(cat.to_string()).or_insert(0) += 1;
        }
    }

    IngestBatchSummary {
        agent_id: agent_id.to_string(),
        touched: by_cat
            .into_iter()
            .map(|(category, count)| TouchedCategory {
                category,
                added: count,
                updated: 0,
            })
            .collect(),
    }
}

/// Infer the missing `kind` discriminator from a plan op's field shape.
///
/// The planner LLM frequently emits an operation whose fields already pin down
/// exactly one `PageOp` variant but forgets the `kind` tag. Dropping such an op
/// (the old behaviour) discarded knowledge the model had already extracted, so
/// instead we recover the label it forgot. Checked most-specific field first so
/// the mapping stays unambiguous. Returns `None` when no variant fits.
pub(crate) fn infer_op_kind(
    op: &serde_json::Map<String, serde_json::Value>,
) -> Option<&'static str> {
    if op.contains_key("from") && op.contains_key("to") {
        return Some("link");
    }
    if op.contains_key("old_path") && op.contains_key("new_path") {
        return Some("supersede");
    }
    if op.contains_key("new_claim") {
        return Some("contradict");
    }
    if op.contains_key("expected_content_hash") {
        return Some("update");
    }
    if op.contains_key("title") || op.contains_key("summary") {
        return Some("create");
    }
    if op.contains_key("new_facts") || op.contains_key("new_links") {
        return Some("append");
    }
    None
}

/// Repair the `kind` discriminator on both tagged arrays of a raw plan JSON
/// before strict deserialization.
///
/// `ops` entries get `kind` inferred from their shape ([`infer_op_kind`]); any
/// op still unidentifiable is dropped rather than failing the whole batch.
/// `schema_proposals` have no reliable shape to infer from, so kindless ones
/// are simply dropped — leaving them in would make `serde` reject the entire
/// plan with `missing field kind`, starving the L1 note layer.
pub(crate) fn repair_kind_tags(mut value: serde_json::Value) -> serde_json::Value {
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    if let Some(arr) = obj.get_mut("ops").and_then(|v| v.as_array_mut()) {
        let before = arr.len();
        arr.retain_mut(|op| {
            let Some(o) = op.as_object_mut() else {
                return false;
            };
            if o.get("kind").and_then(|k| k.as_str()).is_some() {
                return true;
            }
            match infer_op_kind(o) {
                Some(k) => {
                    o.insert("kind".to_string(), serde_json::Value::String(k.to_string()));
                    true
                }
                None => false,
            }
        });
        let dropped = before - arr.len();
        if dropped > 0 {
            warn!("compound plan: dropped {dropped} ops with no identifiable kind");
        }
    }
    if let Some(arr) = obj
        .get_mut("schema_proposals")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|p| {
            p.as_object()
                .and_then(|o| o.get("kind"))
                .and_then(|k| k.as_str())
                .is_some()
        });
    }
    value
}

/// Deserialize an `IngestPlan` element-wise so one malformed item can't sink
/// the whole batch.
///
/// `serde_json::from_value::<IngestPlan>` is all-or-nothing: a single op or
/// schema-proposal that the planner emitted with a missing required field
/// (`kind` on an op, `rationale` on a proposal, etc.) fails the entire parse
/// and starves the L1 note layer. Instead we parse each `ops` / `schema_proposals`
/// entry independently and drop only the entries that fail, keeping every
/// well-formed operation. `reasoning` defaults to empty when absent.
pub(crate) fn parse_plan_lenient(json: serde_json::Value) -> IngestPlan {
    let reasoning = json
        .get("reasoning")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let ops = json
        .get("ops")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|op| match serde_json::from_value::<PageOp>(op.clone()) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        warn!("compound plan: dropping malformed op ({e})");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let schema_proposals = json
        .get("schema_proposals")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| serde_json::from_value::<SchemaProposal>(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    IngestPlan {
        reasoning,
        ops,
        schema_proposals,
    }
}
