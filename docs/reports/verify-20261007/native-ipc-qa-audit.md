# Native IPC QA path — static audit + actual run evidence

Scope: `qa-owned-root` feature of `desktop/shell` + `qa/native_ipc/run.mjs`.
**Status: PARTIAL.** Sections 1-7 are the original static audit, written when nothing had been
compiled or run (only `node --check` on `run.mjs`); they are kept as history. **Update 2026-10-12:**
the QA build was since compiled, `qa_root` unit tests passed, and the harness was run once on Windows
against a fake random-port Gateway with the QA in-memory credential backend — see §6 annotations and
§8. This verifies real Tauri credential IPC + stale-lease controls only; it is **not** production
OS-keyring / protected-pairing proof, not a full-Panel proof, and not a whole-branch approval.
Task41 / Task23 therefore stay PARTIAL.

## 1. Files touched by this change

| File | Change |
|---|---|
| `desktop/shell/Cargo.toml` | `qa-owned-root = []` (non-default; default stays `embedded-core`) |
| `desktop/shell/src/main.rs` | compile gates, `mod qa_root`, arg init, plugin/setup/background gating, WebView2 data dir |
| `desktop/shell/src/qa_root.rs` (new) | argument parsing, root validation, `root()/try_root()`, `webview_data_dir()`, unit tests |
| `desktop/shell/src/connection.rs` | `marker_home` feature arm → `qa_root::try_root()` |
| `desktop/shell/src/keyring_bridge.rs` | `InMemoryQaBackend`; `production()` picks it under the feature |
| `desktop/shell/tauri.qa.conf.json` (new) | QA identifier/binary name, bundle off, updater/deep-link removed |
| `qa/native_ipc/run.mjs`, `README.md` (new) | harness + docs |
| this report | |

`qa/README.md` deliberately untouched (already dirty; the new README lives in `qa/native_ipc/`).

## 2. Gating

* `#[cfg(all(feature="qa-owned-root", not(debug_assertions)))] compile_error!` — release builds fail.
* `#[cfg(all(feature="qa-owned-root", feature="embedded-core"))] compile_error!` — no daemon build.
* With the feature **off**, every changed site compiles to its previous text: `marker_home` keeps the
  `dirs::home_dir()` arm, `production()` keeps `ClientStoreBackend`, the plugin chain / setup /
  `restore_state` / `spawn_background` lite arm are unchanged (`not(qa-owned-root)` guards).
  *Unverified by compiler.* The default/release feature set is the intended diff-free path.
* Under the feature (lite only), compiled out: `single_instance`, `notification`, `autostart`,
  `window_state`, `updater`, `deep_link`, `global_shortcut` plugins; `tray::build`, `hotkey::setup`,
  `deeplink::setup` (Windows registry write), `window.restore_state`, notification bridge, update
  checker, `supervise_remote_lite`. `daemon.rs` / `perm_monitor` are `embedded-core` only → absent.
  Consequence: those paths are **not exercised** by this QA.
* `#![cfg_attr(feature="qa-owned-root", allow(dead_code, unused_imports))]` only silences warnings
  from modules kept for the shared command table.

## 3. Every write path reachable in a QA build

| Path | Writer | Where it lands | Control |
|---|---|---|---|
| `<root>/.aleph/.desktop-shell-panel-target` | harness only (`wx`) | root | shell only reads it at startup |
| `.desktop-shell-panel-gateway-token`, `...-target` (on user switch), `...-trusted-certs` | `connection::save_target`/`commit_target_files`, `cert_trust::{pending,store}` | `marker_path()` → `marker_home()` → `qa_root::try_root()/.aleph/` | all go through `marker_path`; grep found no other `dirs::`/home/env/temp use in non-test shell src |
| `<root>/webview2/` (WebView2 user data) | `qa_root::webview_data_dir()` (`create_dir`, non-recursive) → `WebviewWindowBuilder::data_directory` | root | plain-dir + non-reparse check, canonical parent == root, re-asserted at the call in `build_main_window` |
| credentials | `InMemoryQaBackend` (`Mutex<HashMap>`) | process memory | OS keyring (`ClientStoreBackend`/`aleph_client::DeviceCredentialStore`) not constructed under the feature |
| `grant_remote_drag` runtime capability | Tauri in-memory | memory | harmless, unchanged |
| WebView2 runtime internals, crash dumps | WebView2/OS | possibly outside root (e.g. WER) | **not controlled** — see §6 |

`marker_home` returns `None` if `qa_root` was never initialised (fail-closed → no marker path, no write).
Under `cfg(test)` the pre-existing `test_home` arm still wins.

## 4. Root validation (`qa_root.rs`)

