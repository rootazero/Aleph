# Capability Core provider activation implementation plan / Capability Core 提供者激活实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:executing-plans to execute this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make a second capability kind first-class in the Capability Core — a kind-neutral provider registry that the Core can describe, resolve and *explicitly ask to start*, with Terminal as the first proof instance using real resources.

**Architecture:** The Core keeps owning the contract (descriptor, lease, generation fencing) while owners keep owning the effects (R1). A new Core-side provider registry plus a second `CapabilityBackend` adapter serves `CapabilityKind::Resource` descriptors of any family; the `Zahir` trait gains an explicit `activate` that asks the owner to start and returns a fresh, fenced lease. `resolve` never starts anything. Memory is migrated off its hardcoded facade special case onto the same generic path, so "a Resource capability" has exactly one meaning in the Core. Terminal is proven end to end with real resources (owned driver, PTY sessions, generation fencing).

**Tech Stack:** Rust, Tokio, Axum, Cargo/libtest, Git worktrees, Python documentation checks.

**Prior design (authority):** `docs/superpowers/specs/2026-11-04-capability-core-provider-activation-design.md`
**Prior evidence:** [2026-11-03 capability tool mount evidence run](../runs/2026-11-03-capability-tool-mount-evidence-run.md) · [2026-11-03 Memory source-owned boundary run](../runs/2026-11-03-memory-source-owned-boundary-run.md) · [2026-11-03 Memory source-owned validation closure run](../runs/2026-11-03-memory-source-owned-validation-closure-run.md)

**Status / 状态:** Handoff only; not implemented / 仅交接，尚未实施。

## Global Constraints

