//! Free helper functions for the compound ingestor: candidate construction,
//! prompt assembly, op validation, dedup-text canonicalisation, keyword query
//! extraction, and cosine similarity.

use crate::memory::notes::governance::gate::{CandidateNote, NoteWriteAction};
use crate::memory::notes::ingest::plan::PageOp;
use crate::memory::notes::ingest::ref_table::RefTable;
use crate::memory::notes::ingest::retrieve::RelatedPage;
use crate::memory::notes::KnowledgeNote;

/// Build a `CandidateNote` for the gate from a `PageOp`. Returns `None` for the
/// still-ungated op kinds (Append/Update/Link), which apply immediately without
/// review. `Supersede` is gated too, but out-of-band: it needs the superseded
/// note's on-disk severity, so the ingestor's async `supersede_candidate`
/// handles it before this sync helper is reached.
///
/// - `Create`: `confidence`/`severity` are the planner LLM's self-assessment —
///   the gate defers low-confidence creations and holds `High`/`Critical` pages
///   to a higher bar. Plans predating those fields default to `1.0`/`Low`
///   (serde), preserving the pre-wiring "admit everything" behaviour.
/// - `Contradict`: gated as a review-bound candidate (`contradicts_existing` →
///   Defer) carrying the original op in `replay_op` so the reviewer replays the
///   contradiction verbatim on approval rather than overwriting the note.
///
/// R7: the model judges its own output (confidence / whether to contradict); the
/// gate is a deterministic threshold, not a re-judgement.
pub(crate) fn candidate_from_pageop(agent_id: &str, op: &PageOp) -> Option<CandidateNote> {
    match op {
        PageOp::Create {
            note_path,
            title,
            facts,
            links,
            tags,
            confidence,
            severity,
            ..
        } => {
            let category = note_path
                .split_once('/')
                .map(|(c, _)| c.to_string())
                .unwrap_or_default();
            let note = KnowledgeNote {
                // rust-doctor-disable-next-line excessive-clone
                title: title.clone(),
                // rust-doctor-disable-next-line excessive-clone
                category: category.clone(),
                // rust-doctor-disable-next-line excessive-clone
                tags: tags.clone(),
                // rust-doctor-disable-next-line excessive-clone
                facts: facts.clone(),
                // rust-doctor-disable-next-line excessive-clone
                links: links.clone(),
                confidence: confidence.clamp(0.0, 1.0),
                severity: *severity,
                ..KnowledgeNote::default()
            };
            Some(CandidateNote {
                agent_id: agent_id.to_string(),
                category,
                note,
                action: NoteWriteAction::Create,
                bypass_review: false,
                contradicts_existing: false,
                replay_op: None,
            })
        }
        // A `Contradict` asserts existing knowledge is wrong — the classic
        // anti-feedback vector. Route it through the gate as a review-bound
        // candidate (`contradicts_existing` → Defer) carrying the original op so
        // the reviewer can replay the contradiction verbatim on approval. No new
        // `PageOp` fields needed: deferral is driven by `contradicts_existing`,
        // not confidence. If the op fails to serialize (never expected), fall
        // through to `None` so it applies immediately as before (no silent drop).
        PageOp::Contradict {
            note_path,
            new_claim,
            ..
        } => {
            let replay = serde_json::to_value(op).ok()?;
            let category = note_path
                .split_once('/')
                .map(|(c, _)| c.to_string())
                .unwrap_or_default();
            let note = KnowledgeNote {
                title: note_path
                    .split_once('/')
                    .map(|(_, f)| f.to_string())
                    // rust-doctor-disable-next-line excessive-clone
                    .unwrap_or_else(|| note_path.clone()),
                // rust-doctor-disable-next-line excessive-clone
                category: category.clone(),
                facts: vec![new_claim.clone()],
                ..KnowledgeNote::default()
            };
            Some(CandidateNote {
                agent_id: agent_id.to_string(),
                category,
                note,
                action: NoteWriteAction::Update,
                bypass_review: false,
                contradicts_existing: true,
                replay_op: Some(replay),
            })
        }
        // Ungated here: applied immediately without review. `Supersede` also
        // returns `None` as a fallback, but the ingestor gates it upstream via
        // `supersede_candidate` (severity lookup), so it never reaches here in
        // the gated path.
        PageOp::Append { .. }
        | PageOp::Update { .. }
        | PageOp::Link { .. }
        | PageOp::Supersede { .. } => None,
    }
}

