# Note Keyword Linking — Design Spec

**Date:** 2026-06-11
**Status:** Approved (brainstorming) → ready for writing-plans
**Scope:** Memory subsystem (`src/memory/notes/`), no harness changes (R10-safe)

---

## 1. Problem

Note-to-note wiki linking is supposed to run on every note creation, with the
LLM participating to connect a new note to existing notes via shared keywords.
In practice the live DB shows **17 notes / 0 links / 0 `[[ ]]` in bodies** for
agent `main` — 100% orphans — even though the running `.app` binary (built
2026-06-11 13:25) postdates the link-weaving merge `18fea96d7` (2026-06-10
21:38). So this is not a stale-binary problem; the linking logic genuinely
produces no links.

**Root cause:** the entire linking path is gated on `related` being non-empty,
and `related` comes solely from `gather_related()` — an **embedding-based**
hybrid search. When the embedding endpoint is unreachable (a documented
recurring incident) `related = ∅` → every `Create` is linkless → no links are
written. The mechanism is also **semantic-similarity** based, not
**keyword** based as intended.

The obvious clusters are visible to any reader:
- News/geopolitics: `entity/us-iran-conflict-2026` ↔ `personal/news-monitoring`
  ↔ `interests/geopolitical-monitoring` ↔ `projects/news-summary-cron` ↔
  `events/us-stock-crash-2026-06-05`
- Dreame: `projects/dreame-report` ↔ `entity/dreame`

## 2. Goal

Convert linking from an **embedding single-point dependency** to
**keyword/entity-primary, embedding-supplemented**, robust enough to link even
offline. Backfill the existing 17 notes and guarantee future creations link.

Decisions locked during brainstorming:
1. **Mechanism:** keyword/entity primary + embedding supplement (like
   Understand-Anything: deterministic where possible, LLM for semantics).
2. **Link signal:** LLM extracts an entity/keyword set per note; sets that
   overlap past a threshold get linked.
3. **Delivery:** one engine, two entrypoints (creation path + reworked
   NoteWeave dream stage).

## 3. Engine — `KeywordLinker`

Lives in `src/memory/notes/` (memory subsystem, not `src/harness/` → R10-safe).
A single-purpose unit: input a candidate note set, output link pairs.

- **Extraction (LLM — semantics belong to the model, R7/R9):** for a batch of
  notes, the LLM extracts one set of entity/topic keywords per note (3–6 each,
  drawn from title/summary/facts, not limited to `tags`). One batched LLM call,
  not one-per-note.
- **Keyword persistence:** write the keyword set into note frontmatter as a new
  `keywords:` field (human-readable, no schema migration). Benefit: creating a
  new note only requires extracting **the new note's** keywords and comparing
  against already-stored sets — no need to re-extract the whole corpus.
- **Pairing (deterministic code — mechanical belongs to the system):** two
  notes link when their keyword sets overlap past a threshold. Threshold:
  share ≥1 **specific entity** (e.g. "US-Iran conflict") OR ≥2 generic
  keywords. Tunable.
- **Embedding supplement (optional second gate):** borderline pairs get a
  cosine tiebreak; if embedding is unavailable this step is skipped and the
  pure-keyword result still stands (P7 graceful degradation).
- **Relation label:** `notes_links.relation` is filled with the connecting
  keyword/entity (so the canvas edge carries semantics, richer than a bare
  "related").

## 4. Entrypoint 1 — Creation path (`ingestor.rs`)

Rework `enforce_link_contract` from embedding-gated to **keyword-first**:
- Candidate source no longer relies solely on `gather_related` (embedding).
  Embedding available → still used; embedding down → **pull candidates via
  keyword / FTS** (existing `notes_fts`, purely local).
- New note's keywords → overlap against candidate keyword sets → emit `Link`
  op / merge into `Create.links`.
- Still routes through existing `RefTable` anti-hallucination, bidirectional
  `notes_links` writes, and `[[ ]]` body writes.

## 5. Entrypoint 2 — NoteWeave dream stage (backfill)

Rework `src/memory/dreaming/stages/note_weave.rs` (currently 409 lines,
embedding orphan detection) into **keyword-overlap relinking**: scan all notes
→ extract/read keywords → pair → write links bidirectionally. Reuses the same
`KeywordLinker` (DRY).

## 6. Triggering the backfill (17 notes, immediately)

`try_run_now()` exists and the gateway `dreaming` handler already exposes an
RPC to force a dream cycle. Flow: implement → rebuild `aleph-server` → swap the
`.app` binary and relaunch → force one dream cycle → `notes_links` populated.
**Note:** the running daemon must be rebuilt + swapped; an old daemon cannot
see the new engine.

## 7. Canvas verification (zero panel change)

Confirmed: `graph.query → get_graph_data` sources edges directly from
`notes_links`. Populate links → the canvas renders edges automatically. Use
Playwright (bootstrap-url auth) to open the panel memory/graph mode and
screenshot, verifying the 17 nodes' two clusters connect.

## 8. Success criteria (verifiable)

- ✅ The 17 notes' obvious clusters (geopolitics 5-way, Dreame 2-way) have rows
  in `notes_links` and `[[ ]]` in bodies.
- ✅ The creation path still links when **embedding is unavailable** (integration
  test simulating embedding-down).
- ✅ `KeywordLinker` overlap-algorithm unit tests + NoteWeave keyword-relink test.
- ✅ Canvas screenshot shows the connecting edges.

## 9. Decisions taken on the user's behalf (overridable)

1. Keywords stored in frontmatter `keywords:` (not a new table).
2. `relation` filled with the connecting keyword (not a bare "related").
3. Threshold ≥1 specific entity / ≥2 generic keywords.

## 10. Out of scope

- No panel/canvas code changes (edges already render from `notes_links`).
- No `src/harness/` changes (R10).
- No change to the embedding pipeline itself; it becomes a supplement, not a
  dependency.
