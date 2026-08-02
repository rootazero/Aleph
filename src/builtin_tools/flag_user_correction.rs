//! `flag_user_correction` — record a user-correction signal for later distillation.
//!
//! Phase 3 self-evolution path α: when the main LLM detects that the user has
//! corrected a mistake it made or pushed back on its approach, it calls this
//! tool. The signal is persisted as a typed `RawMemorySource::Correction`
//! row in `raw_memory` under the path prefix `aleph://correction/...`, where
//! the `FeedbackDistill` Dream stage will later read and distill it into a
//! `feedback/` knowledge note.
//!
//! See Phase 3 Schema Decisions D1/D2 in
//! `docs/superpowers/plans/2026-04-29-aleph-self-evolution.md`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::memory::notes::Severity;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments accepted by the `flag_user_correction` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlagUserCorrectionArgs {
    /// The user's correction in your own words (1-2 sentences). Capture the
    /// underlying intent, not just the surface phrasing.
    pub content: String,
    /// Strength of the signal:
    /// - `low`: a one-off preference for this specific case
    /// - `med`: a project-level rule worth remembering generally
    /// - `high`: a strong directive that should override your defaults
    /// - `critical`: an absolute redline (safety / privacy / correctness)
    pub severity: Severity,
    /// Optional one-line imperative for how you should behave next time
    /// (e.g. "never write `JSDoc`", "always quote file paths in shell args").
    #[serde(default)]
    pub suggested_rule: Option<String>,
}

/// Result returned to the LLM.
#[derive(Debug, Clone, Serialize)]
pub struct FlagUserCorrectionOutput {
    pub success: bool,
    pub message: String,
    /// ID of the persisted `RawMemory` row for traceability.
    pub raw_memory_id: String,
    /// Human-readable destination of the record — source material for the
    /// one-sentence acknowledgment the model owes the user after the write.
    pub destination: String,
}

/// Records user-correction signals into `raw_memory` under
/// `aleph://correction/{id}` for later distillation by `FeedbackDistill`.
pub struct FlagUserCorrectionTool {
    store: Arc<dyn RawMemoryStore>,
    agent_id: String,
}

impl FlagUserCorrectionTool {
    pub fn new(store: Arc<dyn RawMemoryStore>, agent_id: impl Into<String>) -> Self {
        Self {
            store,
            agent_id: agent_id.into(),
        }
    }

