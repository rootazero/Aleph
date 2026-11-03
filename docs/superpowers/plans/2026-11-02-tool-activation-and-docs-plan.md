# Tool First-Call Activation and Documentation Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an exact permitted canonical Tool call activate a Cold tool-only WASM owner through the existing registry and retained owner lifecycle, then reconcile the historical capability locator and classify all repository documentation links without fabricating evidence.

**Architecture:** Extend the existing `ToolHandlerRegistry` publication model with an explicit Cold declaration and one exact registration receipt. A Cold registry handler performs only gate-admitted lookup and calls the existing retained `RuntimeOwner::activate_owner`; after the same registry atomically replaces the Cold receipt with a Ready publication, execution re-enters the existing sealed `PluginActivationAuthority` and Ready handler path. Keep `ProviderHandle`, `RuntimeOwner`, `OwnershipTree`, `ExtensionManager`, and `MemoryRuntime` as separate authorities. Reorganize `FEATURE_LOCATOR.md` into current facts, historical evidence, and deferred boundaries, and use a repository-relative link classifier for all `docs/**/*.md` links.

**Tech Stack:** Rust, Tokio, ArcSwap/versioned registry snapshots, real WASM lifecycle fixtures, `cargo test`, Python 3 standard library, Markdown.

**Spec:** `docs/superpowers/specs/2026-11-02-tool-activation-and-docs-design.md`

## Global Constraints

- Cold activation must use the existing canonical `ToolHandlerRegistry`; do not add a second callable registry, owner/generation/disposer map, Capability kind, or universal lifecycle supervisor.
- `ProviderHandle` remains the provider state/generation authority; `NEXT_OWNER_GENERATION` remains a declaration incarnation ID; `OwnershipTree` remains its own owner-generation authority.
- Read-side `describe`, `resolve`, and `catalog` must not call `activate_owner`, mount WASM, create a guest instance, or publish a Ready snapshot.
- Permission/scope admission must precede activation; caller cancellation must not cancel a retained owner activation; stop/reload must close admission before draining in-flight calls and disposing the exact old owner.
- Do not hold lifecycle, registry, or ownership locks across await/I/O. Do not abort, detach, replace, or manually join an admitted production task.
- Transport compatibility remains mounted-only; only the canonical Tool route gains Cold first-call activation.
- Parse repository-relative Markdown links outside fenced/inline code; skip external/protocol URLs, absolute/home paths, placeholders, and examples; never fabricate deleted history.
- Before every Cargo command run `python qa/browser_live/check_memory.py`; require `ALLOW` with at least 4 GiB; set `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; run Cargo serially.
- Rust comments are English; durable documents are bilingual; local commits use conventional messages; do not push or merge.

## Review Focus

- A Cold declaration must be discoverable without starting WASM, while a denied call must not start it. Test in Task 3 with a real WASM fixture and guest-instance counter.
- A first permitted call must share one retained activation task under concurrency and must re-enter the newly published Ready proof. Test in Task 3 with concurrent calls and a single mount assertion.
- A stop/reload racing activation or an in-flight guest call must leave no old admission, receipt, binding, or completion able to affect the replacement. Test in Task 3 with stop/reload barriers and stale proof assertions.
- A Cold receipt-to-Ready replacement must be atomic and revocable through the existing registry authority; no `ready=None` compatibility entry may masquerade as a plugin activation entry. Test in Task 1 and Task 2 with receipt/revision/projection assertions.
- Historical, pseudo, external, and unavailable documentation links must be classified rather than silently deleted or invented. Test in Task 6 with scanner self-tests and the final classification ledger.

---

## File and interface map

- `src/tools/registry.rs`: canonical entry state, immutable publication snapshots, `CanonicalEntryLease`, plugin batch receipts, and atomic Cold-to-Ready registry replacement.
- `src/extension/runtime_owner.rs`: Cold declaration publication, owner receipt replacement, retained activation handoff, and current owner/incarnation validation.
- `src/extension/canonical_handler.rs` or a focused adjacent module: Cold activation handler plus the existing Ready guest handler; no direct guest call from the Cold handler.
- `src/extension/mod.rs`: canonical plugin-tool declaration lookup that includes Cold declarations while preserving the mounted-only compatibility lookup.
- `src/extension/plugin_ops.rs`: registry-owned post-activation rebind/invocation helper; keep `call_plugin_tool_with_authority` Ready-only.
- `src/executor/builtin_registry/registry/free_fns.rs` and `src/executor/builtin_registry/registry/inherent.rs`: canonical resolver source selection so Cold declarations are routable only through canonical dispatch.
- `src/tools/dispatch_verdict.rs`: unchanged sealed proof boundary except for the registry-owned re-entry needed after Ready publication.
- `src/extension/lifecycle.rs`: real-WASM effect-level tests for the new first-call route and existing activation regressions.
- `src/tools/registry.rs` tests or the nearest existing registry test module: receipt, snapshot, and atomic replacement tests.
- `qa/docs/check_markdown_links.py`: new stdlib-only classifier/self-test for repository-relative Markdown links, if the existing QA layout accepts a reusable checker.
- `docs/reference/FEATURE_LOCATOR.md`: current/history/deferred restructuring and historical status normalization.
- `docs/**/*.md`: only files with classified repository-verifiable repairs or explicit unavailable-history annotations.
- `docs/superpowers/runs/2026-11-02-tool-activation-and-docs-run.md`: bilingual evidence ledger, link classification counts, test results, review result, and explicit non-actions.

## Task 1: Add explicit Cold publication state to the canonical registry

**Files:**
- Modify: `src/tools/registry.rs` (`RegistryEntry`, publication snapshots, `CanonicalEntryLease`, `PluginBatchRegistration`, plugin batch publication methods).
- Test: the existing registry unit/integration test module in `src/tools/registry.rs` or its current adjacent test file.

**Interfaces:**
- Produces an explicit crate-private Cold publication identity containing plugin ID, declaration owner incarnation, canonical name, entry revision, and registry identity; it must contain no provider generation, WASM binding, or Ready proof.
- Produces one receipt type that can represent either a Cold declaration batch or a Ready plugin batch and can revoke/replace only the exact captured entries.
- Extends the existing plugin publication API with a Cold batch publication and an atomic `replace_cold_with_ready` operation; preserve all-or-none mutation and registry revision rules.

- [x] **Step 1: Write RED registry tests** for Cold entries: catalog/read projection can expose declaration metadata; `CanonicalEntryLease::validate` accepts the unchanged Cold identity and rejects descriptor, owner-incarnation, revision, or receipt replacement; a Cold entry cannot be treated as a Ready producer lease.
- [x] **Step 2: Write RED receipt tests** proving a Cold receipt can be revoked exactly once, a stale receipt cannot revoke a replacement, and Cold-to-Ready replacement removes the Cold handler and publishes the Ready snapshot in one observable snapshot.
- [x] **Step 3: Run the focused registry tests** after `python qa/browser_live/check_memory.py`, with `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; expected result is RED because Cold publication/receipt APIs do not exist.
- [x] **Step 4: Implement the Cold snapshot, entry marker, receipt variant, and atomic publication/replacement methods** in `src/tools/registry.rs`, retaining `ready: Option<Arc<ReadySnapshot>>` semantics for compatibility and Ready proof.
- [x] **Step 5: Run the focused registry tests** with the required memory gate; expected result is PASS, including unchanged Ready receipt and stale lease tests.

