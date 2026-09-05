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
use crate::memory::store::sqlite::memory_write_decisions::MemoryWriteReason;
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Path prefix every correction row is filed under — the same one
/// `FeedbackDistill` reads back (Phase 3 D2) and the duplicate scan below
/// queries.
///
/// Re-exported from the reader rather than re-declared here. Both sides used
/// to hold their own `"aleph://correction/"` literal under a comment promising
/// they would stay in sync; a promise in a comment is exactly the enforcement
/// §0 says does not exist. The truth source sits on the depended-upon side
/// (`memory`), because that is the direction the crate graph already runs.
use crate::memory::dreaming::stages::feedback_distill::CORRECTION_PATH_PREFIX;

/// How far back the exact-duplicate scan looks, in seconds (24h).
///
/// Scoped to roughly one distillation cycle: while a correction is still
/// waiting to be distilled, re-logging it byte-for-byte is pure noise AND
/// spends a second real-time consolidation drain (an LLM call) on the same
/// lesson. Beyond the window the earlier signal has normally been distilled
/// already, and a fresh flag is legitimate reinforcement rather than a
/// double-log.
const DUPLICATE_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Rows the duplicate scan will read. A bound, not a correctness knob: an
/// agent producing more than this many corrections in a day has a bigger
/// problem than a missed dedupe, and the guard degrades to "allow" rather
/// than to a table scan.
const DUPLICATE_SCAN_LIMIT: usize = 200;

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
    /// ID of the persisted `RawMemory` row for traceability. On a duplicate
    /// rejection this names the EXISTING row the new call collided with, so
    /// the model can see it is already on record rather than lost.
    pub raw_memory_id: String,
    /// Human-readable destination of the record — source material for the
    /// one-sentence acknowledgment the model owes the user after the write.
    ///
    /// `None` — and absent from the serialized shape — when nothing was
    /// persisted. Same rule as `RememberOutput.destination` and
    /// `NoteManageResult.destination`: a receipt naming where a REFUSED write
    /// would have gone is how a model tells the user something was recorded
    /// when it was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

/// Records user-correction signals into `raw_memory` under
/// `aleph://correction/{id}` for later distillation by `FeedbackDistill`.
pub struct FlagUserCorrectionTool {
    store: Arc<dyn RawMemoryStore>,
    agent_id: String,
    /// Where write decisions are audited — the same log `remember` files to
    /// and `memory_trace(kind: "write_decision")` reads back. `None` disables
    /// the audit entirely (tests, early boot): a correction must never fail
    /// because its diagnostic row could not be stored.
    decisions: Option<MemoryBackend>,
}

/// A settled `flag_user_correction` outcome plus the closed-vocabulary reason
/// filed for it. Bundled for the same reason `remember::Decided` is: a branch
/// cannot produce an outcome without naming its reason, and no branch files
/// its own row, so a branch added later cannot silently skip the audit.
struct Decided {
    output: FlagUserCorrectionOutput,
    reason: MemoryWriteReason,
}

impl FlagUserCorrectionTool {
    pub fn new(store: Arc<dyn RawMemoryStore>, agent_id: impl Into<String>) -> Self {
        Self {
            store,
            agent_id: agent_id.into(),
            decisions: None,
        }
    }

    /// Attach the write-decision log. Separate from [`Self::new`] because the
    /// backend is optional exactly where there is no database at all.
    #[must_use]
    pub fn with_decision_log(mut self, db: Option<MemoryBackend>) -> Self {
        self.decisions = db;
        self
    }

