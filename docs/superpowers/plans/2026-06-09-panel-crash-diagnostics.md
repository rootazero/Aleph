# Panel Crash Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the next intermittent panel crash self-report a symbolicated Rust call stack (in the overlay + localStorage history) so the disposal bug can be localized to its exact call site.

**Architecture:** Three independent changes to `interfaces/webchat` only. (1) A dedicated `wasm-release` Cargo profile keeps the wasm name section so stack frames carry Rust symbol names. (2) The panic hook captures the real JS backtrace and renders it in the recovery overlay. (3) A localStorage ring buffer persists the last 10 crash records across reloads. No core/harness changes.

**Tech Stack:** Rust + Leptos 0.8 (CSR, wasm32-unknown-unknown), `js-sys`, `web-sys`, `serde_json`, `wasm-bindgen`, `just`.

---

## File Structure

- **Modify** `Cargo.toml` (workspace root) — add `[profile.wasm-release]`.
- **Modify** `justfile` — `wasm` recipe builds with `--profile wasm-release` and points `wasm-bindgen` at the new output path.
- **Modify** `interfaces/webchat/build.rs` — inject `ALEPH_VERSION` from the root `VERSION` file so crash records can tag the build.
- **Modify** `interfaces/webchat/src/panic_overlay.rs` — capture stack, persist ring buffer (pure `append_capped` helper + tests), render stack + count in the overlay.

All runtime logic stays in the single cohesive `panic_overlay.rs` (the crash module). No new files.

---

## Task 1: Symbol-preserving panel build

**Files:**
- Modify: `Cargo.toml:488-491` (the `[profile.release]` block)
- Modify: `justfile:120,122-124` (the `wasm` recipe)

- [ ] **Step 1: Add the `wasm-release` profile**

In `Cargo.toml`, immediately after the existing `[profile.release]` block (currently lines 488-491):

```toml
# Release profile
[profile.release]
strip = true

# Panel-only release profile: keep the wasm `name` section so browser stack
# traces carry Rust symbol names. `strip` is a final-artifact setting and
# cannot be overridden per-package, so the panel build uses this profile while
# the `aleph-server` binary keeps `strip = true`. See
# docs/superpowers/specs/2026-06-09-panel-crash-diagnostics-design.md.
[profile.wasm-release]
inherits = "release"
strip = false
```

- [ ] **Step 2: Point the `wasm` recipe at the new profile**

In `justfile`, edit the `wasm` recipe. Change the build line (line 120):

```bash
    # 2. Compile Rust → WASM
    cargo build -p aleph-panel --target wasm32-unknown-unknown --profile wasm-release
```

And the `wasm-bindgen` input path (line 124):

```bash
    # 3. Generate JS bindings
    wasm-bindgen --target web --no-typescript \
        --out-dir {{panel_dist}} --out-name aleph_panel \
        target/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm
```

- [ ] **Step 3: Build and verify symbols are present**

Run:
```bash
just wasm
```
Expected: builds successfully, prints `✓ WASM: interfaces/webchat/dist/`.

Then verify the name section survived (with `strip=true` this grep finds nothing; with the new profile it finds the symbol):
```bash
strings target/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm | grep -m1 panic_overlay
```
Expected: a non-empty line containing `panic_overlay` (proves Rust symbol names are retained).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml justfile
git commit -m "build: keep wasm symbols for panel via wasm-release profile"
```

---

## Task 2: Inject `ALEPH_VERSION` into the panel build

**Files:**
- Modify: `interfaces/webchat/build.rs`

The panel `build.rs` does not currently expose the workspace version. Crash
records should tag which build crashed; per CLAUDE.md the version source is the
root `VERSION` file, exposed via `env!("ALEPH_VERSION")`.

- [ ] **Step 1: Add the version injection**

In `interfaces/webchat/build.rs`, inside `fn main()`, **before** the existing
`leptos_i18n_build` lines, add:

```rust
    // Expose the workspace version (root VERSION file) as ALEPH_VERSION so the
    // panic-recovery crash log can record which build crashed. Mirrors the root
    // build.rs version injection; CLAUDE.md forbids hardcoding version numbers.
    let version = std::fs::read_to_string("../../VERSION")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=ALEPH_VERSION={version}");
    println!("cargo:rerun-if-changed=../../VERSION");