## Task 2: Publish Cold declarations and add the canonical first-call rebind

**Files:**
- Modify: `src/extension/runtime_owner.rs` (`declare_cold`, owner receipt storage, activation publication call).
- Modify: `src/extension/canonical_handler.rs` or create `src/extension/cold_handler.rs` for the Cold activation handler.
- Modify: `src/extension/mod.rs` (canonical declaration lookup and owner lookup).
- Modify: `src/extension/plugin_ops.rs` (post-activation registry rebind helper; keep `call_plugin_tool_with_authority` Ready-only).
- Modify: `src/executor/builtin_registry/registry/free_fns.rs` and `src/executor/builtin_registry/registry/inherent.rs` (canonical resolver path).
- Test: existing extension lifecycle tests and any focused extension registry test module.

**Interfaces:**
- `RuntimeOwner::declare_cold(...)` publishes an exact Cold batch through the existing registry and stores the same receipt authority that activation later replaces.
- Add a retained Cold handler constructor with inputs equivalent to `Weak<ExtensionManager>`, `Weak<RuntimeOwner>`, owner incarnation, and the trusted `ToolRegistration`; it returns `Arc<dyn ToolHandler>` and never calls WASM directly.
- Add an ExtensionManager helper with behavior equivalent to `activate_and_invoke_cold_tool(owner, owner_generation, registration, args) -> ExtensionResult<Value>`: validate current owner/incarnation, call retained `activate_owner`, ask the same registry for the current Ready entry, and invoke it through registry-owned publication scope.
- Preserve `resolve_active_plugin_tool` as the transport-facing metadata lookup, now including admitted Cold declarations; canonical dispatch alone may activate after permission/scope admission, while the legacy `call_plugin_tool` compatibility route remains mounted-only.

