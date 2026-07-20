# Process Topology Slimming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the 3 duplicate `AlephBridge` child processes into a single, truly-lazy process-wide singleton so the daemon spawns at most one Swift helper (zero when idle).

**Architecture:** `SwiftBridge` gains a cheap non-blocking `is_running()` diagnostic. The macOS crate replaces per-`MacOSPlatform` bridge construction with a process-wide `OnceLock<Arc<SwiftBridge>>` singleton and removes the construction-time warm-up handshake that was eagerly forcing a child-process spawn. Laziness is preserved by the existing `ensure_running` path: the helper spawns only on the first real `desktop.*` call.

**Tech Stack:** Rust, `std::sync::OnceLock`, `tokio::sync::Mutex`, existing `aleph-desktop` (shared) + `aleph-desktop-macos` crates.

**Spec:** `docs/superpowers/specs/2026-06-11-process-topology-slimming-design.md`

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `desktop/shared/src/bridge/client.rs` | Modify | Add `SwiftBridge::is_running()` diagnostic + test |
| `desktop/macos/src/lib.rs` | Modify | Process-wide singleton bridge; remove construction-time warm-up; add sharing + laziness tests |
| `docs/superpowers/specs/2026-06-11-process-topology-slimming-design.md` | Append (Task 3 only) | Record P2 spike conclusion |

Task 1 is a self-contained diagnostic that Task 2's laziness test depends on. Task 2 is the core change. Task 3 is an independent, time-boxed exploration that produces a written decision (no shipped code).

---

## Task 1: `SwiftBridge::is_running()` diagnostic

**Files:**
- Modify: `desktop/shared/src/bridge/client.rs` (add method after `pub fn new`, ~line 100)
- Test: `desktop/shared/src/bridge/client.rs` (existing `#[cfg(test)] mod tests`, ~line 453)

- [ ] **Step 1: Write the failing test**

Add inside the existing `mod tests` block in `desktop/shared/src/bridge/client.rs` (it already has `install_fake` and `fake_helper_script` helpers):

```rust
    #[tokio::test]
    async fn is_running_reflects_spawn_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        assert!(!bridge.is_running(), "fresh bridge must not be running");
        bridge.ensure_running().await.unwrap();
        assert!(
            bridge.is_running(),
            "bridge must report running after ensure_running"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-desktop --lib is_running_reflects_spawn_state`
Expected: FAIL — compile error `no method named is_running found for struct SwiftBridge`.

- [ ] **Step 3: Write minimal implementation**

In `desktop/shared/src/bridge/client.rs`, immediately after the `pub fn new(binary_path: PathBuf) -> Self { ... }` method (the one closing around line 100), add:

```rust
    /// Returns `true` if the helper subprocess is currently spawned (its
    /// reader loop is live).
    ///
    /// Cheap, non-blocking diagnostic: it `try_lock`s the state slot and never
    /// awaits. Used to verify the bridge stays idle (unspawned) until the first
    /// real `desktop.*` call. A momentary lock contention conservatively
    /// reports `false`.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.try_lock().map(|g| g.is_some()).unwrap_or(false)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-desktop --lib is_running_reflects_spawn_state`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add desktop/shared/src/bridge/client.rs
git commit -m "desktop: add SwiftBridge::is_running() spawn-state diagnostic"
```

---

## Task 2: Process-wide singleton bridge + remove construction-time warm-up

**Files:**
- Modify: `desktop/macos/src/lib.rs` (imports ~line 16; `new()` ~lines 64-100; tests ~line 405)
- Test: `desktop/macos/src/lib.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add these two tests inside the existing `mod tests` block in `desktop/macos/src/lib.rs`:

```rust
    #[test]
    fn platforms_share_one_bridge() {
        let a = MacOSPlatform::new();
        let b = MacOSPlatform::new();
        assert!(
            Arc::ptr_eq(&a.bridge(), &b.bridge()),
            "all platforms must share the process-wide singleton bridge"
        );
    }

    #[tokio::test]
    async fn construction_does_not_spawn_bridge() {
        // No warm-up handshake at construction: the helper stays unspawned
        // until the first real desktop call. Guards against re-introducing an
        // eager warm-up that would fork a child process at construction.
        let platform = MacOSPlatform::new();
        assert!(
            !platform.bridge().is_running(),
            "constructing a platform must not spawn the bridge"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-desktop-macos --lib platforms_share_one_bridge`
Expected: FAIL — assertion fails (`Arc::ptr_eq` is false; each `new()` currently builds its own `SwiftBridge`).

- [ ] **Step 3: Add the singleton + its import**

In `desktop/macos/src/lib.rs`, change the `std::sync` import line (currently `use std::sync::Arc;`, ~line 16) to:

```rust
use std::sync::{Arc, OnceLock};
```

Then add the singleton helper directly above the `impl MacOSPlatform { ... }` block (just before `impl MacOSPlatform`):

```rust
/// Process-wide shared Swift bridge client.
///
/// Every `MacOSPlatform` constructed in this process shares one `SwiftBridge`,
/// so the daemon spawns at most a single `AlephBridge` child regardless of how
/// many subsystems (presence reporter, builtin tool registry, voice handler …)
/// build a platform handle. The client is lazy: the child process is not
/// spawned until the first real `desktop.*` call.
static SHARED_BRIDGE: OnceLock<Arc<SwiftBridge>> = OnceLock::new();

fn shared_bridge() -> Arc<SwiftBridge> {
    SHARED_BRIDGE
        .get_or_init(|| Arc::new(SwiftBridge::new(resolve_helper_path())))
        .clone()
}
```

- [ ] **Step 4: Rewrite `new()` to use the singleton and drop the warm-up**

Replace the entire `pub fn new() -> Self { ... }` body (lines ~64-100, the version that calls `SwiftBridge::new` and `handle.spawn(async move { ... handshake ... })`) with:

```rust
    #[must_use]
    pub fn new() -> Self {
        // Shared, process-wide bridge. No construction-time warm-up: the helper
        // is spawned lazily by `ensure_running` on the first real desktop call,
        // so subsystems that never touch the bridge (e.g. the presence reporter,
        // which only uses `system()`) do not fork an `AlephBridge` child.
        let bridge = shared_bridge();

        Self {
            screen: MacOSScreen::new(Arc::clone(&bridge)),
            automation: MacOSAutomation::new(),
            escape: EscapeListener::new(),
            permission: MacOSPermission::new(Arc::clone(&bridge)),
            pim: MacOSPim::new(Arc::clone(&bridge)),
            system: MacOSSystem::new(),
            ax: BridgeAccessibility::new(Arc::clone(&bridge)),
            power: MacosPower::new(),
            bridge,
        }
    }
```

- [ ] **Step 5: Run the new tests to verify they pass**

Run: `cargo test -p aleph-desktop-macos --lib platforms_share_one_bridge construction_does_not_spawn_bridge`
Expected: PASS (both).

- [ ] **Step 6: Run the full macOS crate test suite for regressions**