    const fn severity_token(s: &Severity) -> &'static str {
        match s {
            Severity::Low => "low",
            Severity::Med => "med",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// File one row in the shared curated-write audit log. Best-effort by
    /// construction — see `RememberTool::record_decision`, which this mirrors.
    fn record_decision(&self, subject: &str, reason: MemoryWriteReason) {
        let Some(db) = self.decisions.as_ref() else {
            return;
        };
        if let Err(e) = db.record_write_decision(&self.agent_id, ACTION_LABEL, reason, subject) {
            tracing::warn!("flag_user_correction: write decision not recorded: {e}");
        }
    }

    /// The id of an existing, byte-identical correction at the SAME severity
    /// inside [`DUPLICATE_WINDOW_SECS`], if one exists.
    ///
    /// Severity is part of the key on purpose: a user pushing harder on the
    /// same point ("I already told you") is a genuine escalation, and
    /// collapsing `low` → `critical` onto the earlier row would throw away the
    /// only signal that says so. What is rejected is the exact re-log.
    ///
    /// R7 note: the *judgement* "is this the same correction" stays the
    /// model's — this only refuses a byte-identical repeat, the same
    /// deterministic rule `CuratedMemoryStore::add` enforces on rung 1. Rung 2
    /// had the rule stated in its prompt ("never log the same correction
    /// twice") and enforced nowhere.
    ///
    /// A read failure returns `None` (allow the write): the guard exists to
    /// suppress noise, and failing it closed would drop a real correction over
    /// a transient database error.
    async fn duplicate_of(&self, content: &str, severity: &str) -> Option<String> {
        let since = chrono::Utc::now().timestamp() - DUPLICATE_WINDOW_SECS;
        let recent = self
            .store
            .get_raw_by_path_prefix_since(
                CORRECTION_PATH_PREFIX,
                &self.agent_id,
                since,
                DUPLICATE_SCAN_LIMIT,
            )
            .await
            .ok()?;
        recent
            .into_iter()
            .find(|r| {
                r.content.trim() == content
                    && matches!(
                        &r.source,
                        RawMemorySource::Correction { severity: s, .. } if s == severity
                    )
            })
            .map(|r| r.id)
    }
}

/// Action label stamped on this tool's rows in the shared write-decision log.
/// Distinct from `remember`'s `add`/`replace`/`remove`/`batch` so a reader can
/// tell which rung of the destination ladder a row is about.
const ACTION_LABEL: &str = "flag_correction";

impl Clone for FlagUserCorrectionTool {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            agent_id: self.agent_id.clone(),
            decisions: self.decisions.clone(),
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
         use the `destination` field from the result. Never quote the stored content back verbatim. \
         A re-log of the identical correction at the same severity returns `success: false` and \
         no `destination`; that is not an error and nothing is lost — `raw_memory_id` names the \
         record already holding it, so do not retry. A user pushing harder on the same point IS \
         new information: re-flag at a higher `severity`. IF NOTHING LANDED (no `destination` in \
         the result): never acknowledge a save that did not happen — say it was already on \
         record, or say nothing about memory at all.";

    type Args = FlagUserCorrectionArgs;
    type Output = FlagUserCorrectionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // What this call is ABOUT, read off the args before `decide` consumes
        // them — the branches that never reach the store still have to be able
        // to name their subject.
        let subject = args.content.trim().to_string();
        let decided = self.decide(args).await?;
        // THE chokepoint: every settled outcome — the landed write and every
        // refusal — is audited here exactly once. A hard store error files no
        // row (it is a system fault, not a decision about the content), which
        // is the same split `remember::reason_of` draws.
        self.record_decision(&subject, decided.reason);
        Ok(decided.output)
    }
}