pub(crate) fn build_user_prompt(
    raws: &[crate::memory::store::raw_memory::RawMemory],
    related: &[RelatedPage],
    observation_date: &str,
) -> String {
    // Temporal grounding: give the model an absolute anchor so it can resolve
    // relative time references ("today", "next week") into permanent dates
    // before they are written into a fact. Without this, an Ephemeral/Contextual
    // fact like "user wants to focus on docs today" becomes uninterpretable on
    // later recall. The model does the resolution (R9: intelligence in prompt).
    let mut out = format!(
        "## Observation date\n\n\
         The current date is {observation_date}. Resolve every relative time \
         reference (\"today\", \"yesterday\", \"next week\", \"in 3 days\", \"last \
         month\") to an absolute date before writing it into a fact, so the memory \
         stays interpretable when recalled later.\n\n"
    );
    out.push_str("## New raw memories\n\n");
    for (i, r) in raws.iter().enumerate() {
        out.push_str(&format!(
            "### raw-{} (id={}, source={:?})\n",
            i + 1,
            r.id,
            r.source
        ));
        out.push_str(&r.content);
        out.push_str("\n\n");
        if let Some(att) = &r.attachment_text {
            out.push_str("[Attachment]\n");
            out.push_str(att);
            out.push_str("\n\n");
        }
    }
    if !related.is_empty() {
        out.push_str(
            "## Related existing pages\n\n\
             Each page below carries a `[P<n>]` reference token. To act on an \
             EXISTING page (append / update / contradict / link / supersede, or \
             a create's `links`), put its token in the path field instead of \
             retyping the path — the system resolves tokens to exact paths.\n\n",
        );
        for (i, p) in related.iter().enumerate() {
            out.push_str(&format!(
                "### {token} path={path} (hash={hash})\n",
                token = RefTable::token(i),
                path = p.path,
                hash = p.content_hash
            ));
            out.push_str(&format!("title: {}\n", p.title));
            if !p.tags.is_empty() {
                out.push_str(&format!("tags: {}\n", p.tags.join(", ")));
            }
            out.push_str("preview:\n");
            out.push_str(&p.content_preview);
            out.push_str("\n\n");
        }
    } else {
        out.push_str("## Related existing pages\n\n(none — empty wiki or no matches)\n");
    }
    out.push_str(
        "Reminder: every object in `ops` and `schema_proposals` MUST begin with \
         its `kind` field. Omitting `kind` makes the operation invalid.\n\n",
    );
    out.push_str("Produce the IngestPlan JSON now.");
    out
}

pub(crate) fn valid_op(op: &PageOp) -> bool {
    match op {
        // A `Create` only needs a well-formed `category/filename` path. It is
        // deliberately NOT required to carry links: on a sparse/just-bootstrapped
        // wiki (or when `gather_related` degraded to an empty set because the
        // embedding endpoint was down) there are no existing pages to link to,
        // and the planner's `[P<n>]` link tokens get stripped as out-of-range by
        // `RefTable::resolve_plan`. Dropping the resulting linkless Create here
        // silently discarded knowledge the model had already extracted — the
        // dominant reason the L1 note layer could not grow past its seed. A seed
        // note is recoverable (dream consolidation links it later); discarded
        // knowledge is gone forever. This mirrors the anti-starvation degradation
        // in `ingest_batch` (related-gathering failure must not starve the layer).
        PageOp::Create { note_path, .. } => note_path.contains('/'),
        PageOp::Append { note_path, .. }
        | PageOp::Update { note_path, .. }
        | PageOp::Contradict { note_path, .. } => note_path.contains('/'),
        PageOp::Link { from, to } => from.contains('/') && to.contains('/') && from != to,
        PageOp::Supersede { old_path, new_path } => {
            old_path.contains('/') && new_path.contains('/') && old_path != new_path
        }
    }
}

/// Build the probe text for a `Create` candidate in the write-time dedup gate.
/// Mirrors the salient content of a note (human title, summary, facts) so the
/// candidate's embedding is comparable to a stored note's vector.
pub(crate) fn candidate_dedup_text(title: &str, summary: &str, facts: &[String]) -> String {
    let mut s = String::new();
    if !title.is_empty() {
        s.push_str(title);
        s.push('\n');
    }
    if !summary.is_empty() {
        s.push_str(summary);
        s.push('\n');
    }
    for f in facts {
        s.push_str(f);
        s.push('\n');
    }
    s
}