Inputs: only `--qa-root <abs>` and `--qa-nonce <hex 16..128>` (space-separated; duplicates, missing,
flag-like values rejected). Checks, all must pass, else `process::exit(2)` before any plugin/window:
absolute; no `.`/`..` components; no `.aleph` component (raw and canonical); `symlink_metadata`
is a directory and not a link/reparse (0x400); canonical == lexical (rejects junction, 8.3, case/alias
tricks fail-closed); canonical parent == canonical `temp_dir()` (**direct child** — stricter than
"strict descendant"); leaf starts with `aleph-shell-qa-` and is longer than it; not inside, and not
containing, canonical home / `APPDATA` / `LOCALAPPDATA` (the "contains" half is needed because the
default Windows temp lives under `LOCALAPPDATA`; the "inside" half is waived only if temp itself is
inside that tree); `.qa-owned` is a regular non-link file ≤256 bytes equal to the nonce.
Home/APPDATA/LOCALAPPDATA are read only to forbid; there is no fallback to them.
Shell never deletes. Unit tests (accept, each rejection class) exist; **not run**.

## 5. Proof the harness provides (when run)

* Owned root: non-recursive `mkdir`, `wx` nonce file; binary must contain the QA marker string.
* Fake Gateway: `127.0.0.1:0` only, port 18790 refused, `GET /ready` → 200, single `POST /result`
  guarded by a per-run token and 64 KB cap.
* Marker origin `http://127.0.0.1:<port>` equals the form `to_persisted` writes; shell mints the
  `panel-keyring` capability for that origin at startup, and the page checks `gateway_url ===
  ws://127.0.0.1:<port>/ws`.
* Steps asserted: IPC present, load (null credential, positive lease), save, load (match), delete
  (`true`), load (null), bogus-lease save and delete **rejected**.
* Cleanup: PID-scoped tree kill + liveness poll, re-run containment proof, tree walk refuses
  links/oversize, then `rmSync`; otherwise `LEFT_IN_PLACE` exit 3. `HOME`/`USERPROFILE` never set and
  reported unchanged. argv carries only root + nonce; no secrets in env.

## 6. Unverified / not done

> 2026-10-12 annotation: each item below keeps its original static-audit wording; the **[now]** note
> gives the current state after the single run documented in §8.

1. Nothing compiled: `qa_root.rs`, `main.rs` gates, `keyring_bridge.rs` backend, tests may have errors.
   **[now] Resolved for the QA feature build:** `qa_root` tests 11/11 passed (167 filtered out), debug
   build with `--no-default-features --features qa-owned-root` finished, 0 warnings both (pre-edit test
   source). Strict clippy: first run RED on test-only `qa_root.rs:587`, fixed test-only; final clippy
   exit 0 / 0 warnings and final tests 11 passed / 167 filtered out (§8). No rustfmt / full-workspace
   test or clippy evidence.
2. `TAURI_CONFIG` merge with `"plugins": {"updater": null, "deep-link": null}` — unproven that
   tauri-build/tauri CLI accepts it; `mainBinaryName` honoured by plain `cargo build` — unproven.
   **[now] `mainBinaryName` is refuted:** a plain `cargo build` produced `aleph-desktop-shell.exe`, not
   `Aleph-panel-qa.exe`. **The `null` plugin-deletion merge built successfully:** the verifier set
   `TAURI_CONFIG` to the exact `tauri.qa.conf.json` contents (independently reported; main verified the
   built marker/hash) and the build passed with 0 warnings. Limitation: the env value is not printed in
   the captured logs.
3. That the lite default capability (`capabilities/default.json` has remote `http://127.0.0.1:18790/*`
   only) does not block `core:default` for the random-port origin: only the `panel-keyring` runtime
   capability is minted for it; `__TAURI_INTERNALS__.invoke` of app commands under the
   `allow-protected-*` permissions should work, but this is the very thing the run is meant to show.
   **[now] Shown for the fake origin `127.0.0.1:58682`:** all 8 steps passed through the real IPC.
4. WebView2 runtime present, navigation to `http://127.0.0.1:<port>` allowed by `on_navigation`,
   `window.__TAURI_INTERNALS__` injected on a remote origin (`withGlobalTauri` is true in base config).
   **[now] Resolved on this machine:** `tauri_internals_present` ok and `originOk: true`.
5. No cross-origin **denial** test (second origin / wrong window label); only the bogus-lease negative.
   **[now] Still open.**
6. Not exercised by design: tray, hotkey, deep link, updater, notifications, window-state, supervisor,
   embedded daemon, real OS keyring, full Panel WASM. **[now] Still open** (OS keyring is
   compile-excluded; the proof is memory-backend IPC only).
7. Writes outside the root that the QA build does not control: WebView2/Chromium crash reporting, WER,
   `%TEMP%` scratch of the OS loader, Tauri/WebView2 registry reads. The `data_directory` redirect
   covers the browser profile only. The harness does not snapshot `%LOCALAPPDATA%` / HKCU before/after.
   **[now] Still open:** no before/after HKCU / real `.aleph` snapshot was taken for the run, so "no
   writes outside the owned root" is not claimed (`envUntouched: true` only covers the harness
   process's own `HOME`/`USERPROFILE`).
