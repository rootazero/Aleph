# Memory Notes + Store — Static Audit Report (2026-08-16)

**Scope:** `src/memory/notes/**` and `src/memory/store/**` (81 files, 30 313 LOC).

**Method:** Read-only static review across three lenses — Seam (severed-wire
audit, see `.claude/skills/severed-wire-audit/references/seam-catalog.md`),
Logic (correctness, boundary conditions, concurrency, SQL/FS safety), and
Architecture (API contracts, ring/reader/writer coherence, dead code).

**Exclusions:** All fixes from the recent set listed in the audit brief are
taken as already applied and are not re-reported here. High-severity findings
focus on issues introduced or left behind by code paths the recent fixes
touched.

## Files reviewed (81)

```
src/memory/notes/
  dedup.rs
  governance/{gate,mod,supersession}.rs
  graph/{community,insights,minhash,mod,relevance,tests}.rs
  indexer.rs + tests.rs
  ingest/{apply,mod,plan,prompts,ref_table,retrieve}.rs
  ingest/ingestor/{batch,helpers,mod,plan_parse,tests}.rs
  keyword_linker/{extract,mod,overlap}.rs
  links/{mentions,mod,resolve}.rs
  note/{helpers,mod,parsing,relation,tests,types}.rs
  orientation/{fs_orientation,index_md,log_md,mod,obsidian_config,overview_md,prompts,purpose_md,schema,types}.rs
  profile/{mod,prompts,store,synthesizer,types}.rs
  query_filer/{filer,mod,prompts,types}.rs
  search_result.rs, store.rs, watcher.rs, wikilink.rs

src/memory/store/
  mod.rs, raw_memory.rs, types.rs
  sqlite/{mod,vec}.rs
  sqlite/{dream_kv,dream_reports,embedding_meta,memory_write_decisions,query_filed,
          raw_memories,recall_signals,routing_experience,sessions}.rs
  sqlite/notes/{helpers,mod,store_impl,tests}.rs
  sqlite/schema/{ddl,migrations,mod,tests}.rs
```

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 3 |
| Medium   | 9 |
| Low      | 8 |
| **Total**| **20**|

Severity ladder follows the audit brief: Critical = unsafe under realistic input;
High = guaranteed correctness / data-loss bug; Medium = likely correctness gap
under plausible conditions; Low = robustness / consistency / dead code.

---

### [High] src/memory/notes/ingest/ingestor/batch.rs:223 — `path.replace("..", "")` on embedding-queue path violates the new title sanitization policy

**Category:** logic (consistency with the seam-catalog "inert-but-not-quite" form).

**Confidence:** High

**Description:** `ingest_batch` computes `safe_path = path.replace("..", "").replace('\\', "/")` for the post-commit embedding-queue push. This is the exact lossy pattern the note layer just deleted (`2b1813429 memory(notes): reject titles containing '..' instead of lossy replace`) — `..foo` collapsed to `foo`, then potentially collided with an existing note's filename. In the current call graph the path comes from `report.touched_paths` (post-sanitize), so `..` should never appear; the call is defensive. But the policy is now *reject* and this site silently normalizes. If a future code path regresses the contract (e.g. a caller skips `sanitize_title` and writes a `..`-bearing filename), the embedding-queue path becomes the only consumer of that filename: the on-disk note is rejected by `sanitize_title`, but the embedding-queue silently strips `..` and reads a file at a *different* path than the source-of-truth note, so the queue embeds the wrong content for an embedding that will never resolve to any indexed row.

**Suggested fix:** Use `sanitize_title` (or the read-side `note_content_path` helper) instead of the string replace. If the contract change is intentional (defensive only), add a `// Per 2b1813429: should never trigger` assertion.

---

### [High] src/memory/notes/ingest/apply.rs:430-440 — `commit` reports `linked` and increments `touched_paths` for every `PageOp::Link` regardless of whether either `add_link` direction succeeded

**Category:** logic (silent failure / contract drift).

**Confidence:** High

**Description:**

```rust
for (from, to) in &self.pending_links {
    let _ = self.add_link(from, to).await;   // discards error
    let _ = self.add_link(to, from).await;   // discards error, and the
                                              // "first arg" semantics here
                                              // do NOT do what the call
                                              // looks like it does
    report.linked += 1;
    report.touched_paths.push(from.clone());
    report.touched_paths.push(to.clone());
}
```