impl FlagUserCorrectionTool {
    /// Run the write and settle it into one [`Decided`].
    async fn decide(&self, args: FlagUserCorrectionArgs) -> Result<Decided> {
        let content = args.content.trim().to_string();
        let severity = Self::severity_token(&args.severity);
        if content.is_empty() {
            return Ok(Decided {
                output: FlagUserCorrectionOutput {
                    success: false,
                    message: "rejected: correction content is empty — describe what the user \
                              corrected, in your own words"
                        .into(),
                    raw_memory_id: String::new(),
                    destination: None,
                },
                reason: MemoryWriteReason::Empty,
            });
        }

        if let Some(existing) = self.duplicate_of(&content, severity).await {
            return Ok(Decided {
                output: FlagUserCorrectionOutput {
                    success: false,
                    message: format!(
                        "rejected: this exact correction is already on record at the same \
                         severity ({CORRECTION_PATH_PREFIX}{existing}) and has not been \
                         distilled yet — logging it again would not strengthen it. Continue \
                         your reply; log a NEW correction only for a different point, or the \
                         same point at a higher severity."
                    ),
                    raw_memory_id: existing,
                    destination: None,
                },
                reason: MemoryWriteReason::Duplicate,
            });
        }

        let raw = RawMemory::new(
            content,
            RawMemorySource::Correction {
                severity: severity.into(),
                suggested_rule: args.suggested_rule,
            },
        )
        .with_agent(&self.agent_id);
        // Embed the raw_memory id in the path so the prefix-query reader can
        // surface every correction without scanning the whole table.
        let raw_memory_id = raw.id.clone();
        let raw = raw.with_path(format!("{CORRECTION_PATH_PREFIX}{raw_memory_id}"));

        self.store.insert_raw_memory(&raw).await?;
        Self::spawn_sedimentation(&self.agent_id);

        Ok(Decided {
            output: FlagUserCorrectionOutput {
                success: true,
                message: "Correction logged.".into(),
                destination: Some(format!(
                    "{CORRECTION_PATH_PREFIX}{raw_memory_id} — flushed immediately; distilled \
                     into a feedback/ note by the nightly dream cycle (high/critical severities \
                     bypass the batch quorum)"
                )),
                raw_memory_id,
            },
            reason: MemoryWriteReason::Written,
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
        )
        .with_decision_log(Some(backend.clone() as MemoryBackend));
        (tool, backend)
    }

    /// Reasons filed for the default agent, newest first.
    fn reasons(db: &Arc<SqliteMemoryBackend>) -> Vec<String> {
        db.recent_write_decisions(DEFAULT_AGENT_ID, None, 50)
            .unwrap()
            .into_iter()
            .map(|r| r.reason)
            .collect()
    }

    fn args(content: &str, severity: Severity) -> FlagUserCorrectionArgs {
        FlagUserCorrectionArgs {
            content: content.into(),
            severity,
            suggested_rule: None,
        }
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
        let dest = out
            .destination
            .as_deref()
            .expect("a landed write carries its receipt");
        assert!(
            dest.contains(&out.raw_memory_id),
            "destination must reference the persisted row"
        );
        assert!(
            dest.starts_with("aleph://correction/"),
            "destination must name the correction path"
        );
        assert!(
            dest.contains("feedback/") && dest.contains("dream"),
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

    /// The writer and the distiller must address the SAME rows.
    ///
    /// This is the assertion the two "must stay in sync" comments used to
    /// stand in for. It does not compare the two constants (they are now one
    /// constant, so that would be tautological) — it drives the real tool, then
    /// asks the dream stage's own gate predicate whether it can see the result.
    /// If the prefix, the agent partitioning, or the watermark key ever drift
    /// apart again, this goes red instead of the correction going silently
    /// unread.
    #[tokio::test]
    async fn a_logged_correction_is_visible_to_the_stage_that_distills_it() {
        use crate::memory::dreaming::stages::feedback_distill::has_undistilled_corrections;

        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let tool = FlagUserCorrectionTool::new(
            backend.clone() as Arc<dyn RawMemoryStore>,
            DEFAULT_AGENT_ID.to_string(),
        );
        let lookback = crate::config::types::memory::default_feedback_lookback();
        let min = crate::config::types::memory::default_feedback_distill_min_candidates();
        let store: MemoryBackend = backend.clone();

        assert!(
            !has_undistilled_corrections(&store, DEFAULT_AGENT_ID, lookback, min).await,
            "nothing logged yet"
        );

        // High severity: a single row passes the gate's urgency bypass, so this
        // stays a one-row test without needing `min` unrelated corrections.
        let out = tool
            .call(args("stop guessing at file paths", Severity::High))
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);

        assert!(
            has_undistilled_corrections(&store, DEFAULT_AGENT_ID, lookback, min).await,
            "the distiller must see the row the tool just wrote — a writer and a \
             reader that address different prefixes both look healthy in isolation"
        );
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
    async fn an_identical_re_log_is_refused_and_writes_nothing() {
        // The rule "never log the same correction twice" was stated in the
        // prompt and enforced nowhere, while rung 1 (`remember`) has refused
        // exact duplicates structurally since day one. Each re-log also spent
        // a real-time consolidation drain — an LLM call — on the same lesson.
        let (tool, backend) = make_tool();
        let first = tool
            .call(args("stop using JSDoc", Severity::Med))
            .await
            .unwrap();
        assert!(first.success);

        let second = tool
            .call(args("stop using JSDoc", Severity::Med))
            .await
            .unwrap();
        assert!(!second.success, "{}", second.message);
        assert!(
            second.message.starts_with("rejected: "),
            "{}",
            second.message
        );
        assert!(
            second.destination.is_none(),
            "a refused write must carry no receipt: {:?}",
            second.destination
        );
        // Not a lost signal: the refusal names the row that already holds it.
        assert_eq!(second.raw_memory_id, first.raw_memory_id);
        // The absence must survive serialization — a shape reader that never
        // looks at `success` must not find a destination key either.
        let json = serde_json::to_value(&second).unwrap();
        assert!(json.get("destination").is_none(), "{json}");

        // Exactly one row on disk.
        let entries = backend
            .get_raw_by_path_prefix(CORRECTION_PATH_PREFIX, DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1, "the re-log must not persist a second row");
    }

    #[tokio::test]
    async fn a_higher_severity_re_log_is_an_escalation_not_a_duplicate() {
        // "I already told you" is new information. Collapsing it onto the
        // earlier row would discard the only signal that says the user is
        // pushing harder, so severity is part of the duplicate key.
        let (tool, backend) = make_tool();
        tool.call(args("stop using JSDoc", Severity::Low))
            .await
            .unwrap();
        let escalated = tool
            .call(args("stop using JSDoc", Severity::Critical))
            .await
            .unwrap();
        assert!(escalated.success, "{}", escalated.message);

        let entries = backend
            .get_raw_by_path_prefix(CORRECTION_PATH_PREFIX, DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn empty_content_is_refused_before_the_store_is_touched() {
        let (tool, backend) = make_tool();
        let out = tool.call(args("   \n ", Severity::Med)).await.unwrap();
        assert!(!out.success);
        assert!(out.message.starts_with("rejected: "), "{}", out.message);
        assert!(out.destination.is_none());
        assert!(backend
            .get_raw_by_path_prefix(CORRECTION_PATH_PREFIX, DEFAULT_AGENT_ID, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn every_outcome_lands_in_the_shared_write_decision_log() {
        // The log's promise — `memory_trace(kind: "write_decision")` answers
        // "why wasn't that saved" — was true for the hot zone and silently
        // empty for the correction rail. Both outcomes must be legible, and
        // under an `action` that keeps the two rails distinguishable.
        let (tool, backend) = make_tool();
        tool.call(args("stop using JSDoc", Severity::Med))
            .await
            .unwrap();
        tool.call(args("stop using JSDoc", Severity::Med))
            .await
            .unwrap();

        let rows = backend
            .recent_write_decisions(DEFAULT_AGENT_ID, None, 50)
            .unwrap();
        assert_eq!(rows.len(), 2, "success and refusal both audited: {rows:?}");
        assert!(rows.iter().all(|r| r.action == "flag_correction"));
        // The subject is what the call was ABOUT, not the outcome message.
        assert!(rows.iter().all(|r| r.subject == "stop using JSDoc"));
        let filed = reasons(&backend);
        assert!(filed.contains(&"written".to_string()), "{filed:?}");
        assert!(filed.contains(&"duplicate".to_string()), "{filed:?}");
    }

    #[tokio::test]
    async fn without_a_backend_the_audit_is_a_no_op_not_a_failure() {
        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let tool = FlagUserCorrectionTool::new(
            backend as Arc<dyn RawMemoryStore>,
            DEFAULT_AGENT_ID.to_string(),
        );
        let out = tool.call(args("still lands", Severity::Med)).await.unwrap();
        assert!(out.success, "{}", out.message);
    }

    #[test]
    fn description_states_the_soft_rejection_shape() {
        // The refusal returns `success: false`. Without saying so here, the
        // model reads it as a tool failure and either retries it or tells the
        // user the correction was lost — both worse than the double-log the
        // guard exists to prevent.
        let d = <FlagUserCorrectionTool as AlephTool>::DESCRIPTION;
        assert!(d.contains("success: false"), "must name the refusal field");
        assert!(
            d.contains("not an error") && d.contains("nothing is lost"),
            "a refusal the model reads as a failure gets retried or reported as data loss"
        );
        assert!(d.contains("higher `severity`"), "escalation must stay open");
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
