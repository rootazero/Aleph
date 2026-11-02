# Documentation Current-State Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one reliable current-state entry to `FEATURE_LOCATOR.md` and reconcile the four owning reference documents without changing production code or lifecycle contracts.

**Architecture:** Keep historical locator entries as evidence, but place a bilingual current-state section near the top that names the authority for each capability/lifecycle boundary and links to the owning reference and source evidence. Make only small corrections or cross-links in `MEMORY_SYSTEM.md`, `PLUGIN_SYSTEM.md`, `ARCHITECTURE.md`, and `TOOL_SYSTEM.md`; each document remains authoritative for its own subject.

**Tech Stack:** Markdown, repository source anchors, existing reference/run/research documents, shell-based Markdown/link/diff checks. No Cargo command is required because this plan changes documentation only.

**Spec:** `docs/superpowers/specs/2026-11-02-documentation-current-state-design.md`

## Global Constraints

- Preserve the selected bounded-reconciliation scope; do not rewrite the historical `FEATURE_LOCATOR.md` dictionary.
- Do not modify production Rust, schemas, Capability contracts, provider lifecycle code, generators, or runtime ownership.
- Keep bilingual documentation synchronized: Chinese and English must state the same current fact.
- Keep one owning document per fact; use links instead of duplicating long contracts.
- Treat research, historical rounds, and run records as evidence or design material, never as proof of current production wiring.
- Keep MemoryRuntime claims bounded to derived resources, admission, retained work, and shutdown cleanup; raw chat/Markdown archival remains independently available Off.
- Keep Tool-only Zahir/ProjectionHost, bounded tool-only WASM activation, provider lifecycle fencing, and current Generic Gateway read/lease facts distinct from deferred Feishu → Zahir and other Capability kinds.
- Do not run Cargo for documentation-only changes; record that omission in the run record or final report.

## Review Focus

- A current entry must not claim that every externally composable subsystem is mounted in Zahir; verify against `src/capability/zahir_facade.rs` and current `ARCHITECTURE.md` evidence in Task 1.
- A provider lifecycle statement must distinguish `ProviderHandle` state/generation admission from `ExtensionManager` discovery/effect/durable enabled-state authority; verify against `src/capability/provider.rs` and plugin lifecycle sections in Task 2.
- A MemoryRuntime statement must preserve raw Off availability and avoid universal scheduler/every-writer-stop/process-isolation/exactly-once claims; verify against `src/memory/runtime/{mod.rs,operation.rs,bundle.rs,session_end.rs}` and the 2026-11-02 run record in Task 2.
- Deferred work must remain visibly deferred, especially Feishu → Zahir, MCP lazy activation, EffectClaim production, Safe Replay, universal scheduler, and other Capability kinds; verify every current-state link in Task 1 and Task 2.
- Bilingual sections must be semantically equivalent and links must resolve; run the structured checks in Task 3.

---