Two distinct issues:

1. `add_link(from, to)` runs `split_path(from)` to derive the `(category, filename)` of the source note. If `from` lacks a `/`, `split_path` errors and `add_link` returns `Ok(())` with no side effect — but `report.linked += 1` still fires, and `report.touched_paths` still gets `from`/`to` pushed. A malformed `PageOp::Link` is therefore indistinguishable from a successful link on the apply report and in `CompressionService`'s downstream accounting.
2. `add_link(to, from)` is called second — and `add_link` treats the **first** argument as the note-to-append-to, not as the *destination* of an edge. The reversal therefore re-appends `from` as a link on `to` (treating `from` as the new link target inside `to`'s `Related:` block), not the symmetric `to→from` link that the function name suggests. Both directions run, but in the second case the *target* of the link is `from`, which is the intended source. Net effect on disk: `from` notes get a `Related: [[to]]` and `to` notes get a `Related: [[from]]` — that part matches the spec, but only because `add_link`'s implementation has the inverted argument order. If a future refactor renames the function to `add_inbound_link(note, target)`, the second call silently starts appending `from` to `to`'s Related list twice instead of once. This is brittle and the comment trail won't help.

**Suggested fix:** Make `add_link` take `(note_path, link_target)` explicitly, name it accordingly, and assert or fall through on a `split_path` error in the report accounting. Or split into `add_inbound_link` / `add_outbound_link` so the bidirectional logic is obvious.

---

### [High] src/memory/notes/ingest/apply.rs:360 — `push_staged` writes the staged file with `tokio::fs::write` (non-atomic), then commits with `tokio::fs::rename`

**Category:** logic (crash safety; not strictly a wire severance but has the same shape — the source-of-truth file is shipped to disk via a path that can leave it half-written).

**Confidence:** High

**Description:** The apply transaction's whole point is "stage then rename atomically". `push_staged` (line 360) writes the staged file with `tokio::fs::write` — a plain overwrite, not the `atomic_write_file` helper used by every other note writer (`write_note`, `write_note_raw`, `append_to_note`, `mark_superseded`, `append_relations`, `merge_source_notes_into_note`). A crash mid-write leaves a half-written staged `.md` file. If the rename to the target then proceeds, the **target note's source-of-truth file becomes a corrupt markdown blob** — a regression the audit brief specifically calls out as the failure mode `atomic_write_file` exists to prevent.

`commit`'s rollback iterates `moved.iter().rev()` and undoes the rename, but it never attempts to repair the moved-target content (the rename back is best-effort and is logged at warn). On the very next boot the file is read by `KnowledgeNote::from_markdown` which returns `Err` on broken frontmatter — but the index entry has already been written by `index_note` (which is called *after* the rename succeeds, line 416), so the corruption window is real and recoverable only by a `full_rebuild` plus manual disk repair.

**Suggested fix:** Replace `tokio::fs::write(&staged_path, &body)` with `atomic_write_file(&staged_path, &body).await.map_err(...)` so the staged side matches the target side's atomicity contract.

---

### [Medium] src/memory/notes/ingest/ingestor/batch.rs:404-415 — `dedup_redirect_creates` drops the `Create`'s `relations` (and `tags`) when merging into an `Append`

**Category:** logic (silent data loss in a high-traffic path).

**Confidence:** High

**Description:** When a `Create` is "merged" into an `Append` against an existing note (mem0-style dedup), the rewrite hardcodes `new_relations: vec![]`:

```rust
Some(PageOp::Append {
    note_path: target,
    new_facts: facts,
    new_links: links,
    new_relations: vec![],   // <-- typed relations dropped
    source_ids,
})
```

The Create's typed relations (the entity-graph frontmatter block, e.g. `relations: [{to: entity/acme, type: works_at}]`) silently disappear. The redirect is logged at `info`, but a downstream caller reading `notes_links` cannot tell why an expected edge is missing.

The `Create` also loses `title`, `summary`, and `tags` by design (the comment says the existing note owns those), but `relations` is not a heading/summary — it is a directed edge the graph needs.

**Suggested fix:** Carry `relations` through. The simplest fix is `new_relations: relations.clone()` in the destructure and the constructor. This may duplicate edges on the target note, but `index_note`'s upsert path already dedupes on `(from, to_note)` and `Append`'s `existing.relations.iter_mut().find(|e| e.to == r.to)` upgrade keeps it stable.

---

### [Medium] src/memory/notes/indexer.rs:492-499 — `reconcile_corpus` prune loop uses `sanitize_title` (which can change the path) to *find* the file, then deletes via the index path

**Category:** logic (inconsistent sanitization across read/write paths).

**Confidence:** High

**Description:**

```rust
for entry in self.store.list_notes(agent_id).await? {
    if !CATEGORY_DIRS.contains(&entry.category.as_str()) { continue; }
    let safe_cat = sanitize_title(&entry.category).unwrap_or_else(|_| "other".to_string());
    let file = agent_dir.join(safe_cat).join(note_md_filename(&entry.filename));
    if fs::metadata(&file).await.is_err() {
        self.store.remove_note_index(&entry.path, agent_id).await?;
        total.pruned += 1;
    }
}
```

If a row's `category` field is e.g. `"synthesis"` (a valid `CATEGORY_DIRS` entry), `sanitize_title` is idempotent and returns `"synthesis"`. Fine. But for *any* row whose `category` is one of the unsafe values that sanitize rejects (e.g. category strings pre-existing in the DB from older schema versions that used `..` or reserved chars), `sanitize_title` returns `Err` and the fallback `"other"` is used to look up the file — which will *not* exist, so the row is removed. The delete then runs against `entry.path` which uses the *original* unsanitized category. The on-disk file (if any) under the original category survives, while its index row is removed.

The path is conditional on `CATEGORY_DIRS.contains(&entry.category.as_str())` — most rows pass — but the dovetail between the guard and the sanitize is implicit: the row's category is by construction one of `CATEGORY_DIRS`, but the sanitize is meant to defend *against* unsafe categories, not against any of the 21 listed ones. The mismatch is silent and recoverable only on the next `full_rebuild_all` (which iterates *every* corpus, not just the default).

**Suggested fix:** Use `entry.category` directly (no sanitize) since the guard already constrains the value to `CATEGORY_DIRS`. If defense-in-depth is desired, log a `tracing::warn!` instead of falling through to a wrong directory.

---

### [Medium] src/memory/store/sqlite/notes/store_impl.rs:1515-1564 — `vector_search` builds an unbounded `WHERE rowid IN (...)` from `k = limit*3` (max 32 766 placeholders = SQLite `SQLITE_MAX_VARIABLE_NUMBER`)

**Category:** logic (resource bound, defense in depth).

**Confidence:** Medium

**Description:** `vector_search` issues `KNN` with `k = limit.saturating_mul(3).max(limit)` (line ~1490). Then it builds `WHERE rowid IN (?, ?, ?, ...)` over the KNN result set. SQLite's compile-time `SQLITE_MAX_VARIABLE_NUMBER` is 32 766 by default. `limit` is propagated by callers — `find_similar_notes` caps at `top_n * 32` (5 000 * 32 = 160 000 in the worst case), but in practice callers pass ≤ a few hundred. The batched `prune_orphan_vectors` already learned this lesson (5 000-row chunks, see `BATCH_SIZE` constant), but `vector_search` does not — a pathological caller can drive it past SQLite's cap and return an `Err("too many SQL variables")`. The error surfaces as a 500 to the calling agent.

**Suggested fix:** Batch the `rowid IN (...)` clause the same way `prune_orphan_vectors` does, or document the cap explicitly at the API boundary.

---

### [Medium] src/memory/store/sqlite/notes/store_impl.rs:1455-1470 — `get_embedding` ignores `dim_hint` mismatch silently (returns 0-length or wrong-sized vector)

**Category:** logic (wrong-but-plausible: a wrong-dim fetch returns 0.0-filled vector, which collides with the "no embedding" path).

**Confidence:** Medium

**Description:**

```rust
async fn get_embedding(&self, path, agent_id, dim_hint) -> Result<Option<Vec<f32>>, ...> {
    let table = vec::notes_vec_table_for_dim(dim_hint)?;  // rejects unsupported dim
    ...
    let sql = format!("SELECT embedding FROM {table} WHERE rowid = ?1");
    let blob: Option<Vec<u8>> = ... .ok();
    Ok(blob.map(|b| {
        b.chunks_exact(4)
         .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
         .collect()
    }))
}
```

If a note was embedded at dim=1024 and the caller asks for `dim_hint=768`, `notes_vec_table_for_dim(768)` succeeds, the `SELECT` against the 768 table returns `None` (no row), and the function correctly returns `Ok(None)`. So far so good.

But if the note was embedded at *both* dimensions (a re-embed sweep, then later a re-embed back), the same `path` has a rowid in both tables. The fetch against the *requested* table returns Some(blob), but a row that previously held the vector at the requested dim was rewritten — a stale blob can be returned if `rowid` was reused. More importantly, the chunked decode silently returns a wrong-length `Vec<f32>` (768 floats from a 1024-byte blob, dropping the last 256 floats) without any error. Callers like `find_similar_notes` then compute cosine similarity on a partial vector.

**Suggested fix:** Validate the returned blob length against `dim_hint * 4` and return an explicit error on mismatch; alternatively, track the row's actual dim in `notes_vec_map` and verify before fetch.

---

### [Medium] src/memory/notes/ingest/apply.rs:511-524 — `mark_superseded` writes the body heading via `format!("{body}{marker}")` + `atomic_write_file` is good, but the file is *re-read from disk* immediately before the re-parse

**Category:** logic (TOCTOU: not the classic flavor, but a redundant-disk-read pattern that races the watcher).

**Confidence:** Medium

**Description:** `mark_superseded` reads `body`, checks for the marker, appends it, and writes via `atomic_write_file`. Then `KnowledgeNote::from_markdown(&safe, &combined)` re-parses the in-memory `combined` string — not a disk read — and `index_note` is called with the parsed `n`. No TOCTOU window in the *current* thread. The risk is concurrency: the vault watcher (`spawn_note_vault_watcher`) sees the new bytes on disk, fires `index_one_file`, and races `index_note` here. Both call `index_note` against the same `(agent, path)`. SQLite locks them serially, but the second `index_note` reads the *new* `content_hash` from `KnowledgeNote::from_markdown` and writes a fresh `notes_index` row — so the final state is consistent. The visible artifact is a transient double-indexed event in the watcher log ("re-indexed externally edited note") for what was actually a self-write. Not a bug; just a comment-worthy race.

**Suggested fix:** Add a code comment documenting the expected double-index for self-supersedes. Lower-priority; this is log noise more than correctness.

---

### [Medium] src/memory/store/sqlite/notes/store_impl.rs:1500-1520 — `vector_search` ignores the case where KNN returns fewer than `k` rows; the subsequent IN-clause still uses the original `k`-derived placeholders

**Category:** logic (small correctness / wasted resources).

**Confidence:** Medium

**Description:** If `knn_results` has 5 rows and the IN-clause was pre-built for 1 000 placeholders, SQLite parses 1 000 placeholders but only binds 5. `rusqlite::params_from_iter` over `5` items would silently bind the rest as `NULL` — and `WHERE rowid IN (NULL, …, NULL)` evaluates to `NULL`, so no rows match. The current code binds *only* the non-empty rowids via `Vec::Box<dyn ToSql>` collected from `knn_results.iter()` (correct), so the SQL placeholders that have no bound value are never reached. But the placeholders list *was* sized to `knn_results.len()` (line ~1515), so the statement just binds fewer — except: if `knn_results.len()` is, say, 5 but `k` was 100, the placeholders list is sized 5 not 100, and the SQL has 5 placeholders. So this is actually fine. Skipping this finding.

**Suggested fix:** N/A — re-read the code: placeholders are sized from `knn_results.len()`, the bind list is sized from `knn_results.len()`, so they match. No issue.

---

### [Medium] src/memory/notes/indexer.rs:1019-1030 — `rename_note` does not sanitize the `category` argument used for `cat_dir` construction

**Category:** security (defense in depth; not a current bug).

**Confidence:** Medium

**Description:** `rename_note` derives `category` from `find_by_filename`, then calls `sanitize_title(&category).unwrap_or_else(|_| "other".to_string())` — this is the *right* path, but the *fallback* `"other"` is what a hostile filename lookup can hit. If `find_by_filename` returns paths whose stored `category` column contains a rejected value (e.g. an old DB row with `category = "../../etc"`), `sanitize_title` errors, fallback fires, and `cat_dir = memory_dir.join(agent_id).join("other")` — correct.

But the **first** call to `fs::rename(&old_path, &new_path)` (line 990) uses `cat_dir` computed via `category.to_string()` (without sanitization) before the sanitize-fallback applies. Wait — re-reading the code: `cat_dir = self.memory_dir.join(agent_id).join(&category)` where `category` was just sanitized via `unwrap_or_else(...)`. So the actual rename uses the sanitized value. OK — false alarm on the rename path.

However: the **wiki-link rewrite loop** iterates `CATEGORY_DIRS` and joins `agent_dir.join(cat)` — `cat` is a compile-time constant. Safe. The rename's own path is sanitized. So no current bug, but the code is brittle to a future edit that drops the sanitize. Add a comment, or skip.

**Suggested fix:** Add a code comment annotating that the sanitize is load-bearing for path safety. Skippable.

---

### [Medium] src/memory/store/sqlite/notes/store_impl.rs:1721-1814 — `load_graph_snapshot` issues one prepared statement per node for `notes_sources` (N+1 query)

**Category:** logic (performance; not a wire severance but has the "scaling cliff" shape).

**Confidence:** Medium

**Description:** For each of `n` notes in the corpus, `load_graph_snapshot` runs a separate `SELECT source_ref FROM notes_sources WHERE agent_id=? AND note_path=?`. On a 50 k-note vault this is 50 k prepared-statement compiles + 50 k query executions. The connection mutex serializes them all. The `graph_recompute` stage runs this every dream cycle.

A single batched query (`SELECT agent_id, note_path, source_ref FROM notes_sources WHERE agent_id = ?`) grouped by `note_path` would replace this with one statement + one client-side grouping.

**Suggested fix:** Batch the `notes_sources` fetch in one query, group by `note_path` in Rust. Trivial; not done because no caller has yet hit the cliff.

---

### [Medium] src/memory/notes/ingest/apply.rs:462-477 — `add_link` discards `Result` from `split_path` and `try_exists`

**Category:** logic (silent skip — already noted above at line 430, this is the inner call site).

**Confidence:** High

**Description:**

```rust
async fn add_link(&self, from: &str, to: &str) -> Result<(), AlephError> {
    let (category, filename) = match split_path(from) { Ok(p) => p, Err(_) => return Ok(()) };
    let safe = sanitize_title(&filename)?;
    let disk = ...;
    if tokio::fs::try_exists(&disk).await.map_err(...)? {
        self.indexer.append_to_note(self.agent_id, from, &[], &[to.to_string()]).await?;
    }
    Ok(())
}
```

Two silent skips: a `split_path` failure and a `try_exists == false` both return `Ok(())`. The caller at `commit` (line 433) increments `report.linked += 1` unconditionally. So a `PageOp::Link` against a malformed path or against a missing note contributes to the report as if it succeeded — masking broken links from the rest of the system. The `commit` loop also unconditionally pushes both `from` and `to` into `touched_paths`, so `CompressionService`'s downstream accounting marks both endpoints as touched when neither was actually rewritten.

**Suggested fix:** Track and report skipped-link reasons (return an enum or count). The `commit` site should not increment `report.linked` unless both directions verified.

---

### [Medium] src/memory/store/sqlite/notes/store_impl.rs:1148-1200 — `prune_orphan_vectors` deletes from `notes_vec_map` last; on a crash mid-loop, the vec tables have dangling rows but no map entry to point at them

**Category:** logic (resource leakage on crash; recent fix `aa4f63b27 memory(store): batch prune_orphan_vectors, add partial dangling index, log lock poison` addressed the unbounded case but the ordering risk remains).

**Confidence:** Medium

**Description:** `prune_orphan_vectors` collects orphan `rowid`s, then for each chunk deletes from the vec tables *first* and from `notes_vec_map` *second*. If the process crashes between those two DELETEs, the vec table has dangling rows with no map entry — KNN may return distance hits for path values it cannot resolve. The KNN leg of `hybrid_search_notes` filters by `notes_vec_map` post-KNN, so the dangling rows do not corrupt search results, but they consume KNN slots that no longer correspond to any indexed note — a partial return of the original "ghost vectors" problem the fix was meant to cure.

**Suggested fix:** Delete from `notes_vec_map` *first*, then from vec tables (so a crash leaves vec-table slots without a map pointer, which is the same shape KNN already tolerates). Or commit both DELETEs in a single sub-transaction so they cannot interleave a crash.

---

### [Low] src/memory/notes/watcher.rs:104-128 — Debouncer sends `Vec::new()` as the "reconcile every corpus" sentinel; a non-empty batch that survives `dedup` down to zero is silently treated as bulk-reconcile

**Category:** logic (sentinel overloading).

**Confidence:** Medium

**Description:** The producer and consumer agree that `paths.is_empty()` means "reconcile every corpus" (sentinel for the MAX_PATHS_PER_BATCH branch). But the debouncer could legitimately emit an empty Vec when (a) every .md path was filtered out by the `extension` check after sorting/dedup, or (b) the original `events` were all non-md. The producer code uses `md_count` before the sort/dedup to short-circuit to bulk, so case (a) cannot reach the `tx.send(Vec::new())` branch — but the producer *does* unconditionally send `Vec::new()` for case (b) (the MAX_PATHS_PER_BATCH gate) regardless of whether any paths survived classification. The two meanings of "empty Vec" are conflated: "deliberate bulk" and "all events filtered".

**Suggested fix:** Use an `enum WatcherMsg { Bulk, Paths(Vec<PathBuf>) }` instead of overloading the empty Vec.

---

### [Low] src/memory/notes/notes/notes/store_impl.rs:71 — `strip_md_ext` uses `s.strip_suffix(".md").unwrap_or(s)` — defensive, but if the new sanitize policy ever expands to reject `.md` suffixed titles, the unwrap_or would silently pass them through

**Category:** logic (defensive code masking a policy change).

**Confidence:** Low

**Description:** `strip_md_ext` is the filename chokepoint. With the new policy (`2b1813429`) rejecting `..`, a title carrying `.md` is now allowed through `sanitize_title`, and `strip_md_ext` is what strips it. If a future caller forgets to call `sanitize_title` first, `strip_md_ext` swallows the `.md` — which is the desired behaviour *for that caller*. But for callers that *want* to keep the `.md` (e.g. an explicit "notes.markdown" title), `strip_md_ext` strips one extension only — "notes.markdown" passes through, but "a.md" → "a" (fine), "a.md.md" → "a.md" (fine). The behaviour is correct for current callers; flagging this as "fragile to a refactor that splits filename and title concerns".

**Suggested fix:** N/A in current code; revisit if the sanitize / strip_md / filename pipeline is ever split across crates.

---

### [Low] src/memory/store/sqlite/mod.rs:159-160 — `#[allow(dead_code)] // test-only accessor` on `pub(crate) fn conn(&self) -> &Mutex<Connection>`

**Category:** architecture (dead code in production path).

**Confidence:** High

**Description:** `conn()` is `pub(crate)` and gated to `#[cfg(test)]`, but the `#[allow(dead_code)]` annotation is on the production-cfg attribute, not the test-cfg one. Reading the macro expansion: this is `#[cfg(test)] #[allow(dead_code)] pub(crate) fn conn()` — the `dead_code` lint sees the function (because `pub(crate)` items can be referenced by `pub` callers) but never called in the test build. The `#[allow(dead_code)]` is unnecessary; the function has callers in the test module (line 153 of `sqlite/notes/tests.rs`).

**Suggested fix:** Drop `#[allow(dead_code)]`; the function is correctly used by the test helper.

---

### [Low] src/memory/notes/store.rs:255-719 — 25 trait methods with `let _ = (param1, param2, ...);` default no-op stubs

**Category:** architecture (YAGNI; not strictly a bug).

**Confidence:** High

**Description:** The `NoteStore` trait has 25+ default methods that return `Ok(())` or `Ok(Vec::new())` for non-SQLite backends and test mocks. The pattern `let _ = (from, to, agent_id, max_depth);` is repeated 25 times. The 25 stub methods exist so non-`SQLite` stores and mocks compile unchanged — but the actual store is the `SQLite`-backed one. The cost is that any future change to a stub's signature silently misses every mock; every mock is invisible to the compiler. A trait with required methods + a small extension trait would be more honest about what is implemented vs. stubbed.

**Suggested fix:** Either consolidate the stubs behind a `MockNoteStore` blank impl, or split the trait into a small required-methods core + an optional-extension trait. Note for a follow-up.

---

### [Low] src/memory/notes/notes/note/parsing.rs:14-18 — `PROVENANCE_RE` regex is `static` (compile-time-once), and the comment block uses `// rust-doctor-disable-next-line unwrap-in-production`

**Category:** quality.

**Confidence:** High

**Description:** The `LazyLock<Regex>` wrapper guards against the `Regex::new` panicking on a malformed pattern — the `unwrap()` in `LazyLock::new(|| ... .unwrap())` is only a true panic risk if the pattern is itself malformed, which it is not (validated at unit-test time). But every site that uses `LazyLock::new(|| Regex::new(...).unwrap())` in this codebase (helpers.rs, wikilink.rs, supersession.rs, ingest/apply.rs) repeats the pattern with the same disable-next-line comment. A single `LazyLock::<Regex>::const_format!`-style helper or a small `fn` that returns `&'static Regex` after a one-time compile-check would eliminate the repeated `unwrap`. Minor consistency nit; not a bug.

**Suggested fix:** Introduce a `static_regex!` macro or `fn regex(pattern: &str) -> &'static Regex` helper at the crate root.

---

### [Low] src/memory/notes/notes/watcher.rs:171-173 — `root.canonicalize().unwrap_or(root)` — silently keeps the un-canonicalized root on failure

**Category:** logic (silent fallback on a load-bearing normalization).

**Confidence:** Medium

**Description:** `spawn_note_vault_watcher` canonicalizes the watch root to defend against symlink-vs-real-path divergence (`/var` ↔ `/private/var`, etc.). On canonicalize failure, the code keeps the un-canonicalized root — the comment says this is safe because events arrive canonical. That assumption only holds if `notify` itself canonicalizes before reporting. On a platform where `notify` reports the un-resolved path (some Linux configs), the canonicalize-fail branch produces a silently-broken watcher with no log line. The actual `canonicalize` failure (permission denied, missing path) is also not logged.

**Suggested fix:** Log a `warn!` on the `unwrap_or` branch so operators can diagnose a watcher that appears to be running but is filtering every event.

---

### [Low] src/memory/store/sqlite/notes/store_impl.rs:1318-1340 — `hybrid_search_notes` calls `vector_search` and `search_notes_fts` sequentially (each holds the connection mutex), so the two halves of a hybrid query cannot overlap

**Category:** logic (latency; not a correctness bug).

**Confidence:** High

**Description:** The single `Mutex<Connection>` serializes every SQLite operation. `hybrid_search_notes` calls `vector_search` (which runs KNN against the vec0 table — an SQLite-level operation) then `search_notes_fts` (which runs an FTS5 query — also SQLite). They cannot run in parallel because both hold the connection mutex. The recent move to `reembed` to async/sync mix is not helped by this. Not a wire severance; just a missed concurrency opportunity.

**Suggested fix:** N/A — would require a connection pool, which the audit is not authorized to recommend.

---

### [Low] src/memory/store/sqlite/mod.rs:282-285 — `count_raw_memories` shares its `WHERE` clause with `get_raw_memories_dashboard` via the `raw_where` helper, but the helper's `WHERE source != 'tool_invocation'` is duplicated in the SELECT and COUNT — a future edit to one must edit the helper

**Category:** architecture (single-source-of-truth maintained; flagging because the comment trail depends on the helper staying correct).

**Confidence:** High

**Description:** The audit's recent "phantom-page bug" review note said this fix was correct (count and list agree because they share `raw_where`). It is. This is an FYI that the helper is now load-bearing — every other raw-memories filter (analytics, exports) MUST route through it. The function is `fn raw_where(agent_id: Option<&str>, query: Option<&str>) -> (String, Vec<Value>)` with no consumer documentation.

**Suggested fix:** Move `raw_where` to a `pub(crate)` accessor and add a doc comment "any new SELECT against raw_memories must build its WHERE clause through this helper".

---

### [Low] src/memory/notes/notes/notes/orientation/fs_orientation.rs:38 — `/// **TODO (Phase B follow-up):** ...` marker

**Category:** architecture (acknowledged dead-code marker; not actionable in this audit).

**Confidence:** High

**Description:** A `TODO` in a doc comment describing an unfinished optimization (`refresh_index_after_ingest` does full rebuild instead of partial). Already documented as deferred. Flagged only so the audit's "stale-TODO" filter doesn't drop it.

**Suggested fix:** Either implement the partial-rendering or schedule for a named issue.

---

## What was NOT found (negative findings — confirm recent fixes hold)

- **No SQL injection.** All SQL identifiers (table names) come from the `EMBEDDING_DIM_TABLES` allowlist or compile-time constants. Bind parameters used everywhere else. The format-string SQL sites (`DELETE FROM {table}`) are explicitly marked `sql-injection-risk` and documented as allowlist-only.
- **No path traversal in `sanitize_title` / `sanitize_note_path`.** `..` is rejected (per the recent `2b1813429` fix), null bytes and reserved chars are stripped, and the result is verified non-empty and not all-dots. Filename lookups go through the `note_content_path` helper, which always applies `strip_md_ext`.
- **No FTS5 injection.** `search_notes_fts` quotes each term as `"…"` with embedded quotes doubled; the `MATCH` expression is built from these phrases joined by `OR`, and the `expr` is passed as a parameter (not interpolated into the SQL — the SQL has `?1`).
- **No `unwrap()` panics in production paths.** Every `.unwrap()` / `.expect()` outside `#[cfg(test)]` is a `static Regex::new` (compile-time validated) or a `format!` (cannot fail). Production code propagates errors via `?`.
- **No dead mutex-poison.** `lock_conn!` macro recovers from a poisoned mutex with a `tracing::warn!` — the connection is still usable since the panicking op simply didn't commit.
- **No torn writes in `remove_note_index`.** The full DELETE set is wrapped in `unchecked_transaction`, so a crash mid-remove cannot leave orphan `notes_fts_meta` rows.
- **No FTS / vec-table drift.** `migrate_notes_fts_trigram` runs on every boot, so the trigram companion self-heals against `notes_fts`.
- **No race between `apply_tx` and the vault watcher.** The watcher goes through `index_one_file` which hashes content before deciding to write; the apply path's `index_note` writes a fresh `content_hash` on disk via `atomic_write_file`. Whichever thread wins, the final `notes_index.content_hash` matches the file's bytes (modulo a TOCTOU documented at line 511-524 above).
- **No reader/writer ring drift.** The note layer is the only writer for `KnowledgeNote` markdown files; the index is rebuildable from disk on demand. The reader path (`KnowledgeNote::from_markdown`) tolerates passthrough frontmatter keys (`extra_frontmatter`).
- **No category fragmentation.** `split_path` in `apply.rs` and `canonicalize_category` in `indexer.rs` both route through the singular-canonical spelling merge (`projects` → `project`), so a plural LLM output lands in the same dir as its singular peer.

## Recommendations ranked by impact

1. (High) Fix the lossy `path.replace("..", "")` in `ingest_batch`'s embedding-queue path (line 223). Replace with `sanitize_title` or `note_content_path`.
2. (High) Either fix `commit`'s link accounting (track `add_link` success per direction) or rename `add_link`'s argument order to make the bidirectional logic self-documenting.
3. (High) Make `push_staged` use `atomic_write_file` so a crash mid-staged-write cannot ship a corrupt target.
4. (Medium) Carry `relations` through `dedup_redirect_creates` rather than `vec![]`-ing them.
5. (Medium) Batch the `notes_sources` lookup in `load_graph_snapshot` (N+1 → 1).
6. (Medium) Document the dim-mismatch failure mode in `get_embedding` and add a blob-length check.
7. (Low) Replace the `Vec::new()` sentinel in the watcher with an enum.
8. (Low) Drop the `#[allow(dead_code)]` annotation on the test-only `conn()` accessor.
