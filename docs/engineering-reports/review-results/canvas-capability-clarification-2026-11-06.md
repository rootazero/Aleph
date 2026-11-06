# Module: canvas / capability / clarification — static audit round 2026-11-06

Scope: whole-module static audit (no diff to review) of `src/canvas`, `src/capability`,
`src/clarification` in the `alephcore` crate.

## Method

- Five read-only reviewer subagents in parallel, one per module (capability split into
  three batches: ownership/descriptor, facade/backend/provider, projection host/census),
  each driven with the `rust-logic-audit` five-phase method and the
  `rust-occams-razor` lens (remove unnecessary concepts, not lines).
- Every finding was re-read by the orchestrator against the current source before a
  fix was accepted. Findings that did not survive that read were dropped, not
  "fixed".
- Semantic code graph used for orientation only: `graphify-out/graph.json`
  (144 055 nodes) via `graphify query` / `graphify explain` / `graphify affected`.
- Classification / routing / severity decisions were made with the **Jev**
  calibrated evaluator (`jev_evaluate`, model `jev-latest`); the reasoning and the
  code reading were done by the reviewer models.

## Jev usage (what was decided by Jev, and what was not)

| Step | Input | Jev decision |
|---|---|---|
| Probe | sample findings from the canvas/clarification batches | sanity check that `jev_evaluate` classifies at all (`warning` 0.59, route `delete` 0.58) |
| Finding triage | all 55 findings, grouped G1–G7 | G1 sync primitives → fix now 0.76 · G2 dead API → delete 0.98 · G3 doc drift → fix now 0.96 · G4 blocking IO → fix now 0.68 · G5 determinism → fix now 1.00 · G6 logic bugs → fix all 0.93 · G7 low-risk cleanups → critical-only 0.63 |
| Module routing | canvas vs clarification groups | canvas → fix now 0.74 (confidence 0.65) · clarification → investigate more 0.56 (confidence 0.41) — the clarification group was therefore routed per item instead of blanket-applied |

Jev did **not** decide: whether a finding is real. That was re-verified by reading the
code in every case.

## Findings and dispositions

55 findings (2 critical, 20 warning, 31 low, 2 info). Full reviewer reports are in
`.review/runs/2026-11-06-canvas-cap-clar/`.

### Fixed in this round

