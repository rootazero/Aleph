//! `memory_trace` — inspect the recorded sources of a memory claim.
//!
//! `walk` resolves same-partition profile, note, and raw sources in bounded
//! steps. An unavailable raw citation remains visible as `pruned: true`.
//!
//! The `write_decision` kind answers the mirror question — "why is this NOT in
//! memory?" — off the curated-write audit log instead of the evidence chain. It
//! lives here rather than in a tool of its own because both are provenance
//! questions about one memory claim, and this tool already holds the two handles
//! the answer needs (the memory backend and the calling agent's id).

mod walk;

use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::sqlite::memory_write_decisions::MAX_DECISIONS_PER_AGENT;
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;
pub use aleph_protocol::memory_trace::{TraceKind, TraceResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Write-decision rows returned when the caller does not ask for a count.
/// Small on purpose: these rows are read back into a model's context, and the
/// question they answer ("what happened to this fact?") is about the last few
/// attempts, not the whole retained window.
const DEFAULT_WRITE_DECISION_ROWS: usize = 20;

/// Arguments for the `memory_trace` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTraceArgs {
    /// What to trace: a note path (`category/name`), a raw memory id, a USER.md
    /// section heading, or — for `write_decision` — a literal substring of the
    /// fact whose fate you are asking about (empty browses recent decisions).
    pub target: String,
    /// The kind of target being traced.
    pub kind: TraceKind,
    /// Maximum rows in `evidence` (up to 500) or `write_decisions` (default 20).
    /// Does not change which note edges or fact annotations are reported.
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// Tool that resolves recorded evidence within the runtime's partition.
pub struct MemoryTraceTool {
    db: MemoryBackend,
    raw_store: Arc<dyn RawMemoryStore>,
    agent_id: String,
    note_memory_dir: PathBuf,
}

impl MemoryTraceTool {
    pub(crate) fn services() -> Vec<aleph_slot_api::capability::Service> {
        vec![crate::memory::runtime::required_service()]
    }

    /// Tool identifier registered with the agent runtime.
    pub const NAME: &'static str = "memory_trace";

    /// Tool description shown to the LLM in its system prompt.
    pub const DESCRIPTION: &'static str =
        "Trace recorded sources of a profile section, Wiki note or raw ID. Source-note \
         edges and raw excerpts stay in one memory partition. Unavailable raw sources \
         are marked pruned. `truncated` means a limit omitted results; `incomplete` \
         means some recorded evidence was unavailable.\n\n\
         `kind: \"note\"` returns the target's fact annotations (`origin`, `inferred`, \
         and optional `source_id`). These describe recorded attribution, not proof of \
         what the user did or did not say; use the actual excerpts to answer that.\n\n\
         Use `kind: \"write_decision\"` for the mirror question — why a fact is NOT in memory. \
         It returns one row per `remember` OR `flag_user_correction` write ATTEMPT (newest \
         first), refusals included, each with a machine-readable `reason` (written / duplicate / \
         over_budget / scanner_rejected / …). `target` is a \
         literal substring of the recorded subject; pass an empty string to browse recent \
         decisions. Answer \"why didn't you remember that?\" from these rows, not from \
         recollection.";

    /// Create a new `MemoryTraceTool`.
    ///
    /// `note_memory_dir` is the base notes directory (the parent of per-agent
    /// subdirectories), matching the path injected into `ProfileSynthesizer` and
    /// the note-wiki tooling.
    pub fn new(
        db: MemoryBackend,
        raw_store: Arc<dyn RawMemoryStore>,
        agent_id: impl Into<String>,
        note_memory_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            raw_store,
            agent_id: agent_id.into(),
            note_memory_dir,
        }
    }

    /// Read the evidence chain, or the separate curated-write decision ledger.
    pub async fn call_impl(&self, args: MemoryTraceArgs) -> anyhow::Result<TraceResult> {
        if !walk::valid_partition(&self.agent_id) {
            return Ok(TraceResult {
                target: args.target,
                ..TraceResult::default()
            });
        }
        if args.kind != TraceKind::WriteDecision {
            return walk::trace(self, args).await;
        }
        let limit = args
            .max_results
            .unwrap_or(DEFAULT_WRITE_DECISION_ROWS)
            .min(MAX_DECISIONS_PER_AGENT);
        let mut rows = self
            .db
            .recent_write_decisions(&self.agent_id, Some(&args.target), limit + 1)
            .map_err(|error| anyhow::anyhow!("recent_write_decisions: {error}"))?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);
        Ok(TraceResult {
            target: args.target,
            write_decisions: rows,
            truncated,
            ..TraceResult::default()
        })
    }
}