- [x] **Step 1: Add a RED source-level routing test** proving a declared Cold tool is returned by the canonical resolver while `resolve_active_plugin_tool` remains mounted-only.
- [x] **Step 2: Add a RED handler test** proving a Cold handler rejects stale owner incarnation and cannot execute without the canonical gate admission.
- [x] **Step 3: Implement Cold declaration publication in `RuntimeOwner::declare_cold`**, creating the handler from trusted declaration data and storing the exact Cold receipt.
- [x] **Step 4: Implement `activate_and_invoke_cold_tool`** so it waits on the existing retained single-flight activation, performs current-owner checks, then invokes only the current Ready registry entry; never mint a Ready proof in the Cold handler.
- [x] **Step 5: Change canonical resolver lookup** to include Cold declarations and leave gateway/transport compatibility on `resolve_active_plugin_tool` and `call_plugin_tool`.
- [x] **Step 6: Change `mount_owner` to atomically replace the owner’s Cold receipt with its Ready batch** and preserve exact failure/revoke cleanup.
- [x] **Step 7: Run focused extension/capability tests** with the required memory gate; expected result is PASS for compilation and the existing mounted-only activation tests, with any failure treated as a contract blocker before Task 3.

## Task 3: Prove first-call activation and lifecycle races with real WASM

**Files:**
- Modify: `src/extension/lifecycle.rs` tests near the existing Cold/activation/stop/reload coverage.
- Modify only the smallest production files from Tasks 1–2 if a test exposes a concrete contract defect.

- [x] **Step 1: Add a RED real-WASM test** that reads/catalogs/resolves a Cold declaration and asserts no guest instance and no mount side effect.
- [x] **Step 2: Add a RED real-WASM test** that invokes one exact permitted canonical call and asserts `Cold → Starting → Ready`, one mount, one Ready publication, and the expected guest result.
- [x] **Step 3: Add a RED concurrent test** with a mount barrier and multiple canonical callers; assert one retained activation task and no duplicate loader state.
- [x] **Step 4: Add a RED denial/failure test**; assert permission denial does not start activation and mount failure closes admission/cleans the exact scope without publishing Ready.
- [x] **Step 5: Add a RED stop/reload test**; hold an in-flight guest call, close admission, await exact cleanup, activate the replacement, then assert the old proof/lease/completion cannot affect the new owner.
- [x] **Step 6: Run only the focused lifecycle tests** with the required memory gate and serial settings; expected result is RED before Task 2 and PASS after it.
- [x] **Step 7: Run the existing adjacent lifecycle tests** to prove boot activation, disabled/trust-denied no-start, stale generation, exact disposal, canonical projection, and legacy mounted-only behavior remain unchanged.

