//! Free helper functions for the compound ingestor: candidate construction,
//! prompt assembly, op validation, dedup-text canonicalisation, keyword query
//! extraction, and cosine similarity.

use crate::memory::notes::governance::gate::{CandidateNote, NoteWriteAction};
use crate::memory::notes::ingest::plan::PageOp;
use crate::memory::notes::ingest::ref_table::RefTable;
use crate::memory::notes::ingest::retrieve::RelatedPage;
use crate::memory::notes::KnowledgeNote;

/// Build a `CandidateNote` for the gate from a `PageOp`. Returns `None` for
/// op kinds that this scoped commit does not gate (Append/Update/Contradict/
/// Link/Supersede). The candidate's `confidence` and `severity` come from
/// the LLM-generated note shape; in this scoped commit `PageOp::Create`
/// does not carry those fields, so we use `KnowledgeNote::default()` (which
/// sets `confidence = 1.0` and `severity = Low`) so the gate's default
/// thresholds (`min_confidence = 0.5`, `high_severity_min_confidence = 0.8`)
/// admit them. Confidence/severity wiring on `PageOp::Create` is a
/// follow-up task once the planner prompt is updated to emit them.
pub(crate) fn candidate_from_pageop(agent_id: &str, op: &PageOp) -> Option<CandidateNote> {
    match op {
        PageOp::Create {
            note_path,
            title,
            facts,
            links,
            tags,
            ..
        } => {
            let category = note_path
                .split_once('/')
                .map(|(c, _)| c.to_string())
                .unwrap_or_default();
            let note = KnowledgeNote {
                title: title.clone(),
                category: category.clone(),
                tags: tags.clone(),
                facts: facts.clone(),
                links: links.clone(),
                ..KnowledgeNote::default()
            };
            Some(CandidateNote {
                agent_id: agent_id.to_string(),
                category,
                note,
                source_path: None,
                fact_provenance: Vec::new(),
                action: NoteWriteAction::Create,
                bypass_review: false,
                contradicts_existing: false,
            })
        }
        // Scoped out in this commit; pass-through.
        PageOp::Append { .. }
        | PageOp::Update { .. }
        | PageOp::Contradict { .. }
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

/// Split free text into a handful of significant lowercase keyword terms for
/// per-keyword FTS probing. Mirrors `note_manage::related_keywords`: skip short
/// words (<4 chars), dedup, cap at a few terms. `search_notes_fts` treats its
/// whole query as one FTS5 phrase, so a multi-word blob would require an exact
/// phrase hit and never match — probe per significant keyword instead.
pub(crate) fn keyword_query_terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.chars().count() < 4 {
            continue;
        }
        let lower = word.to_lowercase();
        if !out.contains(&lower) {
            out.push(lower);
            if out.len() >= 4 {
                break;
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

