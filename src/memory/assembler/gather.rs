//! Stage 1: concurrent candidate gather. Fans out to all sources (notes,
//! prior-session snapshot, raw fragments, user profile, feedback floor,
//! daily insight) and assembles a single pool of [`Candidate`]s with a
//! [`SlotKind`] hint on each.

use super::envelope::{ItemSource, SlotKind};
use super::fallback::Candidate;
use super::feedback_floor::{FeedbackFloorEntry, FeedbackFloorLoader};
use super::profile::UserProfileLoader;
use crate::memory::context::FactSource;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::session_resume::SessionSnapshot;
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
    pub feedback_floor: Arc<FeedbackFloorLoader>,
    /// Mirror of `MemoryConfig.project_scoped`. When true and a project root is
    /// active for the run, note retrieval unions the project's namespace with
    /// the agent's global namespace so project-authored notes surface alongside
    /// cross-project knowledge. Independently, a personal-scoped session
    /// (`crate::scope::current_scope`) always unions in the user's own
    /// namespace regardless of this flag — see `project_scope::session_read_ids`.
    /// The two floors split (P1/P2 "Floors 分床"): the user-profile floor
    /// follows the session's personal scope and vanishes entirely in a shared
    /// project room (`profile_floor_id`), the feedback floor stays under the
    /// base id unconditionally (org-wide standing rules).
    pub project_scoped: bool,
}

