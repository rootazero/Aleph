# Native Panel QA (Task #56) - partial patch cleanup

Date: 2026-10-12. Scope: only the incomplete, never-compiled Task #56 patch
(agent 86cadaba-655e-482) was removed. **No cargo, tests, processes, HOME or
keyring were touched; nothing was compiled or run after the cleanup.**

## Removed (whole files, all introduced by #56)
- desktop/shell/src/qa_keyring.rs
- desktop/shell/src/qa_report.rs
- desktop/shell/capabilities/qa-panel.json
- desktop/shell/permissions/qa.toml

## Minimal exact edits (provenance: each removed block was labelled "Task #56")
- desktop/shell/src/main.rs: removed `mod qa_keyring;` / `mod qa_report;`, the
  pre-Tauri `qa_keyring::early_dispatch` / `qa_driver::validate_args` block, the
  `qa_report::qa_report` handler arm, `qa_driver::install` / `start_handoff` /
  `configure_main` calls, and the whole `mod qa_driver { ... }` (+ its tests).
  Kept the #54 `qa_root` wiring (mod, `init_from_args`, `data_directory`, cfgs).
- desktop/shell/src/keyring_bridge.rs: `production()` back to
  `Arc::new(InMemoryQaBackend::default())` under `qa-owned-root` (dropped the
  `qa_keyring::selected_backend()` opt-in). Rest of file untouched.
- desktop/shell/build.rs: dropped `QA_COMMAND`/`qa_report` conditional manifest
  entry and its doc bullet; `main()` uses `COMMANDS` directly (the #41 manifest).
- desktop/shell/Cargo.toml: `qa-owned-root = ["dep:keyring"]` -> `qa-owned-root = []`
  (kept #54 comment); removed the optional `keyring` dependency and its comment.
- Cargo.lock: removed the single `"keyring",` line from the `aleph-desktop-shell`
  dependency list (the `keyring` package and aleph-client's use are #41, kept).

## Preserved (not touched)
qa/native_ipc/README.md, qa/native_ipc/run.mjs,
docs/reports/verify-20261007/native-ipc-qa-audit.md, qa_root.rs,
tauri.qa.conf.json, credential_store/keyring_bridge production logic,
src/sandbox/platforms/windows/managed_launch.rs and its report (#53), all other
dirty/untracked files. No reset/checkout/commit. After cleanup, the touched tracked
text files were normalized to the repository's LF working form to remove accidental
whole-file CRLF diffs; no semantic change was intended.

## Unresolved / not verified
- The memory-gated offline `cargo check -p aleph-desktop-shell` attempt reached the
  shell build script but failed before Rust checking because the bundled resource
  `binaries\\aleph-server-x86_64-pc-windows-msvc.exe` is absent. The QA-variant
  attempt likewise could not load the supplied Tauri config (`expected value at line
  1 column 1`) and then the normal config hit the same missing-binary resource.
  Therefore no Native source correctness or runtime claim is made.
- Cargo.lock was edited by hand; a later cargo run might re-sort/refresh it.
- Provenance of qa_root.rs / tauri.qa.conf.json as #54 is inferred from mtimes
  (09:32-09:54, before the #54 README 10:07) and absence of any #56 marker.
- No pre-#56 snapshot existed; "#54 state" was reconstructed from #56 markers.
  Backups of the pre-cleanup files are in /tmp (main.rs.bak, keyring_bridge.rs.bak,
  build.rs.bak, Cargo.toml.bak, Cargo.lock.bak) but qa_keyring.rs/qa_report.rs/
  qa-panel.json/qa.toml were deleted without copies.
