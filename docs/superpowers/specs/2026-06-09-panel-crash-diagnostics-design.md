# Panel Crash Diagnostics — Design

**Date**: 2026-06-09
**Status**: Approved (pending spec review)
**Scope**: `interfaces/webchat` (panel) build + panic overlay. No core/harness changes.

## Problem

The desktop panel intermittently crashes with:

```
panicked at reactive_graph-0.2.14/src/traits.rs:361:29:
Tried to access a reactive value that has already been disposed.
```

`traits.rs:361` is reactive_graph's signal **read** path (`.get()`/`.read()`/`.with()`).
The panic means a closure that outlives its owner scope — an async task
(`spawn_local`), a timer (`set_timeout`), or a global event listener — read a
signal/memo **after** the owning component was disposed (component unmount,
`<Show>`/`<Suspense>` switch, session/tab/page switch).

The existing `panic_overlay.rs` already catches the panic and offers a Reload
button (this is why "reload fixes it"), but it does **not** localize the bug:

1. The overlay shows only `info.to_string()` — the panic message plus the
   *library-internal* location. It never shows the call stack into our code.
2. `[profile.release] strip = true` (`Cargo.toml:489`, applied to the
   `--release` panel build in `justfile:120`) strips the wasm **name section**,
   so even the `console_error_panic_hook` stack in devtools is unsymbolicated
   (`wasm-function[12345]`).

The crash is **not reliably reproducible**. Blindly auditing the 349
`spawn_local` / 99 `Effect::new` / many timer sites would be a huge, unverifiable
sweep. Instead we make the **next** crash self-report a readable Rust call stack,
then fix the exact site.

## Goal

When the next crash happens, the user can — without opening devtools — read a
symbolicated Rust call stack that points to the offending component/closure, and
a history of recent crashes is retained across reloads.

Non-goal: fixing the underlying disposal bug in this work. That is a follow-up
once a stack localizes it. Non-goal: app-wide defensive read guards.

## Design

Three small, independent components.

### Component 1 — Symbol-preserving panel build

- **Root cause**: `[profile.release] strip = true` strips the wasm name section.
  `strip` is a final-artifact profile setting and **cannot** be overridden
  per-package, so a per-package override in `profile.release` will not work.
- **Change**: add a dedicated profile to the workspace `Cargo.toml`:

  ```toml
  [profile.wasm-release]
  inherits = "release"
  strip = false
  ```

  This affects only the panel build; the `aleph-server` binary keeps
  `strip = true`.
- **Change**: update the `justfile` `wasm` recipe:
  - build with `--profile wasm-release` instead of `--release`
  - point `wasm-bindgen` at
    `target/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm`
- **Result**: browser stack frames carry Rust function names (the wasm **name
  section** is sufficient; wasm-bindgen preserves it by default and the recipe
  runs no `wasm-opt`). Line numbers are not available without sourcemaps and are
  not pursued — the function name pinpoints the closure/component well enough.
- **Cost**: `.wasm` grows ~10–30%. Acceptable — the asset is served from
  localhost, embedded in the desktop app, never fetched over the internet.

### Component 2 — Capture the real JS stack in `panic_overlay::hook`

- **Current**: `hook()` calls `console_error_panic_hook::hook(info)` then
  `mount_overlay(&info.to_string())`. `info.to_string()` is message + location
  only.
- **Change**: capture the JS backtrace at hook time via
  `js_sys::Error::new("")` → `.stack()` (the same mechanism
  `console_error_panic_hook` uses internally). Combine the panic message and the
  captured stack, render both into the overlay `<details><pre>`, and hand the
  record to Component 3.
- The `console_error_panic_hook::hook(info)` call is **preserved** unchanged so
  dev-console output is unaffected.
- **Result**: clicking "Show details" yields a copy-pasteable Rust stack with no
  devtools required.

### Component 3 — localStorage crash-history ring buffer

- **On panic**: read JSON array at key `aleph.panel.crashes`, push a record
  `{ ts, version, message, stack, url }`, trim to the most recent **N = 10**,
  write back. Synchronous, no backend dependency.
  - `ts` via `js_sys::Date::now()`; `version` via `env!("ALEPH_VERSION")`;
    `url` via `window.location().href()`.
- **Result**: crash records survive reload and accumulate, making "is it always
  the same stack?" answerable at a glance.
- The overlay appends a one-line note: "N earlier crashes saved
  (localStorage key: aleph.panel.crashes)". A dedicated in-app crash viewer is
  out of scope.

## Testing & Verification

- **Pure-function unit tests** (no wasm needed):
  - ring-buffer trim: a pure helper `(existing: Vec<Record>, new: Record, cap)
    -> Vec<Record>` keeping the most recent `cap`. Unit-tested.
  - existing `escape_html` tests retained.
- **wasm/JS boundary** (`Error::stack`, `Storage`, `Date`) stays in thin,
  untested shims.
- **End-to-end manual verification** (closes the loop given no natural repro):
  1. temporarily insert a `panic!("diagnostics smoke test")` on a reachable path
  2. `just wasm` (now using `wasm-release`)
  3. rebuild `aleph-server` so `rust_embed` re-embeds the new dist; swap the
     running binary (per CLAUDE.md refresh chain)
  4. trigger → confirm the overlay `<details>` shows a **readable Rust stack**
     and `localStorage["aleph.panel.crashes"]` has a record
  5. revert the temporary `panic!`

## Build refresh chain (CLAUDE.md)

Panel source changes require `just wasm` **and** an `aleph-server` rebuild
(`rust_embed` embeds `dist/*` at server compile time). A bare `just wasm` only
updates disk; the running daemon must get a replaced binary to serve the new
panel.

## Scope redlines

- **No** static audit / defensive read guards (direction C) — deferred until a
  stack localizes the specific site.
- **No** daemon crash-report RPC — no existing log-ingestion surface; localStorage
  already survives reload. Revisit (aligns with R5) only if localStorage proves
  insufficient.
- **No** changes to `src/harness/` or any core code. Pure panel diagnostics.

## Follow-up (separate work, after a stack lands)

Fix the localized disposal bug at its specific call site (likely a `spawn_local`
that reads a signal after `await`, or a timer/listener closure), using the
idiomatic Leptos guard for that one site.
