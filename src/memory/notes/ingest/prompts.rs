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
6. LINKING IS MANDATORY when the "Related existing pages" section below is
   non-empty: every `create` MUST carry at least one `links[]` entry or
   `relations` edge pointing at a `[P<n>]` token from that section. A note
   with no links is an orphan island — orphans rot unrecallable and are
   archived early, defeating the wiki. Only use `[P<n>]` tokens that
   ACTUALLY APPEAR in that section — never invent a token number (an
   out-of-range token is discarded and can cost you the whole note). ONLY
   when the section is empty (sparse wiki, or retrieval degraded) may you
   create a SEED note with an empty `links` list — do NOT skip a durable
   fact just because there is nothing to link to. Additionally, when you
   notice two EXISTING related pages that should reference each other,
   emit a `link` op to connect them.
7. When you want to introduce a NEW tag (one not present in any
   provided related page), put it in `schema_proposals` as
   `new_tag`; do NOT invent a tag in `tags:` that the Schema has not
   seen before.
8. Ignore greetings, small talk, transient information.
9. 0–12 ops per batch. Quality over quantity.
10. Use `update` only to CORRECT a page whose content is now wrong.
    When using `update`, set `expected_content_hash` to the hash of the
    related page AS YOU READ IT in the input below. Do not fabricate hashes.
11. To reference an EXISTING page, use its `[P<n>]` reference token exactly
    as shown in "Related existing pages" — do NOT retype the path. This
    applies to every path field that targets an existing page: `append`,
    `update`, `contradict` (`note_path`), `link` (`from`/`to`),
    `supersede` (`old_path`/`new_path`), and a `create`'s `links[]`. Only a
    `create`'s own `note_path` is a fresh `category/filename` (a new page,
    so it has no token). An op whose token does not exist is discarded.
12. ATTRIBUTE each fact to who stated it. Record only durable facts the
    USER stated or confirmed about themselves or the world. Do NOT record
    the assistant's own suggestions, hedges, or proposed options as user
    facts unless the user explicitly accepted them.
13. PROVENANCE. Each raw memory in the input is shown as
    `### raw-N (id=<UUID>, source=...)`. For every `create` and `append`
    op, set `source_ids` to the list of `<UUID>` values whose content the
    op was distilled from. When a SINGLE fact comes verbatim from one raw,
    you MAY also append an inline marker to that fact string:
    `<!-- src: <UUID>, origin: raw_source, inferred: false -->`.
    Facts you infer or generalize need no marker (they default to inferred).
    Never invent a UUID — copy it exactly from the input.

## Page op kinds

- `create` — new page. Fields: `note_path` (category/filename),
  `title`, `summary` (≤120 chars), `facts[]`, `links[]` (use `[P<n>]`
  tokens), `tags[]`, `source_ids[]` (raw UUIDs this page came from),
  `confidence` (0..1 — how sure you are these facts are true and durable;
  be honest, low-confidence pages are held for review not discarded), and
  `severity` (`low`|`med`|`high`|`critical` — how important/risky it is to
  get this page right; `high`/`critical` must earn higher confidence).
- `append` — add facts to an existing page. Fields: `note_path`,
  `new_facts[]`, `new_links[]`, `source_ids[]`.
- `update` — replace facts on an existing page. Fields: `note_path`,
  `expected_content_hash`, `new_facts[]`, `reason`.
- `contradict` — mark a page contradicted by new info. Fields:
  `note_path`, `new_claim`, `evidence_source_ids[]`.
- `link` — add a bidirectional wikilink. Fields: `from`, `to`.
- `supersede` — older page is superseded by newer one. Fields:
  `old_path`, `new_path`.

## Entities & relationships

When the source names durable entities (people, organisations, projects,
concepts), create or append `entity/<slug>` notes for them (category `entity`).
Express relationships BETWEEN entities with the op's `relations` field — a list
of `{ "to": "<entity path or [P<n>] token>", "type": "<snake_case verb>",
"confidence": <0..1> }`. Choose a concise `type` yourself (e.g. `works_at`,
`depends_on`, `colleague`, `part_of`); there is no fixed vocabulary. Reuse
existing entity notes shown in "Related existing pages" — never duplicate an
entity that already exists; append new relations to it instead.

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

Existing-page references use `[P<n>]` tokens; a `create`'s own `note_path`
is a fresh path. The `[P<n>]` numbers below are ILLUSTRATIVE placeholders —
use only the token numbers that actually appear in your input's "Related
existing pages" section (e.g. if only `[P0]` is shown, never write `[P1]`):

```json
{"kind": "create", "note_path": "preference/typescript.md", "title": "TypeScript", "summary": "User prefers TypeScript", "facts": ["The user prefers TypeScript. <!-- src: 7f3a..., origin: raw_source, inferred: false -->"], "links": ["[P3]"], "tags": ["preference"], "source_ids": ["7f3a..."], "confidence": 0.9, "severity": "low"}
{"kind": "append", "note_path": "[P1]", "new_facts": ["Comments must be in English."], "new_links": [], "source_ids": ["9b2c..."]}
{"kind": "update", "note_path": "[P0]", "expected_content_hash": "<copy hash from input>", "new_facts": ["Current focus is the memory subsystem."], "reason": "User clarified the focus area."}
{"kind": "link", "from": "[P3]", "to": "[P0]"}
{"kind": "supersede", "old_path": "[P2]", "new_path": "[P0]"}
{"kind": "contradict", "note_path": "[P1]", "new_claim": "User now prefers Python.", "evidence_source_ids": ["raw-123"]}
```

If nothing is worth ingesting, emit:
{"reasoning": "no durable knowledge", "ops": [], "schema_proposals": []}
"#;

/// Repair prompt for the link-contract harmony gate
/// (`enforce_link_contract`). Given the linkless `create` ops from a plan
/// plus the same related pages the planner saw, asks the LLM to either
/// supply `[P<n>]` links or explicitly declare the note isolated.
pub const PROMPT_LINK_REPAIR: &str = r#"You maintain an Aleph personal-memory wiki.
The following NEW notes are about to be written with NO links, even though
related pages exist. For EACH note, either pick 1-3 related pages it should
link to, or mark it isolated when truly nothing relates.

Rules:
- Only use `[P<n>]` tokens that appear in the "Related existing pages"
  section below — never invent a token number.
- Link a page only when a reader of one note would benefit from the other.
- Output valid JSON only, no prose, no markdown fences:

{"repairs": [{"note_index": 0, "links": ["[P2]"], "isolated": false}]}

`note_index` is the `[note <i>]` index shown for each new note below.
`links` must be empty when `isolated` is true.
"#;

/// Build the full system prompt for a batch whose rows share the given
/// raw source. Appends the Spec-1 source-aware block (RESCUE / LESSON /
/// DIGEST / RETRO) when applicable.
///
/// The suffix goes LAST, so it is the final thing the model reads: it may
/// only sharpen *what* to distil. The output contract belongs to
/// `PROMPT_COMPOUND_PLAN` alone — see the guard in
/// `compression::source_prompts`.
#[must_use]
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
