//! Stage 2: LLM re-rank prompt + response validation.

use super::envelope::SlotKind;
use super::error::AssemblerError;
use super::fallback::Candidate;
use serde::Deserialize;
use std::collections::HashSet;

pub(crate) const RERANK_PROMPT_V1: &str = r#"You are a Working Memory Assembler. Given the user's current query and a pool of memory candidates, decide which to include and allocate a token budget across slots: session_recent, relevant_notes, raw_fragments. (user_profile is pre-included; nudges are reserved for future use.)

Query: {query}

Total budget: {budget} tokens. Your slot budgets must sum to at most {max_sum} tokens (reserving 30% for the LLM reply).

Candidates (id | title | slot_hint | relevance | summary):
{candidates}

Return STRICT JSON (no prose, no markdown fences) matching:
{
  "slots": [
    {"kind": "relevant_notes",  "item_ids": ["..."], "tokens_budget": N},
    {"kind": "session_recent",  "item_ids": ["..."], "tokens_budget": N},
    {"kind": "raw_fragments",   "item_ids": ["..."], "tokens_budget": N}
  ],
  "reasoning": "one-line explanation (optional)"
}

Rules:
- item_ids MUST be a subset of the candidate ids above.
- Omit a slot entirely if you would not include anything in it.
- Order within item_ids is priority (most important first).
- Do NOT include the user_profile slot — it is pre-populated.
"#;