## Task 4: Rewrite the FEATURE_LOCATOR historical dictionary

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`.
- Read/reference: `docs/reference/ARCHITECTURE.md`, `PLUGIN_SYSTEM.md`, `TOOL_SYSTEM.md`, `MEMORY_SYSTEM.md`, `docs/reference/SUBSYSTEM_ROUTING.md`, current Rust authority files, and the approved design.

- [x] **Step 1: Generate a heading/status inventory** of all existing locator sections and classify each as current fact, historical evidence, deferred/NO-GO, or unresolved.
- [x] **Step 2: Rewrite the document entry structure** into `Current facts / 当前事实`, `Historical evidence / 历史证据`, and `Deferred / NO-GO / Deferred` while preserving historical source text that remains useful.
- [x] **Step 3: Normalize each retained history entry** with an explicit status, evidence link, and current owner/locator link; replace stale “not implemented” claims when current source proves Tool activation, while preserving the historical date and original evidence.
- [x] **Step 4: Add the current Tool first-call activation contract** with exact source links to the registry, RuntimeOwner, canonical resolver, and lifecycle tests; keep provider/owner incarnation/generation terms distinct.
- [x] **Step 5: Validate the locator** for bilingual headings, no trailing whitespace, links to existing local targets, explicit deferred markers, and no claims broader than current source.

## Task 5: Classify and repair all repository-verifiable documentation links

**Files:**
- Create: `qa/docs/check_markdown_links.py` (stdlib-only scanner with `--self-test` and repository report), unless an existing QA owner is selected during implementation review.
- Modify: every `docs/**/*.md` file containing a classified current/historical link repair or an explicit unavailable-history annotation.
- Test: `qa/docs/check_markdown_links.py --self-test` and the real repository scan.

- [x] **Step 1: Write scanner self-tests** for fenced code, inline code, external/protocol links, placeholders, absolute/home paths, valid relative targets, missing current targets, and historical-unavailable classification.
- [x] **Step 2: Run the scanner self-test**; expected result is PASS for fixtures before the repository parser is complete.
- [x] **Step 3: Implement Markdown link parsing and classification** rooted at the repository, reporting `valid`, `repairable-current`, `historical-migrate`, `historical-unavailable`, `code/example`, and `external/placeholder` without modifying files. The implementation report uses concrete parser buckets (`valid`, `external_or_protocol`, `absolute_or_private`, `placeholder`, `code_or_example`, and `missing_repository_targets`); repair/migration decisions are recorded in the run record.
- [x] **Step 4: Run the real scanner** and save the complete classification report in the run record; reconcile the 228 raw findings into exact counts before editing. The scanner intentionally does not fully parse nested, multiline, escaped, or angle-bracket Markdown syntax; such cases are recorded as a limitation and are not silently treated as valid.
- [x] **Step 5: Repair only repository-verifiable current links**, including the known `TOOL_SYSTEM.md`, `MEMORY_SYSTEM.md`, `ARCHITECTURE.md`, and `SUBSYSTEM_ROUTING.md` candidates, and migrate historical links only when a surviving evidence target is identified.
- [x] **Step 6: Mark genuinely unavailable historical artifacts explicitly** with bilingual wording and no fake target; leave code/example/external/placeholder cases outside the repository link contract.
- [x] **Step 7: Run the real scanner again**, require zero unclassified current-reference failures, and compare before/after counts in the run record.

## Task 6: Integrated validation, review, run record, and local commit

**Files:**
- Modify: `docs/superpowers/runs/2026-11-02-tool-activation-and-docs-run.md`.
- Review: complete working-tree diff, all changed Rust/docs/QA files.

- [x] **Step 1: Run focused Rust tests** for registry, extension lifecycle, canonical routing, and real-WASM first-call behavior; execute the memory check before every Cargo command and record exact selected/pass/fail counts.
- [x] **Step 2: Run `cargo check -p alephcore --tests`** and `cargo check -p alephcore --bin aleph-server` serially with `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; run justified clippy only after checks pass.
- [x] **Step 3: Run documentation scanner self-test, repository scan, UTF-8/trailing-whitespace checks, Markdown diff check, and locator structure checks; record the classified link totals.
- [x] **Step 4: Update the bilingual run record** with the Rust gap, implementation evidence, test results, link categories, review findings/fixes, remaining unrelated failures, and explicit non-actions. Do not claim universal lifecycle ownership, native unload, RSS reduction, process isolation, or exactly-once effects.
- [ ] **Step 5: Obtain a fresh read-only review of the entire diff** focused on Cold/Ready proof, authority duplication, stale generation, stop/reload cleanup, canonical versus compatibility routing, historical-document truthfulness, and link classification.
- [ ] **Step 6: Fix concrete review findings and rerun only affected checks**, then update the run record with the final verdict.
- [ ] **Step 7: Commit locally** with a conventional message such as `feat(extension): activate cold tools on canonical calls`; verify `git status --short`, `git diff --check`, and no push/merge.

## Self-review

- **Spec coverage:** Cold registry representation is Task 1; owner publication/rebind and canonical routing are Task 2; all required behavior evidence is Task 3; locator current/history/deferred rewrite is Task 4; link classification/repair is Task 5; full validation/review/commit is Task 6.
- **Step granularity:** each step has one implementation or verification action and names its expected result; no task relies on an undefined generic “handle edge cases” step.
- **Type consistency:** Task 1 produces the Cold publication identity and unified receipt used by `RuntimeOwner::declare_cold` and `mount_owner` in Task 2; Task 2 produces the canonical rebind helper used by Task 3; Task 5 produces the scanner report consumed by Task 6.
- **Review focus coverage:** all five review-focus conditions have tests in Tasks 1–3 or scanner self-tests in Task 5.
- **Proportion:** the plan keeps production changes to the existing registry/extension/resolver boundary and treats the broad documentation cleanup as a separate task; it does not redesign unrelated Capability kinds or Memory lifecycle.

## Handoff

After user review and execution-method selection, use `superpowers:executing-plans` for native execution or `superpowers:subagent-driven-development` for task-by-task delegation. No production edits begin before that handoff.