Run: `cargo test -p aleph-desktop-macos --lib`
Expected: PASS — including the pre-existing `construct_includes_bridge` (its `Arc::strong_count(&bridge) >= 2` still holds, now also counting the static's reference) and `screen_is_some` / `create_default`.

- [ ] **Step 7: Verify the `tracing` import is still used (no unused-import warning)**

The removed warm-up was the only `tracing::info!`/`warn!` call site in `lib.rs`. Check:

Run: `cargo build -p aleph-desktop-macos 2>&1 | grep -i "unused" || echo "no unused warnings"`
Expected: `no unused warnings`. If `use tracing::debug;` (line ~32) is now flagged unused, confirm whether other code in `lib.rs` still uses `debug!`; if nothing does, remove the `use tracing::debug;` line. (Do not remove it if any remaining `debug!` call uses it.)

- [ ] **Step 8: Commit**

```bash
git add desktop/macos/src/lib.rs
git commit -m "desktop: share one process-wide AlephBridge; drop eager warm-up spawn"
```

- [ ] **Step 9 (manual, optional): Confirm live process count**

This requires a full app rebuild and is expensive (shared target-dir build lock). Run only if doing end-to-end acceptance; the unit tests above are the primary gate.

```bash
# Rebuild server + bridge, relaunch the running daemon, then:
pgrep -fl AlephBridge | wc -l   # expect 0 before any desktop tool is used
# After invoking a desktop.* tool (e.g. screenshot / OCR):
pgrep -fl AlephBridge | wc -l   # expect exactly 1, stable across subsystems
```

---

## Task 3 (Spike): P2 — splash `tauri://localhost` WebContent reclaim

> **Time-boxed exploration (≤90 min). Produces a written decision, not shipped code.** Per the spec, P2 is "verify first, decide later" and is NOT committed to ship. Do not implement a WebKit reclaim in this plan; if the spike succeeds, a follow-up spec/plan owns the real change.

**Files:**
- Append (conclusion only): `docs/superpowers/specs/2026-06-11-process-topology-slimming-design.md` (§3 P2)

- [ ] **Step 1: Reproduce the stranded process**

Build/run the desktop app, let it navigate splash → Panel, then capture the topology:

```bash
pgrep -fl Aleph    # confirm both a tauri://localhost and a http://127.0.0.1:18790 WebContent exist
```

Record the resident size of the `tauri://localhost` WebContent (Activity Monitor or `ps`).

- [ ] **Step 2: Try candidate A — drop the suspended process after navigation**

In `desktop/shell/src/main.rs`, in the navigation-to-Panel path (`navigate_to_panel`, ~line 351), after a successful `window.navigate(panel_url)`, evaluate whether Tauri/WKWebView exposes a hook to discard the prior origin's process (e.g. clearing back-forward cache via `window.with_webview(...)` and a WKWebView API). If a clean API exists, prototype it on a scratch branch and re-measure with `pgrep`.

- [ ] **Step 3: Decide and record**

Append a short "P2 Spike Result" subsection to the spec's §3 P2 with one of: `RECLAIMABLE` (API + measured drop, hand to follow-up spec), `NOT-WORTH-IT` (only ~14MB, no clean API), or `BLOCKED` (WebKit retains regardless). Include the measured numbers and the API path tried.

- [ ] **Step 4: Commit the decision**

```bash
git add docs/superpowers/specs/2026-06-11-process-topology-slimming-design.md
git commit -m "docs: record P2 splash-process reclaim spike result"
```

---

## Self-Review

**Spec coverage:**
- P1 singleton (3→1) → Task 2 (Steps 3-4) ✅
- P1 remove construction-time warm-up / true laziness (idle=0) → Task 2 (Step 4) + `construction_does_not_spawn_bridge` test, backed by Task 1's `is_running()` ✅
- P1 concurrency safety (InflightTable multiplex) → relies on existing behavior; no code change needed, asserted via spec §3 (unchanged call path) ✅
- P1 acceptance "idle=0 / active=1" → Task 2 Step 5 (programmatic idle=0) + Step 9 (manual active=1) ✅
- P2 spike (verify-first, no ship) → Task 3 ✅
- Out-of-scope items (daemon inline, WebKit helpers, 159MB WASM, Linux/Windows) → not tasked, by design ✅

**Placeholder scan:** No TBD/TODO; every code step shows complete code; commands have expected output. Task 3 is intentionally an investigation with a written deliverable, not vague code.

**Type consistency:** `is_running(&self) -> bool` defined in Task 1, consumed identically in Task 2's `construction_does_not_spawn_bridge`. `shared_bridge() -> Arc<SwiftBridge>` defined and consumed in Task 2. `bridge()` accessor is pre-existing (`lib.rs:103`). Crate names verified: `aleph-desktop` (shared), `aleph-desktop-macos` (macOS).