| Finding | Location | Disposition |
|---|---|---|
| Document-byte gate sized the COMPACT encoding while the store persists PRETTY JSON — a document could pass the gate and be written over `MAX_DOCUMENT_BYTES`, producing a canvas file the read path refuses to load | `src/canvas/validate.rs`, `src/canvas/doc_io.rs`, `src/canvas/store.rs` | one `serialize` helper feeds both `write` and the new `persisted_size`; the gate moved to `check_document_size`, run after the revision/timestamp bump and before the commit, i.e. against the exact bytes written |
| Blocking `std::fs` open + `set_modified` inside async `put_asset`, under the per-canvas write lock | `src/canvas/assets.rs` | moved to `tokio::task::spawn_blocking`; both warn messages and their conditions preserved |
| Blocking `Path::is_dir()` stat inside async `list_entries` | `src/canvas/store.rs` | `tokio::fs::metadata` (still follows symlinks, so a symlinked canvas directory keeps counting as a row) |
| Module doc claimed the unit tests drive the in-apply orphan sweep through real `apply` calls | `src/canvas/assets.rs` | doc corrected to describe what the tests actually cover |
| A completed answer that reached nobody published NO terminal frame — the card outlived the vanished run until the request deadline, while the sibling abandoned/expired branch does publish one | `src/clarification/session.rs` | the undelivered branch now publishes `ClarificationOutcome::Cancelled` and still returns `ResolveOutcome::Stale` |
| Model-supplied option labels/descriptions were written into the message body verbatim while only header/prompt were escaped — a label like `[Approve](https://evil.example)` becomes a live link in the Markdown-rendered channel | `src/clarification/render.rs` | labels/descriptions escaped with the existing `escape_markdown`, applied after whitespace collapse and truncation so the length caps still measure raw text; button text and the numbering prefix are not escaped |
| Production `.expect()` on `ClarificationRequest::first()` (the field is `pub(crate)`, so an empty request is constructible in-crate) | `src/clarification/ask.rs` | `RetireOnAbandon::receiver()` now returns `Option<&mut oneshot::Receiver<ClarificationResult>>`; the request is read with `if let Some(first) = request.first()`, an empty request logs `warn!("ask: cannot deliver an empty clarification request")` and returns `false` (the same undeliverable path as a failed channel send), and the "no waiter" branch now runs `cleanup_expired()` before answering with `ClarificationResult::timeout()` |
| Stale `#[allow(clippy::len_without_is_empty)]` comment pointing at a deleted `is_empty` | `src/clarification/mod.rs` | comment corrected |
| Thread-safe types imported from `std::sync` instead of the mandated `crate::sync_primitives` facade | `src/capability/{ownership,projection_host,versioned_registry,provider_registry,diagnostic_control,facade,zahir_facade,provider,mod}.rs`, `src/sync_primitives.rs` | imports routed through the facade; `src/sync_primitives.rs` gained the missing re-exports (`Weak`, `Barrier`, `std::sync::mpsc`). The facade re-exports the same concrete types, so this is type-identical |
| `.expect("ownership mutex poisoned")` on a lock result, where every sibling lock site recovers with `PoisonError::into_inner` | `src/capability/ownership.rs` (`OwnershipTree::is_revoked`) | made consistent with the siblings — a poisoned lock no longer panics. `versioned_registry.rs` already recovered; only its `std::sync::Mutex` type reference moved to the facade |
| Census file order was nondeterministic (`read_dir` walk, never sorted) | `src/capability/census.rs` | the path list is sorted inside the census, in a new `fn sorted_sources(root: &std::path::Path) -> Vec<(String, String)>` used by every census walk (the shared `source_scan` helper was left alone on purpose — it is used by many other modules' census guards) |
| The census counted `pub(crate)` / `pub(super)` / `pub(in path)` statics as "visible outside its module", although the rule it serves is about a handle reachable from EVERY module | `src/capability/census.rs` (`is_public_decl`) | only an unrestricted `pub` qualifies now; the restricted forms stop inside the module tree. The change only relaxes the offender set of `every_public_container_static_is_rostered_or_self_initialising` (a previously green set stays green), and a synthetic test pins the predicate in both directions so it cannot go blind |
| Module docs claimed one live capability backend and omitted the activation entry point | `src/capability/backend.rs`, `src/capability/facade.rs` | docs corrected: the Tool and provider backends are both live, and `Zahir::activate` is documented as the one non-read operation |

### Deferred (deliberately not changed)

| Finding | Why deferred |
|---|---|
| `ZahirFacade::activate` runs the activation side effect before the ownership/generation check (`src/capability/zahir_facade.rs`) | reordering is a contract/ownership decision (the binding must exist in the tree before it can be checked), not a mechanical fix |
| `resources_in_scope` / `resolve_provider` do not filter providers by generation/revocation the way the tool path does | same family as the row above: the provider path's ownership story is unsettled |
| `ZahirFacade::validate_lease` flattens every `BindingValidationError` of a `MemoryResource` lease into `ResolveError::StaleOwner` | error taxonomy change with caller-visible consequences |
| "Dead" public API in `src/capability` | `OwnershipTree::register` is documented in the project constitution as intentionally public; EffectClaim producer integration, automatic Safe Replay, external-effect exactly-once, a universal durable scheduler and the full ACP server gate are documented as DEFERRED. Nothing was deleted on the strength of a grep |
| `read_asset` follows symlinks while the write path refuses to dedupe onto a symlink (`src/canvas/assets.rs`) | needs a threat-model decision, not a patch |
| The canvas shape-cap simulation counts distinct ids (`HashSet`) | the id set and the shape vector hold the same ids by construction, so the counts agree |
| `ops.to_vec()` clone in `emit_updated`, `lock_for_delete` collapsing read errors into "no document" | micro-optimisation / documented intent (a document that cannot be read must still be deletable) |
| The Panel face drops the rendered "next question" (`src/gateway/handlers/clarification.rs::handle_resolve`) | the Panel re-renders from the structured `clarification.pending` view, and the channel already receives the next frame through the published `ask_user_frame` — a design choice, not a proven bug |
| 10 000-byte reply truncation without a marker (`src/clarification/session.rs`) | user-visible behaviour change, out of scope for an audit round |
| `src/capability/projection_host.rs::close_and_await` "lost wakeup" | **false positive** — tokio's `Notify` re-checks the notify-waiters counter at poll time, so a `notify_waiters()` racing between `notified()` creation and the first poll is still observed. "Fixing" it would have added a redundant `enable()` |

## Verification

One unified check, after all three modules were reviewed and merged (per the round
instruction: no per-module checks).

```
python3 qa/browser_live/check_memory.py --required-bytes 4294967296   # before every cargo run
CARGO_BUILD_JOBS=1 cargo check -p alephcore --tests
```

`main` @ `4ea10be04` (canvas `f20ed1bf1`, clarification `8e8ab3cd7`, capability
`4ea10be04`) — **0 errors**. Cold run 14m21s; the reruns after each cherry-pick were
incremental (the second one waited 23m06s behind the first one for the build lock).

The warning set is byte-for-byte the pre-existing one — the round introduced no new
warning:

- `src/extension/projection.rs:143` — field `panel_snapshot` never read
- `src/memory/runtime/mod.rs:209` — methods `provider` / `worker_stop` never used
- `src/tools/registry.rs:294` — method `validate_capture` never used
- `src/tools/registry.rs:1336` — method `publish_plugin_batch_with_ready_commit` never used
- `tests/panel_project_context.rs` — struct `PanelRequestContext` never constructed
- `tests/gateway_method_census.rs` — `string_literals_in_fn`, `rust_sources_under_root` never used

All six are outside `src/canvas`, `src/capability` and `src/clarification` and were
already there before the round. One new warning appeared mid-round (an unused `Mutex`
import left by the capability branch in `src/capability/zahir_facade.rs`) and was
removed by moving the import into the `#[cfg(test)]` module that uses it.

`cargo check --tests` is the right target here: plain `cargo check` does not compile
`#[cfg(test)]` code, and the round's new tests and the whole census
(`src/capability/census.rs` is `#[cfg(test)] pub(crate) mod census;`) live there. The
new test code is therefore type-checked by the run above.

## Not done (the negative list)

- The new tests were **not executed**: `cargo test -p alephcore --lib` needs the
  alephcore lib-test binary, whose link is the documented memory hazard of this
  machine (GNU ld exceeds 15GB; `lld` is configured in `.cargo/config.toml`). While
  the round ran, a concurrent session was active in the same repository and the
  machine was already at 18/19GB with 12GB of swap in use, so the run was started
  and then abandoned rather than risk an OOM kill of somebody else's work. The
  census predicate change and the two new tests are therefore compile-verified only.
- The census predicate change (`is_public_decl`) is verified by reading, not by
  running the census guard: the change only shrinks the offender set of
  `every_public_container_static_is_rostered_or_self_initialising` (a green set stays
  green) and the synthetic predicate test pins both directions, but the guard itself
  has not been re-run.
- `canvas.create` still has no byte gate — a new document is bounded by an empty
  doc, but `owner_user_id` / `project_id` are caller-supplied and unbounded. Left
  alone: it is outside the byte-budget finding and touches the create contract.
- The deferred rows above were re-read, not re-verified by test; none of them was
  changed, deleted or "fixed" on the strength of a grep.
- No other module was reviewed; the five reviewer batches covered `src/canvas`,
  `src/clarification` and `src/capability` only.
- The semantic graph was used for orientation, not as evidence: no finding was
  accepted because the graph said so.