### Task 1: Add the authoritative current-state locator

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` near the document entry point before historical/current feature sections
- Read: `src/capability/zahir_facade.rs`, `src/capability/backend.rs`, `src/capability/ownership.rs`, `src/capability/provider.rs`, `src/memory/runtime/mod.rs`, `src/memory/runtime/operation.rs`, `src/memory/runtime/bundle.rs`, `src/memory/runtime/session_end.rs`, `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/PLUGIN_SYSTEM.md`, `docs/reference/TOOL_SYSTEM.md`, `docs/superpowers/runs/2026-11-02-memory-derived-writer-boundary-run.md`

**Interfaces:**
- Consumes: current source authority and bounded-status evidence from the listed files.
- Produces: a bilingual current-state section that later reference documents can link to as the locator entry.

- [ ] **Step 1: Record the exact source anchors and existing owning sections**

  Confirm the current names before editing: `ToolHandlerRegistry` is the callable Tool authority; `OwnershipTree` owns ownership/generation evidence; `ZahirFacade`/ProjectionHost are Tool-only derived projection/notification surfaces; `ProviderHandle` fences provider state/generation; `ExtensionManager`/`RuntimeOwner` govern the bounded tool-only WASM activation slice; `MemoryRuntime` owns bounded Memory-derived resources and admitted retained work. Record the existing heading anchors in the plan execution notes so links target stable headings rather than research prose.

- [ ] **Step 2: Add the bilingual current-state entry before the historical dictionary**

  Add a clearly dated `Current state / 当前现状` section with these subsections:

  1. authority and projection rule: semantic Capability, one owner/source, projections do not become a second registry or lifecycle authority;
  2. Tool/Plugin production slice: callable Tool registry, bounded tool-only WASM activation, provider state/generation fencing, and read-side no-start behavior;
  3. provider lifecycle boundary: `ProviderHandle` and `BackendLease` admission are distinct from `ExtensionManager` discovery/effect/durable enabled-state authority;
  4. MemoryRuntime boundary: derived resource/admission/retained-work ownership, operation propagation across spawned work, raw archival/search/chat remaining available Off, and exact limits from the run record;
  5. independent/deferred surfaces: Gateway/ChannelRegistry, Terminal/PtyManager, Memory, Feishu, MCP lazy, replay, EffectClaim, universal scheduler and other Capability kinds retain their own authorities or remain deferred;
  6. evidence links: owning reference docs, current source paths, the Memory-derived writer run record, and the two research documents labeled as design/history rather than implementation.

  Keep the entry concise. Use links to the owning documents for detailed contracts and explicitly state that historical sections below remain evidence indexes, not current authority.

- [ ] **Step 3: Check the locator semantics before moving on**

  Verify that every current-state sentence has either a source path or an owning-document link, no deferred item is phrased as shipped, and Chinese/English paragraphs have matching claims. Use `git diff --check` on the file.

---

### Task 2: Reconcile the four owning reference documents

**Files:**
- Modify: `docs/reference/MEMORY_SYSTEM.md`
- Modify: `docs/reference/PLUGIN_SYSTEM.md`
- Modify: `docs/reference/ARCHITECTURE.md`
- Modify: `docs/reference/TOOL_SYSTEM.md`
- Read: `docs/reference/FEATURE_LOCATOR.md` current-state entry from Task 1 and the source anchors listed there

**Interfaces:**
- Consumes: the current-state locator from Task 1.
- Produces: four owning documents that agree on authority names, bounded status, and links without duplicating the full locator contract.

- [ ] **Step 1: Align `MEMORY_SYSTEM.md` with the locator**

  Preserve its existing MemoryRuntime contract and add or correct only the cross-link and boundary wording needed to point to the locator. Ensure it says that MemoryRuntime owns bounded derived resources, admission and retained production work; raw chat/Markdown archival/search remains independently available Off; standalone constructors and raw-only paths remain intentional; and the contract does not imply universal scheduling, every-writer stop, native unload, process isolation, RSS reclamation, or exactly-once external effects. Link the 2026-11-02 run record for the 33-row census rather than duplicating it.

- [ ] **Step 2: Align `PLUGIN_SYSTEM.md` with the locator**

  Keep `ExtensionManager`/`RuntimeOwner` as the bounded tool-only WASM discovery/activation authority and `ToolHandlerRegistry` as callable authority. Make the read-side no-start behavior and the deferred channel/PTTY/memory/MCP/other-provider-kind boundary match the current locator. Link to the locator for the cross-subsystem map and avoid claiming a universal Capability lifecycle.

- [ ] **Step 3: Align `ARCHITECTURE.md` with the locator**

  Preserve the cross-subsystem split: `ZahirFacade`/ProjectionHost are Tool-only projection/notification surfaces, provider lifecycle is separately fenced, Generic Gateway read/lease is a bounded source-owned read model, and Feishu → Zahir remains NO-GO/deferred. Add a short link to the current locator and remove only wording that could be read as a production mount or all-capability migration claim.

- [ ] **Step 4: Align `TOOL_SYSTEM.md` with the locator**

  Preserve canonical scoped Tool dispatch, raw `tools.invoke` compatibility semantics, B4–B6 tool/plugin lifecycle boundaries, snapshot/live-notification behavior, and the absence of durable registry replay. Add the current locator link and ensure deferred MCP lazy, Safe Replay, EffectClaim, universal scheduler and other capability-kind claims remain explicitly deferred.

- [ ] **Step 5: Check cross-document consistency**

  Search all five documents for `Zahir`, `MemoryRuntime`, `ProviderHandle`, `ExtensionManager`, `RuntimeOwner`, `deferred`, `NO-GO`, `tools.invoke`, and `raw`. Confirm that each occurrence either owns the fact or links to the owner, and that no document silently broadens the bounded production slice.

---

### Task 3: Validate the documentation change and record the result

**Files:**
- Test/validate: `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/PLUGIN_SYSTEM.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/TOOL_SYSTEM.md`
- Update if required: `docs/superpowers/runs/2026-11-02-memory-derived-writer-boundary-run.md`

**Interfaces:**
- Consumes: completed documentation edits from Tasks 1–2.
- Produces: reviewable diff with passing structural/link checks and an explicit record of checks not run.

- [ ] **Step 1: Run Markdown hygiene and bilingual-structure checks**

  Run a small Python or Bun script that checks:
  - all five files are UTF-8 and have no trailing whitespace;
  - the new current-state section contains both language labels and all required headings;
  - links to local files resolve from the document directory;
  - required deferred markers and bounded MemoryRuntime non-goals are present;
  - no production source files changed.

  Expected: all checks report PASS.

- [ ] **Step 2: Review the complete diff against the design**

  Run `git diff --check`, inspect `git diff --stat`, and read the complete diff. Confirm the diff is documentation-only, historical sections were not rewritten, no research claim was promoted without source evidence, and all four owning documents link to the current locator where appropriate.

- [ ] **Step 3: Perform a fresh read-only documentation review**

  Ask a fresh read-only reviewer to check the complete documentation diff for stale claims, authority conflation, broken links, bilingual mismatches, and accidental universal lifecycle claims. Fix concrete findings and re-run the checks.

- [ ] **Step 4: Update the bilingual run record**

  Add the documentation phase, files changed, exact structural/link-check results, review verdict, and explicit non-actions. State that Cargo, native execution, production runtime tests, and full workspace checks were not run because this phase changes documentation only.

- [ ] **Step 5: Commit locally**

  After the review and checks pass, commit the design, plan, and documentation changes with a conventional message such as:

  ```bash
  git add docs/reference/FEATURE_LOCATOR.md docs/reference/MEMORY_SYSTEM.md docs/reference/PLUGIN_SYSTEM.md docs/reference/ARCHITECTURE.md docs/reference/TOOL_SYSTEM.md docs/superpowers/specs/2026-11-02-documentation-current-state-design.md docs/superpowers/plans/2026-11-02-documentation-current-state-plan.md docs/superpowers/runs/2026-11-02-memory-derived-writer-boundary-run.md
  git commit -m "docs: reconcile capability current state"
  ```

  Verify the final worktree is clean and do not push or merge.

---

## Self-review

- Spec coverage: intent, selected bounded approach, owning-document split, bilingual requirement, research/history separation, non-goals, and documentation-only validation are covered by Tasks 1–3.
- Step granularity: each step has one observable document or validation result; no production implementation step is included.
- Type/interface consistency: Task 1 produces the current-state locator; Task 2 consumes it; Task 3 validates and records both.
- Review focus: all five failure classes are attached to the owning task and its checks.
- Proportion: the plan is longer than the eventual text edits because it fixes file ownership, source evidence, link targets, and review gates; it does not prescribe a full rewrite of the historical dictionary.