/// Split free text into a handful of significant keyword terms for per-keyword
/// FTS probing. `search_notes_fts` treats its whole query as one FTS5 phrase, so
/// a multi-word blob would require an exact phrase hit and never match — probe
/// per significant term instead.
///
/// Latin words are lowercased, length-gated (≥4 chars), and deduped. CJK runs
/// need their own branch: CJK ideographs are Unicode-alphanumeric, so the
/// char-class split keeps a space-free run (`深色编辑器主题`) as ONE token — and
/// the FTS `unicode61` tokenizer keeps it as one token too, so that giant phrase
/// never matches a peer note, leaving CJK notes as orphan islands. For CJK
/// tokens we instead emit adjacent 2-gram windows (`深色`/`色编`/`编辑`/`辑器`/…),
/// which DO match peer notes under `unicode61`. Total terms are capped so a long
/// run cannot explode into dozens of `search_notes_fts` round-trips. This only
/// widens deterministic FTS candidate recall; the primary linker is still the
/// LLM `extract_keywords`, so it stays R7/P8-safe.
pub(crate) fn keyword_query_terms(text: &str) -> Vec<String> {
    /// CJK ideograph test for the common ranges.
    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{4E00}'..='\u{9FFF}'    // CJK Unified Ideographs
            | '\u{3400}'..='\u{4DBF}'  // CJK Extension A
            | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
        )
    }

    const MAX_TERMS: usize = 8;
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if out.len() >= MAX_TERMS {
            break;
        }
        if word.chars().any(is_cjk) {
            // A 2-char CJK term carries about as much signal as a 4-char Latin
            // word; emit adjacent 2-gram windows of the run as FTS probes.
            let chars: Vec<char> = word.chars().collect();
            for w in chars.windows(2) {
                if out.len() >= MAX_TERMS {
                    break;
                }
                let gram: String = w.iter().collect();
                if !out.contains(&gram) {
                    out.push(gram);
                }
            }
        } else if word.chars().count() >= 4 {
            let lower = word.to_lowercase();
            if !out.contains(&lower) {
                out.push(lower);
            }
        }
    }
    out
}

/// Cosine similarity in `[-1, 1]`. Returns `0.0` for mismatched-length,
/// empty, or zero-norm vectors so such pairs are treated as "not similar"
/// and never trigger a dedup redirect.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::note::types::Severity;

    fn create_op(confidence: f32, severity: Severity) -> PageOp {
        PageOp::Create {
            note_path: "reference/x".into(),
            title: "X".into(),
            summary: String::new(),
            facts: vec![],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence,
            severity,
        }
    }

    #[test]
    fn candidate_carries_llm_confidence_and_severity() {
        // The gate can only defer low-confidence / high-severity creations if
        // the candidate actually reflects the planner's self-assessment rather
        // than a hardcoded default — this is the wiring the gate depends on.
        let c = candidate_from_pageop("default", &create_op(0.42, Severity::High))
            .expect("Create must be gated");
        assert_eq!(c.note.confidence, 0.42);
        assert_eq!(c.note.severity, Severity::High);
    }

    #[test]
    fn candidate_clamps_out_of_range_confidence() {
        let c = candidate_from_pageop("default", &create_op(1.5, Severity::Low)).unwrap();
        assert_eq!(c.note.confidence, 1.0);
    }

    #[test]
    fn link_and_other_ops_stay_ungated() {
        let op = PageOp::Link {
            from: "a/b".into(),
            to: "c/d".into(),
        };
        assert!(candidate_from_pageop("default", &op).is_none());
        let op = PageOp::Append {
            note_path: "a/b".into(),
            new_facts: vec![],
            new_links: vec![],
            new_relations: vec![],
            source_ids: vec![],
        };
        assert!(candidate_from_pageop("default", &op).is_none());
    }

    #[test]
    fn contradict_is_gated_with_replay_op() {
        // A Contradict must become a review-bound candidate: it defers via
        // `contradicts_existing` and carries the original op for verbatim replay
        // on approval (so approval re-applies the contradiction, not an overwrite).
        let op = PageOp::Contradict {
            note_path: "reference/rust".into(),
            new_claim: "tokio is not single-threaded".into(),
            evidence_source_ids: vec!["raw-1".into()],
        };
        let c = candidate_from_pageop("default", &op).expect("Contradict must be gated");
        assert!(
            c.contradicts_existing,
            "must route to the gate's Defer path"
        );
        assert_eq!(c.note.category, "reference");
        assert_eq!(c.note.facts, vec!["tokio is not single-threaded"]);
        let replay = c.replay_op.expect("must carry the original op for replay");
        // Round-trips back to the same op.
        let back: PageOp = serde_json::from_value(replay).unwrap();
        assert!(matches!(back, PageOp::Contradict { .. }));
    }
}
