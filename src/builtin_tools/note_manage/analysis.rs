//! Read-only analysis surface: `insights` (knowledge-graph health) and
//! `evolution` (why memory changed, or didn't, in recent dream cycles).
//!
//! Both read state materialized by the dream daemon, so an empty result means
//! "not recomputed yet", never an error.

use crate::error::{AlephError, Result};
use crate::memory::notes::store::NoteStore;

use super::args::{NoteManageArgs, NoteManageResult};
use super::NoteManageTool;

impl NoteManageTool {
    /// Read materialized knowledge-graph health insights for the agent: knowledge
    /// gaps (isolated notes), sparse communities, bridge notes, and surprising
    /// cross-community connections. Read-only — the insights are materialized by
    /// `GraphRecomputeStage` during dreaming, so an empty result simply means the
    /// graph has not been recomputed yet rather than an error.
    pub(super) async fn handle_insights(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();
        let rows = self
            .indexer
            .store()
            .read_graph_insights(agent_id, None)
            .await
            .map_err(|e| AlephError::tool(format!("read insights failed: {e}")))?;
        let mut content = String::from("# Knowledge Graph Insights\n\n");
        if rows.is_empty() {
            content.push_str(
                "_No materialized insights yet (graph recompute runs during dreaming)._\n",
            );
        } else {
            for (kind, payload) in &rows {
                content.push_str(&format!("## {kind}\n```json\n{payload}\n```\n\n"));
            }
        }
        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!("Graph insights ({} kinds)", rows.len()),
            destination: None,
            note_path: None,
            content: Some(content),
            notes: None,
            search: None,
        })
    }

    /// Read-only: summarize the memory-evolution gate from the last few dream
    /// cycles' event log. Surfaces the health score trend, best-ever score, the
    /// gate verdict, rejected merges, and any churn-pathology cooldown.
    pub(super) async fn handle_evolution(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        use crate::memory::dreaming::evolution::GateOutcome;
        use crate::memory::dreaming::{EventLog, GateDecision};

        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();
        let agent_dir = self.indexer.memory_dir().join(agent_id);
        let events = EventLog::new(&agent_dir)
            .read_last(5)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(?e, "handle_evolution: failed to read event log");
                Vec::new()
            });

        let mut content = String::from("# Memory Evolution Gate\n\n");
        if events.is_empty() {
            content.push_str("_No dream cycles recorded yet — the evolution gate runs nightly during memory consolidation._\n");
            return Ok(NoteManageResult {
                related_notes: None,
                success: true,
                message: "No dream cycles recorded yet".to_string(),
                destination: None,
                note_path: None,
                content: Some(content),
                notes: None,
                search: None,
            });
        }

        for ev in events.iter().rev() {
            content.push_str(&format!(
                "## Cycle {} · strategy `{}`\n",
                ev.cycle, ev.strategy
            ));
            match &ev.report.evolution {
                Some(e) => {
                    let verdict = match e.outcome {
                        GateOutcome::AcceptNewBest => "✅ accepted (new best)",
                        GateOutcome::Accept => "✅ accepted",
                        GateOutcome::Reject => "⛔ rejected (no improvement)",
                    };
                    content.push_str(&format!(
                        "- health: {:.3} → {:.3} (best {:.3}) — {verdict}\n",
                        e.baseline, e.candidate, e.best
                    ));
                    if e.merges_rejected > 0 {
                        content.push_str(&format!(
                            "- {} proposed merge(s) rejected by the gate (would fuse distinct knowledge)\n",
                            e.merges_rejected
                        ));
                    }
                }
                None => content.push_str("- (no evolution score for this cycle)\n"),
            }
            if let GateDecision::Conserve {
                reason,
                cooldown_remaining,
            } = &ev.gate_decision
            {
                content.push_str(&format!(
                    "- ⚠️ churn pathology: {reason} (cooldown {cooldown_remaining})\n"
                ));
            }
            content.push_str(&render_distill_actions(&ev.report.distill_actions));
            content.push('\n');
        }

        let latest = events.last();
        let msg = latest
            .and_then(|e| e.report.evolution.as_ref())
            .map_or_else(
                || "Evolution gate state (no score)".to_string(),
                |e| {
                    format!(
                        "Evolution gate: health {:.3} (best {:.3})",
                        e.candidate, e.best
                    )
                },
            );

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: msg,
            destination: None,
            note_path: None,
            content: Some(content),
            notes: None,
            search: None,
        })
    }
}

/// Render one cycle's per-action distillation ledger.
///
/// Without this, `DreamReport.distill_actions` existed only in
/// `dream_events.jsonl`: the ledger that answers "why was this lesson NOT
/// remembered" was written for every distilling stage (`skill_distill`,
/// `feedback_distill`, `tool_failure_distill`) and read by nobody the model
/// can reach. A ledger that covers a question on only one rail answers it on
/// only that rail — and here it answered on none.
///
/// Grouped by stage so a reader can tell which production line dropped what;
/// the reason (`error`, which also carries an LLM `skip` rationale) is the
/// whole point of the row and is never elided.
fn render_distill_actions(actions: &[crate::memory::dreaming::DistillActionRecord]) -> String {
    use crate::memory::dreaming::DistillOutcome;

    if actions.is_empty() {
        return String::new();
    }

    let mut stages: Vec<&str> = actions.iter().map(|a| a.stage.as_str()).collect();
    stages.sort_unstable();
    stages.dedup();

    let mut out = String::from("- distillation ledger:\n");
    for stage in stages {
        out.push_str(&format!("  - `{stage}`\n"));
        for a in actions.iter().filter(|a| a.stage == stage) {
            let outcome = match a.outcome {
                DistillOutcome::Applied => "applied",
                DistillOutcome::FilteredNonCandidate => "dropped (path not offered to the model)",
                DistillOutcome::FilteredInvalid => "dropped (format/safety gate)",
                DistillOutcome::FilteredEvidence => "dropped (recall evidence outweighed it)",
                DistillOutcome::Error => "errored while applying",
            };
            let subject = a
                .title
                .as_deref()
                .or(a.target_path.as_deref())
                .unwrap_or("(untitled)");
            out.push_str(&format!(
                "    - {} `{}` → {outcome}",
                a.action_kind, subject
            ));
            if let Some(reason) = a.error.as_deref().filter(|r| !r.is_empty()) {
                out.push_str(&format!(" — {reason}"));
            }
            out.push('\n');
        }
    }
    out
}