- Discover the active checkout from Git (`git rev-parse --show-toplevel`, `git branch --show-current`, `git status --short`); do not assume an absolute path. Work only on the local branch the user has checked out; do not switch the primary checkout.
- Verify the task-start commit is an ancestor of HEAD before editing, and record the task-start commit hash in the run record — it is the baseline for full-suite attribution.
- No fetch, push, merge, rebase, reset, or history rewrite unless the user asks in that session. Do not overwrite user changes; if the tree is dirty with unrelated edits, stop and report.
- Before every Cargo command run `python qa/browser_live/check_memory.py`; proceed only when it prints `ALLOW` with at least `4294967296` available bytes. Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`, and `--test-threads=1` for focused and full-suite runs.
- Every focused Cargo test must select at least one test. Zero-selection, compile errors and source scans are not behaviour evidence.
- Test real behaviour: real Core, real owners, real resources. No sleeps, no scheduler luck, no global mocks, no public test-only API, no second registry, no second lifecycle authority.
- Production code changes are allowed **only** where a real behavioural RED demonstrates a defect in this slice's contract. Do not repair unrelated full-suite failures; do not refactor modules outside the slice.
- R1: the Core never performs platform work (no thread start, no transport open, no dynamic library load, no IPC). The Core *requests*; the owner *performs*.
- The Core contract must stay kind-neutral: no family-specific branch inside `src/capability/`. Any family is an instance, never the vocabulary.
- Out of scope for this slice: Gateway channels / Feishu / MCP / browser / sandbox / computer-use registration, EffectClaim producers, external-effect exactly-once, Safe Replay, universal durable scheduling, native unload, RSS, process isolation, "code not linked into the binary".
- `bun run check` does not exist in this checkout; do not run it or claim a substitute. Linux/Windows native execution is unavailable on this host; record it as not executed.

## Review Focus

- The Core must remain kind-neutral: grep the Core for family names (`terminal`, `pty`, `memory`, `channel`, `feishu`) and find only generic contract code. A family-specific branch is a Critical finding.
- `resolve` must never start anything; activation must be an explicit separate request, performed by the owner, and fail-closed (no stale lease on failure).
- Every provider lease must fail closed after stop and after replacement. The evidence must be a real effect (resource released / new generation), not an `Arc` identity check or a bookkeeping flag alone.
- The Memory migration must not leave two mechanisms for `CapabilityKind::Resource`. One statement per fact.
- The Terminal proof must show real resource behaviour: cold means no PTY resource exists; activation really starts the owned driver; stop really releases it.
- Claims in the run record and reference docs must match exactly what the tests proved — no "universal", no "on-demand start of any capability", no "unloadable".
- Temporary worktrees and target dirs must be isolated and only the worktree created by this plan may be removed.

---

### Task 1: Provider family census and contract freeze

**Files:**
- Create: `docs/superpowers/runs/2026-11-04-capability-core-provider-activation-run.md` (starting state + census).
- Modify only if the census contradicts it: `docs/superpowers/specs/2026-11-04-capability-core-provider-activation-design.md` (record the deviation, do not silently absorb it).

**Interfaces:**
- Consume (read-only): `src/capability/{provider.rs,backend.rs,descriptor.rs,facade.rs,zahir_facade.rs,ownership.rs,mod.rs}`, `src/gateway/pty/manager.rs`, `src/extension/runtime_owner.rs`, `src/gateway/channel_registry/`, `src/memory/runtime/`, `src/bin/aleph-server/commands/start/`.
- Produce: the census table (family → authority → lifecycle handle → lifecycle states → real resources → how it is started today → how it is stopped → whether the Core can see it today) and the frozen contract vocabulary.

- [ ] **Step 1: Record starting state**

  Record the git root, branch, HEAD, `git status --short`, the task-start commit hash, and the memory-gate reading in the run record. State which commit is the attribution baseline.

- [ ] **Step 2: Census every provider family in the tree**

  For each family — WASM/plugin owner, Terminal/`PtyManager`, Gateway channels, Feishu webhook server, Memory runtime, plus any browser / sandbox / computer-use owner you find — record: the owning authority, whether it already exposes a `ProviderHandle`, its lifecycle states, the real resources it holds, how it is started today, how it is stopped, and whether the Core can currently describe/resolve it. Cite `file:line` for every claim. Mark CPU-only or test-only shapes as out of scope with the reason.

- [ ] **Step 3: Verify the design's assumptions against the current tree**

  Confirm or refute each of: `CapabilityBackend`'s trait shape is kind-agnostic; `ToolBackendAdapter::in_scope` / `to_descriptor` are Tool-shaped; `ZahirFacade::resolve_with_capture` rejects non-tool namespaces; `ZahirFacade` carries a hardcoded Memory provider; `CapabilityKind::Resource` is already used by Memory and by Gateway channels; the `Zahir` trait has no activation path; `ProviderLifecycle` is already used by four families. Every refutation is a ledgered ruling, not a silent edit.

- [ ] **Step 4: Freeze the contract vocabulary**

  Fix the exact names and module split for the provider registry, the provider-backed adapter, the owner activation hook, the provider lease source evidence, and the activation error set. Record the `MemoryState → ProviderState` mapping that the Memory adapter will use. Write the frozen contract into the run record; the design doc stays the authority and any deviation is recorded beside it.

- [ ] **Step 5: Review the census for shape leakage**

  Re-read the frozen contract as if the family were Terminal, then as if it were a channel, then as if it were a sandbox provider. If the vocabulary needs a family-specific term, fix the contract now, before code exists. Record the outcome.

### Task 2: Core provider registry and provider-backed backend

**Files:**
- Create: `src/capability/provider_registry.rs`, `src/capability/provider_backend.rs` (names per Task 1's freeze).
- Modify: `src/capability/mod.rs`, `src/capability/zahir_facade.rs`, `src/capability/facade.rs`, `src/capability/backend.rs` (only where the composition requires it), `src/memory/runtime/mod.rs` (register the Memory provider entry), `src/bin/aleph-server/commands/start/` (registration order), plus the affected tests.

**Interfaces:**
- Consume: the existing kind-agnostic `CapabilityBackend` trait, `OwnershipTree` (`register_if_absent`, `rotate_capability`, `generation`), `ProviderLifecycle`/`ProviderHandle`, `Scope { kind, namespace, facet, visibility }`.
- Produce: `describe` / `enumerate` / `snapshot` serving provider descriptors; `resolve` returning a provider lease that carries both the ownership generation and the provider generation; the Memory entry served by the generic path (no facade special case).

- [ ] **Step 1: Write the failing contract test first**

  A provider registered in the Core's registry must be visible through `describe` under `Scope { kind: Some(CapabilityKind::Resource) }`, resolvable into a lease whose source evidence names the provider, and rejectable by an unknown id. The Tools path must be unchanged by the same test. Run it and watch it fail for the right reason.

- [ ] **Step 2: Implement the registry and the adapter**

  The registry is the single authority for provider-shaped capabilities; the adapter implements the existing backend trait over it. Do not add a second tool registry and do not give the registry a callable surface.

- [ ] **Step 3: Compose the facade**

  `ZahirFacade` serves callables from the tool adapter and providers from the provider adapter, with the same scope semantics for both. Remove the hardcoded Memory field once Memory is served by the generic path — do not keep both mechanisms alive.

- [ ] **Step 4: Migrate Memory onto the generic path**

  `MemoryRuntime` registers its provider entry (descriptor plus the adapter that maps `MemoryState` onto `ProviderState` in one place). The existing memory capability tests — `projection_host_memory_tests`, `zahir_facade_memory_tests` — are the regression net for the generic path and must stay green.

- [ ] **Step 5: Wire registration in startup**

  Register the Memory entry at the point where the runtime is created and the Core is composed. No second authority, no boot-time duplicate, and the registration must not start anything the runtime does not already start.

- [ ] **Step 6: Run the affected tests under the memory gate**

  Run the capability suites and the memory capability suites with a positive selection, and record the exact names and counts.

### Task 3: Explicit activation contract

**Files:**
- Modify: `src/capability/facade.rs` (`Zahir` trait), `src/capability/zahir_facade.rs`, `src/capability/provider_registry.rs` (activation hook), plus a `#[cfg(test)]` synthetic owner inside the Core.

