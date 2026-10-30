# Stage H Reference Documentation Implementation Plan

> **For agentic workers:** This documentation-only plan is ready for task-by-task execution. Do not modify production Rust, tests, `AGENTS.md`, or `CLAUDE.md`.

**Goal:** Correct current-looking Stage H reference claims so the docs accurately distinguish declaration-only discovery, explicit bounded Tool-only WASM activation, and derived projection surfaces.

**Architecture:** Apply the approved point-correction design to three reference documents. Preserve dated historical records, but qualify any sentence that could be read as current wiring and add source-backed current/deferred cross-references. Produce the stale-claim census in the final delivery report rather than adding another repository document.

**Tech Stack:** Markdown reference documents, `rg`, small validation scripts, Git diff checks.

**Spec:** `docs/superpowers/specs/2026-10-30-stage-h-reference-docs-design.md`

## Global Constraints

- Modify only `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/PLUGIN_SYSTEM.md`, unless the audit proves another current-looking claim must be corrected.
- Keep all new or changed documentation bilingual.
- Treat the current checkout at `main` / `be9c68c46e7253b815d0ff8e175a803e9dbfb5f7` as authoritative.
- `ExtensionManager` / `RuntimeOwner` own the bounded ExtensionManager-managed Tool-only WASM activation slice.
- `ensure_discovered()` is declaration-only and must not be documented as provider activation.
- `ToolHandlerRegistry` is the canonical callable publication source.
- `ProjectionHost` / `ZahirFacade` are derived read/projection/notification/host-local worker surfaces and do not own provider lifecycle.
- Feishu, other Gateway channels, Terminal, Memory, Sandbox, EffectClaim producer, Safe Replay, universal durable scheduler, and remaining capability kinds remain deferred.
- Do not claim native unload, JIT/engine reclamation, OS mapping or executable-page reclamation, thread shutdown, RSS reduction, or process isolation.
- Do not run full Cargo fmt/clippy fixes, commit, merge, push, or publish.

## Review Focus

- A historical pre-Stage-H snapshot must not read like current production wiring; validate every retained old claim has a date/scope qualifier.
- Declaration-only reads must not imply guest bytes, instances, provider resources, or callable publication; validate `ensure_discovered()` wording against `DiscoveryMode::DeclarationsOnly` and Stage H callers.
- Explicit activation must remain distinct from read/projection paths; validate `ensure_loaded()`, enable/reload/control, `describe`, `resolve`, `project`, and ProjectionHost wording.
- Tool host production composition must not be mistaken for Feishu or universal capability integration; validate Stage G and B8.2 cross-references.
- English and Chinese paragraphs must carry the same date, scope, boundary, and deferred limitations; validate paired sections before finalizing.

---

