//! `memory_trace` — drill a high-level memory claim down to ground-truth
//! evidence: profile section → notes → raw memories → transcript text.
//!
//! Walking direction: profile section / note path / raw id
//!   → source notes → raw memory rows → original transcript content.
//!
//! Missing raws degrade to a `pruned: true` node, never an error.

use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}

/// Arguments for the `memory_trace` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTraceArgs {
    /// What to trace: a note path (`category/name`), a raw memory id, or a USER.md section heading.
    pub target: String,
    /// The kind of target being traced.
    pub kind: TraceKind,
    /// Maximum number of evidence items to return (default: unlimited). This
    /// caps the returned `evidence` list; it is not a graph-traversal depth.
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

/// Result of walking the evidence chain.
#[derive(Debug, Clone, Serialize)]
pub struct TraceResult {
    /// The original target string.
    pub target: String,
    /// Note paths visited during the walk.
    pub notes: Vec<String>,
    /// Evidence items collected.
    pub evidence: Vec<EvidenceItem>,
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
         missing raws are marked as pruned rather than causing an error.";

    /// Create a new `MemoryTraceTool`.
    ///
    /// `note_memory_dir` is the base notes directory (the parent of per-agent
    /// subdirectories), matching the path injected into `ProfileSynthesizer` and
    /// the note-wiki tooling.
    pub fn new(
        db: MemoryBackend,
        agent_id: impl Into<String>,
        note_memory_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            agent_id: agent_id.into(),
            note_memory_dir,
        }
    }

    /// Execute the evidence-chain walk.
    pub async fn call_impl(&self, args: MemoryTraceArgs) -> anyhow::Result<TraceResult> {
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
                let store =
                    crate::memory::notes::profile::ProfileStore::new(agent_dir);
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
        };

        // 2. Each note → its source raw ids → fetch rows (graceful prune for missing).
        let mut evidence = Vec::new();
        for note in &notes {
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
                .get_raws_by_ids(agent, &[args.target.clone()])
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

        Ok(TraceResult {
            target: args.target,
            notes,
            evidence,
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
        let mut r =
            RawMemory::new("user: I prefer TypeScript".into(), RawMemorySource::Transcript);
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
        backend.index_note(&note, agent, "preference").await.unwrap();

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
    }
}