```

- [ ] **Step 2: Verify it compiles and the env is visible**

Run:
```bash
cargo build -p aleph-panel --target wasm32-unknown-unknown --profile wasm-release 2>&1 | tail -5
```
Expected: builds successfully (no `environment variable ALEPH_VERSION not defined` error will occur until Task 4 uses it, but the build.rs change itself must compile cleanly here).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/build.rs
git commit -m "build: expose ALEPH_VERSION to the panel for crash reports"
```

---

## Task 3: Pure `append_capped` ring-buffer helper (TDD)

**Files:**
- Modify: `interfaces/webchat/src/panic_overlay.rs` (add helper + tests to the existing `#[cfg(test)] mod tests`)
- Test: same file, run on host via `cargo test`

This is the only branching logic in the persistence path, so it is extracted as
a pure function and unit-tested on the host (no wasm needed).

- [ ] **Step 1: Write the failing tests**

In `interfaces/webchat/src/panic_overlay.rs`, add these tests inside the
existing `#[cfg(test)] mod tests { ... }` block:

```rust
    #[test]
    fn append_capped_adds_to_empty_log() {
        let out = append_capped("[]", r#"{"message":"boom"}"#, 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["message"], "boom");
    }

    #[test]
    fn append_capped_treats_corrupt_existing_as_empty() {
        let out = append_capped("not json at all", r#"{"message":"boom"}"#, 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn append_capped_keeps_only_most_recent() {
        // Start with a full log of 3, cap 3, add one more.
        let existing = r#"[{"message":"a"},{"message":"b"},{"message":"c"}]"#;
        let out = append_capped(existing, r#"{"message":"d"}"#, 3);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 3);
        // Oldest ("a") dropped; newest ("d") retained at the end.
        assert_eq!(arr[0]["message"], "b");
        assert_eq!(arr[2]["message"], "d");
    }

    #[test]
    fn append_capped_skips_invalid_new_record() {
        let out = append_capped(r#"[{"message":"a"}]"#, "garbage", 10);
        let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["message"], "a");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p aleph-panel --lib panic_overlay::tests::append_capped 2>&1 | tail -20
```
Expected: FAIL — `cannot find function 'append_capped' in this scope`.

- [ ] **Step 3: Write the helper**

In `interfaces/webchat/src/panic_overlay.rs`, add this function at module level
(e.g. just above the `#[cfg(test)]` block):

```rust
/// Append `new_record_json` (a JSON object string) to the JSON array held in
/// `existing_json`, keeping only the most recent `cap` entries. Robust to a
/// missing or corrupt existing value (treated as an empty array) and to an
/// unparseable new record (skipped). Returns the serialized JSON array.
///
/// Pure — no DOM/JS dependency — so it is unit-testable on the host.
fn append_capped(existing_json: &str, new_record_json: &str, cap: usize) -> String {
    let mut arr: Vec<serde_json::Value> =
        serde_json::from_str(existing_json).unwrap_or_default();
    if let Ok(record) = serde_json::from_str::<serde_json::Value>(new_record_json) {
        arr.push(record);
    }
    if arr.len() > cap {
        let drop = arr.len() - cap;
        arr.drain(0..drop);
    }
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p aleph-panel --lib panic_overlay::tests 2>&1 | tail -20
```
Expected: PASS — all `append_capped_*` tests plus the existing `escape_html` / `install_is_idempotent` tests pass.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/panic_overlay.rs
git commit -m "panel: add pure append_capped ring-buffer helper for crash log"
```

---

## Task 4: Capture stack, persist, and render in the overlay

**Files:**
- Modify: `interfaces/webchat/src/panic_overlay.rs` (`hook`, `mount_overlay`, add `capture_js_stack` / `persist_crash` / `current_url`, constants)

These are thin wasm/JS shims (`js_sys::Error`, `js_sys::Date`, `web_sys::Storage`, `Location`) plus wiring; not host-testable. They are verified by compilation here and the end-to-end run in Task 5.

- [ ] **Step 1: Add constants near the top of the file**

In `interfaces/webchat/src/panic_overlay.rs`, below the existing
`const OVERLAY_ID: &str = "aleph-panic-overlay";` line, add:

```rust
/// localStorage key holding the crash-history ring buffer (JSON array).
const CRASH_LOG_KEY: &str = "aleph.panel.crashes";
/// Maximum number of crash records retained across reloads.
const CRASH_LOG_CAP: usize = 10;
```

- [ ] **Step 2: Rewrite `hook` to capture the stack, persist, and pass both to the overlay**

Replace the existing `hook` function (currently lines 35-45) with:

```rust
fn hook(info: &PanicHookInfo<'_>) {
    // 1. Preserve dev-console behavior — same trace, same formatting.
    console_error_panic_hook::hook(info);

    // 2. Capture a symbolicated JS backtrace, persist a crash record, and
    //    mount a one-shot recovery overlay. If anything here panics (it
    //    shouldn't — DOM + localStorage only) swallow it so we don't recurse.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = info.to_string();
        let stack = capture_js_stack();
        let crash_count = persist_crash(&message, &stack);
        mount_overlay(&message, &stack, crash_count);
    }));
}
```

- [ ] **Step 3: Add the capture / persist / url helpers**

In `interfaces/webchat/src/panic_overlay.rs`, add these functions just below
`hook`:

```rust
/// Best-effort capture of the JS backtrace at panic time. With the wasm name
/// section preserved (the `wasm-release` profile), these frames carry Rust
/// symbol names. Returns an empty string if unavailable.
fn capture_js_stack() -> String {
    js_sys::Error::new("")
        .stack()
        .as_string()
        .unwrap_or_default()
}

