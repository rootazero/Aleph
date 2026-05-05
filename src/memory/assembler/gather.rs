//! Stage 1: concurrent candidate gather. Fans out to all four sources and
//! assembles a single pool of [`Candidate`]s with a [`SlotKind`] hint on each.

use super::envelope::{ItemSource, SlotKind};
use super::fallback::Candidate;
use super::profile::UserProfileLoader;
use crate::memory::context::FactSource;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::session_search_summary::FactSourceFilter;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::memory::store::MemoryBackend;
use crate::memory::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use tracing::warn;

pub(crate) struct GatherInputs {
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub pool_limit: usize,
    pub filter: FactSourceFilter,
}

pub(crate) struct Gatherer {
    pub retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
    pub snapshots: Arc<SnapshotReader>,
    pub backend: MemoryBackend,
    pub profile: Arc<UserProfileLoader>,
}

impl Gatherer {
    pub async fn gather(&self, input: &GatherInputs) -> Vec<Candidate> {
        let (notes, snapshot, raws, profile) = tokio::join!(
            self.fetch_notes(&input.query, &input.agent_id, input.pool_limit),
            self.fetch_snapshot(input.session_id.as_deref()),
            self.fetch_raws(&input.agent_id, input.session_id.as_deref(), &input.filter),
            self.profile.load(&input.agent_id),
        );

        let mut pool = Vec::with_capacity(notes.len() + raws.len() + 2);
        pool.extend(notes);
        pool.extend(snapshot);
        pool.extend(raws);
        if let Some(body) = profile {
            pool.push(Candidate {
                id: "note://personal/profile".into(),
                title: "User profile".into(),
                full_content: body,
                source: ItemSource::Note {
                    path: "personal/profile".into(),
                    category: "personal".into(),
                },
                relevance: 1.0,
                updated_at: chrono::Utc::now().timestamp(),
                slot_hint: SlotKind::UserProfile,
                fact_source: FactSource::Extracted,
            });
        }

        // Post-gather filter: drop candidates that don't match the requested
        // FactSourceFilter. FactSourceFilter::Any is a no-op (passes everything).
        pool.retain(|c| input.filter.matches(c.fact_source));
        pool
    }

    async fn fetch_notes(&self, query: &str, agent_id: &str, limit: usize) -> Vec<Candidate> {
        match self.retrieval.retrieve(query, agent_id, limit).await {
            Ok(results) => results
                .into_iter()
                .map(|sf| {
                    let category = sf.fact.note_type.to_category_dir().to_string();
                    // MemoryFact.path is the note://category/filename form when
                    // produced by NoteSearchResult::to_memory_fact. Normalise.
                    let display_id = if sf.fact.path.starts_with("note://") {
                        sf.fact.path.clone()
                    } else {
                        format!("note://{}", sf.fact.path)
                    };
                    let path_no_scheme = display_id.trim_start_matches("note://").to_string();
                    let title = path_no_scheme
                        .rsplit('/')
                        .next()
                        .unwrap_or(&path_no_scheme)
                        .to_string();
                    Candidate {
                        id: display_id,
                        title,
                        full_content: sf.fact.content.clone(),
                        source: ItemSource::Note {
                            path: path_no_scheme,
                            category,
                        },
                        relevance: sf.score,
                        updated_at: sf.fact.updated_at,
                        slot_hint: SlotKind::RelevantNotes,
                        fact_source: FactSource::Extracted,
                    }
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, "assembler.gather: notes retrieval failed");
                Vec::new()
            }
        }
    }

    async fn fetch_snapshot(&self, session_id: Option<&str>) -> Vec<Candidate> {
        // `SnapshotReader::load_latest` returns the most recent snapshot
        // EXCLUDING the named session. For our purposes — giving the LLM
        // context from the previous session — we pass the current session id
        // as the exclude so the reader hands us the prior session's snapshot.
        let exclude = session_id.unwrap_or("");
        let Some(snap) = self.snapshots.load_latest(exclude) else {
            return Vec::new();
        };
        let body = format!(
            "Summary: {}\nKey decisions: {}\nActive files: {}\nPending: {}",
            snap.summary,
            snap.key_decisions.join("; "),
            snap.active_files.join(", "),
            snap.pending_tasks.join("; "),
        );
        let sid = snap.session_id.clone();
        vec![Candidate {
            id: format!("aleph://session/{sid}/snapshot"),
            title: format!("Session {sid} snapshot"),
            full_content: body,
            source: ItemSource::Summary {
                layer: "d1".into(),
                session_id: sid,
            },
            relevance: 0.9,
            updated_at: snap.created_at.timestamp(),
            slot_hint: SlotKind::SessionRecent,
            fact_source: FactSource::Summary,
        }]
    }