    const fn severity_token(s: &Severity) -> &'static str {
        match s {
            Severity::Low => "low",
            Severity::Med => "med",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

impl Clone for FlagUserCorrectionTool {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            agent_id: self.agent_id.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for FlagUserCorrectionTool {
    const NAME: &'static str = "flag_user_correction";
    const DESCRIPTION: &'static str = "Record a user-correction signal so the system can learn from it. \
         Call when the user corrects a mistake you made or pushes back on your approach. \
         Durable preferences go to `remember` instead. Use conservatively — do NOT flag praise, \
         neutral acknowledgement, or your own internal reasoning, and never log the same correction \
         twice. Continue the conversation normally after calling, then close your reply with ONE \
         short sentence, in the user's language, acknowledging where the correction was recorded — \
         use the `destination` field from the result. Never quote the stored content back verbatim.";

    type Args = FlagUserCorrectionArgs;
    type Output = FlagUserCorrectionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let raw = RawMemory::new(
            args.content,
            RawMemorySource::Correction {
                severity: Self::severity_token(&args.severity).into(),
                suggested_rule: args.suggested_rule,
            },
        )
        .with_agent(&self.agent_id);
        // Embed the raw_memory id in the path so the prefix-query reader can
        // surface every correction without scanning the whole table.
        let raw_memory_id = raw.id.clone();
        let raw = raw.with_path(format!("aleph://correction/{raw_memory_id}"));

        self.store.insert_raw_memory(&raw).await?;
        Self::spawn_sedimentation(&self.agent_id);

        Ok(FlagUserCorrectionOutput {
            success: true,
            message: "Correction logged.".into(),
            destination: format!(
                "aleph://correction/{raw_memory_id} — flushed immediately; distilled into a \
                 feedback/ note by the nightly dream cycle (high/critical severities bypass \
                 the batch quorum)"
            ),
            raw_memory_id,
        })
    }
}

impl FlagUserCorrectionTool {
    /// Kick an immediate compress→link drain for this agent, off the critical path.
    ///
    /// The model has just made the exact judgement the old keyword `SignalDetector`
    /// tried to guess at — *"the user corrected me"* — so this is the R7-clean place
    /// to trigger an immediate consolidation. Previously the two were inverted: a
    /// substring match on "不对"/"actually" compressed instantly, while the model's
    /// own high-confidence signal just sat in `raw_memory` waiting for the next dream
    /// cycle (hours). Now the LLM's judgement is the fast path and there is no keyword
    /// table at all.
    ///
    /// The `FlushGuard` is taken SYNCHRONOUSLY, before the spawn — acquiring it inside
    /// the task would race a follow-on session's `await_ready` (`tokio::spawn` returns
    /// before the task is first polled, so the waiter could observe an empty registry
    /// and silently skip the gate). Same hazard, same fix, as the session-end site.
    ///
    /// Fire-and-forget: the drain runs an LLM ingest call, and the turn must never
    /// block on it.
    fn spawn_sedimentation(agent_id: &str) {
        // No CompressionService registered → memory isn't configured; nothing to
        // drain into. Also keeps this a no-op in unit tests (and outside a runtime).
        let Some(compression) = crate::thinker::memory_context_provider::session_end_compression()
        else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let agent = agent_id.to_string();
        let guard = crate::memory::flush::global_registry().begin(&agent);
        rt.spawn(async move {
            crate::memory::flush::flush_agent_memory(guard, agent, compression).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::routing::DEFAULT_AGENT_ID;

    fn make_tool() -> (FlagUserCorrectionTool, Arc<SqliteMemoryBackend>) {
        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let tool = FlagUserCorrectionTool::new(
            backend.clone() as Arc<dyn RawMemoryStore>,
            DEFAULT_AGENT_ID.to_string(),
        );
        (tool, backend)
    }

    #[tokio::test]
    async fn flag_correction_writes_typed_raw_memory() {
        let (tool, backend) = make_tool();
        let out = tool
            .call(FlagUserCorrectionArgs {
                content: "user pushed back on JSDoc".into(),
                severity: Severity::Med,
                suggested_rule: Some("never write JSDoc".into()),
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(!out.raw_memory_id.is_empty());
        // D4 acknowledgment contract: the result names its destination so the
        // model can tell the user where the lesson landed in one sentence.
        assert!(
            out.destination.contains(&out.raw_memory_id),
            "destination must reference the persisted row"
        );
        assert!(
            out.destination.starts_with("aleph://correction/"),
            "destination must name the correction path"
        );
        assert!(
            out.destination.contains("feedback/") && out.destination.contains("dream"),
            "destination must explain the distillation rail"
        );

        // Read back via the same prefix FeedbackDistill will use (Phase 3 D2).
        let entries = backend
            .get_raw_by_path_prefix("aleph://correction/", DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.content, "user pushed back on JSDoc");
        match &e.source {
            RawMemorySource::Correction {
                severity,
                suggested_rule,
            } => {
                assert_eq!(severity, "med");
                assert_eq!(suggested_rule.as_deref(), Some("never write JSDoc"));
            }
            other => panic!("expected Correction variant, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn flag_correction_path_uses_correction_prefix() {
        let (tool, backend) = make_tool();
        let out = tool
            .call(FlagUserCorrectionArgs {
                content: "x".into(),
                severity: Severity::Low,
                suggested_rule: None,
            })
            .await
            .unwrap();
        let entries = backend
            .get_raw_by_path_prefix("aleph://correction/", DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        let path = entries[0].path.as_deref().unwrap_or("");
        assert!(
            path.starts_with("aleph://correction/"),
            "path must use correction prefix, got {path:?}"
        );
        assert!(
            path.contains(&out.raw_memory_id),
            "path must embed the raw_memory id"
        );
    }

    #[tokio::test]
    async fn flag_correction_without_suggested_rule_works() {
        let (tool, backend) = make_tool();
        tool.call(FlagUserCorrectionArgs {
            content: "no rule".into(),
            severity: Severity::High,
            suggested_rule: None,
        })
        .await
        .unwrap();
        let entries = backend
            .get_raw_by_path_prefix("aleph://correction/", DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].source {
            RawMemorySource::Correction {
                severity,
                suggested_rule,
            } => {
                assert_eq!(severity, "high");
                assert!(suggested_rule.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn flag_correction_round_trips_critical_severity() {
        let (tool, backend) = make_tool();
        tool.call(FlagUserCorrectionArgs {
            content: "absolute redline".into(),
            severity: Severity::Critical,
            suggested_rule: Some("never bypass the safety check".into()),
        })
        .await
        .unwrap();
        let entries = backend
            .get_raw_by_path_prefix("aleph://correction/", DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        match &entries[0].source {
            RawMemorySource::Correction { severity, .. } => assert_eq!(severity, "critical"),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn flag_correction_args_reject_invalid_severity_string() {
        let bad = r#"{"content":"x","severity":"WRONG"}"#;
        let r: std::result::Result<FlagUserCorrectionArgs, _> = serde_json::from_str(bad);
        assert!(r.is_err(), "invalid severity must fail at deserialize");
    }

    #[test]
    fn description_matches_ladder_rung_two() {
        // Destination-ladder alignment: rung 2 is mistake-correction /
        // pushback ONLY. Preference capture belongs to rung 1 (`remember`) —
        // the description must not compete for it.
        let d = <FlagUserCorrectionTool as AlephTool>::DESCRIPTION;
        assert!(
            d.contains("corrects a mistake you made or pushes back on your approach"),
            "trigger wording must match the memory-protocol ladder's rung 2"
        );
        assert!(
            !d.contains("clear preference") && !d.contains("strong-preference"),
            "preference-capture phrasing must not resurface here"
        );
        assert!(
            d.contains("Durable preferences go to `remember`"),
            "must redirect durable preferences to rung 1"
        );
        // The destination/acknowledgment contract stays intact.
        assert!(
            d.contains("`destination` field"),
            "ack contract must keep pointing at the destination field"
        );
    }
}
