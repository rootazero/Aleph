//! `memory_trace` — drill a high-level memory claim down to ground-truth
//! evidence: profile section → notes → raw memories → transcript text.
//!
//! Walking direction: profile section / note path / raw id
//!   → source notes → raw memory rows → original transcript content.
//!
//! Missing raws degrade to a `pruned: true` node, never an error.
//!
//! The `write_decision` kind answers the mirror question — "why is this NOT in
//! memory?" — off the curated-write audit log instead of the evidence chain. It
//! lives here rather than in a tool of its own because both are provenance
//! questions about one memory claim, and this tool already holds the two handles
//! the answer needs (the memory backend and the calling agent's id).

use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::sqlite::memory_write_decisions::{
    MemoryWriteDecisionRow, MAX_DECISIONS_PER_AGENT,
};
use crate::memory::store::MemoryBackend;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Write-decision rows returned when the caller does not ask for a count.
/// Small on purpose: these rows are read back into a model's context, and the
/// question they answer ("what happened to this fact?") is about the last few
/// attempts, not the whole retained window.
const DEFAULT_WRITE_DECISION_ROWS: usize = 20;

/// Which kind of target to trace.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// Trace a note by its path (`category/name`).
    Note,
    /// Trace a raw memory by its id.
    Raw,
    /// Trace a USER.md section heading (e.g. `## Sources` key) to all citing notes.
    ProfileSection,
    /// Look up what the curated hot memory (`remember`) did with a fact: one
    /// row per write ATTEMPT, refusals included.
    WriteDecision,
}

/// Arguments for the `memory_trace` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTraceArgs {
    /// What to trace: a note path (`category/name`), a raw memory id, a USER.md
    /// section heading, or — for `write_decision` — a literal substring of the
    /// fact whose fate you are asking about (empty browses recent decisions).
    pub target: String,
    /// The kind of target being traced.
    pub kind: TraceKind,
    /// Maximum number of items to return. Caps the `evidence` list (default:
    /// unlimited) or the `write_decisions` list (default: 20); it is not a
    /// graph-traversal depth.
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// A single piece of ground-truth evidence in the chain.
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceItem {
    /// The raw memory id.
    pub raw_id: String,
    /// The note path that cites this raw (if reached via a note).
    pub via_note: Option<String>,
    /// The session id the raw was recorded in (if present).
    pub via_session: Option<String>,
    /// First 800 chars of raw content; `None` when `pruned`.
    pub content: Option<String>,
    /// `true` when the raw id was referenced but the row is missing from the store.
    pub pruned: bool,
}

/// One fact of a note beside where that specific line came from.
///
/// The evidence chain above answers "which raw memories fed this note". This
/// answers the finer question the chain cannot: *which line*, and whether the
/// model quoted it or inferred it.
///
/// Every fact carries an inline `<!-- origin: ..., inferred: ... -->` marker
/// written at ingest. This is the first *structured* read of them. The only
/// other path that reaches a caller at all is `note_manage(action='get')`,
/// which hands back the whole file — capped at its own char ceiling, markers
/// buried in prose as HTML comments, and nothing anywhere saying what they
/// mean; every other rendering (`body_text_for_fts`, the panel drawer, note
/// listings) strips them.
#[derive(Debug, Clone, Serialize)]
pub struct FactOrigin {
    /// The note this fact belongs to.
    pub note_path: String,
    /// Position of the fact within that note, 0-based.
    pub fact_idx: usize,
    /// The fact as a reader sees it, provenance marker removed.
    pub text: String,
    /// `raw_source` / `prior_note` / `inferred` / `system` / `legacy`.
    /// `legacy` means the line carries no marker at all — written before the
    /// markers existed, or edited in by hand in Obsidian.
    pub origin: &'static str,
    /// The model's own claim about whether it invented this line.
    pub inferred: bool,
    /// Raw-memory id or note path this fact was quoted from, when the marker
    /// named one.
    pub source_id: Option<String>,
}

/// Result of walking the evidence chain.
#[derive(Debug, Clone, Serialize)]
pub struct TraceResult {
    /// The original target string.
    pub target: String,
    /// Note paths visited during the walk.
    pub notes: Vec<String>,
    /// Evidence items collected.
    pub evidence: Vec<EvidenceItem>,
    /// Curated-memory write decisions matching `target`, newest first — only
    /// populated for `kind: "write_decision"`. Absent from the serialized shape
    /// when empty, so the evidence-chain kinds keep the output they had.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write_decisions: Vec<MemoryWriteDecisionRow>,
    /// Per-fact origins — only populated for `kind: "note"`, and empty when the
    /// note cannot be read or parsed. Absent from the serialized shape when
    /// empty, so the other kinds keep the output they had.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<FactOrigin>,
}

/// Tool that walks a memory claim down to ground-truth evidence.
pub struct MemoryTraceTool {
    db: MemoryBackend,
    agent_id: String,
    note_memory_dir: PathBuf,
}

