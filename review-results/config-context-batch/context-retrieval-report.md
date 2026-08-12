# Severed-Wire Audit — `src/context/retrieval/`

**Scope:** `src/context/retrieval/mod.rs` (15 lines), `src/context/retrieval/content_index.rs` (1380 lines)
**Method:** Read-only static review. Full read of both files, `grep`-verified wiring against every downstream consumer, `graphify` orientation query for the `ContentIndex` community.
**Prior audit:** flagged this file LOW-severity on file-size alone. This pass re-scans specifically for wiring desync (SQLite + BM25 + writeback are three parts that commonly drift).

## Verdict

**No severed wires found.** Every `pub fn` on `ContentIndex`, every field on `IndexOutcome`/`SearchHit`, and the `pub(crate)` re-export `sanitize_fts_query` all have exactly one live producer and at least one live consumer, confirmed by direct grep of call sites (not just `dead_code`-lint absence). The previously-audited edge (`ContentIndex::open` ↔ `tools/result_store.rs:314`) is still wired and now traced two hops further downstream to the model-facing tool (`ctx_search`) and the write-side hint builder (`result_processing.rs`).

One real (but low-severity, and largely inert in production) issue found: **§4**, a Drop-order hazard in `StoreInner` that only threatens the Windows disk-cleanup path, not correctness or search results.

---

## Phase 1 — Seam scan

### 1. `ContentIndex` parity (every `pub fn` → every consumer)

| `ContentIndex` method | Producer (file:line) | Consumer (file:line) | Status |
|---|---|---|---|
| `open` | `content_index.rs:114` | `result_store.rs:314` (`fn index()`, lazy `OnceLock::get_or_init`) | ✅ wired |
| `open_in_memory` | `content_index.rs:121` | only `content_index.rs`'s own `#[cfg(test)]` module (20+ call sites) | ✅ wired (test-only by design, doc says so) |
| `index_text` | `content_index.rs:179` | `result_store.rs:348` (`ToolResultStore::index_output`) | ✅ wired |
| `search` | `content_index.rs:268` | not called outside tests — see note below | ⚠️ see 1a |
| `search_sessions` | `content_index.rs:286` | `result_store.rs:396` (`ToolResultStore::search`) | ✅ wired |
| `len` | `content_index.rs:325` | not called outside tests — see note below | ⚠️ see 1a |
| `len_sessions` | `content_index.rs:332` | `result_store.rs:409` (`ToolResultStore::indexed_sections`) | ✅ wired |
| `list_sessions` | `content_index.rs:356` | `result_store.rs:513` (`ToolResultStore::sweep_stale`) | ✅ wired |
| `clear` | `content_index.rs:378` | `result_store.rs:480`, `result_store.rs:520` (`ToolResultStore::purge_all`, sweeper GC) | ✅ wired |

**1a.** `search()` and `len()` (the single-session convenience wrappers) are never called by production code — only their `_sessions` siblings are (`search_sessions`, `len_sessions`), because `ToolResultStore` always searches the epoch-widened `read_scope_keys()` set, even for a handle with no epoch history (a 1-element slice). This is **not a severed wire**: `search`/`len` exist as the ergonomic single-session entry point for the module's own test suite (11+ call sites in `#[cfg(test)]`) and are part of the public contract (`pub fn`, doc-commented, used to `assert!` isolation invariants like `search_never_crosses_session_boundary`). Nothing to fix — flagging only so the "consumer" column above doesn't read as false-positive dead code.

### 2. `SearchHit` / `IndexOutcome` field parity

`SearchHit { source, chunk_no, title, snippet, score }` (`content_index.rs:84-97`) →  consumed at `ctx_search.rs:118-123`:
```rust
.map(|h| CtxSearchHit { source: h.source, section: h.chunk_no, title: h.title, excerpt: h.snippet })
```
`source`, `chunk_no`, `title`, `snippet` — all four read. **`score` is intentionally dropped** — not exposed on `CtxSearchHit` at all. This is documented as deliberate at `content_index.rs:93-96`: *"hits are returned pre-sorted descending, so callers can rely on order without re-sorting by score."* Not a severed wire — a field that exists for the fusion/rerank pipeline's internal ordering, not for external consumption. No action needed, but worth naming explicitly since an unread struct field is exactly the shape severed-wire audits look for.

