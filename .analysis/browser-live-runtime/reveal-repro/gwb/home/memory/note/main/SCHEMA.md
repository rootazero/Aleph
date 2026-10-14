---
schema_version: 1
updated: "2026-04-14"
---
# Memory Schema

## Domain
Aleph personal memory — general purpose.

## Categories (fixed by Aleph)
preference | plan | learning | project | personal | tool | lesson | skill | reference | other
Special: synthesis (weekly dream output). skill/reference have extra frontmatter — see NOTES.md §3.

## Tag Taxonomy
<!-- LLM maintained. New tag MUST appear here before use. -->
- rust
- async
- memory
- tooling

## Page Thresholds
- create: a topic appearing in 2+ sources, or centrally in one source.
- append: an existing page already covers the topic.
- contradict: new content conflicts with an existing claim — mark instead of silent overwrite.

## Update Policy
- Conflict → keep both claims with dates and sources; frontmatter `contradictions: [path]`.
- Supersede → append `## Superseded by [[path]] (YYYY-MM-DD)` to the old page.
- New tags → add to Tag Taxonomy first, then use.
