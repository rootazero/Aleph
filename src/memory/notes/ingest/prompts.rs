//! System prompts for the compound-ingest LLM call.

use crate::memory::compression::source_prompts::prompt_for;
use crate::memory::store::raw_memory::RawMemorySource;

/// Base system prompt. Instructs the LLM to read related pages and return
/// a cross-page `IngestPlan` as JSON.
pub const PROMPT_COMPOUND_PLAN: &str = r#"You maintain an Aleph personal-memory wiki. Given a batch of raw
conversation memories plus the set of already-existing pages most relevant
to them, emit an IngestPlan that updates the wiki.

## Ingest rules

1. Look across ALL related pages, not just one. A single batch usually
   touches 3–12 pages.
2. Every fact must be in THIRD PERSON ("The user prefers X").
3. When a new claim CONFLICTS with content already on a related page,
   emit `contradict` — do NOT silently append.
4. When an existing page already covers a topic, emit `append` rather
   than creating a duplicate.
5. When new info SUPERSEDES an older page entirely, emit `supersede`.
6. Every `create` must include at least two `links` to existing pages
   (otherwise the new page is an orphan). If you cannot find two
   existing pages to link, the claim likely belongs as an `append` on
   an existing page instead.
7. When you want to introduce a NEW tag (one not present in any
   provided related page), put it in `schema_proposals` as
   `new_tag`; do NOT invent a tag in `tags:` that the Schema has not
   seen before.
8. Ignore greetings, small talk, transient information.
9. 0–12 ops per batch. Quality over quantity.
10. Use `update` only to CORRECT a page whose content is now wrong.
    When using `update`, set `expected_content_hash` to the hash of the
    related page AS YOU READ IT in the input below. Do not fabricate hashes.

## Page op kinds

- `create` — new page. Fields: `note_path` (category/filename),
  `title`, `summary` (≤120 chars), `facts[]`, `links[]`, `tags[]`.
- `append` — add facts to an existing page. Fields: `note_path`,
  `new_facts[]`, `new_links[]`.
- `update` — replace facts on an existing page. Fields: `note_path`,
  `expected_content_hash`, `new_facts[]`, `reason`.
- `contradict` — mark a page contradicted by new info. Fields:
  `note_path`, `new_claim`, `evidence_source_ids[]`.
- `link` — add a bidirectional wikilink. Fields: `from`, `to`.
- `supersede` — older page is superseded by newer one. Fields:
  `old_path`, `new_path`.

## Output

Valid JSON only. No prose, no markdown fences. Shape:

{
  "reasoning": "2-3 sentence explanation of what you did and why",
  "ops": [ /* PageOp objects, EACH MUST CARRY A "kind" FIELD */ ],
  "schema_proposals": [ /* optional new_tag / new_rule / domain_update */ ]
}

Every entry inside `ops` MUST start with the discriminator field
`"kind"` whose value is exactly one of:
`"create"`, `"append"`, `"update"`, `"contradict"`, `"link"`, `"supersede"`.
Operations missing `kind` are silently discarded — do not omit it.
Concrete shapes:

```json
{"kind": "create", "note_path": "personal/li-wei.md", "title": "Li Wei", "summary": "User's partner of six years", "facts": ["Li Wei works in tech."], "links": ["personal/zou-guojun.md"], "tags": ["personal"]}
{"kind": "append", "note_path": "preferences/coding-style.md", "new_facts": ["Comments must be in English."], "new_links": []}
{"kind": "update", "note_path": "projects/aleph.md", "expected_content_hash": "<copy hash from input>", "new_facts": ["Current focus is the memory subsystem."], "reason": "User clarified the focus area."}
{"kind": "link", "from": "personal/zou-guojun.md", "to": "projects/aleph.md"}
{"kind": "supersede", "old_path": "projects/old-aleph.md", "new_path": "projects/aleph.md"}
{"kind": "contradict", "note_path": "preferences/coding-style.md", "new_claim": "User now prefers Python.", "evidence_source_ids": ["raw-123"]}
```

If nothing is worth ingesting, emit:
{"reasoning": "no durable knowledge", "ops": [], "schema_proposals": []}
"#;

/// Build the full system prompt for a batch whose rows share the given
/// raw source. Appends the Spec-1 source-aware block (RESCUE / LESSON /
/// DIGEST / RETRO) when applicable.
pub fn build_compound_system_prompt(source: &RawMemorySource) -> String {
    let mut out = String::from(PROMPT_COMPOUND_PLAN);
    if let Some(suffix) = prompt_for(source) {
        out.push_str("\n\n## Source-specific guidance\n\n");
        out.push_str(suffix);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_prompt_snapshot() {
        insta::assert_snapshot!("compound_plan_base_prompt", PROMPT_COMPOUND_PLAN);
    }

    #[test]
    fn base_prompt_mentions_every_op_kind() {
        for kind in [
            "create",
            "append",
            "update",
            "contradict",
            "link",
            "supersede",
        ] {
            assert!(
                PROMPT_COMPOUND_PLAN.contains(&format!("`{kind}`")),
                "missing op kind: {kind}"
            );
        }
    }

    #[test]
    fn precompress_prompt_appends_rescue_block() {
        let p = build_compound_system_prompt(&RawMemorySource::PreCompress);
        assert!(p.starts_with("You maintain"));
        assert!(p.contains("Source-specific guidance"));
        assert!(p.contains("memory rescue assistant"));
    }

    #[test]
    fn legacy_source_has_no_suffix() {
        let p = build_compound_system_prompt(&RawMemorySource::Transcript);
        assert_eq!(p.as_str(), PROMPT_COMPOUND_PLAN);
    }
}
