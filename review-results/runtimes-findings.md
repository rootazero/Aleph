# Logic Review Report
**Module**: src/runtimes
**Scope**: 10 files (~3755 LOC), end-to-end audit
**Date**: 2026-08-28
**Mode**: strict

## Findings

### Critical

#### CRIT-1 — Concurrency: every gateway handler reloads the ledger from disk; concurrent RPCs race and silently lose updates
**Files**:
- `src/gateway/handlers/mod.rs:163-172` (`make_runtime_ledger`)
- `src/gateway/handlers/agents.rs:91-114` (`runtimes.install` registration)
- `src/gateway/handlers/runtimes.rs:71-102` (`handle_refresh`)
- `src/gateway/handlers/runtimes.rs:108-180` (`handle_install` + spawned task)

**Observed**: `make_runtime_ledger()` is called per RPC, each call returning a **fresh** `CapabilityLedger` instance loaded from `~/.aleph/runtimes/ledger.json`. `handle_refresh` walks `SPECS`, probes each, and writes the ledger back. `handle_install` spawns a task that calls `ensure_capability` (which writes back after success / failure). All three (`runtime_startup_warmup`, `bootstrap_runtime`, and these two RPC handlers) load → modify → persist independently.

**Race**: Two concurrent callers can both load the same on-disk state, each modify their own copy, each persist back via atomic write (write-to-temp + rename). The atomic write protects against file corruption, but **last-write-wins silently drops the other writer's changes**. Concrete reproductions:

1. Panel clicks "Install fnm"; user runs `aleph-server bootstrap-runtime --only uv` in parallel. fnm-install updates the ledger entry for `fnm`; uv-install updates the entry for `uv`. Whichever finishes second overwrites the other side's update.
2. `handle_refresh` (full sweep) racing `handle_install("node")`: refresh sees node's old status, install updates node, refresh overwrites with its older view.
3. `runtime_startup_warmup` is spawned at server boot via `tokio::spawn(runtime_startup_warmup())` (`src/bin/aleph-server/commands/start/mod.rs:143`). If a Panel "Refresh" RPC arrives before warmup finishes, the two writes race.

**Why this matters**: The `CapabilityLedger::persist` comment claims concurrency-safety via pid+sequence temp naming (`ledger.rs:336-340`), but that only protects against temp-file clobbering, not against lost updates of the underlying file.

**Severity**: Critical. User-visible state corruption: capabilities reported as Ready when they aren't, or vice versa. The Panel's read of the ledger reflects whichever writer happened to flush last.

**Proposed patch**:
1. Make the ledger a process-wide singleton owned by `AppContext`, plumbed through every RPC handler that needs it:
   - At `gateway/handlers/mod.rs:163-172`, change `make_runtime_ledger()` to receive an `Arc<AsyncRwLock<CapabilityLedger>>` from the gateway's app-state instead of reloading the file. Have the gateway initialize it **once** during start-up (after `runtime_startup_warmup` finishes its initial probe-and-persist).
2. Pass the shared ledger into both `handle_install` and `handle_refresh` (already an `Arc<RwLock<>>` parameter in `runtimes.rs:71` / `runtimes.rs:109`) and `agents.rs:99-114`. All callers now share in-memory state; the file write still happens under the write lock so disk visibility is consistent.
3. For the CLI (`bootstrap_runtime`) and the warmup, inject the same singleton via a separate CLI flag or env var so the binary has one shared instance per process.

The `ensure_capability` per-capability `tokio::sync::Mutex` already serializes same-capability installs; the gap is between the *different* capabilities and between handlers that don't go through `ensure_capability` at all (`handle_refresh`).

---

#### CRIT-2 — `fnm exec --using lts` runs BEFORE the `lts` alias is created → silently returns nothing for `node` install
**File**: `src/runtimes/bootstrap.rs:300-348` (`enrich_path_for_via_parent` → `fnm_lts_bin_dir`)

**Observed**: `install()` flow for the `node` spec:
1. Run `fnm install --lts` — drops node into `~/.fnm/node-versions/<v>/installation/bin/`. **No `lts` alias yet** (that is the *next* step).
2. `enrich_path_for_reprobe()` — adds `~/.fnm` to PATH.
3. `enrich_path_for_via_parent("fnm")` (line 110-111, 307-318) → calls `fnm_lts_bin_dir()` (line 323-348) → runs `fnm exec --using lts -- node -e '...'` → `fnm` returns "lts version not found" → returns `None`. **Silently does nothing.**
4. Re-probe: relies on `install_dir_candidates()` walking `fnm_node_bin_dirs(~/.fnm)` which falls through `aliases/` (empty) → enumerates `node-versions/<v>/installation/bin`. **Works, but only by accident**.
5. Post-install: `FnmAlias { alias_name: "lts" }` (post_install.rs:104-128) creates the alias — **after** the re-probe.

The `fnm_lts_bin_dir()` function therefore **always returns `None` during the first install of node**, because the `lts` alias it depends on hasn't been created yet. The function is dead code in the production install path; the bin dir is found by `fnm_node_bin_dirs` instead.