/// Append a crash record to the localStorage ring buffer and return the new
/// record count. No-op (returns 0) if localStorage is unavailable. Never
/// panics — every fallible step degrades to a default.
fn persist_crash(message: &str, stack: &str) -> usize {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    else {
        return 0;
    };
    let existing = storage
        .get_item(CRASH_LOG_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "[]".to_string());
    let record = serde_json::json!({
        "ts": js_sys::Date::now(),
        "version": env!("ALEPH_VERSION"),
        "message": message,
        "stack": stack,
        "url": current_url(),
    });
    let new_log = append_capped(&existing, &record.to_string(), CRASH_LOG_CAP);
    let _ = storage.set_item(CRASH_LOG_KEY, &new_log);
    serde_json::from_str::<Vec<serde_json::Value>>(&new_log)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Current page URL, or an empty string if unavailable.
fn current_url() -> String {
    web_sys::window()
        .and_then(|w| w.location().href().ok())
        .unwrap_or_default()
}
```

- [ ] **Step 4: Update `mount_overlay` to render the stack and the crash count**

Change the `mount_overlay` signature and the details/note rendering. Replace the
signature line (currently `fn mount_overlay(message: &str) {`) with:

```rust
fn mount_overlay(message: &str, stack: &str, crash_count: usize) {
```

Then replace the `let escaped = escape_html(message);` line (currently line 82)
with:

```rust
    // Details pane shows the panic message followed by the symbolicated stack.
    let mut details = escape_html(message);
    if !stack.is_empty() {
        details.push_str("\n\n");
        details.push_str(&escape_html(stack));
    }
    let note = format!(
        "{crash_count} crash report(s) saved · localStorage key: {CRASH_LOG_KEY}"
    );
```

In the `overlay.set_inner_html(&format!(...))` block, change the `<pre>...{escaped}...</pre>`
interpolation to use `{details}`, and add a note line just before the buttons
`<div>`. The `<details>` element and the buttons row become:

```rust
           <details style=\"margin-bottom:18px;\">\
             <summary style=\"cursor:pointer;font-size:12px;color:#a1a1aa;user-select:none;\">Show details</summary>\
             <pre style=\"margin:8px 0 0;padding:12px;background:#0a0a0a;border-radius:8px;\
                          font-size:11px;color:#fca5a5;overflow:auto;max-height:200px;\
                          white-space:pre-wrap;word-break:break-word;\">{details}</pre>\
           </details>\
           <p style=\"margin:0 0 18px;font-size:11px;color:#71717a;\">{note}</p>\
           <div style=\"display:flex;gap:10px;justify-content:flex-end;\">\
```

And update the trailing named arguments of the `format!` macro (currently
`OVERLAY_ID = OVERLAY_ID, escaped = escaped,`) to:

```rust
        OVERLAY_ID = OVERLAY_ID,
        details = details,
        note = note,
```

- [ ] **Step 5: Verify the panel compiles for wasm**

Run:
```bash
cargo build -p aleph-panel --target wasm32-unknown-unknown --profile wasm-release 2>&1 | tail -15
```
Expected: builds successfully (this confirms `js_sys::Error::stack`, `js_sys::Date::now`, `Location::href`, `env!("ALEPH_VERSION")`, and the `format!` argument changes all compile).

- [ ] **Step 6: Verify host tests still pass**

Run:
```bash
cargo test -p aleph-panel --lib panic_overlay::tests 2>&1 | tail -15
```
Expected: PASS — all tests green.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/panic_overlay.rs
git commit -m "panel: surface symbolicated stack + persist crash log in panic overlay"
```

---

## Task 5: End-to-end verification and build refresh

**Files:** none committed (temporary edit reverted). Verifies the full chain.

Because the crash is not naturally reproducible, force one panic to confirm the
overlay shows a readable Rust stack and localStorage records it.

- [ ] **Step 1: Insert a temporary panic on a reachable path**

In `interfaces/webchat/src/app.rs`, inside the top-level component body (after
the existing setup, before the returned view), temporarily add:

```rust
    // TEMP diagnostics smoke test — REMOVE before commit.
    leptos::task::spawn_local(async {
        gloo_timers::future::TimeoutFuture::new(3_000).await;
        panic!("diagnostics smoke test");
    });
```

(`gloo-timers` and `leptos::task::spawn_local` are already used across the
panel; if the exact import path differs in `app.rs`, use the same `spawn_local`
form already imported there.)

- [ ] **Step 2: Build the panel and re-embed it into the daemon**

Per the CLAUDE.md refresh chain (the running daemon serves the panel embedded at
server compile time — `just wasm` alone is not enough):

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```

Then swap the running binary so the supervisor relaunches it (dev daemon):
```bash
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```

- [ ] **Step 3: Trigger and observe**

Open the panel, wait ~3s for the forced panic, then:
1. Confirm the "Aleph Panel crashed" overlay appears.
2. Click **Show details** → confirm the `<pre>` contains a **readable Rust
   stack** with symbol names (e.g. frames mentioning `aleph_panel`,
   `app`, `panic_overlay`) — not bare `wasm-function[12345]`.
3. In devtools console run `localStorage.getItem('aleph.panel.crashes')` →
   confirm a JSON array with one record containing `message`, `stack`,
   `version`, `ts`, `url`.
4. Confirm the overlay shows the `N crash report(s) saved` note line.

Expected: all four hold. If the stack is still unsymbolicated, re-check Task 1
Step 3 (the `strings` grep) — the build may not have used `wasm-release`.

- [ ] **Step 4: Revert the temporary panic**

Remove the TEMP block added in Step 1 from `interfaces/webchat/src/app.rs`.
Confirm it is gone:
```bash
git diff interfaces/webchat/src/app.rs
```
Expected: empty diff (no residual changes).

- [ ] **Step 5: Rebuild clean and final-verify**

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server 2>&1 | tail -5
```
Expected: builds successfully with no diagnostics smoke-test code remaining.

---

## Self-Review notes

- **Spec coverage:** Component 1 → Task 1; Component 2 → Task 4 (`capture_js_stack` + overlay render); Component 3 → Task 3 (pure helper) + Task 4 (`persist_crash`); version tag → Task 2; testing/verification → Task 3 + Task 5; build refresh chain → Task 5.
- **Type consistency:** `append_capped(&str, &str, usize) -> String` defined in Task 3, called identically in Task 4. `persist_crash -> usize` feeds `mount_overlay(.., crash_count: usize)`. `capture_js_stack() -> String` feeds both `persist_crash` and `mount_overlay`.
- **Scope:** single subsystem (panel diagnostics); no core/harness changes; defensive read-guard audit explicitly deferred per spec.