`IndexOutcome { sections, previews }` (`content_index.rs:74-80`) → consumed at `result_processing.rs:287,439-454` (`search_hint()`): both fields read (`sections` for the count message, `previews` joined for the "First sections: …" orientation text). ✅ full parity.

`IndexError` (`content_index.rs:66-70`) → every call site pattern-matches `Ok`/`Err` and logs via `tracing::warn!` on the error branch, degrading to `None`/`Vec::new()`/`0` rather than propagating. This is the documented fail-soft contract ("Kept local … so callers translate or log-and-fall-back as needed"), consistently applied at all 6 call sites in `result_store.rs`. ✅ consistent, not silently dropped (every branch logs).

`sanitize_fts_query` (`pub(crate)`, `content_index.rs:819`, re-exported `mod.rs:14`) → consumed at `content_index.rs:295` (its own module) **and** `src/session/store.rs:593`, exactly as the doc comment claims ("the session-event FTS index (`session::store`) reuses the same hardening instead of duplicating it"). ✅ wired, cross-module reuse confirmed live, not aspirational.

### 3. Stub sweep

`grep -n "TODO|FIXME|unimplemented!|todo!|unreachable!\(\)"` over the scope: one hit, `content_index.rs:1009`, inside a test fixture *string literal* (`"fn getUserPaymentRefund() {\n    todo!()\n}\n"` — sample source text being indexed, not executable code). No real stubs, no empty match arms, no `unimplemented!()` in production code. ✅ clean.

---

## Phase 2 — Candidates enumerated

| # | Producer | Consumer | Note |
|---|---|---|---|
| 1 | `content_index.rs:268` `ContentIndex::search` | none in production | Not a bug — see 1a |
| 2 | `content_index.rs:325` `ContentIndex::len` | none in production | Not a bug — see 1a |
| 3 | `content_index.rs:96` `SearchHit::score` | not read by `ctx_search.rs` | Not a bug — documented, ordering carries the signal |
| 4 | `result_store.rs:127-141` `StoreInner::Drop` | `remove_dir_all` races the still-open `Connection` in the same struct | Real, low-severity — see Phase 4 |

No candidates required Phase 3 triage beyond what's already resolved inline above — the seam scan and grep-verification collapsed straight to triage for each. Formal triage table below.

---

## Phase 3 — Triage (read-first)

- **#1 / #2** (`search`/`len` singular forms): grepped every call site in the repo (`\.search\(`, `\.len\(` are too common to grep precisely across the whole tree, so I grepped the typed variable `idx.` in `content_index.rs` tests and the full `ContentIndex::` / method-call surface in `result_store.rs`). Confirmed: zero production callers, 30+ test callers. **Live, intentional API surface — not dead code, not a severed wire.**
- **#3** (`score` field): grepped `\.score` across `src/builtin_tools/ctx_search.rs` and `src/tools/result_processing.rs` — zero hits. Confirmed unread outside `content_index.rs` internals (`rrf_fuse`, `proximity_rerank`, `finalize` all consume it before `SearchHit` is built). **Intentional, documented — not a defect.**
- **#4** (Drop order): read `StoreInner` (`result_store.rs:120-142`) and `ContentIndex` (`content_index.rs:108-110`, `conn: Mutex<Connection>`). Confirmed the global process singleton (`ToolResultStore::new("global")`, sole production call site `src/bin/aleph-server/commands/start/mod.rs:2966`) is stored in a `'static OnceLock` (`GLOBAL_STORE`) that is never actually dropped at process exit (Rust does not run destructors on `'static` values on normal process termination) — so this Drop path is **not exercised in production at all**, only by ephemeral/test bootstraps that construct a `ToolResultStore` directly and let it fall out of scope. **Real but low-impact — detailed below.**

---

## Phase 4 — Fix recommendations

### Finding: `StoreInner::Drop` deletes the blob directory before its own `ContentIndex`/`Connection` field is dropped