**Why this matters**:
- The doc comment on `enrich_path_for_via_parent` (lines 301-306) claims it resolves the bin dir for **both `node` (parent `fnm`) and `playwright-cli` (parent `node`)**. **But `playwright-cli` is `InstallStrategy::NpmGlobal`, not `InstallStrategy::Via`** (`specs.rs:147-156`), so the `parent: "node"` arm of `enrich_path_for_via_parent` is **unreachable**. The function effectively only matches `parent: "fnm"`, and even then the result is `None` on first install.
- If a future spec uses `parent: "node"` (e.g. an `npm exec --` parented on node), `enrich_path_for_via_parent` returns `None` because the `lts` alias hasn't been set up yet for a fresh node install. The re-probe would still likely succeed via the `node-versions/<v>/installation/bin` fallback, but the function's stated contract is misleading.

**Proposed patch**:
1. Re-order the install flow so the alias (if any) is created BEFORE re-probe and path enrichment. In `bootstrap.rs:88-141`:
   ```rust
   // Step 1: install command (already there)
   // Step 2: enrich PATH for re-probe (already there)
   // Step 3: PRE-INSTALL — run any pre-reprobe post-install actions that set up
   //         search paths (e.g. FnmAlias). Either split the PostInstallAction
   //         enum into PreInstallAction / PostInstallAction, or re-run the
   //         `FnmAlias` action unconditionally here for `Via`-parent specs.
   if let InstallStrategy::Via { parent: "fnm", .. } = &os_install.strategy {
       for action in spec.post_install {
           if matches!(action, PostInstallAction::FnmAlias { .. }) {
               post_install::run(action, &bin_path).await?;
           }
       }
   }
   // Step 4: enrich_path_for_via_parent (now finds the alias)
   // Step 5: re-probe
   // Step 6: run remaining post_install actions
   ```
2. Delete the `parent: "node"` arm of `enrich_path_for_via_parent` (line 308-310) and the `parent: "node"` arm of `run_via_parent` (line 396-401) — both are unreachable today; restore them only when a spec actually needs them.
3. Remove the `parent: "node"` line from the comment (line 304).

---

#### CRIT-3 — `gateway/handlers/runtimes.rs:246` test comment lies about the spec — `cargo` has install strategies, not `install: &[]`
**File**: `src/gateway/handlers/runtimes.rs:246`

**Observed**: Test comment:
```rust
// cargo has `install: &[]` → Unsupported → ensure_capability Err path
```
But `specs.rs:198-211` defines cargo's `install` as **non-empty** — a Shell strategy for Unix, a PowerShell strategy for Windows.

**Race**: On a machine where cargo isn't installed, the test actually invokes `rustup` (curl-pipe-to-bash on Unix, `winget install Rustlang.Rustup` on Windows). The test only "passes" today because (a) the CI host probably has cargo installed (probe succeeds → "done" event), or (b) the test environment captures a `Failed` event from a network error, both of which coincidentally have a `stderr` field per the test's other assertion. The test does **not** exercise the `BootstrapResult::Unsupported` path as the comment claims.

**Why this matters**: The test is "green" for the wrong reason. If cargo were ever changed to `install: &[]` (deliberately or by accident), no test would catch the resulting regression in the install-progress event payload. The test's protective value is zero today.

**Proposed patch**:
1. Replace the comment with a true-`Unsupported` capability. There is no current spec with empty install; introduce a synthetic spec only in a test helper, or pick a spec with a current `BootstrapResult::Failed` path (e.g., a fake capability that points at a non-existent `--prefix`).
2. Better: add a new `RuntimeSpec` with `install: &[]` to `SPECS` gated behind `#[cfg(test)]`, with `name: "test-unsupported"` — then the test exercises the real path.
3. Alternative: drop the comment-and-test's claim about `Unsupported` and assert on `done` only. The `stderr`-field shape is the same in both cases per the test's other assertion; tightening the comment to match.

---

### Warning

#### WARN-1 — `gateway/handlers/mod.rs:169-172` `make_runtime_ledger()` reloads the ledger on every RPC, also racy with warmup
**Files**: same as CRIT-1.

**Observed**: Even after fixing the handler-to-handler race (CRIT-1), the per-call ledger reload still creates N independent copies during one RPC round-trip. The whole `runtimes.list`, `runtimes.refresh`, `runtimes.install` set, plus `runtime_startup_warmup` (background-spawned at boot), plus `bootstrap_runtime` (separate process — separate file lock, but no inter-process lock), all read+write the same file.

**Why this matters**: Even if the same-process races are eliminated, the CLI (`bootstrap-runtime`) runs in a separate process with its own ledger instance. Two simultaneous CLI invocations (e.g. a cron + a user) race on the JSON.

**Proposed patch**: A file-level advisory lock (`flock(2)` on Unix, `LockFileEx` on Windows) wrapping every `persist()`. This is a separate concern from in-process concurrency and is only reachable for the CLI process; for in-process handlers the AppContext singleton from CRIT-1 is sufficient.

---

#### WARN-2 — `bootstrap_runtime --force` uses `update_status(Missing)` instead of `mark_missing`; old path/version persist on a stale entry
**File**: `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs:81-85`

**Observed**:
```rust
if args.force {
    let mut g = ledger.write().await;
    g.update_status(cap, CapabilityStatus::Missing);
}
```
`update_status` only flips the status; it does **not** clear `bin_path`/`version`. After `--force`, the ledger has `status: Missing` but still carries the old path/version. The very next `ensure_capability` call:

1. Fast path check: `status(capability) == Ready` → false (Missing). Skip. Good.
2. Probe: runs, finds the binary at the old path, sets `bin_path` to the same path (or empty if probe failed). Status set to `Ready` with the *new* path. The stale `bin_path` was overwritten.
3. If probe fails: bootstrap runs.

So in practice the `--force` flag works because `ensure_capability` re-probes. But the ledger briefly shows `status: Missing, bin_path: /old/path` — a confusing intermediate state. Worse, if `--force` is followed by a concurrent `handle_refresh` (see CRIT-1), the refresh sees the new path → updates to Ready → overwrites `--force`'s Missing state. The user's intent to "force re-bootstrap" is silently undone.

**Proposed patch**: Use `mark_missing(cap)` (which clears path and version) instead of `update_status`. This matches `handle_refresh`'s choice at `runtimes.rs:96-98` and makes `--force`'s intent clear in the on-disk JSON.

---

#### WARN-3 — Sync primitives import rule violation throughout `src/runtimes/` and consumers
**Files**:
- `src/runtimes/ensure.rs:15` `use std::sync::OnceLock;`
- `src/runtimes/ensure.rs:16` `use tokio::sync::RwLock;`
- `src/runtimes/bootstrap.rs:4` `use std::sync::Mutex;`
- `src/runtimes/bootstrap.rs:449` `fn lock_path_env() -> std::sync::MutexGuard<'static, ()>`
- `src/runtimes/ledger.rs:13` `use std::sync::atomic::AtomicU64;`
- `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs:11` `use std::sync::Arc;`
- `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs:14` `use tokio::sync::RwLock;`
- `src/bin/aleph-server/commands/start/runtime_warmup.rs:30-31` `use std::sync::Arc; use tokio::sync::RwLock;`
- `src/gateway/handlers/runtimes.rs:14` `use tokio::sync::RwLock;`

**Observed**: AGENTS.md enforces "Sync Primitives Import Rule: `Arc/Mutex/RwLock/atomics` from `crate::sync_primitives`". `crate::sync_primitives` (defined at `src/sync_primitives.rs:25-37`) re-exports `std::sync::Arc`, `tokio::sync::RwLock as AsyncRwLock`, `std::sync::Mutex`, `std::sync::MutexGuard`, and the standard `Atomic*` types. The motivation is loom compatibility: tests that swap to `loom::sync` types must see a single chokepoint.

`ensure.rs:15` `OnceLock` is not currently re-exported by `sync_primitives`; **either add the re-export** (preferred — `OnceLock` has a stable loom story) or document the exception in `sync_primitives.rs`. `ledger.rs:13`'s `std::sync::atomic::AtomicU64` is explicitly justified by the doc comment ("loom's `new` is not `const fn`"); **lift that justification into `sync_primitives.rs` as a documented exception** so future audits do not flag it.

**Proposed patch**:
1. Add to `src/sync_primitives.rs`:
   ```rust
   pub use std::sync::OnceLock;       // Not under loom — sync-only atomic-static initializer
   ```
2. Replace `use std::sync::OnceLock;` → `use crate::sync_primitives::OnceLock;` (ensure.rs:15).
3. Replace `use tokio::sync::RwLock;` → `use crate::sync_primitives::AsyncRwLock as RwLock;` (or import as `AsyncRwLock` consistently) across all consumers.
4. Replace `use std::sync::Mutex;` → `use crate::sync_primitives::Mutex;` (bootstrap.rs:4).
5. Replace `use std::sync::Arc;` → `use crate::sync_primitives::Arc;` (bootstrap_runtime/mod.rs:11, runtime_warmup.rs:30).
6. Replace `std::sync::MutexGuard` → `crate::sync_primitives::MutexGuard` (bootstrap.rs:449).
7. Move the `AtomicU64` exception justification from the in-file comment to `sync_primitives.rs` as a documented carve-out.

---

#### WARN-4 — `enrich_path_for_reprobe` mutates the daemon's process-wide PATH; not reverted on failure or retry
**File**: `src/runtimes/bootstrap.rs:209-298`

**Observed**: `prepend_existing_dirs` calls `std::env::set_var("PATH", joined)` under `PATH_LOCK`. The mutation is permanent for the daemon's lifetime. Subsequent probes, retries, and concurrent installers all see the widened PATH. **This is intentional** (so the re-probe `which` finds the just-installed binary), but:

1. If `bootstrap::install` fails between `enrich_path_for_reprobe()` (line 103-106) and the error return, the PATH is widened with no corresponding success.
2. If the caller calls `bootstrap::install` twice with different capabilities, the second call sees the first's path additions (idempotent — good).
3. **Test pollution**: `test_enrich_path_for_reprobe_is_idempotent` (bootstrap.rs:482-498) does not restore PATH after the test. As long as no test parallel to this one asserts on PATH contents, this is benign; but the contract is implicit.
4. The `PATH_LOCK` (`std::sync::Mutex`) is **held inside the sync function** but the caller (`bootstrap::install`) is async. Holding `std::sync::MutexGuard` across `.await` would be unsound — there is no `.await` inside `prepend_existing_dirs`, so OK today. **A future maintainer who adds an `.await` inside `prepend_existing_dirs` will create a soundness bug.**

