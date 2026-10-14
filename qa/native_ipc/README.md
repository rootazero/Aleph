# qa/native_ipc — real Tauri IPC ACL check for `panel_credential_*`

**Status: PARTIAL. Compiled, unit-tested and run once on Windows (2026-10-12): real Tauri credential IPC + stale-lease controls verified against a fake random-port Gateway and the QA in-memory backend. Not production-ready; not a whole-branch approval.**
Evidence: [Actual run (2026-10-12)](#actual-run-2026-10-12) below. Original static audit (written under a "no cargo, no launch" restriction, kept as history): [`docs/reports/verify-20261007/native-ipc-qa-audit.md`](../../docs/reports/verify-20261007/native-ipc-qa-audit.md).

## What it proves (verified by the 2026-10-12 run)

A debug lite shell built with `--features qa-owned-root`, pointed at a throw-away fake Gateway
on `127.0.0.1:<random>`, lets that page invoke `panel_credential_load` → `save` → `load` → `delete`
→ `load` through the **real** `window.__TAURI_INTERNALS__.invoke` and the real runtime capability
(`panel-keyring`) — and rejects calls with a bogus lease (the negative controls require the exact native error `stale_lease`, not any rejection). Credentials live in a process-local
in-memory backend; the OS keyring is never touched.

It does **not** prove: Panel WASM behaviour, a cross-origin *denial* (no second origin is tried),
the production OS-keyring backend or protected pairing persistence (the OS keyring is compile-excluded
in the QA build; this is memory-backend IPC proof only), tray/hotkey/deep-link/updater/notifications
(all compiled out under the feature), or macOS/Linux.

## Actual run (2026-10-12)

Windows, single machine. Logs: `.analysis/browser-live-runtime/native-ipc-qa-{test.log,build.log,run.json,run.stderr.log}`.

* `qa_root` unit tests: **11/11 passed** (167 filtered out), 0 warnings.
* `--no-default-features --features qa-owned-root` debug build: finished, 0 warnings.
* Strict lint (final, 2026-10-12): the first QA-feature strict clippy run (`-D warnings`, `native-ipc-qa-clippy.log`) was RED on test-only code, `desktop/shell/src/qa_root.rs:587` (`cloned_ref_to_slice_refs`). The main agent changed exactly `&[root.clone()]` to `std::slice::from_ref(&root)` (test-only; no new shell build or launch; the runtime snapshot above remains exactly one invocation). Final logs: `native-ipc-qa-clippy-final.log` (clippy over the QA bin + tests, `-D warnings`) exit 0, 0 warnings; `native-ipc-qa-test-final.log` `qa_root` 11 passed, 0 failed, 167 filtered out, 0 warnings. Scope: QA-feature `desktop/shell` only; no whole-workspace clippy/test claim. Scope of the QA strict clippy: `desktop/shell/src/main.rs:16` carries a feature-only `#![cfg_attr(feature = "qa-owned-root", allow(dead_code, unused_imports))]` because the host side-effect modules are compiled out under the feature; every other lint ran at `-D warnings`. Release/production builds are not covered, and zero warnings does not prove the code is free of unused items.
* Actual exe: `target/debug/aleph-desktop-shell.exe`, sha256
  `79ba76a3ec878140b19e8739cf6911f469e279e8a76fc7e67789e881a2d6f90b`.
* **One** visible WebView2 harness invocation: exit 0, `pass: true`, all 8 steps ok
  (`tauri_internals_present`, `load_initial`, `save`, `load_after_save`, `delete`,
  `load_after_delete`, `save_bogus_lease_rejected`, `delete_bogus_lease_rejected`); fake Gateway origin
  `http://127.0.0.1:58682`; both negative controls returned the exact native error `stale_lease`;
  `originOk: true`.
* Cleanup: `cleanup: "removed"` for `C:/Users/zou/AppData/Local/Temp/aleph-shell-qa-2cb8fd1b7808dfd6`
  (141 inventoried files: `.qa-owned` nonce, `.aleph/.desktop-shell-panel-target` marker, 139 WebView2
  profile files); `envUntouched: true` (the harness process's own `HOME`/`USERPROFILE` unchanged; says nothing about the shell child). The main agent
  independently read the JSON and confirmed the root is gone; a later CIM query found zero
  `msedgewebview2.exe` / `aleph-desktop-shell.exe` processes whose command line contains that exact root,
  and the Gateway (PID 12704, this worktree) still answered HTTP 200.

Not shown by this run: CIM cannot prove the absence of child processes whose command line is unreadable or
missing, so **no global WebView2 shutdown claim** is made. No before/after snapshot of HKCU or the real
`.aleph` was taken, so "no Windows runtime writes outside the profile" is **not** claimed. Stdout of the
harness was read for report booleans only.

## Build

From `desktop/shell`, debug profile, **lite** (`--no-default-features`; the feature refuses to
compile together with `embedded-core` or without `debug_assertions`):

```powershell
$env:TAURI_CONFIG = Get-Content -Raw tauri.qa.conf.json   # RFC 7396 merge patch over tauri.conf.json
cargo build --no-default-features --features qa-owned-root
Remove-Item Env:TAURI_CONFIG
# exe: <target dir>\debug\aleph-desktop-shell.exe  (the cargo bin name)
```

Correction: `mainBinaryName` (`Aleph-panel-qa` in `tauri.qa.conf.json`) does **not** rename the output of a
plain `cargo build`; the exe actually produced and run was `target/debug/aleph-desktop-shell.exe`.
Point `--exe` at that file. The harness only needs the QA marker string inside the exe.

Config merge: the verifier built with `TAURI_CONFIG` set to the exact contents of `tauri.qa.conf.json` (independently reported; the main agent verified the built marker and exe hash), and that build passed with 0 warnings, so tauri-build accepted the `null` plugin deletions (`plugins.updater`, `plugins.deep-link`) in that run. Limitation: the env value was not printed into the captured logs, so the logs alone do not show it.

## Run

```powershell
node qa/native_ipc/run.mjs --exe <absolute path to aleph-desktop-shell.exe> [--timeout-ms 60000]
```

Exit codes: `0` all steps passed, `1` failed/timeout, `2` refused up-front (bad args, not a QA
build, not Windows), `3` cleanup ambiguous (directory **left in place**; inspect and delete by hand).
A JSON report (steps, root inventory, cleanup verdict) goes to stdout, progress to stderr.
A visible WebView2 window appears for a few seconds.

## Safety model

1. **Harness owns the root.** `os.tmpdir()` (realpath) + `aleph-shell-qa-<hex>`, created with
   non-recursive `mkdir` (fails if it exists), `.qa-owned` nonce file written with `wx`, plus
   `.aleph/` inside it. The shell never creates the root and never deletes anything.
2. **Shell refuses anything else** (`desktop/shell/src/qa_root.rs`): needs explicit
   `--qa-root <abs>` + `--qa-nonce <hex>`; root must be a *direct child* of canonical temp,
   leaf `aleph-shell-qa-*`, no `.aleph` component in the root path itself, not a link/junction/8.3
   alias, not an ancestor of home/`APPDATA`/`LOCALAPPDATA`, and not inside such a tree unless the canonical OS temp dir itself is (normally it is, under `LOCALAPPDATA`; the root is still only ever that owned direct child of temp), and `.qa-owned` must equal the nonce.
   No fallback to the home directory or environment.
3. **Binary check**: `--exe` must contain the literal `aleph-qa-owned-root-build-v1`, so a normal
   build (which would ignore `--qa-root` and use the real `~/.aleph` + keyring) is refused.
4. **Fake Gateway** binds `127.0.0.1:0` only, aborts if the port is 18790, answers `GET /ready`
   with 200, serves one page, accepts one authenticated-by-run-token `POST /result` (≤64 KB).
5. **No secrets in argv/env**: only the root path and the nonce are passed. The page generates
   its own dummy credential; only booleans/ids-matched flags are posted back. `HOME` / `USERPROFILE`
   are never set (and checked unchanged in the report).
6. **Cleanup** only after: child tree killed by PID (`taskkill /PID n /T /F`) and confirmed gone,
   root re-proven (plain dir, canonical == lexical, direct child of temp, prefix, nonce), and a tree
   walk found no symlink/junction. Otherwise `LEFT_IN_PLACE`, exit 3.

## Known blockers / limits

* No WebView2 driver is used: the page self-reports over HTTP, so no UI automation is needed, but
  that also means the window is not screenshotted or inspected.
* The harness has been run exactly once (see "Actual run"); there is no repeat/flake data.
* If WebView2 runtime is missing or the window never loads the fake Gateway page, the run ends in
  the timeout (exit 1) and the report carries the shell's stderr tail.