pub(crate) fn build_prompt(query: &str, candidates: &[Candidate], total_budget: u32) -> String {
    let max_sum = ((total_budget as f32) * 0.7) as u32;
    let cand_block: String = candidates
        .iter()
        .filter(|c| c.slot_hint != SlotKind::UserProfile)
        .map(|c| {
            let summary: String = c.full_content.chars().take(30).collect();
            format!(
                "  [{}] | \"{}\" | {} | {:.2} | {}",
                c.id,
                c.title.replace('"', "'"),
                slot_name(c.slot_hint),
                c.relevance,
                summary,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    RERANK_PROMPT_V1
        .replace("{query}", query)
        .replace("{budget}", &total_budget.to_string())
        .replace("{max_sum}", &max_sum.to_string())
        .replace("{candidates}", &cand_block)
}

fn slot_name(k: SlotKind) -> &'static str {
    match k {
        SlotKind::UserProfile => "user_profile",
        SlotKind::SessionRecent => "session_recent",
        SlotKind::RelevantNotes => "relevant_notes",
        SlotKind::RawFragments => "raw_fragments",
        SlotKind::Nudges => "nudges",
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RerankResponse {
    #[serde(default)]
    pub slots: Vec<RerankSlot>,
    #[serde(default)]
    #[allow(dead_code)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RerankSlot {
    pub kind: String,
    #[serde(default)]
    pub item_ids: Vec<String>,
    pub tokens_budget: u32,
}

/// Parse and validate an LLM response against the candidate pool + total budget.
/// Returns a sanitized `Vec<(SlotKind, Vec<String>, u32)>` — unknown kinds
/// dropped, hallucinated ids filtered, per-slot budgets scaled proportionally
/// if their sum exceeds the 70% cap.
pub(crate) fn parse_response(
    raw: &str,
    candidates: &[Candidate],
    total_budget: u32,
) -> Result<Vec<(SlotKind, Vec<String>, u32)>, AssemblerError> {
    let trimmed = strip_json_fences(raw);
    let resp: RerankResponse = serde_json::from_str(trimmed)
        .map_err(|e| AssemblerError::RerankParse(format!("json: {e}")))?;

    let valid_ids: HashSet<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
    let mut sanitized: Vec<(SlotKind, Vec<String>, u32)> = Vec::new();
    for slot in resp.slots {
        let Some(kind) = parse_slot_kind(&slot.kind) else {
            continue;
        };
        if matches!(kind, SlotKind::UserProfile) {
            continue;
        }
        let ids: Vec<String> = slot
            .item_ids
            .into_iter()
            .filter(|id| valid_ids.contains(id.as_str()))
            .collect();
        if ids.is_empty() || slot.tokens_budget == 0 {
            continue;
        }
        sanitized.push((kind, ids, slot.tokens_budget));
    }

    if sanitized.is_empty() {
        return Err(AssemblerError::RerankEmpty);
    }

    let sum: u64 = sanitized.iter().map(|(_, _, b)| *b as u64).sum();
    let cap = ((total_budget as f32) * 0.7) as u64;
    if sum > cap {
        let scale = cap as f32 / sum as f32;
        for (_, _, b) in sanitized.iter_mut() {
            *b = ((*b as f32) * scale).floor() as u32;
        }
    }
    Ok(sanitized)
}

fn parse_slot_kind(s: &str) -> Option<SlotKind> {
    match s {
        "user_profile" => Some(SlotKind::UserProfile),
        "session_recent" => Some(SlotKind::SessionRecent),
        "relevant_notes" => Some(SlotKind::RelevantNotes),
        "raw_fragments" => Some(SlotKind::RawFragments),
        "nudges" => Some(SlotKind::Nudges),
        _ => None,
    }
}

fn strip_json_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t.strip_prefix("```json").unwrap_or(t);
    let t = t.strip_prefix("```").unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::ItemSource;

    fn cand(id: &str, slot: SlotKind) -> Candidate {
        Candidate {
            id: id.into(),
            title: id.into(),
            full_content: "body".into(),
            source: ItemSource::Note {
                path: id.into(),
                category: "reference".into(),
            },
            relevance: 0.5,
            updated_at: 0,
            slot_hint: slot,
            fact_source: crate::memory::context::FactSource::Extracted,
        }
    }

    #[test]
    fn valid_response_parses() {
        let c = [
            cand("note://a", SlotKind::RelevantNotes),
            cand("note://b", SlotKind::SessionRecent),
        ];
        let raw = r#"{"slots":[
            {"kind":"relevant_notes","item_ids":["note://a"],"tokens_budget":1000},
            {"kind":"session_recent","item_ids":["note://b"],"tokens_budget":500}
        ]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn invalid_json_errors() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        assert!(matches!(
            parse_response("{bogus", &c, 4000).unwrap_err(),
            AssemblerError::RerankParse(_)
        ));
    }

    #[test]
    fn hallucinated_ids_filtered() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://fake","note://a"],"tokens_budget":500}]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        assert_eq!(out[0].1, vec!["note://a"]);
    }

    #[test]
    fn over_budget_scales_proportionally() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://a"],"tokens_budget":10000}]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        assert!(out[0].2 <= 2800 && out[0].2 > 0);
    }

    #[test]
    fn empty_slots_errors() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[]}"#;
        assert!(matches!(
            parse_response(raw, &c, 4000).unwrap_err(),
            AssemblerError::RerankEmpty
        ));
    }

    #[test]
    fn unknown_kind_dropped() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"bogus","item_ids":["note://a"],"tokens_budget":500}]}"#;
        assert!(matches!(
            parse_response(raw, &c, 4000).unwrap_err(),
            AssemblerError::RerankEmpty
        ));
    }

    #[test]
    fn user_profile_slot_dropped() {
        let c = [cand("note://a", SlotKind::UserProfile)];
        let raw =
            r#"{"slots":[{"kind":"user_profile","item_ids":["note://a"],"tokens_budget":500}]}"#;
        assert!(matches!(
            parse_response(raw, &c, 4000).unwrap_err(),
            AssemblerError::RerankEmpty
        ));
    }

    #[test]
    fn markdown_fences_stripped() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = "```json\n{\"slots\":[{\"kind\":\"relevant_notes\",\"item_ids\":[\"note://a\"],\"tokens_budget\":500}]}\n```";
        assert!(parse_response(raw, &c, 4000).is_ok());
    }

    #[test]
    fn prompt_renders_candidates_and_budget() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let p = build_prompt("question", &c, 4000);
        assert!(p.contains("question"));
        assert!(p.contains("note://a"));
        assert!(p.contains("4000"));
        assert!(p.contains("2800"));
    }
}