**Interfaces:**
- Consume: `ProviderHandle::transition` / `acquire_call` / `generation`, `BackendLease`, `validate_lease`.
- Produce: an explicit activation request that (a) never happens implicitly, (b) is performed by the owner, (c) fails closed, and (d) returns a lease bound to the new provider generation.

- [ ] **Step 1: Write the failing activation tests first**

  Four behaviours, each with a real observable: resolving a cold provider does not call the owner's activator; an explicit activation calls it exactly once and the provider becomes ready; a failing activation returns an error, leaves the capability unavailable and yields no lease; a second activation after stop produces a **new provider generation** while the first lease stays invalid.

- [ ] **Step 2: Implement the activation path**

  The Core asks the owner through the activation hook and never performs the effect itself (R1). The activation result is reflected in the provider lifecycle so `describe` reports the new state, and the returned lease is bound to the generation that activation produced.

- [ ] **Step 3: Fence provider leases**

  `validate_lease` re-checks the provider generation for provider-sourced leases, so a lease captured before stop or replacement fails closed. Prove it with a real effect, not an identity comparison.

- [ ] **Step 4: Run the tests under the memory gate**

  Positive selection required; record exact names and counts.

### Task 4: Terminal proof instance

**Files:**
- Modify: `src/gateway/pty/` (provider adapter: descriptor + activation hook), `src/capability/` (registration), `src/bin/aleph-server/commands/start/` (registration order), plus the PTY tests.
- Do **not** change `PtyManager`'s own state machine (`ProviderState` transitions, `DriverState`, `stop_wait`, residual handling).

**Interfaces:**
- Consume: `PtyManager::activate(bus)`, `begin_stop`, `lifecycle_snapshot`, the existing `ProviderHandle` in `src/gateway/pty/manager.rs:318-328`.
- Produce: Terminal as a `CapabilityKind::Resource` provider visible through the Core, with real resource evidence for cold / activate / stop / replace.

- [ ] **Step 1: RED — cold means cold**

  Describe the Terminal capability through the Core: it must be visible, its provider state must be cold, and **no PTY resource may exist** (no owned driver task, no session). This test fails while Terminal is invisible to the Core.

- [ ] **Step 2: RED — resolve does not start it**

  Resolving the Terminal capability yields a lease while the provider stays cold and no driver exists.

- [ ] **Step 3: RED — activation starts the real driver**

  An explicit activation moves the provider to ready and the owned driver actually runs (prove it with a real observable — e.g. a produced frame or an active session — not a state flag alone). The returned lease validates.