#[cfg(test)]
mod walk_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::markdown::MarkdownSessionStore;
    use crate::memory::store::{
        MemoryBackend, RawMemory, RawMemorySource, RawMemoryStore, SqliteMemoryBackend,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn trace_note_to_raw_and_graceful_prune() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());

        // A note that cites raw-present (exists) and raw-missing (does not).
        let note = KnowledgeNote {
            title: "typescript".into(),
            category: "preference".into(),
            facts: vec!["prefers ts".into()],
            source_notes: vec!["raw-present".into(), "raw-missing".into()],
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "preference")
            .await
            .unwrap();

        // Insert only the "present" raw.
        let mut r = RawMemory::new(
            "user: I prefer TypeScript".into(),
            RawMemorySource::Transcript,
        );
        r.id = "raw-present".into();
        r.agent_id = "default".into();
        raw_store.insert_raw_memory(&r).await.unwrap();

        // note_memory_dir unused for TraceKind::Note; any path is fine.
        let tool = MemoryTraceTool::new(backend, raw_store, "default", dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "preference/typescript".into(),
                kind: TraceKind::Note,
                max_results: None,
            })
            .await
            .unwrap();

        // One raw resolved with content, one pruned.
        assert!(
            out.evidence
                .iter()
                .any(|e| e.raw_id == "raw-present" && e.content.is_some()),
            "raw-present should have content"
        );
        assert!(
            out.evidence
                .iter()
                .any(|e| e.raw_id == "raw-missing" && e.pruned),
            "raw-missing should be pruned"
        );
    }

    /// North-star (spec §7.5): a high-level L3 profile claim drills all the way
    /// down to the L0 raw utterance. Section "Identity" → session id → raws in
    /// that session → notes citing them → those notes' source raws → raw text
    /// mentioning "TypeScript". The whole chain must connect end-to-end.
    #[tokio::test]
    async fn north_star_profile_section_drills_to_raw_typescript() {
        use crate::memory::notes::profile::{render_user_md, ProfileStore};
        use std::collections::BTreeMap;

        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let agent = "main";

        // L0: a raw memory carrying the ground-truth utterance, tagged with a session.
        let mut raw = RawMemory::new(
            "user: I prefer TypeScript for all new projects".into(),
            RawMemorySource::Transcript,
        );
        raw.id = "raw-ts".into();
        raw.agent_id = agent.into();
        raw.session_id = Some("ses_x".into());
        raw_store.insert_raw_memory(&raw).await.unwrap();

        // L1: a note distilled from that raw (source_notes cites it → notes_sources).
        let note = KnowledgeNote {
            title: "typescript".into(),
            category: "preference".into(),
            facts: vec!["The user prefers TypeScript.".into()],
            source_notes: vec!["raw-ts".into()],
            ..Default::default()
        };
        backend
            .index_note(&note, agent, "preference")
            .await
            .unwrap();

        // L3: USER.md whose `## Sources` maps the "Identity" section to ses_x.
        let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
        sources.insert("Identity".into(), vec!["ses_x".into()]);
        let sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let md = render_user_md(1, "ses_x", "high", &sections, &sources);
        ProfileStore::new(dir.path().join(agent))
            .write(&md, None)
            .await
            .unwrap();

        // Drill from the L3 section down to L0 raw content.
        let tool = MemoryTraceTool::new(backend, raw_store, agent, dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "Identity".into(),
                kind: TraceKind::ProfileSection,
                max_results: None,
            })
            .await
            .unwrap();

        assert!(
            out.evidence.iter().any(|e| e
                .content
                .as_deref()
                .map(|c| c.contains("TypeScript"))
                .unwrap_or(false)),
            "ProfileSection 'Identity' must drill to raw content mentioning TypeScript; got {:?}",
            out.evidence
        );
    }

    /// "Why didn't you remember that?" must be answerable from data: the
    /// refusal reason comes back as a token, filtered to the fact asked about.
    #[tokio::test]
    async fn write_decision_kind_returns_the_refusal_reason() {
        use crate::memory::store::sqlite::memory_write_decisions::MemoryWriteReason;

        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        backend
            .record_write_decision("main", "add", MemoryWriteReason::OverBudget, "prefers tabs")
            .unwrap();
        backend
            .record_write_decision("main", "add", MemoryWriteReason::Written, "lives in Berlin")
            .unwrap();

        let tool = MemoryTraceTool::new(backend, raw_store, "main", dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "tabs".into(),
                kind: TraceKind::WriteDecision,
                max_results: None,
            })
            .await
            .unwrap();

        assert_eq!(out.write_decisions.len(), 1, "{:?}", out.write_decisions);
        assert_eq!(out.write_decisions[0].reason, "over_budget");
        assert_eq!(out.write_decisions[0].action, "add");
        // A write-decision trace is not an evidence walk.
        assert!(out.notes.is_empty());
        assert!(out.evidence.is_empty());
    }

    /// The new field must not change the shape the evidence-chain kinds emit.
    #[tokio::test]
    async fn evidence_kinds_carry_no_write_decisions_key() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let tool = MemoryTraceTool::new(backend, raw_store, "main", dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "preference/nothing".into(),
                kind: TraceKind::Note,
                max_results: None,
            })
            .await
            .unwrap();
        let json = serde_json::to_value(&out).unwrap();
        assert!(json.get("write_decisions").is_none(), "{json}");
    }

    /// Of the lines in my memory, which did I say and which did the model
    /// invent? The markers carrying that answer are written on every fact at
    /// ingest and stripped from every rendering except the raw-file read
    /// `note_manage(action='get')` — where they arrive as unexplained HTML
    /// comments inside prose, under a truncation cap. `facts[]` is the first
    /// form of them a caller can act on per line.
    #[tokio::test]
    async fn note_kind_reports_which_facts_were_quoted_and_which_were_inferred() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let note_dir = dir.path().join("main").join("preference");
        tokio::fs::create_dir_all(&note_dir).await.unwrap();
        // Built line-by-line on purpose: `extract_facts` treats an indented
        // `- ` as a continuation of the fact above it, so a stray two spaces
        // here would silently collapse three facts into one — which is exactly
        // what the first version of this fixture did.
        let md = [
            "---",
            "category: preference",
            "---",
            "",
            "- the user prefers TypeScript <!-- src: raw-7, origin: raw_source, inferred: false -->",
            "- the user probably dislikes Flow <!-- origin: inferred, inferred: true -->",
            "- a line typed by hand in Obsidian",
        ]
        .join("\n");
        tokio::fs::write(note_dir.join("typescript.md"), md)
            .await
            .unwrap();

        let tool = MemoryTraceTool::new(backend, raw_store, "main", dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "preference/typescript".into(),
                kind: TraceKind::Note,
                max_results: None,
            })
            .await
            .unwrap();

        assert_eq!(out.facts.len(), 3, "{:?}", out.facts);

        assert_eq!(out.facts[0].origin, "raw_source");
        assert!(!out.facts[0].inferred);
        assert_eq!(out.facts[0].source_id.as_deref(), Some("raw-7"));
        assert_eq!(
            out.facts[0].text, "the user prefers TypeScript",
            "the marker is machinery, not something a reader should be shown"
        );

        assert_eq!(out.facts[1].origin, "inferred");
        assert!(out.facts[1].inferred, "this line was never said by anyone");
        assert!(out.facts[1].source_id.is_none());

        assert_eq!(
            out.facts[2].origin, "legacy",
            "no marker at all — hand-edited, or written before markers existed"
        );
        assert_eq!(out.facts[2].fact_idx, 2);
    }

    /// Only the note kind carries them, and an unreadable note degrades to an
    /// absent key rather than taking the evidence chain down with it.
    #[tokio::test]
    async fn facts_are_absent_for_other_kinds_and_for_a_note_that_is_not_there() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let tool = MemoryTraceTool::new(backend, raw_store, "main", dir.path().to_path_buf());

        for (target, kind) in [
            ("preference/missing", TraceKind::Note),
            ("raw-1", TraceKind::Raw),
        ] {
            let out = tool
                .call_impl(MemoryTraceArgs {
                    target: target.into(),
                    kind,
                    max_results: None,
                })
                .await
                .unwrap();
            assert!(out.facts.is_empty());
            let json = serde_json::to_value(&out).unwrap();
            assert!(json.get("facts").is_none(), "{json}");
        }
    }

    /// `sanitize_note_path` collapses an all-unsafe path to the empty string;
    /// joining that would hand back the agent directory itself.
    #[tokio::test]
    async fn a_traversal_target_reads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());
        let raw_store = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let tool = MemoryTraceTool::new(backend, raw_store, "main", dir.path().to_path_buf());
        let out = tool
            .call_impl(MemoryTraceArgs {
                target: "../../..".into(),
                kind: TraceKind::Note,
                max_results: None,
            })
            .await
            .unwrap();
        assert!(out.facts.is_empty());
    }

    #[test]
    fn tool_name_and_description() {
        assert_eq!(MemoryTraceTool::NAME, "memory_trace");
        assert!(!MemoryTraceTool::DESCRIPTION.is_empty());
    }

    #[test]
    fn args_deserialize() {
        let json = r#"{"target": "preference/typescript", "kind": "note"}"#;
        let args: MemoryTraceArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.target, "preference/typescript");
        assert_eq!(args.kind, TraceKind::Note);
        assert!(args.max_results.is_none());
        // The token the model actually sends for the write-decision log.
        let wd: MemoryTraceArgs =
            serde_json::from_str(r#"{"target": "", "kind": "write_decision"}"#).unwrap();
        assert_eq!(wd.kind, TraceKind::WriteDecision);
    }
}