    async fn fetch_raws(
        &self,
        agent_id: &str,
        session_id: Option<&str>,
        filter: &FactSourceFilter,
    ) -> Vec<Candidate> {
        // When filtering to SessionCompressed only and there is no active
        // session, fetch all SessionCompressed rows for this agent (cross-
        // session retrieval path).
        if matches!(
            filter,
            FactSourceFilter::Only(FactSource::SessionCompressed)
        ) && session_id.is_none()
        {
            return self.fetch_session_compressed(agent_id).await;
        }

        let Some(sid) = session_id else {
            return Vec::new();
        };
        let prefix = format!("aleph://session/{sid}/raw/");
        match self
            .backend
            .get_raw_by_path_prefix(&prefix, agent_id, 5)
            .await
        {
            Ok(raws) => raws.into_iter().map(raw_to_candidate).collect(),
            Err(e) => {
                warn!(error = %e, session = sid, "assembler.gather: raw fetch failed");
                Vec::new()
            }
        }
    }

    /// Fetch all `SessionCompressed` raw memories for an agent, regardless of
    /// session. Used by the cross-session `session_search` path.
    async fn fetch_session_compressed(&self, agent_id: &str) -> Vec<Candidate> {
        match self
            .backend
            .get_raw_by_source(RawMemorySource::SessionCompressed, agent_id, 20)
            .await
        {
            Ok(raws) => raws.into_iter().map(raw_to_candidate).collect(),
            Err(e) => {
                warn!(error = %e, agent = agent_id, "assembler.gather: session_compressed fetch failed");
                Vec::new()
            }
        }
    }
}

fn raw_to_candidate(r: RawMemory) -> Candidate {
    let session_id = r.session_id.clone().unwrap_or_default();
    let fact_source = raw_source_to_fact_source(&r.source);
    let path = r.path.clone();
    Candidate {
        id: format!("aleph://session/{session_id}/raw/{}", r.id),
        title: format!("Raw fragment {}", r.id),
        full_content: r.content,
        source: ItemSource::Raw {
            raw_id: r.id,
            session_id,
            path,
        },
        relevance: 0.6,
        updated_at: r.created_at,
        slot_hint: SlotKind::RawFragments,
        fact_source,
    }
}

/// Map storage-layer [`RawMemorySource`] to semantic-layer [`FactSource`].
///
/// Only `SessionCompressed` has a meaningful 1:1 mapping. All other variants
/// are transcript/tool-output-like content and map to `Extracted`.
fn raw_source_to_fact_source(src: &RawMemorySource) -> FactSource {
    match src {
        RawMemorySource::SessionCompressed => FactSource::SessionCompressed,
        _ => FactSource::Extracted,
    }
}

#[cfg(test)]
mod tests {
    // Live fan-out coverage comes from the integration tests in
    // tests/integration.rs (Task 12). These inline tests keep the raw→Candidate
    // conversion tight and dependency-free.

    use super::*;
    use crate::memory::store::raw_memory::RawMemorySource;

    #[test]
    fn raw_to_candidate_populates_source() {
        let raw =
            RawMemory::new("content".into(), RawMemorySource::Transcript).with_session("sess-1");
        let c = raw_to_candidate(raw);
        match &c.source {
            ItemSource::Raw { session_id, .. } => assert_eq!(session_id, "sess-1"),
            _ => panic!("expected ItemSource::Raw"),
        }
        assert_eq!(c.slot_hint, SlotKind::RawFragments);
        assert_eq!(c.fact_source, FactSource::Extracted);
    }

    #[test]
    fn session_compressed_maps_to_correct_fact_source() {
        let raw = RawMemory::new(
            "compressed summary".into(),
            RawMemorySource::SessionCompressed,
        )
        .with_session("sess-2");
        let c = raw_to_candidate(raw);
        assert_eq!(c.fact_source, FactSource::SessionCompressed);
    }

    #[test]
    fn raw_source_to_fact_source_mapping() {
        assert_eq!(
            raw_source_to_fact_source(&RawMemorySource::SessionCompressed),
            FactSource::SessionCompressed
        );
        assert_eq!(
            raw_source_to_fact_source(&RawMemorySource::Transcript),
            FactSource::Extracted
        );
        assert_eq!(
            raw_source_to_fact_source(&RawMemorySource::ToolOutput),
            FactSource::Extracted
        );
    }
}
