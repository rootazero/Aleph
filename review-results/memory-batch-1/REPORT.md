# Memory Batch 1 — `src/memory/notes/*` Code Review

**Date**: 2026-08-12
**Path**: `src/memory/notes/*` (59 files, ~19 172 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    4 |     8 |    5 |   17 |

---

## Findings

### [HIGH] `notes/note/helpers.rs:144-170` — `sanitize_title` collapses `..` to empty, then accepts a single dot — silently kills `..foo.md` and `..` itself
- **Category**: security / path traversal
- **Description**: `replace("..", "")` is order-dependent and lossy. The first pass strips `..`, the result may be empty or `.`, and the trailing `cleaned.chars().all(|c| c == '.' || c.is_whitespace())` check does catch an all-dots residue — so the all-`.` branch is safe — but the deletion of `..` is not idempotent. `....` (four dots) collapses to `''` and the title is rejected, but `..foo` collapses to `foo`, which is silently different from a legitimate `..foo` title. The same title becomes `foo` and collides with an existing `foo.md`. The only path-traversal defense is the all-dots check; the `..` strip is decorative.
- **Suggested fix**: Reject any title containing `..` *before* stripping, and drop the lossy `replace("..", "")` entirely. A title that mentions `..` is always an LLM mistake or an attack; the operator should re-prompt. Keep the other character replacements (path separators, null, etc.) — those are legitimately the kind of value a sanitiser removes.

### [HIGH] `notes/store.rs:739-758, 800-826, 1100-1117` — `add_link_with_relation` / index writes hold `Mutex<Connection>` across `await`-ed `note_path_resolver` calls
- **Category**: race condition / deadlock
- **Description**: `index_note` and friends take the connection mutex, then call `links::resolve()` (a sync fn) and `super::helpers::build_resolve_context` (a sync fn) — but the function is `async fn` and `lock_conn!` returns the sync guard into the async body, so a future `await` inside the same function (e.g. an embed call added in a future PR) would hold the mutex across an await point. The current code happens to be all-sync between `lock_conn!` and the function end, but the trait is `async_trait`, so the door is one careless future patch away from a deadlock.
- **Suggested fix**: At the top of every body that uses `lock_conn!`, do all the `links::resolve()` / `super::helpers::*` work *before* `lock_conn!(self)?`, collect the result into owned structs, then take the lock only for the SQL execution. The pattern is already in place in the function's later `links` loop — apply it to the `desired` and `existing` build too.

### [HIGH] `notes/watcher.rs:175-260` — vault watcher debouncer uses unbounded channel; `MAX_PATHS_PER_BATCH = 128` cap races a `git checkout`
- **Category**: DoS
- **Description**: A `git checkout` that swaps hundreds of files can fire thousands of debounced events. The code's defence is to escalate to `reconcile_corpus` per agent when `keys.len() > MAX_PATHS_PER_BATCH`, but the unbounded `mpsc` carries the full settled `Vec<PathBuf>` per tick, and a sustained sync (a `git pull` + rebuild) can deliver a message every `DEBOUNCE_MS = 750ms` indefinitely. Each message allocates a fresh `Vec<PathBuf>`, sorts and dedupes it. The classify pass then walks `parts: Vec<&str>` per path. Net cost is dominated by the allocations, not the actual reconcile work, and they grow with vault size, not sync size.
- **Suggested fix**: Bound the per-message payload: if `paths.len() > MAX_PATHS_PER_BATCH` *before* sort/dedup, switch the whole batch to "reconcile every corpus touched" and drop the per-path Vec after one classification. Today's code does this AFTER classify, so the worst case (a 10k-file `git checkout`) still allocates a 10k `Vec<PathBuf>` per tick.

### [HIGH] `notes/indexer.rs:1000-1060` — `relink_unresolved` and `prune_orphan_vectors` have no batch limit; `O(n)` SQL round-trips
- **Category**: DoS
- **Description**: `prune_orphan_vectors` collects *every* orphan rowid into a `Vec<i64>` then loops `vec::all_notes_vec_tables()` (currently 5 dimensions), executing one `DELETE` per `(rowid, table)` pair. A 50k-note vault with all dimensions in use = 50k × 5 = 250k round-tripped `DELETE`s in a single transaction, holding the connection mutex the whole time. Concurrent recall calls block.
- **Suggested fix**: Either (a) cap the per-transaction batch (commit every N deletes) or (b) use a single multi-table `DELETE FROM notes_vec_{dim} WHERE rowid IN (...)` per dimension with bind params. (b) is the right shape — sqlite supports `IN` with up to 32k binds before complaining, and a 5k batch is one round-trip per dimension.

### [MEDIUM] `notes/ingest/apply.rs:30-35` — `static LazyLock<Regex>` for the page-op separator is unanchored and unbounded
- **Category**: quality / DoS
- **Description**: The regex parses LLM-authored page-ops. There is no max-length guard on the input string. The trait is `#[async_trait]`; a large response (a 1 MB model reply) reaches `apply()` and is regex-scanned character by character. The regex is anchored (`^` and `$`) but the input is not size-bounded, so a hostile model can spend many CPU seconds on a single response.
- **Suggested fix**: Bound the input at the caller (`parse_plan` already gets the LLM text) with a `MAX_INGEST_INPUT_BYTES` constant. Reject early.

### [MEDIUM] `notes/graph/community.rs:75-90` — Louvain-style community loop has a `loop {}` with no iteration cap
- **Category**: logic
- **Description**: The community-detection loop is `loop { ... break; }` with the break condition being `improvement < threshold`. For a graph that never converges (a near-bipartite citation graph) the loop will run until the caller's `notes` budget is exhausted. The owning stage (`GraphRecomputeStage`) does have a `MAX_PASSES` ceiling — but the function is also used by the API surface (`community_ids` -> ingestion) without that ceiling.
- **Suggested fix**: Add an explicit iteration cap inside the loop, e.g. `for _ in 0..MAX_Louvain_ITERS` with the existing `improvement` break preserved. The cap belongs to the function, not to its caller.

### [MEDIUM] `notes/governance/supersession.rs:17-21` — supersession regex compiled via `unwrap()` on `LazyLock` first call
- **Category**: quality
- **Description**: `static SUPERSEDED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(...).unwrap())`. A bad pattern would crash the process on first use; the pattern is correct, but `unwrap()` in a static is a code-smell. A typo during refactor surfaces as a startup-time crash instead of a test failure.
- **Suggested fix**: Use `Regex::new(...).expect("static supersession pattern is verified by tests")` and add a test that matches the exact pattern string. Cheaper than chasing the panic in production.

### [MEDIUM] `notes/links/mentions.rs:60` — `body_norm[from..].find(name_norm)` quadratic on dense link graphs
- **Category**: performance
- **Description**: For every (note, mention) pair the search restarts from `from` in the body. A 100-line note with 50 mention candidates is `O(L * N)` char-compares. With the `Vec<&str>` candidate list built from the wikilink scanner this becomes 5 000 substring searches per note.
- **Suggested fix**: Use the Aho-Corasick multi-pattern matcher already in the workspace (`aho-corasick` is a transitive dep) for the per-name scan, or fold the names into one regex and let the existing RE engine handle the alternation. A single `RegexSet` over all candidate names is sub-linear in the number of names and linear in body length.

### [MEDIUM] `notes/indexer.rs:435-470` — `full_rebuild` reads every file serially with `fs::read_to_string` even when `set.join_next` is used
- **Category**: performance
- **Description**: The function uses a `JoinSet` to fan out, but the closure is the per-file `index_one_file` which itself does the file read; the dispatcher has no parallelism on the file read. tokio's async `read_to_string` on a 100k-note vault holds the runtime's I/O budget for many seconds, starving the rest of the system. The "concurrent" claim in the doc-comment is half true: the indexer-level work is concurrent, but each indexer still serialises the file read + parse.
- **Suggested fix**: Use `tokio::task::spawn_blocking` (or `rayon` over a `par_iter`) for the I/O+parse leg, and only the SQL round-trip stays on the async runtime. Same total work, but the runtime is no longer the bottleneck.

### [MEDIUM] `notes/store.rs:200-220` — `note_md_filename` is computed on every read; `get_note_index` calls `strip_md_ext` on every path
- **Category**: performance
- **Description**: Every reader (`note_content_path`, `get_note_index`, `find_by_filename`, the watcher) calls `strip_md_ext` on a path that has already been validated at the write chokepoint. Stripping again on read is pure overhead, and the function is `&str`-to-`&str` so it cannot return a fresh allocation — fine — but the lookups happen thousands of times per session.
- **Suggested fix**: Either store titles extensionless and trust the index (i.e. the invariant that `note.title` has no `.md` suffix), or precompute the stripped form when the `NoteIndexEntry` is materialised. The current double-check is a defensive belt over suspenders that adds a `str::strip_suffix` per call.

### [MEDIUM] `notes/orientation/log_md.rs:222` — `read_dir` loop has no error handling for entries that vanish between `next_entry` and the `metadata()` call
- **Category**: logic
- **Description**: An `iter` that returns `Err(_)` mid-walk is silently dropped. For a `git checkout` on the orientation dir, the `metadata()` of a freshly removed file is `NotFound`; the current code path treats that as "skip" so the `Ok(_)` counter is wrong, not catastrophic, but the audit log says nothing about the missing file.
- **Suggested fix**: Distinguish "file gone" from "stat failed" and emit `tracing::debug!` for the former so an operator can correlate orientation drift.

### [MEDIUM] `notes/ingest/ingestor/mod.rs:340-380` — `helpers.rs:329, 367, 374` — `expect()` on the LLM-replay plan and `Create` gate
- **Category**: logic
- **Description**: These three `expect`s are guarded by an upstream `match`, but the surrounding function takes a `&str` candidate path; an LLM can emit a `Contradict` referencing a path the model "should" have created, and the `expect("Contradict must be gated")` panics. The replay code is meant to be deterministic; a panic at this site aborts the dream cycle. The plan parser should `return Err` on the inconsistent case and let the dream stage report `DistillOutcome::Error` with the actual reason.
- **Suggested fix**: Replace the `expect`s with `?` returning a typed `ApplyError::PlanInconsistent`. The stage-level error path already exists; the plan is just failing open into a panic.

### [LOW] `notes/wikilink.rs:5` — `use regex::Regex;` is fine, but the wikilink extractor in `extract_wikilinks_with_alias` is hand-rolled state machine vs. using `regex`/`once_cell` patterns already in the project
- **Category**: architecture
- **Description**: Hand-rolled state machines drift. A new wikilink form (`[[path|alias|extra]]`) requires editing the parser *and* the resolver; today the two are synchronised only by tests.
- **Suggested fix**: Hoist the hand-rolled parser to a `nom` or `pest` grammar; the same `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-assembler-design.md` lineage is the right place to document the grammar.

### [LOW] `notes/indexer.rs:999` — `entries.next_entry()` loop ignores `Err(_)` from `walkdir`
- **Category**: quality
- **Description**: A `walkdir` mid-walk error is dropped. For a permissions failure on a subdirectory, the rebuild silently indexes a partial vault.
- **Suggested fix**: `if let Err(e) = entry { tracing::warn!(...); continue; }`. Already done elsewhere; this site is the outlier.

### [LOW] `notes/note/mod.rs:441` — `while let Some(last) = lines.last()` on a `Vec<String>` is O(1) per step but the surrounding `pop`/`push` pattern is not guarded against an empty input
- **Category**: logic
- **Description**: The loop relies on an upstream guard, but a hand-edited note with no body lines still triggers the `while let Some(last) = lines.last()`. The body short-circuits via the inner `if`, so the net effect is a no-op, but the pattern is fragile.
- **Suggested fix**: Replace `while let Some(last)` with `for last in lines.iter().rev().take(N)`; an empty `lines` is then a single `next()` returning `None`, not a separate code path.

### [LOW] `notes/keyword_linker/extract.rs` — TF-IDF style scoring is hand-rolled; an LLM-friendly extract of "shared tokens" would do better and match R7
- **Category**: architecture
- **Description**: The keyword linker computes token overlap. The LLM already reads the source body and the candidate body; emitting a "related" relation with `confidence: 0.5` and "shared tokens: ..." does not need a heuristic at all.
- **Suggested fix**: Note for the next re-architecture pass; not blocking.

### [LOW] `notes/graph/relevance.rs:1-200` — `struct RelevanceScorer` has 8 fields and no constructor
- **Category**: quality
- **Description**: The struct is built by struct-literal at every call site. The `Default` impl is correct, but the call sites do not use it.
- **Suggested fix**: Add `RelevanceScorer::new()` and route the four call sites through it. Pure refactor; no behavior change.

## Cross-References

- `notes/watcher.rs:175-260` and `notes/indexer.rs:435-470` — both consume the note vault. A `git checkout`-style burst hits the watcher first (it escalates to `reconcile_corpus`) and the full-rebuild path second; the two share no rate-limit. The single chokepoint is missing.
- `notes/store.rs:200-220` `note_md_filename` and `notes/note/parsing.rs:267` `and_hms_opt` both normalise filenames. The `strip_md_ext` invariant is checked at write but the read path also calls it; if the two ever drift (e.g. a `+ .md.tmp` atomic write is left on disk) the read path silently mis-reads.
- `notes/wikilink.rs` and `notes/links/resolve.rs` — the hand-rolled extractor and the resolver are not symmetric. New wikilink forms added to one must be hand-synced in the other.
- `notes/ingest/apply.rs:30-35` and `notes/ingest/ingestor/batch.rs` — both run regex/LLM-pipeline work. A per-call input cap is the right place to bound model spend.

## Strengths

- `note/helpers.rs` `yaml_scalar` and `yaml_inline_array` are the right shape — they preserve round-trip for every reserved YAML character class. The drop-passthrough-on-serialize-failure fallback (`yaml_extra_block`) is exactly the kind of last-resort guard that prevents a single bad frontmatter from bricking a vault.
- `note/parsing.rs` distinguishes `KnowledgeNote::is_permanent()` from the frontmatter `permanent: true` and the `permanent` / `pinned` tags — three encodings for the same fact, all merged. That is R7 done right.
- `indexer.rs` `index_one_file` skips on hash match, so Aleph's own writes never re-trigger a self-index. The bulk path `reconcile_corpus` is the right shape for `git checkout` scale; the per-file path is the right shape for the editor case.
- `watcher.rs` classifies only `{root}/{agent_id}/{category}/*.md` and explicitly excludes `archive/`, dot-directories, and atomic-write staging files. The boundary is sharp; what the watcher does *not* index is as well-defined as what it does.
- `governance/gate.rs` `DefaultNoteWriteGate` returns a typed `GateOutcome`; the consumer (ingest) cannot accidentally bypass it.
- `link_surface.rs` (referenced in `note_retrieval::relation_surface`) carries typed relations forward; the structural-strong `superseded_by` edges are correctly force-surfaced.