**Proposed patch**:
1. Add a small RAII guard `PathEnrichmentGuard` that snapshots PATH on creation and restores on Drop. Hold the guard for the rest of `bootstrap::install`; on early-return error paths the guard restores the original PATH.
2. Document the invariant: "any future `await` inside `prepend_existing_dirs` is unsound — switch to `tokio::sync::Mutex` and acquire in async context."

---

#### WARN-5 — `probe::probe` builds the enriched PATH on every call; not memoized
**File**: `src/runtimes/probe.rs:91-103` (`enriched_search_path`)

**Observed**: `enriched_search_path()` is called twice per `probe_system_path` invocation (once for `find_on_path`, once for `get_version`), and `install_dir_candidates()` rebuilds a fresh `Vec<PathBuf>` each call — including 10+ env var lookups (`HOME`, `USERPROFILE`, `CARGO_HOME`, `ASDF_DATA_DIR`, `APPDATA`, `LOCALAPPDATA`, `SCOOP`, `npm_config_prefix`, `NPM_CONFIG_PREFIX`, `FNM_DIR`) and many `PathBuf` allocations. For `runtime_startup_warmup` probing all 6 SPECS, that's ~12 invocations × ~20 env var reads = ~240 syscalls per server start.

`gateway/handlers/runtimes.rs::handle_refresh` re-probes all 6 SPECS on every "Refresh" click. Same redundancy.

**Proposed patch**: Cache `install_dir_candidates()` in a `OnceLock<Vec<PathBuf>>` that recomputes when an env var invalidates it. Use a simple version counter or a wall-clock TTL (e.g. 30 s). Avoid caching during bootstrap (where PATH genuinely changes); only cache the read-only probe path.

---

#### WARN-6 — `bootstrap::install` returns generic error message when `bootstrap::install` itself returns `Err`; actionable hint skipped
**File**: `src/runtimes/ensure.rs:177-191`

**Observed**:
```rust
let bootstrap_result = match bootstrap::install(capability).await {
    Ok(result) => result,
    Err(e) => {
        ledger.write().await.update_status(capability, CapabilityStatus::Missing);
        return Err(AlephError::runtime(
            capability,
            format!("Bootstrap failed: {e}"),
        ));
    }
};
```
For `BootstrapResult::Failed { stderr }` / `PathNotFound` / `Unsupported` / `UnknownCapability`, the caller uses `runtime_error(capability, reason, Some(&stderr))` which builds a three-line actionable message (CLI fix, Panel fix, manual install_hint). **For `Err(BootstrapError::PostInstall(...))`**, none of that is built — the user sees only `"Bootstrap failed: post-install action failed: <stderr>"` with no install_hint.

`BootstrapError::PostInstall` carries the stderr inside `PostInstallError::SubcommandFailed { stderr }` (post_install.rs:14). It is reachable.

**Proposed patch**:
```rust
Err(BootstrapError::PostInstall(post_install::PostInstallError::SubcommandFailed { stderr })) => {
    ledger.write().await.update_status(capability, CapabilityStatus::Missing);
    return Err(runtime_error(capability, "post-install action failed", Some(&stderr)));
}
Err(other) => { /* existing generic handler */ }
```
Distinguish the `SubcommandFailed` variant from `Io`/`Timeout`/`NoNodeVersion`/`RepairFailed`/`HomeNotSet` so each gets a tailored actionable hint.

---

#### WARN-7 — `bootstrap_runtime` `--force` does not actually force re-bootstrap when probe succeeds
**File**: `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs:81-92`