impl Gatherer {
    pub async fn gather(&self, input: &GatherInputs) -> Vec<Candidate> {
        // The user-profile floor follows personal scope and is ABSENT in a
        // shared project room; the feedback floor stays under the base id
        // (org-wide) regardless — see the field doc on `project_scoped` and
        // project_scope.rs's "Floors 分床" invariant. `None` here means "no
        // profile is admissible", not "resolve it somewhere else": the whole
        // reason `profile_floor_id` returns an `Option` is that a room has no
        // "the user" to have a profile.
        let user_floor_id = crate::memory::project_scope::profile_floor_id(
            &input.agent_id,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        // Every partition this session may read — the SAME derivation
        // `fetch_notes` uses below. Handing the feedback floor the bare persona
        // made it scan a directory no writer ever targets:
        // `flag_user_correction` writes through `caller_memory_partition`, and
        // even a zero-config loopback Panel session is `Personal(u-owner)`, so
        // the factory-default corrections land in `main__u-owner/feedback/`.
        // Base stays in this set, so org-wide standing rules still reach
        // everyone.
        let feedback_floor_ids = crate::memory::project_scope::session_read_ids(
            &input.agent_id,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        let (notes, snapshot, raws, profile, feedback_floor, daily_insight) = tokio::join!(
            self.fetch_notes(&input.query, &input.agent_id, input.pool_limit),
            self.fetch_snapshot(&input.agent_id, input.session_id.as_deref()),
            self.fetch_raws(&input.agent_id, input.session_id.as_deref(), &input.filter),
            async {
                match user_floor_id.as_deref() {
                    Some(id) => self.profile.load(id).await,
                    None => None,
                }
            },
            self.feedback_floor.load_many(&feedback_floor_ids),
            self.fetch_daily_insight(),
        );

        let mut pool = Vec::with_capacity(notes.len() + raws.len() + feedback_floor.len() + 3);
        // Track feedback notes the query already surfaced so the always-on
        // floor only ADDS the High/Critical rules retrieval missed — query
        // matches keep their real relevance score.
        let mut seen_feedback: std::collections::HashSet<String> = notes
            .iter()
            .filter(|c| c.slot_hint == SlotKind::Feedback)
            // rust-doctor-disable-next-line excessive-clone
            .map(|c| c.id.clone())
            .collect();
        pool.extend(notes);
        pool.extend(snapshot);
        pool.extend(raws);
        pool.extend(daily_insight);
        for entry in feedback_floor {
            let id = format!("note://{}", entry.path);
            // rust-doctor-disable-next-line excessive-clone
            if !seen_feedback.insert(id.clone()) {
                continue;
            }
            pool.push(feedback_entry_to_candidate(id, entry));
        }
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

        // Bound the merged pool. Six legs (notes, snapshot, raws, profile,
        // feedback floor, daily insight) each contribute up to their own
        // ceiling (notes is bounded by `pool_limit`; the others return
        // whatever they return — typically small but unbounded in the
        // raws leg). Without this cap the post-merge pool can reach
        // `N × pool_limit` and the downstream LLM rerank pays for every
        // extra candidate. Keep the highest-relevance candidates so the
        // budget buys ranking quality, not raw breadth.
        if pool.len() > input.pool_limit {
            pool.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            pool.truncate(input.pool_limit);
        }
        pool
    }

    async fn fetch_notes(&self, query: &str, agent_id: &str, limit: usize) -> Vec<Candidate> {
        // Session-scope-aware union: a personal-scoped session unions in the
        // user's own namespace regardless of `project_scoped`; otherwise a
        // project-scoped session unions the active project's namespace with
        // the agent's global namespace. With neither active, `session_read_ids`
        // collapses to `[agent_id]` and we take the single-agent fast path —
        // byte-identical to the pre-feature behaviour.
        let ids = crate::memory::project_scope::session_read_ids(
            agent_id,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        let fetched = if ids.len() <= 1 {
            self.retrieval.retrieve(query, agent_id, limit).await
        } else {
            self.retrieval
                .retrieve_multi_agent(query, &ids, limit)
                .await
        };
        match fetched {
            Ok(results) => results
                .into_iter()
                .map(|sf| {
                    let category = sf.fact.note_type.to_category_dir().to_string();
                    // MemoryFact.path is the note://category/filename form when
                    // produced by NoteSearchResult::to_memory_fact. Normalise.
                    // rust-doctor-disable-next-line excessive-clone
                    let display_id = if sf.fact.path.starts_with("note://") {
                        // rust-doctor-disable-next-line excessive-clone
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
                    let slot_hint = slot_hint_for_path(&path_no_scheme);
                    Candidate {
                        id: display_id,
                        title,
                        // rust-doctor-disable-next-line excessive-clone
                        full_content: sf.fact.content.clone(),
                        source: ItemSource::Note {
                            path: path_no_scheme,
                            category,
                        },
                        relevance: sf.score,
                        updated_at: sf.fact.updated_at,
                        slot_hint,
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

    async fn fetch_snapshot(&self, agent_id: &str, session_id: Option<&str>) -> Vec<Candidate> {
        // The snapshot is partitioned like every other source this function's
        // siblings gather: notes go through `session_read_ids`, the profile
        // floor through `profile_floor_id`, and the snapshot through
        // `session_resume::snapshot_partition` — the SAME derivation the
        // writer stamped with. `agent_id` alone is the base id, shared by every
        // user of that agent, so it never separated alice's `/end-summary` from
        // bob's.
        //
        // Unlike notes, this is a single partition and NOT a union with the
        // base: `session_read_ids`' second member is the org tier, which is
        // shared by design for extracted facts but would re-open exactly the
        // leak above for a verbatim session transcript summary.
        let partition = crate::memory::session_resume::snapshot_partition(agent_id);
        // A room partition is readable only by the room's roster. This is the
        // ambient-resolver twin of the gateway's `partition_visible`: memory
        // assembly runs on the far side of the run's `tokio::spawn`, where
        // `CALLER_USER` is dead, so the gateway-side predicate would be
        // constantly true here — i.e. no gate at all.
        if !crate::gateway::visibility::ambient_partition_visible(&partition) {
            return Vec::new();
        }
        // `load_latest_in_partition` returns the requesting agent's most recent
        // snapshot in that partition EXCLUDING the named session. For our
        // purposes — giving the LLM context from the previous session — we pass
        // the current session id as the exclude so the reader hands us the
        // prior session's snapshot.
        let exclude = session_id.unwrap_or("");
        let Some(snap) = self
            .snapshots
            .load_latest_in_partition(agent_id, &partition, exclude)
        else {
            return Vec::new();
        };
        vec![snapshot_to_candidate(snap)]
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
            // Current-session raws are the live working set → `SessionRecent`
            // slot so the render-time cognitive view classifies them `Working`.
            Ok(raws) => raws
                .into_iter()
                .map(|r| raw_to_candidate(r, SlotKind::SessionRecent))
                .collect(),
            Err(e) => {
                warn!(error = %e, session = sid, "assembler.gather: raw fetch failed");
                Vec::new()
            }
        }
    }

    /// Fetch the most recent daily digest written by the dream daemon's
    /// `DailyDigestStage` (today's, falling back to yesterday's). Before this
    /// arm existed the digest was write-only: `upsert_daily_insight` ran
    /// nightly but `get_daily_insight` had no production caller, so the
    /// LLM-generated summary never reached a prompt.
    async fn fetch_daily_insight(&self) -> Vec<Candidate> {
        use crate::memory::store::DreamStore;

        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let yesterday = (now - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();

        for date in [today, yesterday] {
            match self.backend.get_daily_insight(&date).await {
                Ok(Some(insight)) => return vec![insight_to_candidate(insight)],
                Ok(None) => {}
                Err(e) => {
                    // Continue to the next date (today → yesterday) so a
                    // transient SQLite hiccup on today's date does not
                    // silently drop yesterday's digest too. Only fall
                    // through to `Vec::new()` after both dates are tried.
                    warn!(error = %e, date = %date, "assembler.gather: daily insight fetch failed; falling back to previous date");
                }
            }
        }
        Vec::new()
    }

    /// Fetch all `SessionCompressed` raw memories for an agent, regardless of
    /// session. Used by the cross-session `session_search` path.
    ///
    /// Mirrors [`Self::fetch_notes`]: `session_read_ids` unions in the
    /// session's personal namespace when personal scope is active, or the
    /// active project's namespace when project scoping is on inside a
    /// project. With neither active this collapses to the single base id —
    /// one query, unchanged.
    async fn fetch_session_compressed(&self, agent_id: &str) -> Vec<Candidate> {
        let ids = crate::memory::project_scope::session_read_ids(
            agent_id,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        let mut out = Vec::new();
        for id in &ids {
            match self
                .backend
                .get_raw_by_source(RawMemorySource::SessionCompressed, id, 20)
                .await
            {
                // Cross-session compressed rows are audit substrate → `Raw` tier.
                Ok(raws) => out.extend(
                    raws.into_iter()
                        .map(|r| raw_to_candidate(r, SlotKind::RawFragments)),
                ),
                Err(e) => {
                    warn!(error = %e, agent = %id, "assembler.gather: session_compressed fetch failed");
                }
            }
        }
        out
    }
}

/// Category-prefix of `feedback/` notes (written by `FeedbackDistill`).
const FEEDBACK_CATEGORY_PREFIX: &str = "feedback/";

/// Category-prefix of `goal-lessons/` notes (written by `GoalLessonsPromoteStage`).
/// Distilled goal lessons are behavioural directives, so they share the
/// `Feedback` slot's standing-directive treatment rather than the generic
/// `RelevantNotes` slot.
const GOAL_LESSONS_CATEGORY_PREFIX: &str = "goal-lessons/";

/// Decide which slot a retrieved note belongs to from its scheme-less path
/// (`"{category}/{filename}"`).
///
/// Routing is keyed on the on-disk path prefix. `NoteType::Feedback` now exists
/// so `to_category_dir()` would also yield `"feedback"`, but the physical path
/// is the durable signal: it stays correct for legacy notes written before the
/// variant existed (whose persisted type mangled to `Other`).
///
/// `feedback/` and `goal-lessons/` both route to [`SlotKind::Feedback`]: both are
/// distilled behavioural rules that, once a query matches them, must survive the
/// LLM re-rank (see `hybrid.rs` Feedback pre-population) and render under the
/// standing-directive label. Routing here does NOT make them always-on — the
/// always-on floor is a separate physical scan of `feedback/*.md`
/// (`feedback_floor.rs`); these still surface only when retrieval matches.
fn slot_hint_for_path(path_no_scheme: &str) -> SlotKind {
    if path_no_scheme.starts_with(FEEDBACK_CATEGORY_PREFIX)
        || path_no_scheme.starts_with(GOAL_LESSONS_CATEGORY_PREFIX)
    {
        SlotKind::Feedback
    } else {
        SlotKind::RelevantNotes
    }
}

/// Convert an always-on [`FeedbackFloorEntry`] into a `Feedback`-slotted
/// candidate. Relevance is pinned to `1.0` (like the user profile) so it is
/// never edged out, and `hybrid.rs::run_rerank` pre-populates the Feedback slot
/// unconditionally — these standing rules surface regardless of query match.
fn feedback_entry_to_candidate(id: String, entry: FeedbackFloorEntry) -> Candidate {
    Candidate {
        id,
        title: entry.title,
        full_content: entry.body,
        source: ItemSource::Note {
            path: entry.path,
            category: "feedback".into(),
        },
        relevance: 1.0,
        updated_at: entry.updated_at,
        slot_hint: SlotKind::Feedback,
        fact_source: FactSource::Extracted,
    }
}

/// Convert the prior session's [`SessionSnapshot`] into a `SessionRecent`-slotted
/// candidate.
///
/// The body is the summary VERBATIM. That summary is the `/end-summary`, which
/// is mandated to carry filled `## Key Decisions` / `## Files & Code` /
/// `## Pending` sections — so the old render, which appended
/// `Key decisions: …\nActive files: …\nPending: …` from snapshot fields no
/// producer ever filled, handed the model a filled answer and then, four lines
/// later, an empty one it would read last. Those fields are gone (R7/P8: their
/// only possible source was scraping this very natural-language summary).
fn snapshot_to_candidate(snap: SessionSnapshot) -> Candidate {
    let sid = snap.session_id;
    Candidate {
        id: format!("aleph://session/{sid}/snapshot"),
        title: format!("Session {sid} snapshot"),
        full_content: snap.summary,
        source: ItemSource::Summary {
            layer: "d1".into(),
            session_id: sid,
        },
        relevance: 0.9,
        updated_at: snap.created_at.timestamp(),
        slot_hint: SlotKind::SessionRecent,
        fact_source: FactSource::Summary,
    }
}

/// Convert a dream-daemon [`DailyInsight`] into a `SessionRecent`-slotted
/// candidate. Relevance sits below the prior-session snapshot (0.9) — the
/// digest is ambient daily context, not a direct continuation of this session.
fn insight_to_candidate(insight: crate::memory::dreaming::DailyInsight) -> Candidate {
    Candidate {
        id: format!("aleph://insight/{}", insight.date),
        title: format!("Daily digest {}", insight.date),
        full_content: insight.content,
        source: ItemSource::Summary {
            layer: "daily_digest".into(),
            session_id: insight.date,
        },
        relevance: 0.7,
        updated_at: insight.created_at,
        slot_hint: SlotKind::SessionRecent,
        fact_source: FactSource::Summary,
    }
}

/// Convert a raw memory row into a candidate. `slot` decides its cognitive tier
/// via `render::cognitive_layer`: current-session live fragments (the working
/// set) pass `SlotKind::SessionRecent` → `Working`; cross-session
/// `SessionCompressed` rows pass `SlotKind::RawFragments` → `Raw` audit substrate.
fn raw_to_candidate(r: RawMemory, slot: SlotKind) -> Candidate {
    let session_id = r.session_id.unwrap_or_default();
    let fact_source = raw_source_to_fact_source(&r.source);
    let path = r.path;
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
        slot_hint: slot,
        fact_source,
    }
}

/// Map storage-layer [`RawMemorySource`] to semantic-layer [`FactSource`].
///
/// Only `SessionCompressed` has a meaningful 1:1 mapping. All other variants
/// are transcript/tool-output-like content and map to `Extracted`.
const fn raw_source_to_fact_source(src: &RawMemorySource) -> FactSource {
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
        let c = raw_to_candidate(raw, SlotKind::RawFragments);
        match &c.source {
            ItemSource::Raw { session_id, .. } => assert_eq!(session_id, "sess-1"),
            _ => panic!("expected ItemSource::Raw"),
        }
        assert_eq!(c.slot_hint, SlotKind::RawFragments);
        assert_eq!(c.fact_source, FactSource::Extracted);
    }

    #[test]
    fn current_session_raw_maps_to_working_tier() {
        use crate::memory::assembler::render::cognitive_layer;
        use crate::memory::context::CognitiveLayer;
        let raw = RawMemory::new("live".into(), RawMemorySource::Transcript).with_session("sess-3");
        let c = raw_to_candidate(raw, SlotKind::SessionRecent);
        assert_eq!(c.slot_hint, SlotKind::SessionRecent);
        // Regression: the live working set must render as `Working`, not the
        // `Raw` audit tier (four-layer cognitive view — MEMORY_SYSTEM.md §3.1).
        assert_eq!(
            cognitive_layer(c.slot_hint, &c.source),
            CognitiveLayer::Working
        );
    }

    #[test]
    fn session_compressed_maps_to_correct_fact_source() {
        let raw = RawMemory::new(
            "compressed summary".into(),
            RawMemorySource::SessionCompressed,
        )
        .with_session("sess-2");
        let c = raw_to_candidate(raw, SlotKind::RawFragments);
        assert_eq!(c.fact_source, FactSource::SessionCompressed);
    }

    #[test]
    fn feedback_path_routes_to_feedback_slot() {
        assert_eq!(slot_hint_for_path("feedback/no-jsdoc"), SlotKind::Feedback);
        // Substring, not prefix — must NOT match.
        assert_eq!(
            slot_hint_for_path("reference/feedback-loops"),
            SlotKind::RelevantNotes
        );
        assert_eq!(
            slot_hint_for_path("reference/rust-ownership"),
            SlotKind::RelevantNotes
        );
    }

    #[test]
    fn snapshot_candidate_body_is_the_summary_verbatim() {
        // Regression: the render used to append `Key decisions:` /
        // `Active files:` / `Pending:` labels fed by fields that had no
        // producer, so every injected snapshot ended with three empty labels
        // contradicting the summary's own filled sections directly above.
        let summary = "## Key Decisions\n- chose SQLite\n\n## Pending\n- ship the migration";
        let snap = SessionSnapshot {
            session_id: "agent:main:prev".into(),
            agent_id: "main".into(),
            scope_id: Some("main".into()),
            created_at: chrono::Utc::now(),
            summary: summary.into(),
        };
        let c = snapshot_to_candidate(snap);
        assert_eq!(c.full_content, summary);
        assert_eq!(c.slot_hint, SlotKind::SessionRecent);
        assert_eq!(c.fact_source, FactSource::Summary);
        assert_eq!(c.id, "aleph://session/agent:main:prev/snapshot");
    }

    #[test]
    fn insight_to_candidate_routes_to_session_recent_slot() {
        let insight = crate::memory::dreaming::DailyInsight::new(
            "2026-06-10".into(),
            "Worked on compaction caching.".into(),
            4,
        );
        let c = insight_to_candidate(insight);
        assert_eq!(c.slot_hint, SlotKind::SessionRecent);
        assert_eq!(c.fact_source, FactSource::Summary);
        assert_eq!(c.id, "aleph://insight/2026-06-10");
        match &c.source {
            ItemSource::Summary { layer, session_id } => {
                assert_eq!(layer, "daily_digest");
                assert_eq!(session_id, "2026-06-10");
            }
            _ => panic!("expected ItemSource::Summary"),
        }
    }

    /// P1 "Floors 分床": the user-profile floor must follow the session's
    /// personal scope, while the feedback floor is not *narrowed* to it — it
    /// reads base ∪ this session's partition. Effect assertion via real
    /// tempdir-backed loaders, not a mock — both loaders join `agent_id`
    /// straight into their own path, so the right content landing in the pool
    /// IS the proof of which id each loader was actually asked for.
    ///
    /// Both halves are asserted deliberately. Checking only the base rule is
    /// what let the floor ship empty: it proved the reader looks at `main/`,
    /// and never crossed the seam to where `flag_user_correction` actually
    /// writes.
    #[tokio::test]
    async fn user_floor_is_scoped_feedback_floor_is_not() {
        use crate::scope::{with_scope, ScopeAttribution};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().to_path_buf();

        // Scoped profile — only visible if the profile floor resolves the
        // composed personal-scope id ("main__u-alice").
        let scoped_profile_dir = memory_dir.join("main__u-alice").join("personal");
        std::fs::create_dir_all(&scoped_profile_dir).unwrap();
        std::fs::write(scoped_profile_dir.join("profile.md"), "alice profile body").unwrap();

        // Base feedback rule — an org-wide standing rule must still surface
        // even though the session is personal-scoped.
        let feedback_dir = memory_dir.join("main").join("feedback");
        std::fs::create_dir_all(&feedback_dir).unwrap();
        std::fs::write(
            feedback_dir.join("rule.md"),
            "---\ncategory: feedback\nseverity: high\nconfidence: 0.9\n---\n\n- Always do X\n",
        )
        .unwrap();

        // …and a correction written where `flag_user_correction` actually puts
        // one for this session. Reading only the base id makes this invisible,
        // which was the shipped behaviour.
        let scoped_feedback_dir = memory_dir.join("main__u-alice").join("feedback");
        std::fs::create_dir_all(&scoped_feedback_dir).unwrap();
        std::fs::write(
            scoped_feedback_dir.join("mine.md"),
            "---\ncategory: feedback\nseverity: critical\nconfidence: 0.9\n---\n\n- Never do Y\n",
        )
        .unwrap();

        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let indexer = Arc::new(crate::memory::notes::NoteIndexer::new(
            memory_dir.join("notes_idx"),
            backend.clone(),
        ));
        let retrieval = Arc::new(NoteFactRetrieval::new_fts_only(indexer));
        let snapshots = Arc::new(SnapshotReader::new(tmp.path().join("snap")));
        let profile = UserProfileLoader::new(memory_dir.clone());
        let feedback_floor = FeedbackFloorLoader::new(memory_dir);
        let gatherer = Gatherer {
            retrieval,
            snapshots,
            backend,
            profile,
            feedback_floor,
            project_scoped: false,
        };
        let inputs = GatherInputs {
            query: "q".into(),
            agent_id: "main".into(),
            session_id: None,
            pool_limit: 20,
            filter: FactSourceFilter::Any,
        };

        let pool = with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            gatherer.gather(&inputs),
        )
        .await;

        let profile_hit = pool
            .iter()
            .find(|c| c.slot_hint == SlotKind::UserProfile)
            .expect("profile floor must load under personal scope");
        assert_eq!(profile_hit.full_content, "alice profile body");

        let feedback: Vec<&str> = pool
            .iter()
            .filter(|c| c.slot_hint == SlotKind::Feedback)
            .map(|c| c.full_content.as_str())
            .collect();
        assert!(
            feedback.iter().any(|c| c.contains("Always do X")),
            "an org-wide standing rule must still reach a personal session: {feedback:?}"
        );
        assert!(
            feedback.iter().any(|c| c.contains("Never do Y")),
            "a correction written to this session's own partition must reach \
             the always-on floor — that is where every writer puts it: {feedback:?}"
        );
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