- **Producer:** `src/tools/result_store.rs:127-141` (`impl Drop for StoreInner`)
- **Consumer:** N/A — this is an internal ordering hazard, not a missing wire
- **Severity:** LOW
- **Triage:** DECIDE (worth a one-line comment / possible reorder; not urgent)
- **Reason:** `impl Drop for StoreInner::drop()` runs `std::fs::remove_dir_all(&self.base_dir)` (`result_store.rs:133`) as its **entire** body. Rust guarantees the custom `drop()` body runs to completion *before* the struct's own fields (`base_dir: PathBuf`, `index: OnceLock<Option<ContentIndex>>`) are field-wise dropped — so the `ContentIndex`'s `Mutex<Connection>` (holding the open `index.db` SQLite file handle) is still alive and holding the file open at the moment `remove_dir_all` tries to delete the directory containing it. On Unix this is harmless (unlink-while-open is POSIX-legal, the inode survives until the fd closes). On Windows, SQLite by default opens files without `FILE_SHARE_DELETE`, so `remove_dir_all` can fail on `index.db` specifically with a sharing violation — the code already handles this soft-fail-safe (`tracing::warn!` + continue, `result_store.rs:134-138`), so it degrades to "index.db and possibly its directory are left on disk" rather than crashing or losing data. Given this repo's project rules place heavy emphasis on Windows correctness (`WINDOWS_RUNTIME.md`, DPI/handle-lifecycle judgment criteria throughout `CLAUDE.md`), this is the kind of drop-order bug the project's own judgment criteria call out ("a value has storage form and display form... conversion only at the outbound edge" pattern-cousin: here it's "a resource has an owner and a cleanup ritual, and the ritual currently outruns the owner's own teardown").
  - **Practically inert today**: the only production `ToolResultStore` is the process-wide `'static` singleton behind `GLOBAL_STORE: OnceLock<Arc<ToolResultStore>>` (`result_store.rs:70`), and `'static` values are not destructed on normal process exit — so `StoreInner::drop()` never actually fires in the shipped server. It *does* fire for the ~15+ test helpers (`test_store(...)`) and any future non-global bootstrap that builds a scoped, non-static `ToolResultStore` and lets it go out of scope (e.g. a hypothetical per-request or per-plugin store).
- **Proposed fix:** Either (a) explicitly close/drop the `ContentIndex` field first inside `StoreInner::drop()` (e.g. `let _ = self.index.take();` before the `remove_dir_all`, if `OnceLock` is swapped for a type that supports explicit teardown, or restructure `index` as `Mutex<Option<ContentIndex>>` so it can be `.take()`n) so the SQLite connection closes before the directory delete is attempted, or (b) leave as-is but add a one-line comment on the `Drop` impl noting the ordering is intentional-benign only because it's never exercised on the hot path, so a future non-global caller doesn't inherit a silent Windows cleanup gap. Given the current blast radius is "test/ephemeral cleanup can leave a stray `index.db`," (a) is a small, low-risk change if anyone wants to close it; not urgent enough to block anything.

*No other findings reach fix-recommendation status — Phase 1/2/3 candidates #1–#3 are confirmed-intentional design, not defects.*

---

## Phase 5 — Guard recommendation

The prior audit's flagged edge — `ContentIndex::open` (`content_index.rs:114`) is the sole producer, `ToolResultStore::index()` at `result_store.rs:314` the sole consumer — **is** encoded in tests, on both sides of the seam:

- **Schema/migration contract:** `content_index.rs:1344-1379` `opening_a_pre_scope_index_recreates_it_instead_of_erroring` hand-builds the pre-`session_id` legacy schema on disk, then asserts `ContentIndex::open` recreates it empty instead of erroring — directly guards the `drop_pre_scope_tables` migration path (`content_index.rs:402-428`).
- **Open/reopen persistence contract:** `content_index.rs:980-996` `persists_to_disk_and_reopens` guards that a `ContentIndex::open(db)` → write → drop → `ContentIndex::open(db)` round-trip on the same path is queryable — i.e. the producer's on-disk contract (schema name, `INDEX_DB_NAME` path join) matches what `ToolResultStore::index()` expects when it reopens across process restarts.
- **End-to-end wire contract (not just the constructor):** `result_store.rs`'s own test module exercises the full `ToolResultStore → ContentIndex` path, not just `ContentIndex` in isolation — e.g. `sweep_at_store_root_removes_stale_sessions_but_keeps_root_and_index` (`result_store.rs:1056+`) asserts the sweeper's `list_sessions()` → `clear()` calls don't collide with the live index, and the epoch/session-isolation tests (`search_sessions_spans_epochs_of_one_key`-equivalents in `result_store.rs`, e.g. lines ~1091-1229) call `store.search(...)`/`store.index_output(...)` — the actual `ToolResultStore` methods, not the raw `ContentIndex` — so a regression in the `for_session`/`read_scope_keys` glue between the two files would fail these, not just the `content_index.rs`-local unit tests.

**No additional guard needed for the `open`/`index()` edge** — it's already a tested contract on both the constructor and the full call chain. The one gap worth a guard (not present today) would be a regression test asserting **BM25 freshness after write** — i.e. `index_output()` immediately followed by `search()` in the *same* `ToolResultStore` handle returns the just-written content with no delay/async gap. This is implicitly covered by nearly every existing test (write-then-search is the standard test shape throughout both files) but there's no test whose *name* asserts "no staleness window exists" as an invariant — worth naming explicitly if the write path ever grows an async/batched indexing stage in the future, since today's synchronous call makes it trivially true (see next section) and that guarantee is exactly the kind of thing that erodes silently under a later "batch these writes for perf" refactor.

---

## Explicitly checked per mission brief

**"Check for BM25 search index staleness — is the index rebuilt after writes?"**
No rebuild needed because there is no staleness window: `index_output()` (`result_store.rs:333-360`) is called synchronously, in-line, immediately after `persist_if_large()` succeeds, in the very same function (`recovery_footer`, `result_processing.rs:272-291`, lines 280 → 286). Both are blocking `std::fs`/`rusqlite` calls on the calling thread — there is no queue, channel, or background task between "tool output persisted" and "tool output indexed." By the time the model sees the `[Full output persisted: …] [Indexed N sections — use ctx_search…]` marker in its context, the FTS5 rows already exist and are immediately searchable (FTS5 has no separate commit-visibility delay within one connection; `index_text`'s `tx.commit()` at `content_index.rs:233` finalizes before `index_output` returns `Some(outcome)`). **No staleness bug; confirmed synchronous by design.**

**"Check for sqlx pool / SQLite connection lifecycle — leaks, double-closes, etc."**
This module uses `rusqlite` (not `sqlx`), a single `Connection` behind a `Mutex` (`content_index.rs:109`), opened exactly once per process via `OnceLock::get_or_init` (`result_store.rs:309-327`) against the sole production `ToolResultStore::new("global")` call site (`aleph-server/commands/start/mod.rs:2966`). No connection pool exists (by design — one shared file-backed connection, session-scoped by a `WHERE session_id` predicate rather than by separate connections/pools). No double-open: `for_session()` (`result_store.rs:209-214`) clones the `Arc<StoreInner>`, sharing the same `OnceLock`/`Connection`, never re-opening. No double-close: `Connection`'s own `Drop` runs exactly once, when the last `Arc<StoreInner>` reference drops. The one lifecycle wrinkle found is the Drop-*ordering* issue in Phase 4 above (directory delete racing the connection's own close) — not a leak or double-close, and inert in production per the analysis there.

---

## Summary

| Category | Count |
|---|---|
| Confirmed-wired producer↔consumer edges | 9 (all `ContentIndex` pub methods) + 3 (struct-field parity) + 1 (`sanitize_fts_query` cross-module reuse) |
| False-positive candidates resolved as intentional design | 3 (`search`/`len` singular forms, `SearchHit::score`) |
| Real findings | 1 (LOW, Drop-order on `StoreInner`, inert on the production hot path) |
| Stub markers (TODO/todo!/unimplemented!) | 0 in production code |
| BM25 staleness risk | None — synchronous write-then-index, no async gap |
| SQLite lifecycle risk | None beyond the LOW Drop-order finding — single connection, single owner, no pool, no double-open/close |