impl MemoryTraceTool {
    /// Tool identifier registered with the agent runtime.
    pub const NAME: &'static str = "memory_trace";

    /// Tool description shown to the LLM in its system prompt.
    pub const DESCRIPTION: &'static str =
        "Drill a memory claim down to ground-truth evidence: profile section / note / raw id \
         → source notes → raw memories → original transcript text. Returns the evidence chain; \
         missing raws are marked as pruned rather than causing an error.\n\n\
         `kind: \"note\"` also returns `facts[]`: each fact of that note with its `origin` \
         (raw_source / prior_note / inferred / system / legacy), an `inferred` flag and the \
         `source_id` it was quoted from. Answer \"did I actually say that?\" from these rows; \
         `inferred: true` means no one ever said it.\n\n\
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
    pub fn new(db: MemoryBackend, agent_id: impl Into<String>, note_memory_dir: PathBuf) -> Self {
        Self {
            db,
            agent_id: agent_id.into(),
            note_memory_dir,
        }
    }

    /// Read one note's facts and the origin marker on each.
    ///
    /// Reads the markdown, not `notes_provenance`. The table is a projection of
    /// these same markers, rebuilt on every index pass, and it does not store
    /// the fact text — so a caller that wants both would pair text from the
    /// file with provenance from the index and mis-attribute any fact whose
    /// note changed since. From the file the two always describe the same
    /// bytes. (The table earns its place on the other axis: `notes_citing`
    /// reads it to find fact-level citations of a source across notes, which
    /// no single file can answer.)
    ///
    /// Every failure returns an empty list rather than an error: this is one
    /// section of a trace answer, and a note that was pruned, renamed, or
    /// hand-edited into unparseable shape should not take the evidence chain
    /// down with it.
    async fn fact_origins(&self, agent: &str, note_path: &str) -> Vec<FactOrigin> {
        let safe = crate::memory::notes::sanitize_note_path(note_path);
        // `sanitize_note_path` returns "" when every segment is unsafe. Joining
        // that would open the agent directory itself.
        if safe.is_empty() {
            return Vec::new();
        }
        let file = self
            .note_memory_dir
            .join(agent)
            .join(format!("{safe}.md"));
        let Ok(content) = tokio::fs::read_to_string(&file).await else {
            return Vec::new();
        };
        let stem = safe.rsplit('/').next().unwrap_or(&safe);
        let Ok(note) = crate::memory::notes::KnowledgeNote::from_markdown(stem, &content) else {
            return Vec::new();
        };
        note.facts_with_origin()
            .into_iter()
            .enumerate()
            .map(|(fact_idx, (text, prov))| FactOrigin {
                note_path: note_path.to_string(),
                fact_idx,
                text,
                origin: prov.origin.as_str(),
                inferred: prov.inferred,
                source_id: prov.source_id,
            })
            .collect()
    }