### Task 1: Correct FEATURE_LOCATOR current and historical claims

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md:6060-6138` (Capability Phase 4 / B5 / B6)
- Modify: `docs/reference/FEATURE_LOCATOR.md:6246-6250` (Stage F historical evidence)
- Preserve and cross-reference: `docs/reference/FEATURE_LOCATOR.md:6262-6276` (Stage G/H current anchors)

**Interfaces:**
- Consumes: `src/extension/mod.rs:698,738`; `src/extension/lifecycle.rs:146-149,357-360,379-382,546-550`; `src/extension/runtime_owner.rs`; `src/tools/registry.rs:999,1009`; `src/capability/projection_host.rs`; `src/capability/zahir_facade.rs`.
- Produces: source-backed bilingual wording that states the current bounded production slice, declaration-only read boundary, activation boundaries, and deferred capability scope for Tasks 2–4.

- [ ] **Step 1: Replace or scope the stale §3.5d tri-state snapshot**

  Mark the 2026-10-22 “只做设计 + 入口，不接线” and “production only calls `new()` / no production transition / dormant gate” text as a pre-Stage-H historical snapshot, or replace only its current-looking conclusion with a dated historical qualifier. Add the current bounded wiring: `ExtensionManager` / `RuntimeOwner` drive Tool-only WASM activation; `new_with_provider` remains a constructor/assembly seam and is not the plugin lifecycle authority.

- [ ] **Step 2: Strengthen B6 read and activation boundaries**

  Add matching English/Chinese statements that `ensure_discovered()` performs declaration-only discovery, while `ensure_loaded()` and explicit enable/reload/control are activation boundaries. State that `describe`, `resolve`, `project`, and ProjectionHost read-side calls do not start providers. Qualify the historical B6→B7 Feishu sentence and point to current B8.2 deferred status.

- [ ] **Step 3: Scope the Stage F `ZahirFacade::new` sentence**

  Rewrite the sentence saying no production `ZahirFacade::new` mount was connected so it explicitly applies to the Stage F historical scope. Add the current Stage G Tool host mount cross-reference while retaining that Feishu/channel Capability integration is deferred.

- [ ] **Step 4: Run targeted stale-claim search**

  Run:

  ```bash
  rg -n -C 2 '只做设计|不接线|no production|production caller|没有生产|未接线|new_with_provider|ZahirFacade::new|B7|Feishu' docs/reference/FEATURE_LOCATOR.md
  ```

  Expected: stale current-looking claims are qualified or removed; retained history has an explicit date/scope; deferred Feishu/channel statements remain.

---

### Task 2: Clarify ARCHITECTURE historical/current boundaries

**Files:**
- Modify: `docs/reference/ARCHITECTURE.md:122-217` (B5/B6 and Provider lifecycle foundation)
- Preserve: `docs/reference/ARCHITECTURE.md:177-185` Stage H facts, with only consistency edits if needed

**Interfaces:**
- Consumes: Task 1’s current terminology; `src/extension/mod.rs:698,738`; `src/extension/runtime_owner.rs`; `src/bin/aleph-server/commands/start/mod.rs:269-275`; B8.2 deferred wording in `FEATURE_LOCATOR.md`.
- Produces: bilingual architecture narrative that cannot be read as current Feishu wiring or as an unwired ProviderLifecycle foundation.

- [ ] **Step 1: Qualify B6 historical Feishu wording**

  Preserve the dated B6 delivery record, but state that its B7/Feishu reference describes the historical scope at that time. Add a current cross-reference: the production bounded slice is Tool-only WASM, while Feishu→Zahir and other channel Capability integration remain deferred.

- [ ] **Step 2: Reconcile Provider lifecycle foundation wording**

  Where the preceding foundation text says provider transitions were not wired, make the historical date/scope explicit. Keep the current statement that `ExtensionManager` / `RuntimeOwner` drive the bounded Tool-only WASM transition and that `ProviderLifecycle` itself remains side-effect free.

- [ ] **Step 3: Verify paired sections and architecture links**

  Check English/Chinese paragraph pairs around B5/B6, Provider lifecycle, and Stage H for identical scope and deferred claims. Validate every new internal reference resolves to an existing file or heading.

---

### Task 3: Reconcile PLUGIN_SYSTEM Provider foundation with Stage H

**Files:**
- Modify: `docs/reference/PLUGIN_SYSTEM.md:16-28` (Provider lifecycle foundation)
- Preserve or minimally align: `docs/reference/PLUGIN_SYSTEM.md:210-280` (Stage E/H current bounded slice)

**Interfaces:**
- Consumes: Tasks 1–2 terminology; `src/capability/provider.rs`; `src/extension/runtime_owner.rs`; `src/tools/registry.rs:1009`.
- Produces: bilingual Provider foundation that distinguishes the 2026-10-21 historical baseline from current bounded production wiring and preserves `new_with_provider` as a compatibility constructor.

- [ ] **Step 1: Rewrite the contradictory historical/current pair**

  Keep the 2026-10-21 date, but say that “not yet wired” describes the baseline at that time. Immediately state the current bounded production fact: `ExtensionManager` / `RuntimeOwner` drive Tool-only WASM provider transitions; `ProviderLifecycle` owns state/generation semantics and does not itself start side effects.

- [ ] **Step 2: Preserve the constructor and lifecycle authority boundary**

  Ensure both languages say `ToolHandlerRegistry::new_with_provider(provider: ProviderHandle)` is a builtin compatibility-provider constructor or assembly seam, not the plugin lifecycle authority. Keep Stage H’s declaration-only and explicit-activation wording unchanged unless a bilingual mismatch appears.

- [ ] **Step 3: Run targeted consistency search**

  Run:

  ```bash
  rg -n -C 2 '未来由|drive later|not wired|尚未接入|new_with_provider|ensure_discovered|ensure_loaded|Projection|Feishu' docs/reference/PLUGIN_SYSTEM.md
  ```

  Expected: the historical baseline is dated, current bounded wiring is explicit, and deferred limitations remain identical in English and Chinese.

---

### Task 4: Build the stale-claim census and run document validation

**Files:**
- No new repository file; deliver as a dedicated final-report section.
- Inspect: all `docs/reference/*.md` search hits, with edits limited by Tasks 1–3 and the approved scope.

**Interfaces:**
- Consumes: changed lines from Tasks 1–3 and source anchors listed in the design spec.
- Produces: final census with old claim, handling (changed / retained historical / explicitly deferred), and source anchor.

- [ ] **Step 1: Re-run the broad search and classify hits**

  Run a broad search across `docs/reference/*.md` for `no production`, `production caller`, `没有生产`, `未接线`, `只做设计`, `lazy activation`, `new_with_provider`, `ProjectionHost`, `ZahirFacade`, `ensure_discovered`, `ensure_loaded`, and `Cold`. Record only claims changed or intentionally retained because their date/scope is explicit; state that other hits are dated historical records left unchanged.

- [ ] **Step 2: Validate Markdown links**

  Use a repository-local link checker or a small script that resolves relative Markdown links from each changed document. Expected: zero missing local targets; report external links as not fetched if validation is local-only.

- [ ] **Step 3: Validate bilingual pairing**

  Inspect every changed English/Chinese pair and compare dates, scope, activation/read boundary, source anchors, and deferred list. Expected: no semantic mismatch.

- [ ] **Step 4: Run final repository checks**

  ```bash
  git diff --check
  git status --short
  git diff --stat
  ```

  Expected: only approved documentation/design/plan files are changed; no whitespace errors; no production Rust or test changes.

- [ ] **Step 5: Report explicit non-actions**

  In the final report, state that no Cargo checks were run for this documentation-only task, no full fmt/clippy baseline was changed, no provider or capability kind was wired, no commit/merge/push/destructive/external operation was performed, and all specified deferred boundaries remain deferred.