8. 8.3 short-name or substituted temp paths make the shell refuse (fail-closed), not adapt.
   **[now]** The run's root (`C:/Users/zou/AppData/Local/Temp/aleph-shell-qa-2cb8fd1b7808dfd6`) was
   accepted; the refusal path was not exercised.
9. `taskkill` tree-kill could leave a detached `msedgewebview2` that outlives the root PID; the
   liveness check covers the root PID only, and a lingering process holding files makes `rmSync`
   fail → `LEFT_IN_PLACE`, not silent deletion.
   **[now] Partly addressed:** cleanup reported `removed`, and a later CIM query found no
   `msedgewebview2.exe` / `aleph-desktop-shell.exe` whose command line contains that root. CIM cannot
   see children with unreadable/missing command lines, so no global WebView2 shutdown is claimed.
10. No commit, no reset, no edits to unrelated dirty files.

## 7. Recommended next step (originally needing explicit user authorization)

Build once (`cargo check`/`build` with the §README command), run the `qa_root` unit tests, then
one visible `run.mjs` run; afterwards diff `%LOCALAPPDATA%`/HKCU/`~/.aleph` mtimes to confirm nothing
leaked outside the owned root.

**[now]** Build, `qa_root` tests and one visible run are done (§8); the final mtime/HKCU diff was
**not** done (no "before" snapshot exists). Remaining follow-ups: print the `TAURI_CONFIG` value into
the build log on a repeat run, take before/after snapshots on a repeat run, add a cross-origin denial control, and cover the
production OS-keyring / protected pairing path and the real Panel WASM (separate work).

## 8. Actual run evidence (2026-10-12, Windows, single machine)

Logs: `.analysis/browser-live-runtime/native-ipc-qa-{test.log,build.log,run.json,run.stderr.log}`
(harness stdout was read for report booleans only).

| Item | Result |
|---|---|
| `qa_root` unit tests | 11 passed, 0 failed, 167 filtered out, 0 warnings |
| `--no-default-features --features qa-owned-root` debug build | finished, 0 warnings |
| Strict clippy (QA feature, `-D warnings`) | **First run RED** (`native-ipc-qa-clippy.log`): `clippy::cloned_ref_to_slice_refs` at test-only `desktop/shell/src/qa_root.rs:587`, `&[root.clone()]`. Main changed exactly that to `std::slice::from_ref(&root)`. **Final:** `native-ipc-qa-clippy-final.log` exit 0, 0 warnings; `native-ipc-qa-test-final.log` 11 passed, 0 failed, 167 filtered out, 0 warnings (QA-feature `desktop/shell` only) |
| Exe | `target/debug/aleph-desktop-shell.exe`, sha256 `79ba76a3ec878140b19e8739cf6911f469e279e8a76fc7e67789e881a2d6f90b` |
| Harness invocations | exactly one WebView2 run; exit 0; `pass: true` |
| Steps (8/8 ok) | `tauri_internals_present`, `load_initial`, `save`, `load_after_save`, `delete`, `load_after_delete`, `save_bogus_lease_rejected`, `delete_bogus_lease_rejected` |
| Fake Gateway origin | `http://127.0.0.1:58682` (random port); `originOk: true` |
| Negative controls | both returned the exact native error `stale_lease` |
| Cleanup | `removed` `C:/Users/zou/AppData/Local/Temp/aleph-shell-qa-2cb8fd1b7808dfd6`; 141 inventoried files = `.qa-owned` + `.aleph/.desktop-shell-panel-target` + 139 WebView2 profile files; `envUntouched: true` |
| Independent re-check | root absence and JSON read directly; later CIM: zero `msedgewebview2.exe` / `aleph-desktop-shell.exe` with that exact root in the command line; Gateway PID 12704 (this worktree) still HTTP 200 |

Scope of the QA strict clippy: `desktop/shell/src/main.rs:16` carries a feature-only `#![cfg_attr(feature = "qa-owned-root", allow(dead_code, unused_imports))]` because the host side-effect modules are compiled out under the feature; every other lint ran at `-D warnings`. Release/production builds are not covered, and zero warnings does not prove the code is free of unused items.

Snapshot ordering: the 11/11 tests, build, exe hash and runtime run precede the test-only
assertion allocation edit above. Product code and the QA binary were not changed by it and no second
window was run; the post-edit final clippy and test logs above are read and clean. The runtime
snapshot remains exactly one harness invocation.

Scope of the claim: real Tauri credential IPC (`panel_credential_*`) and stale-lease rejection against
the QA in-memory backend. **Not** proven: production OS-keyring / protected pairing persistence, real
Panel WASM, cross-origin denial, macOS/Linux, global WebView2 shutdown, absence of Windows runtime
writes outside the profile. The #48 / #53 managed blockers are unchanged.