- [ ] **Step 4: RED — stop releases the resource and fences the lease**

  Stopping the provider joins the owned driver and closes sessions; the lease captured before the stop then fails closed.

- [ ] **Step 5: RED — replacement fences the old lease**

  A second activation after stop yields a new provider generation; the old lease stays invalid.

- [ ] **Step 6: Implement the adapter and registration**

  Keep the adapter thin: descriptor + activation hook + lifecycle mapping. The Core must not learn the word "terminal".

- [ ] **Step 7: Run the Terminal and Core suites under the memory gate**

  Positive selection required; record exact names and counts.

### Task 5: Final evidence, attribution, review, delivery

**Files:**
- Update: `docs/superpowers/runs/2026-11-04-capability-core-provider-activation-run.md`.
- Modify only for factual calibration: `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/MEMORY_SYSTEM.md`.

- [ ] **Step 1: Run the full serial suite and attribute any new failure**

  Run `cargo test -p alephcore --lib -- --test-threads=1` at the slice's HEAD. If any failure is not already known to fail at the task-start commit, create one detached temporary worktree at the task-start commit with an isolated `CARGO_TARGET_DIR`, run the identical command there, and compare **named** failure sets. Report same / only-current / not-reproduced. Do not repair unrelated failures, and never claim a failure is pre-existing unless the same named test fails at the pinned baseline.

- [ ] **Step 2: Write the bilingual run record**

  Include the census table, the frozen contract, the evidence per test (exact names, counts, commands, memory-gate readings), the attribution result, the review outcome, and an explicit non-claims list.

- [ ] **Step 3: Run documentation, formatting and diff checks**

  ```bash
  python qa/docs/check_markdown_links.py --self-test
  python qa/docs/check_markdown_links.py
  python qa/browser_live/check_docs.py --self-test
  rustfmt --edition 2021 --check --config skip_children=true <touched rust files>
  git diff --check
  git diff --cached --check
  ```

  Record exact output. Do not run `bun run check` unless a `check` script has appeared in the checkout.

- [ ] **Step 4: Obtain one fresh supported read-only whole-diff review**

  Review against the task-start commit, with this plan's Review Focus verbatim. Fix concrete findings, then rerun the affected checks.

- [ ] **Step 5: Commit and verify locally**

  Create a conventional local commit (e.g. `feat(capability): make providers first-class in the core`), verify a clean tree, confirm the task-start commit is still an ancestor, and remove only the temporary worktree and files this plan created.

- [ ] **Step 6: Calibrate the reference docs**

  Update `docs/reference/FEATURE_LOCATOR.md` and `docs/reference/MEMORY_SYSTEM.md` factually: what the Core can now see, start and fence, and — just as importantly — what still remains deferred (channels, Feishu, MCP, browser, sandbox, computer-use, EffectClaim, exactly-once, binary slimness).

## Completion Criteria

- A provider family is visible through `describe`, resolvable into a lease, and explicitly startable through the Core, with no family-specific code inside `src/capability/`.
- `resolve` provably never starts a provider; activation failures are fail-closed; leases fail closed across stop and replacement.
- Terminal's evidence is real-resource evidence: cold means no PTY resource exists, activation really starts the owned driver, stop really releases it.
- Memory is served by the same generic mechanism; no second Resource mechanism survives; the existing memory capability tests stay green.
- Every new claim has a deterministic positive-count test, or is recorded as an unclosed limitation with the reason a safe deterministic test was not possible.
- No unrelated failure was repaired; no new public test-only API, no second registry, no second lifecycle authority; no channel/Feishu/MCP/browser/sandbox wiring.
- The run record and reference docs describe exact evidence and the remaining platform limits; a local conventional commit exists and the tree is clean.

## Deferred / Deferred

This slice does not register Gateway channels, Feishu, MCP, browser, sandbox or computer-use; does not build EffectClaim producers, external-effect exactly-once, Safe Replay or a universal durable scheduler; does not attempt native unload, RSS reclamation or process isolation; does not make code unloadable from the binary; does not run Linux/Windows native checks; and does not close the remaining derived-consumer / RPC operation-escape census from the Memory slices.