    /// Execute the evidence-chain walk.
    pub async fn call_impl(&self, args: MemoryTraceArgs) -> anyhow::Result<TraceResult> {
        // BT-D-R4-09: cap the evidence walk. The previous shape had no
        // upper bound — notes_citing for ProfileSection could return
        // thousands of paths, each triggering another DB round-trip
        // (sources_of, get_raws_by_ids), each accumulating evidence
        // items that grew the response without bound. 500 evidence
        // items is far above any sensible "trace a claim" request
        // and caps the wall-time at ~500 small DB reads. Earlier
        // entries are preserved; later ones are dropped.
        const MAX_TRACE_EVIDENCE: usize = 500;
        let mut evidence = Vec::with_capacity(MAX_TRACE_EVIDENCE.min(64));
        let agent = &self.agent_id;

        // 1. Resolve the set of note paths to inspect.
        let notes: Vec<String> = match args.kind {
            TraceKind::Note => vec![args.target.clone()],
            TraceKind::Raw => self
                .db
                .notes_citing(agent, &args.target)
                .await
                .map_err(|e| anyhow::anyhow!("notes_citing: {e}"))?,
            TraceKind::ProfileSection => {
                // section heading → session ids (from USER.md Sources map)
                // → raw memory rows → citing notes
                let agent_dir = self.note_memory_dir.join(agent.as_str());
                let store = crate::memory::notes::profile::ProfileStore::new(agent_dir);
                let profile = store.read().await.ok().flatten();
                let mut notes = Vec::new();
                if let Some(p) = profile {
                    if let Some(sessions) = p.sources.get(&args.target) {
                        for sid in sessions {
                            let raws = self
                                .db
                                .get_raws_by_session(agent, sid)
                                .await
                                .map_err(|e| anyhow::anyhow!("get_raws_by_session: {e}"))?;
                            for raw in raws {
                                let citing = self
                                    .db
                                    .notes_citing(agent, &raw.id)
                                    .await
                                    .map_err(|e| anyhow::anyhow!("notes_citing: {e}"))?;
                                for n in citing {
                                    if !notes.contains(&n) {
                                        notes.push(n);
                                    }
                                }
                            }
                        }
                    }
                }
                notes
            }
            // A write-decision trace visits no notes: the audit log is read
            // directly below, so the evidence walk is a no-op for this kind.
            TraceKind::WriteDecision => Vec::new(),
        };

        // 2. Each note → its source raw ids → fetch rows (graceful prune for missing).
        for note in &notes {
            if evidence.len() >= MAX_TRACE_EVIDENCE {
                break;
            }
            let raw_ids = self
                .db
                .sources_of(agent, note)
                .await
                .map_err(|e| anyhow::anyhow!("sources_of: {e}"))?;
            let fetched = self
                .db
                .get_raws_by_ids(agent, &raw_ids)
                .await
                .map_err(|e| anyhow::anyhow!("get_raws_by_ids: {e}"))?;
            for rid in &raw_ids {
                if evidence.len() >= MAX_TRACE_EVIDENCE {
                    break;
                }
                let found = fetched.iter().find(|r| &r.id == rid);
                evidence.push(EvidenceItem {
                    raw_id: rid.clone(),
                    via_note: Some(note.clone()),
                    via_session: found.and_then(|r| r.session_id.clone()),
                    content: found.map(|r| r.content.chars().take(800).collect()),
                    pruned: found.is_none(),
                });
            }
        }

        // 3. For Raw kind: also surface the raw itself if present.
        if args.kind == TraceKind::Raw {
            let fetched = self
                .db
                .get_raws_by_ids(agent, std::slice::from_ref(&args.target))
                .await
                .map_err(|e| anyhow::anyhow!("get_raws_by_ids(raw): {e}"))?;
            if let Some(r) = fetched.first() {
                evidence.push(EvidenceItem {
                    raw_id: r.id.clone(),
                    via_note: None,
                    via_session: r.session_id.clone(),
                    content: Some(r.content.chars().take(800).collect()),
                    pruned: false,
                });
            }
        }

        // 4. Cap the number of returned evidence items if requested.
        if let Some(max) = args.max_results {
            evidence.truncate(max);
        }

        // 5. The curated-write audit log: one row per `remember` attempt,
        //    refusals included. Clamped to the table's own per-agent ceiling so
        //    a large `max_results` cannot ask for more than is retained.
        let write_decisions = if args.kind == TraceKind::WriteDecision {
            let limit = args
                .max_results
                .unwrap_or(DEFAULT_WRITE_DECISION_ROWS)
                .min(MAX_DECISIONS_PER_AGENT);
            self.db
                .recent_write_decisions(agent, Some(args.target.as_str()), limit)
                .map_err(|e| anyhow::anyhow!("recent_write_decisions: {e}"))?
        } else {
            Vec::new()
        };

        // 6. Per-fact origins for a note trace. The evidence chain says which
        //    raws fed the note; this says which line came from which, and
        //    whether the model quoted or inferred it.
        let facts = if args.kind == TraceKind::Note {
            self.fact_origins(agent, &args.target).await
        } else {
            Vec::new()
        };

        Ok(TraceResult {
            target: args.target,
            notes,
            evidence,
            facts,
            write_decisions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::{
        MemoryBackend, RawMemory, RawMemorySource, RawMemoryStore, SqliteMemoryBackend,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn trace_note_to_raw_and_graceful_prune() {
        let dir = tempfile::tempdir().unwrap();
        let backend: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap());

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
        backend.insert_raw_memory(&r).await.unwrap();

        // note_memory_dir unused for TraceKind::Note; any path is fine.
        let tool = MemoryTraceTool::new(backend, "default", dir.path().to_path_buf());
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
        let agent = "main";

        // L0: a raw memory carrying the ground-truth utterance, tagged with a session.
        let mut raw = RawMemory::new(
            "user: I prefer TypeScript for all new projects".into(),
            RawMemorySource::Transcript,
        );
        raw.id = "raw-ts".into();
        raw.agent_id = agent.into();
        raw.session_id = Some("ses_x".into());
        backend.insert_raw_memory(&raw).await.unwrap();

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
        let tool = MemoryTraceTool::new(backend, agent, dir.path().to_path_buf());
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
        backend
            .record_write_decision("main", "add", MemoryWriteReason::OverBudget, "prefers tabs")
            .unwrap();
        backend
            .record_write_decision("main", "add", MemoryWriteReason::Written, "lives in Berlin")
            .unwrap();

        let tool = MemoryTraceTool::new(backend, "main", dir.path().to_path_buf());
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
        let tool = MemoryTraceTool::new(backend, "main", dir.path().to_path_buf());
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

        let tool = MemoryTraceTool::new(backend, "main", dir.path().to_path_buf());
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
        let tool = MemoryTraceTool::new(backend, "main", dir.path().to_path_buf());

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
        let tool = MemoryTraceTool::new(backend, "main", dir.path().to_path_buf());
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