**Observed**: `--force` sets status to Missing, then calls `ensure_capability`. `ensure_capability`:
1. Fast path: `status == Ready` → false (we forced Missing). Skip.
2. Acquire cap_lock.
3. Probe phase: runs `probe::probe(cap)`. If probe finds the binary (because it's still on PATH), the entry is updated to Ready with the same path. **Bootstrap is not run.**
4. If probe fails: bootstrap runs.

So `--force` only forces bootstrap when the binary is missing from PATH. **It does not force re-installation when the binary exists.** A user upgrading Rust or reinstalling a corrupted uv expects `--force` to re-download.

**Proposed patch**:
- Rename the flag's intent: `--refresh` (re-probe and ensure Ready; do not re-bootstrap if probe succeeds) vs `--reinstall` (force bootstrap regardless of probe). Two flags with distinct semantics are easier to reason about than one overloaded flag.

---

#### WARN-8 — `bootstrap.rs:300-348` `fnm_lts_bin_dir` always returns `None` on first install; dead code in the production path
**File**: `src/runtimes/bootstrap.rs:323-348`

**Observed**: See CRIT-2. The function calls `fnm exec --using lts -- node -e '...'`, but the `lts` alias doesn't exist until post_install runs (after the re-probe). So the function always returns `None` for the `node` install flow.

**Why this matters**: The function contributes to the `enrich_path_for_via_parent` decision. If it returned `Some(...)`, `prepend_existing_dirs(vec![dir])` would add the lts bin dir to PATH BEFORE re-probe. As-is, the re-probe relies on `install_dir_candidates()` walking `fnm_node_bin_dirs` (which falls through to `node-versions/<v>/installation/bin`). Both work today, but the comment on `enrich_path_for_via_parent` overstates the function's contribution.

**Proposed patch**: Either fix the ordering (see CRIT-2) so the alias is created first, or delete `fnm_lts_bin_dir` entirely and document that `enrich_path_for_via_parent` is currently a no-op for the only `Via` spec (`node` → fnm).

---

### Suggested Test

#### TEST-1 — Add a regression test for the `--force` "binary exists on PATH" path
**Why**: WARN-7 documents that `--force` silently no-ops when probe succeeds. There is no test that demonstrates the user-intended re-bootstrap actually happens.

**Proposed patch** (in `bootstrap_runtime/mod.rs`):
```rust
#[tokio::test]
async fn test_force_reruns_bootstrap_even_when_probe_succeeds() {
    // Lay out a fake "uv" binary on PATH, mark it Ready in the ledger, then
    // run with --force and assert the install command was invoked (counter).
    // Counters go on `bootstrap::install` via a trait seam or a test-only
    // feature flag; the alternative is to assert that `bin_path` was replaced
    // by the fresh install path.
}
```
Or accept that `--force`'s current semantics are "ensure Ready, but re-bootstrap if missing" and rename the flag.

---

#### TEST-2 — Add a test for `bootstrap::install` `Err(BootstrapError::PostInstall)` → error message format
**Why**: WARN-6. There is no test that the post-install failure path produces the actionable three-line error message.

**Proposed patch**:
```rust
#[tokio::test]
async fn test_post_install_failure_includes_actionable_hint() {
    // Lay out a fake uv that succeeds for `uv --version` but fails for
    // `uv venv ~/.aleph/.venv` (write-protected dir, etc.). Assert that the
    // resulting error message contains "Fix options:" and the install_hint.
}
```

---

#### TEST-3 — Add a concurrent-install test for the new shared ledger (after CRIT-1 fix)
**Why**: CRIT-1. The atomic-write protects file integrity, not lost updates. A regression test should fail if any future change re-introduces per-call ledger reload.

**Proposed patch**:
```rust
#[tokio::test]
async fn test_concurrent_install_and_refresh_share_state() {
    // Spawn two tasks: one calls ensure_capability("fnm"), the other calls
    // a hypothetical handle_refresh that updates another capability. Assert
    // both writes land in the final on-disk ledger.
}
```

---

#### TEST-4 — Add a `mark_missing` test in `bootstrap_runtime` for `--force`
**Why**: WARN-2 documents that `--force` uses `update_status` instead of `mark_missing`. A test asserting that the on-disk JSON's `bin_path` and `version` are cleared after `--force` would catch the regression.

**Proposed patch**: small integration test in `bootstrap_runtime/mod.rs`:
```rust
#[tokio::test]
async fn test_force_clears_stale_bin_path_and_version() {
    // Pre-populate the ledger with a Ready entry pointing at /nonexistent/uv.
    // Run `bootstrap_runtime --force --only uv` against a mocked install that
    // succeeds. Assert the intermediate on-disk JSON has bin_path="" between
    // the status flip and the bootstrap completion.
}
```

---

#### TEST-5 — Add a coverage test for the dead `parent: "node"` arms
**Why**: CRIT-2 / WARN-8. The `parent: "node"` arms in `enrich_path_for_via_parent` and `run_via_parent` are unreachable today. A test demonstrating they would behave correctly IF a future spec used `Via { parent: "node", .. }` would lock in the design intent.

**Proposed patch**: Either delete the dead arms (preferred — YAGNI) or add a test-only `RuntimeSpec` for a hypothetical "yarn" that uses `Via { parent: "node", .. }`.

---

## Wiring Gaps (this module → outside)

| Item | Type | Status | Should be used by |
|------|------|--------|------------------|
| `CapabilityLedger` is reloaded per-RPC instead of plumbed through `AppContext` | Gap | Wrong | `gateway/handlers/runtimes.rs::{handle_list, handle_refresh, handle_install}`; `gateway/handlers/agents.rs:91-114`; `bin/aleph-server/commands/bootstrap_runtime/mod.rs`; `bin/aleph-server/commands/start/runtime_warmup.rs` |
| `make_runtime_ledger()` should be deprecated for in-process callers | API | Wrong | `gateway/handlers/mod.rs:163-172` |
| `bootstrap::install` `Err` path bypasses `runtime_error()` helper | Wiring | Inconsistent | `ensure.rs:177-191` should call `runtime_error(...)` for the `SubcommandFailed` variant |
| `enrich_path_for_via_parent` `parent: "node"` arm unreachable | Dead code | Unused | No spec uses `Via { parent: "node", .. }` (playwright-cli is NpmGlobal) |
| `run_via_parent` `parent: "node"` arm unreachable | Dead code | Unused | Same |
| `npm_global::prefix()` reads env vars; if missing, falls back to node version manager tree | Documented | Working | `run_npm_global` (`bootstrap.rs:360-389`) warns and proceeds; install lands inside fnm's tree, but no recovery |
| `format_entries_for_prompt` only consumes `Ready` entries | OK | Working | `orchestrator/harness_bridge/prompt_build.rs:447` |
| `build_enhanced_path` re-reads ledger on every call | Perf gap | Working | `builtin_tools/code_exec.rs:379`; `builtin_tools/code_check.rs:194` (both fire per tool call) |
| `tools/probes/browser.rs::managed_cli_path()` re-reads ledger on every probe | Perf gap | Working | `BrowserRuntimeProbe::probe` (5-min TTL — bounded) |
| `browser/playwright_cli.rs:107-109` re-reads ledger per browser call | Perf gap | Working | `PlaywrightCli` — every browser tool invocation |
| `fnm_lts_bin_dir` returns `None` for first install | Misleading comment | Working | Always falls through to `fnm_node_bin_dirs` fallback (works) |
| `registry.register("runtimes.list"|"runtimes.refresh")` in `gateway/handlers/mod.rs:847-861` rebuild ledger per call | Wiring | Wrong | Should use the shared AppContext ledger (CRIT-1 fix) |
| `registry.register("runtimes.install")` in `gateway/handlers/agents.rs:99-114` | Wiring | Inconsistent | Comment says "overrides the event_bus-less placeholder" — correct, but ledger still ad-hoc per call |

## Lock/Cross-Module Concerns

| Concern | File | Severity | Detail |
|---------|------|----------|--------|
| **No documented lock hierarchy position for runtimes** | `src/sync_primitives.rs:14-21` lists levels 0-3 but excludes runtimes | Warning | `runtimes::CapabilityLedger` uses `tokio::sync::RwLock` (re-exported as `AsyncRwLock`). The bootstrap flow acquires both a per-capability `tokio::sync::Mutex` (ensure.rs:99) and the ledger's `RwLock` in alternating order (read → write). No other module holds both — `gateway/handlers/runtimes.rs` only holds the ledger, `bin/aleph-server/commands/bootstrap_runtime/mod.rs` only holds the ledger, `bin/aleph-server/commands/start/runtime_warmup.rs` only holds the ledger. So there is no observed cross-module deadlock today. **Document the lock order: `cap_lock` (per-capability) → `ledger` (any)** in `ensure.rs:30-50` so a future cross-module caller does not introduce an ABBA cycle. |
| **Sandbox uses `runtimes::post_install::HomeEnvGuard`** | `src/sandbox/workspace/mod.rs:2020`; `src/sandbox/proxy/netns_bridge.rs:356` | OK | Both acquire HOME for read-only protection (test-only). The sandbox does NOT depend on the ledger or capability state. No cross-module lock. |
| **Skills/discovery use `HomeEnvGuards`** | `src/skill/mod.rs:765,974`; `src/discovery/scanner.rs:500`; `src/utils/paths.rs:1880` | OK | Acquire both HOME and ALEPH_HOME in the fixed order. Cross-module with runtimes is via the test helper only. |
| **`browser::playwright_cli::ensure_capability` consumes the ledger with no shared instance** | `src/browser/playwright_cli.rs:107-126` | Critical (CRIT-1) | Loads ledger from disk per call; racy with concurrent installs. |
| **`tools::probes::browser::BrowserRuntimeProbe::probe` reads ledger under no lock** | `src/tools/probes/browser.rs:181-183` | Warning | The probe runs on `spawn_blocking`, reads the ledger off-disk. No cross-module lock; relies on file isolation. After CRIT-1, plumb the shared `AppContext` ledger into the probe. |
| **`bin/aleph-server::start::runtime_warmup` runs concurrently with Panel RPCs** | `src/bin/aleph-server/commands/start/mod.rs:143` | Critical (CRIT-1) | `tokio::spawn(runtime_startup_warmup())` returns immediately; the warmup may still be writing the ledger when the first Panel "Install" arrives. |
| **`src/capability/census.rs:1849` mentions `runtimes::bootstrap::install` only in a doc comment** | `src/capability/census.rs:1849` | OK | No actual cross-module call. The "capability" module here is the wrapper/census subsystem, distinct from runtime capabilities. |
| **`extension::watcher` calls `get_runtimes_dir().ok()`** | `src/extension/watcher.rs:335` | OK | Just a directory lookup; no shared state. |

---

## Per-File Sub-Findings (lower-severity notes that don't warrant a dedicated finding)

### bootstrap.rs
- `enrich_path_for_reprobe` adds `~/.fnm` (the root) but not the inner `node-versions/<v>/installation/bin` (line 230). The re-probe relies on `install_dir_candidates()` walking fnm-managed dirs explicitly. **Self-consistent, but easy to misread.**
- `install_dir_candidates` and `enrich_path_for_reprobe` duplicate the OS-specific lists (probe.rs:226-269 vs bootstrap.rs:215-260). **Refactor opportunity**: extract a `static INSTALL_DIR_CANDIDATES_FN: fn() -> Vec<PathBuf>` shared between probe and bootstrap. The two functions differ slightly (bootstrap also adds `/usr/bin` on Linux; probe adds `~/.local/bin` from probe only). Pin the differences in the shared helper with named parameters.
- `BOOTSTRAP_TIMEOUT_SECS = 600` (line 167). For uv (small download) this is overkill; for rustup (download + compile) it may be tight. Consider per-spec timeouts via `RuntimeSpec::bootstrap_timeout_secs: Option<u64>`.
- `run_shell` calls `Command::new("sh")` directly (line 189-191). On Windows this would fail. The strategy dispatcher routes Windows-only commands through `run_powershell`, so the only way to reach `run_shell` on Windows is via `Select_install` returning a Unix strategy for a Windows host — which `TargetOs::matches` correctly prevents. **Verified safe.**

### capability.rs
- `format_entries_for_prompt` (line 22-44) gracefully handles `&[&CapabilityEntry]` but writes to a `String` via `writeln!` and discards `Result` with `let _ = writeln!(...)`. The `writeln!` to `String` cannot fail. **Fine.**
- Tests at line 49-105 hard-code capability fields. **Standard.**
- `get_hint_from_spec` (line 14) calls `find_spec(runtime_id).and_then(|s| s.llm_hint)`. If the entry name is a runtime not in SPECS (e.g., an old custom one), returns `None`. The "unknown_runtime" test (line 89-104) verifies this path. **OK.**

### ensure.rs
- `MAX_BOOTSTRAP_DEPTH = 10` (line 58) caps recursive dep resolution. With current SPECS the longest chain is `playwright-cli → node → fnm` (depth 2). The cap is generous. **OK.**
- `ensure_capability_recursive` (line 61-258) holds the `cap_lock` across the bootstrap call — this is the intended serialization, but it also serializes the recursive `ensure_capability_recursive(dep, ...)` calls. **Cross-capability parallelism is preserved** (different `cap_lock`s for different names), but a chain like `playwright-cli → node → fnm` is fully serialized even if three sibling tasks were requesting different leaves. **Acceptable.**
- The `Box::pin(ensure_capability_recursive(dep, ledger, depth + 1)).await?` pattern (line 150) is a workaround for Rust 2024's `async fn` recursion limit. **OK.**
- `runtime_error` truncates stderr to 400 chars using `is_char_boundary` walk-back (line 281-285). **Safe.**
- The `format_entries_for_prompt` is consumed only by `orchestrator/harness_bridge/prompt_build.rs:447` (verified by grep). No other consumers. **OK.**

### ledger.rs
- `CapabilityEntry::new_ready` (line 73-87) does NOT set `last_probed` to the current time, but rather to `now_secs()`. Wait — let me re-read: line 87: `last_probed: now_secs()`. **OK, sets to now.** The constructor is consistent with `update` callers.
- `load_or_create` (line 119-155) returns the same in-memory instance across two callers only if they call with the same `persist_path` AND the same process. **No caching across processes — fine.**
- `revalidate_ready` (line 240-258) demotes `Ready` entries whose `bin_path` no longer exists. **Documented regression for the fnm-alias-evaporates-on-node-upgrade failure.**
- `build_path` (line 260-309) emits a best-effort concatenation if `env::join_paths` fails. The fallback uses `to_string_lossy()` which replaces invalid UTF-8 with U+FFFD. **Rare PATHs have invalid UTF-8.**
- `persist` (line 327-352) uses pid+sequence for the temp file. **Documented at line 333-339.**
- `TMP_SEQ` (line 17) uses `std::sync::atomic::AtomicU64` directly. **Documented exception.**
- The `CapabilityLedger` derives `Clone` (line 96) — but `persist_path` is `#[serde(skip)]`. After `Clone`, both clones share the same `persist_path` (because PathBuf is Clone). **No surprise.**
- Test `revalidate_treats_a_dangling_symlink_as_gone` (line 568-584) is gated `#[cfg(unix)]` — correct, symlinks are Unix-only.

### mod.rs
- Re-exports are clean (line 23-41). No dead re-exports (every exported name is used externally — verified by grep).
- `get_runtimes_dir` (line 50-52) just delegates to `crate::utils::paths::get_runtimes_dir`. **Fine.**

### npm_global.rs
- `prefix_from` (line 53-62) takes the env lookup as a parameter, **deliberately decoupled from process-global state**. Test-friendliness. **Excellent.**
- `default_prefix` (line 65-85) handles the no-HOME case on Windows (reconstructs from `USERPROFILE`). **Documented.**
- The test `default_prefix_is_outside_any_node_version_tree` (line 96-114) asserts that the prefix doesn't contain markers like `node-versions`, `/fnm/`, `/.nvm/`, etc. **Regression coverage.**

### os.rs
- `TargetOs::current` is `const fn` (line 27-44). **Fine.**
- `TargetOs::matches` (line 45-55) handles `AnyOs` (matches all), `AnyUnix` (matches Mac/Linux), and concrete-to-concrete (same only). **Documented.**
- `Wildcard` variants (`AnyUnix`, `AnyOs`) are forbidden as the `current` argument in real use (only valid inside `OsInstall`). **Documented at line 6-8.**

### post_install.rs
- `expand_home` (line 30-46) replaces `/bin/python` → `\Scripts\python.exe` and `/` → `\` on Windows (line 39-44). **Cross-platform handling is critical for the uv AssetProbe spec.**
- `run_subcommand` (line 78-103) creates the target_dir's parent before invoking the subcommand. **Documented at line 86-91.**
- `create_fnm_alias` (line 104-128) parses `fnm list` output by looking for a line starting with `*`. **Brittle to fnm output format changes.** A unit test would catch this.
- `verify_or_repair` (line 130-152) checks the asset path post-repair and errors if still missing. **Documented at line 144-149.**
- `POST_INSTALL_TIMEOUT_SECS = 300` (line 50). For `playwright-cli install-browser chromium` (a Chromium download) this may be tight. **Consider per-spec timeout.**
- `HomeEnvGuard::Drop` (line 204-209) restores HOME BEFORE releasing the mutex (because the struct's `Drop::drop` body runs before field destructors). **Correct ordering.**
- `HomeEnvGuards` (line 240-258) fixes the order: ALEPH_HOME_LOCK first, HOME_LOCK second. **Documented rationale.**
- The static source-scan test `nothing_acquires_the_two_env_locks_separately` (line 277-329) prevents future ABBA deadlocks. **Excellent regression coverage.**

### probe.rs
- `enriched_search_path` (line 91-103) builds the enriched PATH per call. **See WARN-5.**
- `find_on_path` (line 112-134) tries `which`/`where` first, then a manual PATH walk. **Documented fallback rationale at line 110-111.**
- `extend_path` (line 160-177) dedups via `HashSet<&Path>`. **Note**: `cand.clone()` is required (line 172). The `// rust-doctor-disable-next-line excessive-clone` comment is necessary, but clippy's `excessive_clone` could also be silenced project-wide if these are pervasive.
- `install_dir_candidates` (line 226-269) is the single source of truth for which paths to search. **Documented at line 178-189.**
- `fnm_data_dir_candidates` (line 298-325) is cross-platform: includes scoop on Windows, $FNM_DIR override, XDG default on Unix. **Documented.**
- `fnm_node_bin_dirs` (line 356-370) walks aliases AND node-versions; falls back to `installation/` when `installation/bin/` doesn't exist (Windows). **Documented at line 339-355.**
- `REGEX_CACHE` (line 374-377) uses `Lazy<Mutex<HashMap>>`. **Fine.**
- `get_compiled_regex` (line 379-402) caches compiled `Regex` per pattern; clones the value to return. **OK.**
- `version_lt` (line 432-444) compares only major.minor. **Documented limitation at line 428-431.**

### specs.rs
- The `RuntimeSpec` struct (line 5-22) has 11 fields. **All used.**
- `InstallStrategy` enum (line 35-65) has 4 variants: `Shell`, `PowerShell`, `Via { parent, subcommand }`, `NpmGlobal { package }`. **All exercised by SPECS.**
- `PostInstallAction` enum (line 47-60) has 3 variants: `RunSubcommand`, `FnmAlias`, `AssetProbe`. **All exercised.**
- `SPECS` (line 61-275) declares 6 capabilities: fnm, node, uv, playwright-cli, cargo, git. **All have install strategies on every supported OS.**
- `find_spec` (line 278-280) is a linear scan over `SPECS`. For 6 entries this is fine. **OK.**
- `select_install` (line 283-285) picks the first matching `OsInstall`. If a spec accidentally declares two `OsInstall`s for the same OS, the first wins silently. **Not currently an issue.**
- `supported_on_current_os` (line 288-291) returns true iff `find_spec` AND `select_install` both succeed. **Documented.**
- Test `no_spec_installs_a_global_npm_package_through_via` (line 397-426) is a regression guard against hand-rolled `Via { parent: "node", subcommand: ["npm", "install", "-g", ...] }`. **Excellent.**
- Test `test_deps_reference_known_specs` (line 366-376) ensures `deps` resolves. **OK.**
- Test `test_via_parent_in_deps` (line 380-394) ensures `parent` is in `deps`. **OK.**

---

## Top Concerns, Ranked

1. **CRIT-1** — Concurrent RPCs lose ledger updates because every handler reloads the ledger from disk. Affects every Panel "Refresh" / "Install" + concurrent warmup + concurrent CLI invocation.
2. **CRIT-2** — `fnm exec --using lts` runs before the `lts` alias is created, silently no-ops. The dead `parent: "node"` arms + misleading doc comment compound the issue.
3. **CRIT-3** — Test comment in `gateway/handlers/runtimes.rs:246` lies about the spec; cargo has install strategies, not `install: &[]`. The test passes for the wrong reason.
4. **WARN-3** — Sync primitives import rule violation across runtimes + 4 consumer files. AGENTS.md mandate.
5. **WARN-6** — `bootstrap::install` `Err(PostInstall)` skips the actionable three-line error message that `BootstrapResult::Failed` provides.

---

## Deliberately NOT (per project guidelines)

- Did NOT modify any source file.
- Did NOT run `cargo` commands.
- Did NOT create git commits.
- Did NOT run any stateful server.

The findings above are observation-only. Every proposed patch is **suggested**, not applied. Severity assignments are subjective but anchored to the project's `rust-logic-audit` skill conventions: Critical = data loss / security / correctness with no current mitigation; Warning = behavior gap that has a current workaround or is bounded; Suggested Test = missing coverage.

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 3 |
| Warning | 8 |
| Suggested Test | 5 |
| Per-file sub-findings | 21 (mostly observational) |
| Wiring gaps | 12 entries |
| Cross-module concerns | 7 entries |